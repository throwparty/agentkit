//! Diff an acp-tck JSON report against the committed conformance baseline.
//!
//! `just tackle conformance` writes a JSON report at a fixed path. This
//! binary reads it, extracts every MANDATORY requirement that failed, and
//! diffs that set against the committed baseline.
//!
//! The exit status is non-zero when a MANDATORY failure is absent from the
//! baseline, when the report negotiated a different protocol version than the
//! one under test, when the verdict is blocked by a version mismatch or
//! authentication, when the verdict is NOT CONFORMANT, or when an advertised
//! capability failed. Baseline ids that now pass or that no longer appear are
//! reported as drift and tolerated. The report is read-only here.

use clap::Parser;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EXIT_NEW_FAILURE: u8 = 1;
const EXIT_ERROR: u8 = 2;

#[derive(Debug, Parser)]
struct Args {
    /// Path to the acp-tck JSON report.
    #[arg(long)]
    report: PathBuf,

    /// Path to the committed baseline.
    #[arg(long)]
    baseline: PathBuf,

    /// Write the baseline from this report's current MANDATORY failures.
    #[arg(long)]
    seed: bool,

    /// Assert the report negotiated this protocol version.
    #[arg(long)]
    version: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Report {
    requirements: Vec<Requirement>,
    #[serde(default)]
    protocol_version: u64,
    #[serde(default)]
    verdict: Verdict,
}

#[derive(Debug, Deserialize)]
struct Requirement {
    id: String,
    tier: String,
    status: String,
}

#[derive(Debug, Default, Deserialize)]
struct Verdict {
    #[serde(default)]
    conformant: bool,
    #[serde(default)]
    blocked_by_version_mismatch: bool,
    #[serde(default)]
    blocked_by_auth: bool,
    #[serde(default)]
    tier_counts: BTreeMap<String, BTreeMap<String, u64>>,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn run(args: Args) -> Result<u8, String> {
    let report = load_report(&args.report)?;
    summarise(&report);

    let failures = mandatory_failures(&report);

    if args.seed {
        write_baseline(&args.baseline, &failures)?;
        println!(
            "seeded {} with {} MANDATORY failure(s)",
            args.baseline.display(),
            failures.len()
        );
        return Ok(0);
    }

    let baseline = load_baseline(&args.baseline)?;
    let statuses = requirement_statuses(&report);

    let new_failures: BTreeSet<&String> = failures.difference(&baseline).collect();
    let now_passing: Vec<&String> = baseline
        .iter()
        .filter(|id| statuses.get(id.as_str()) == Some(&"PASS"))
        .collect();
    let stale: Vec<&String> = baseline
        .iter()
        .filter(|id| !statuses.contains_key(id.as_str()))
        .collect();

    if !now_passing.is_empty() {
        println!("\ndrift: baseline ids that now pass (tighten the baseline):");
        for id in &now_passing {
            println!("  {id}");
        }
    }
    if !stale.is_empty() {
        println!("\ndrift: baseline ids no longer in the report (tolerated):");
        for id in &stale {
            println!("  {id}");
        }
    }

    // The verdict and its blocking signals are acceptance criteria, not
    // advisory: a run that negotiated the wrong version, was auth-blocked,
    // or failed an advertised capability must not pass just because its
    // MANDATORY failures are baselined.
    if let Some(expected) = args.version {
        if report.protocol_version != expected {
            eprintln!(
                "\nprotocol version mismatch: report negotiated {}, expected {expected}",
                report.protocol_version
            );
            return Ok(EXIT_NEW_FAILURE);
        }
    }
    if report.verdict.blocked_by_version_mismatch {
        eprintln!("\nverdict is blocked by a version mismatch");
        return Ok(EXIT_NEW_FAILURE);
    }
    if report.verdict.blocked_by_auth {
        eprintln!("\nverdict is blocked by authentication");
        return Ok(EXIT_NEW_FAILURE);
    }
    if !report.verdict.conformant {
        eprintln!("\nverdict is NOT CONFORMANT");
        return Ok(EXIT_NEW_FAILURE);
    }
    let capability_failures: BTreeSet<&String> = report
        .requirements
        .iter()
        .filter(|requirement| requirement.tier == "CAPABILITY" && requirement.status == "FAIL")
        .map(|requirement| &requirement.id)
        .collect();
    if !capability_failures.is_empty() {
        eprintln!("\nadvertised-capability failures:");
        for id in &capability_failures {
            eprintln!("  {id}");
        }
        return Ok(EXIT_NEW_FAILURE);
    }

    if !new_failures.is_empty() {
        eprintln!("\nnew MANDATORY failures absent from the baseline:");
        for id in &new_failures {
            eprintln!("  {id}");
        }
        return Ok(EXIT_NEW_FAILURE);
    }

    println!("\nbaseline diff: no new MANDATORY failures");
    Ok(0)
}

fn load_report(path: &Path) -> Result<Report, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "report not found: {}: {error}\nrun the conformance recipe first",
            path.display()
        )
    })?;
    serde_json::from_str(&text)
        .map_err(|error| format!("report is not valid JSON: {}: {error}", path.display()))
}

fn summarise(report: &Report) {
    let label = if report.verdict.conformant {
        "CONFORMANT"
    } else {
        "NOT CONFORMANT"
    };
    println!("verdict: {label}");
    for (tier, counts) in &report.verdict.tier_counts {
        let pairs = counts
            .iter()
            .map(|(status, count)| format!("{status}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {tier}: {pairs}");
    }
}

fn mandatory_failures(report: &Report) -> BTreeSet<String> {
    report
        .requirements
        .iter()
        .filter(|requirement| requirement.tier == "MANDATORY" && requirement.status == "FAIL")
        .map(|requirement| requirement.id.clone())
        .collect()
}

fn requirement_statuses(report: &Report) -> BTreeMap<&str, &str> {
    report
        .requirements
        .iter()
        .map(|requirement| (requirement.id.as_str(), requirement.status.as_str()))
        .collect()
}

fn load_baseline(path: &Path) -> Result<BTreeSet<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "baseline not found: {}: {error}\n\
             seed it from a reviewed report with:\n  \
             tackle-conformance-check --report <report.json> --baseline <baseline.txt> --seed",
            path.display()
        )
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect())
}

fn write_baseline(path: &Path, failures: &BTreeSet<String>) -> Result<(), String> {
    let mut lines = vec![
        "# MANDATORY acp-tck requirements expected to fail.".to_owned(),
        "# Known gaps, to be burned down. Regenerate with --seed after review.".to_owned(),
        String::new(),
    ];
    lines.extend(failures.iter().cloned());
    std::fs::write(path, lines.join("\n") + "\n")
        .map_err(|error| format!("cannot write baseline {}: {error}", path.display()))
}
