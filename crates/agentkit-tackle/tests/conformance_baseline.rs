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

/// Writes a temp copy of a fixture that is CONFORMANT and has no failing
/// advertised capabilities, so the MANDATORY-baseline branches are reachable.
fn conformant_report(name: &str, directory: &Path) -> PathBuf {
    let mut report = load_report(name);
    report["verdict"]["conformant"] = Value::Bool(true);
    for requirement in report["requirements"]
        .as_array_mut()
        .expect("requirements array")
    {
        if requirement["tier"] == "CAPABILITY" && requirement["status"] == "FAIL" {
            requirement["status"] = Value::String("PASS".to_owned());
        }
    }
    let path = directory.join("conformant.json");
    std::fs::write(&path, serde_json::to_string(&report).expect("serialize report"))
        .expect("write report");
    path
}

/// Writes a temp copy that is CONFORMANT but still has the fixture's
/// advertised-capability failure.
fn conformant_report_with_capability_failure(name: &str, directory: &Path) -> PathBuf {
    let mut report = load_report(name);
    report["verdict"]["conformant"] = Value::Bool(true);
    let path = directory.join("capability-failure.json");
    std::fs::write(&path, serde_json::to_string(&report).expect("serialize report"))
        .expect("write report");
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

fn run_checker_version(report: &Path, baseline: &Path, version: u64) -> Output {
    Command::new(CHECKER)
        .arg("--report")
        .arg(report)
        .arg("--baseline")
        .arg(baseline)
        .arg("--version")
        .arg(version.to_string())
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
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let failures = ids_with_status(&load_report("report_minimal.json"), "MANDATORY", Some("FAIL"));
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no new MANDATORY failures"));
}

#[test]
fn not_conformant_verdict_fails() {
    let report = load_report("report_minimal.json");
    assert_eq!(report["verdict"]["conformant"].as_bool(), Some(false));
    let failures = ids_with_status(&report, "MANDATORY", Some("FAIL"));
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report_path("report_minimal.json"), &baseline);
    assert_eq!(exit_code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("NOT CONFORMANT"));
}

#[test]
fn new_mandatory_failure_is_detected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let mut failures = ids_with_status(&load_report("report_minimal.json"), "MANDATORY", Some("FAIL"));
    let missing = "ACME-MAND-002";
    assert!(failures.remove(missing));
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 1);
    assert!(stderr(&output).contains(missing));
}

#[test]
fn capability_failure_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report_with_capability_failure("report_minimal.json", dir.path());
    assert!(ids_with_status(&load_report("report_minimal.json"), "CAPABILITY", Some("FAIL"))
        .contains("ACME-CAP-001"));
    let failures = ids_with_status(&load_report("report_minimal.json"), "MANDATORY", Some("FAIL"));
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("advertised-capability"));
}

#[test]
fn now_passing_baseline_id_is_reported_as_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let passing = "ACME-MAND-001";
    let mut baseline_ids = ids_with_status(&load_report("report_minimal.json"), "MANDATORY", Some("FAIL"));
    baseline_ids.insert(passing.to_owned());
    let baseline = write_baseline(dir.path(), &baseline_ids);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("now pass"));
    assert!(stdout(&output).contains(passing));
}

#[test]
fn stale_baseline_id_is_reported_and_tolerated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let stale = "ACME-GONE-999";
    let mut baseline_ids = ids_with_status(&load_report("report_minimal.json"), "MANDATORY", Some("FAIL"));
    baseline_ids.insert(stale.to_owned());
    let baseline = write_baseline(dir.path(), &baseline_ids);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no longer in the report"));
    assert!(stdout(&output).contains(stale));
}

#[test]
fn version_mismatch_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let baseline = write_baseline(dir.path(), &BTreeSet::new());

    let output = run_checker_version(&report, &baseline, 2);
    assert_eq!(exit_code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("protocol version mismatch"));
}

#[test]
fn blocked_by_version_mismatch_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    value["verdict"]["blocked_by_version_mismatch"] = Value::Bool(true);
    std::fs::write(&report, serde_json::to_string(&value).unwrap()).unwrap();
    let baseline = write_baseline(dir.path(), &BTreeSet::new());

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("version mismatch"));
}

#[test]
fn blocked_by_auth_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    value["verdict"]["blocked_by_auth"] = Value::Bool(true);
    std::fs::write(&report, serde_json::to_string(&value).unwrap()).unwrap();
    let baseline = write_baseline(dir.path(), &BTreeSet::new());

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("authentication"));
}

#[test]
fn missing_baseline_errors_with_seeding_guidance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report_minimal.json", dir.path());
    let baseline = dir.path().join("absent.txt");

    let output = run_checker(&report, &baseline);
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
    let failures = ids_with_status(&report, "MANDATORY", Some("FAIL"));
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
    let dir = tempfile::tempdir().expect("tempdir");
    let report = conformant_report("report.json", dir.path());
    let failures = ids_with_status(&load_report("report.json"), "MANDATORY", Some("FAIL"));
    assert!(
        !failures.is_empty(),
        "fixture should carry known MANDATORY gaps"
    );
    let baseline = write_baseline(dir.path(), &failures);

    let output = run_checker(&report, &baseline);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("no new MANDATORY failures"));
}
