//! Prototype harness for the devshell-environment ADR.
//!
//! It imports nothing from the workspace and is not a workspace member. The
//! point is to measure what a step whose kind the contract does not name costs,
//! and what a backend had to be asked, not to become production code.

// A failure is built once per failed operation, never in a loop; boxing it
// would only add noise to every constructor and call site.
#![allow(clippy::result_large_err)]
// Requirements arrive with the check that uses them; until then every helper
// is defined ahead of its call site.
#![allow(dead_code)]

mod checks;
mod closure;
mod failure;
mod launch;
mod model;
mod prepare;
mod util;


use std::path::PathBuf;
use std::time::Instant;

use failure::Failure;
use model::{Settings, Step};
use prepare::Consent;
use util::Caller;

fn main() {
    if let Err(failure) = run() {
        eprintln!("{failure}");
        std::process::exit(failure.class.exit_code());
    }
}

/// The command line, parsed once, with every setting independent of the order
/// its flag appeared in.
#[derive(Default)]
struct Cli {
    command: Option<String>,
    step: Option<PathBuf>,
    installable: Option<String>,
    project: Option<PathBuf>,
    out: Option<PathBuf>,
    workspace: Option<PathBuf>,
    bwrap: Option<PathBuf>,
    whole: bool,
    allow_project_setup: bool,
    require_paths: Vec<String>,
    workload: Vec<String>,
}

fn run() -> Result<(), Failure> {
    let cli = parse_cli(std::env::args().skip(1))?;
    let command = cli
        .command
        .clone()
        .ok_or_else(|| Failure::misconfigured(usage()))?;

    let nix = util::resolve("nix")?;
    let caller = Caller::new()?;

    match command.as_str() {
        "check" => {
            let checks = checks::readiness(&nix, &caller);
            print!("{}", checks::report(&checks));
            match checks::refusal(&checks) {
                Some(refusal) => Err(refusal),
                None => Ok(()),
            }
        }
        "prepare" => {
            let step = step_of(&cli)?;
            let out = out_of(&cli)?;
            let project = project_of(&cli)?;
            let preparation =
                prepare::prepare(&step, &project, &out, &nix, &caller, consent_of(&cli))?;
            print!("{}", preparation.provenance);
            println!("{}", preparation.measurements.report());
            Ok(())
        }
        "closure" => {
            let out = out_of(&cli)?;
            let (installable, output) = manifest(&out)?;
            if let Some(named) = &cli.installable
                && named != &installable
            {
                return Err(Failure::misconfigured(format!(
                    "the prepared environment at '{}' belongs to '{installable}', not '{named}'",
                    out.display()
                ))
                .with_remedy("prepare again, or point --out at this installable's directory"));
            }
            let mounts = closure::closure(&nix, &caller, &output, cli.whole)?;
            util::write_nul_paths(&out.join("mounts.nul"), &mounts)?;
            println!("{} mounts", mounts.len());
            Ok(())
        }
        "run" => run_all(&cli, &nix, &caller),
        other => Err(Failure::misconfigured(format!(
            "unknown command: {other}\n{}",
            usage()
        ))),
    }
}

fn run_all(cli: &Cli, nix: &std::path::Path, caller: &Caller) -> Result<(), Failure> {
    let step = step_of(cli)?;
    let out = out_of(cli)?;
    let project = project_of(cli)?;
    let workspace = cli.workspace.clone().ok_or_else(|| {
        Failure::misconfigured("--workspace is required: it is what the sandbox sees")
    })?;

    // Consent is settled before anything runs on the host: a refusal must not
    // leave a nix process behind, so the gate is the first thing create does.
    prepare::consent_gate(&step, consent_of(cli))?;

    let checks = checks::readiness(nix, caller);
    print!("{}", checks::report(&checks));
    if let Some(refusal) = checks::refusal(&checks) {
        return Err(refusal);
    }

    let preparation = prepare::prepare(&step, &project, &out, nix, caller, consent_of(cli))?;
    println!("{}", preparation.measurements.report());
    println!("store root held: {}", preparation.gc_root);

    let mounts = closure::closure(nix, caller, &preparation.output_path, cli.whole)?;
    util::write_nul_paths(&out.join("mounts.nul"), &mounts)?;
    let view = launch::View::new(mounts, &workspace, cli.whole)?;
    println!("{}", view.store_view());

    let bwrap = match &cli.bwrap {
        Some(path) => path.clone(),
        None => util::resolve("bwrap")?,
    };
    let workload = if cli.workload.is_empty() {
        vec![
            "cargo".to_string(),
            "test".to_string(),
            "-p".to_string(),
            "agentkit-common".to_string(),
        ]
    } else {
        cli.workload.clone()
    };
    let environment = &preparation.contributions.environment;

    let started = Instant::now();
    let (status, argv_bytes) = launch::launch(&bwrap, &view, environment, &workload)?;
    let launch_ms = started.elapsed().as_millis();

    println!("mounts: {}", view.reads.len());
    println!("argv: {argv_bytes} bytes");
    println!("launch: {launch_ms} ms");
    util::write_private(
        &out.join("launch.txt"),
        &format!(
            "mounts={}\nargv_bytes={}\nlaunch_ms={}\nexit={:?}\n",
            view.reads.len(),
            argv_bytes,
            launch_ms,
            status.code()
        ),
    )?;

    if !status.success() {
        return Err(Failure::failed(format!("the workload exited {status}"))
            .with_step(&step.identity, step.kind())
            .with_remedy("run the workload on the host to see why"));
    }
    Ok(())
}

fn step_of(cli: &Cli) -> Result<Step, Failure> {
    if let Some(path) = &cli.step {
        let text = std::fs::read_to_string(path).map_err(|error| {
            Failure::misconfigured(format!("could not read {}: {error}", path.display()))
        })?;
        let table: toml::Table = toml::from_str(&text).map_err(|error| {
            Failure::misconfigured(format!("{}: {error}", path.display()))
        })?;
        return model::parse_step(&table);
    }
    let installable = cli.installable.clone().ok_or_else(|| {
        Failure::misconfigured("a step is required: pass --step <file> or --installable <ref>")
    })?;
    Ok(Step {
        identity: "devshell".to_string(),
        settings: Settings::Devshell { installable },
    })
}

fn consent_of(cli: &Cli) -> Consent {
    if cli.allow_project_setup {
        Consent::Allowed
    } else {
        Consent::Refused
    }
}

/// The output directory, made absolute so the harness, which creates it relative
/// to its own working directory, and the child, which runs in the project, agree
/// on where it is.
fn out_of(cli: &Cli) -> Result<PathBuf, Failure> {
    let out = cli
        .out
        .clone()
        .ok_or_else(|| Failure::misconfigured("--out is required"))?;
    if out.is_absolute() {
        return Ok(out);
    }
    let cwd = std::env::current_dir().map_err(|error| {
        Failure::failed(format!("could not read the working directory: {error}"))
    })?;
    Ok(cwd.join(out))
}

fn project_of(cli: &Cli) -> Result<PathBuf, Failure> {
    let project = cli.project.clone().unwrap_or_else(|| PathBuf::from("."));
    project.canonicalize().map_err(|error| {
        Failure::misconfigured(format!(
            "the project root '{}' does not resolve: {error}",
            project.display()
        ))
        .with_remedy("pass the checkout the installable is relative to")
    })
}

/// The installable and output path a preparation recorded, refusing a stale
/// directory that belongs to another installable.
fn manifest(out: &std::path::Path) -> Result<(String, String), Failure> {
    let text = std::fs::read_to_string(out.join("manifest")).map_err(|error| {
        Failure::misconfigured(format!(
            "no prepared environment at '{}': {error}",
            out.display()
        ))
        .with_remedy("run the prepare command first")
    })?;
    let mut installable = None;
    let mut output = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("installable=") {
            installable = Some(value.to_string());
        }
        if let Some(value) = line.strip_prefix("output_path=") {
            output = Some(value.to_string());
        }
    }
    match (installable, output) {
        (Some(installable), Some(output)) => Ok((installable, output)),
        _ => Err(Failure::misconfigured(format!(
            "the manifest at '{}' is incomplete",
            out.display()
        ))
        .with_remedy("prepare again")),
    }
}

fn parse_cli(args: impl Iterator<Item = String>) -> Result<Cli, Failure> {
    let mut cli = Cli::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                cli.workload.extend(args.by_ref());
                break;
            }
            "--step" => cli.step = Some(PathBuf::from(value(&mut args, "--step")?)),
            "--installable" => cli.installable = Some(value(&mut args, "--installable")?),
            "--project" => cli.project = Some(PathBuf::from(value(&mut args, "--project")?)),
            "--out" => cli.out = Some(PathBuf::from(value(&mut args, "--out")?)),
            "--workspace" => cli.workspace = Some(PathBuf::from(value(&mut args, "--workspace")?)),
            "--bwrap" => cli.bwrap = Some(PathBuf::from(value(&mut args, "--bwrap")?)),
            "--require-path" => cli.require_paths.push(value(&mut args, "--require-path")?),
            "--whole" => cli.whole = true,
            "--allow-project-setup" => cli.allow_project_setup = true,
            other if other.starts_with("--") => {
                return Err(Failure::misconfigured(format!(
                    "unknown option: {other}\n{}",
                    usage()
                )));
            }
            other => {
                if cli.command.is_some() {
                    return Err(Failure::misconfigured(format!(
                        "unknown argument: {other}\n{}",
                        usage()
                    )));
                }
                cli.command = Some(other.to_string());
            }
        }
    }
    Ok(cli)
}

fn value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Failure> {
    args.next()
        .ok_or_else(|| Failure::misconfigured(format!("{flag} requires a value")))
}

fn usage() -> String {
    String::from(
        "usage: devshell-poc <check|prepare|closure|run> [options] [-- <command>...]\n\
         \n\
         options:\n\
         \x20 --step <file>             a declared step, as TOML (identity, kind, installable)\n\
         \x20 --installable <ref>       the devShell installable, when no step file is given\n\
         \x20 --project <dir>           the checkout the installable is relative to (default: .)\n\
         \x20 --out <dir>               where the preparation is recorded\n\
         \x20 --workspace <dir>         the path the sandbox sees its work at (run only)\n\
         \x20 --bwrap <path>            the bubblewrap binary (default: bwrap on PATH)\n\
         \x20 --whole                   the whole store view instead of the closure\n\
         \x20 --allow-project-setup     consent to evaluate this project's code on the host\n\
         \x20 --require-path <path>     a path the sandbox must provide (repeatable)\n",
    )
}
