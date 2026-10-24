//! Fixture-driven tests for the conformance baseline checker.
//!
//! Every branch of `tackle-conformance-check` is exercised offline: the
//! inputs are committed report fixtures and the tests assert the checker's
//! exit code and output directly. No test reaches the network.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHECKER: &str = env!("CARGO_BIN_EXE_tackle-conformance-check");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/fixtures")
}

fn report_path(name: &str) -> PathBuf {
    fixtures().join(name)
}

fn load_report(name: &str) -> Value {
    let text = std::fs::read_to_string(report_path(name)).expect("read report fixture");
    serde_json::from_str(&text).expect("parse report fixture")
}

fn ids_with_status(report: &Value, tier: &str, status: Option<&str>) -> BTreeSet<String> {
    report["requirements"]
        .as_array()
        .expect("requirements array")
        .iter()
        .filter(|requirement| requirement["tier"] == tier)
        .filter(|requirement| {
            let actual = requirement["status"].as_str().expect("status");
            match status {
                None => actual != "PASS",
                Some(wanted) => actual == wanted,
            }
        })
        .map(|requirement| requirement["id"].as_str().expect("id").to_owned())
        .collect()
}

fn write_baseline(directory: &Path, ids: &BTreeSet<String>) -> PathBuf {
    let path = directory.join("baseline.txt");
    let mut body = String::from("# test baseline\n");
    for id in ids {
        body.push_str(id);
        body.push('\n');
    }
    std::fs::write(&path, body).expect("write baseline");
    path
}

fn run_checker(report: &Path, baseline: &Path) -> Output {
    Command::new(CHECKER)
        .arg("--report")
        .arg(report)
        .arg("--baseline")
        .arg(baseline)
        .output()
        .expect("run checker")
}

fn run_checker_seed(report: &Path, baseline: &Path) -> Output {
    Command::new(CHECKER)
        .arg("--report")
        .arg(report)
        .arg("--baseline")
        .arg(baseline)
        .arg("--seed")
        .output()
        .expect("run checker")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().expect("checker exited via signal")
}

#[test]
fn known_failures_are_tolerated() {
    let report = load_report("report_minimal.json");
    let failures = ids_with_status(&report, "MANDATORY", None);
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no new MANDATORY failures"));
}

#[test]
fn not_conformant_verdict_alone_does_not_fail() {
    let report = load_report("report_minimal.json");
    assert_eq!(report["verdict"]["conformant"].as_bool(), Some(false));
    let failures = ids_with_status(&report, "MANDATORY", None);
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("verdict: NOT CONFORMANT"));
}

#[test]
fn new_mandatory_failure_is_detected() {
    let report = load_report("report_minimal.json");
    let mut failures = ids_with_status(&report, "MANDATORY", None);
    let missing = "ACME-MAND-003";
    assert!(failures.remove(missing));
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 1);
    assert!(stderr(&output).contains(missing));
}

#[test]
fn capability_failure_does_not_fail() {
    let report = load_report("report_minimal.json");
    assert!(ids_with_status(&report, "CAPABILITY", Some("FAIL")).contains("ACME-CAP-001"));
    let failures = ids_with_status(&report, "MANDATORY", None);
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
}

#[test]
fn now_passing_baseline_id_is_reported_as_drift() {
    let report = load_report("report_minimal.json");
    let passing = "ACME-MAND-001";
    assert!(ids_with_status(&report, "MANDATORY", Some("PASS")).contains(passing));
    let mut baseline_ids = ids_with_status(&report, "MANDATORY", None);
    baseline_ids.insert(passing.to_owned());
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &baseline_ids);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("now pass"));
    assert!(stdout(&output).contains(passing));
}

#[test]
fn stale_baseline_id_is_reported_and_tolerated() {
    let report = load_report("report_minimal.json");
    let stale = "ACME-GONE-999";
    let mut baseline_ids = ids_with_status(&report, "MANDATORY", None);
    baseline_ids.insert(stale.to_owned());
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &baseline_ids);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no longer in the report"));
    assert!(stdout(&output).contains(stale));
}

#[test]
fn missing_baseline_errors_with_seeding_guidance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = dir.path().join("absent.txt");

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 2);
    assert!(stderr(&output).contains("baseline not found"));
    assert!(stderr(&output).contains("--seed"));
}

#[test]
fn missing_requirements_field_is_an_input_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("empty.json");
    std::fs::write(&report, "{}").expect("write report");
    let baseline = write_baseline(dir.path(), &BTreeSet::new());

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 2);
    assert!(stderr(&output).contains("missing field"));
    assert!(stderr(&output).contains("requirements"));
}

#[test]
fn seed_writes_the_current_failures() {
    let report = load_report("report_minimal.json");
    let failures = ids_with_status(&report, "MANDATORY", None);
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = dir.path().join("seeded.txt");

    let output = run_checker_seed(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));

    let seeded: BTreeSet<String> = std::fs::read_to_string(&baseline)
        .expect("read seeded baseline")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    assert_eq!(seeded, failures);
}

#[test]
fn real_report_known_failures_are_tolerated() {
    let report = load_report("report.json");
    let failures = ids_with_status(&report, "MANDATORY", None);
    assert!(
        !failures.is_empty(),
        "fixture should carry known MANDATORY gaps"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report.json"), &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no new MANDATORY failures"));
}
