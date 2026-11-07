//! Host readiness: the checks that mutate nothing.
//!
//! Readiness answers "can this host run the step at all". It never evaluates the
//! installable and never runs the hook: both write to the store, and both belong
//! to preparation, which reports its own failure with the same structure.

use std::path::Path;

use crate::failure::{Class, Failure};
use crate::util::{Caller, argv, run};

/// One host check's outcome, structured so a test can assert its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub class: Class,
    pub ok: bool,
    pub detail: String,
    pub remedy: Option<String>,
}

impl Check {
    fn holds(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            class: Class::Unavailable,
            ok: true,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn missing(
        name: &'static str,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            name,
            class: Class::Unavailable,
            ok: false,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// The host requirements, in the order a failure is reported.
///
/// Every check here is a query: `nix --version`, the configured experimental
/// features, and whether the store answers. None of them evaluates a flake.
pub fn readiness(nix: &Path, caller: &Caller) -> Vec<Check> {
    let mut checks = Vec::new();

    let version = run(nix, &argv(&["--version"]), caller, None);
    let nix_check = match &version {
        Ok(output) if output.success() => {
            Check::holds("nix", String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(output) => Check::missing(
            "nix",
            output.stderr_excerpt(),
            "install nix and put it on PATH",
        ),
        Err(failure) => Check::missing(
            "nix",
            failure.reason.clone(),
            "install nix and put it on PATH",
        ),
    };
    let nix_present = nix_check.ok;
    checks.push(nix_check);
    if !nix_present {
        return checks;
    }

    let features = run(
        nix,
        &argv(&["config", "show", "experimental-features"]),
        caller,
        None,
    );
    checks.push(match &features {
        Ok(output) if output.success() => {
            let shown = String::from_utf8_lossy(&output.stdout).to_string();
            if flakes_enabled(&shown) {
                Check::holds("flakes", shown.trim().to_string())
            } else {
                Check::missing(
                    "flakes",
                    format!("flakes are not in the experimental features: {}", shown.trim()),
                    "set experimental-features = nix-command flakes in nix.conf",
                )
            }
        }
        Ok(output) => Check::missing(
            "flakes",
            output.stderr_excerpt(),
            "set experimental-features = nix-command flakes in nix.conf",
        ),
        Err(failure) => Check::missing(
            "flakes",
            failure.reason.clone(),
            "set experimental-features = nix-command flakes in nix.conf",
        ),
    });

    let store = run(nix, &argv(&["store", "ping"]), caller, None);
    checks.push(match &store {
        Ok(output) if output.success() => {
            let detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if detail.is_empty() {
                output.stderr_excerpt()
            } else {
                detail
            };
            Check::holds("store", detail)
        }
        Ok(output) => Check::missing(
            "store",
            output.stderr_excerpt(),
            "start the nix daemon or check the store URL",
        ),
        Err(failure) => Check::missing(
            "store",
            failure.reason.clone(),
            "start the nix daemon or check the store URL",
        ),
    });

    checks
}

/// Whether the experimental features list contains flakes.
pub fn flakes_enabled(shown: &str) -> bool {
    shown.split_whitespace().any(|feature| feature == "flakes")
}

/// The first failed check, as the failure create reports.
pub fn refusal(checks: &[Check]) -> Option<Failure> {
    checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| {
            let mut failure = Failure::new(check.class, format!("{} is not ready: {}", check.name, check.detail))
                .with_primitive(check.name);
            if let Some(remedy) = &check.remedy {
                failure = failure.with_remedy(remedy.clone());
            }
            failure
        })
}

pub fn report(checks: &[Check]) -> String {
    let mut rendered = String::new();
    for check in checks {
        let mark = if check.ok { "ok" } else { "MISSING" };
        rendered.push_str(&format!("{mark:8} {}: {}\n", check.name, check.detail));
        if let Some(remedy) = &check.remedy {
            rendered.push_str(&format!("         remedy: {remedy}\n"));
        }
    }
    rendered
}

/// The same report, as JSON, so the doctor's output can be read by a program.
pub fn report_json(checks: &[Check]) -> String {
    let checks: Vec<serde_json::Value> = checks
        .iter()
        .map(|check| {
            serde_json::json!({
                "name": check.name,
                "class": check.class.as_str(),
                "ok": check.ok,
                "detail": check.detail,
                "remedy": check.remedy,
            })
        })
        .collect();
    serde_json::to_string_pretty(&checks).unwrap_or_else(|_| "[]".to_string())
}

/// The output path print-dev-env reports, if any.
pub fn output_path(json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    value
        .get("variables")?
        .get("out")?
        .get("value")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flakes_are_read_from_the_feature_list() {
        assert!(flakes_enabled("nix-command flakes"));
        assert!(flakes_enabled("flakes nix-command\n"));
        assert!(!flakes_enabled("nix-command"));
        assert!(!flakes_enabled(""));
        assert!(
            !flakes_enabled("ca-derivations"),
            "a feature that merely mentions flakes is not flakes"
        );
    }

    #[test]
    fn a_reported_check_carries_its_class_and_remedy() {
        let checks = vec![Check::missing("flakes", "disabled", "enable them")];
        let failure = refusal(&checks).expect("a refusal");
        assert_eq!(failure.class, Class::Unavailable);
        assert_eq!(failure.primitive.as_deref(), Some("flakes"));
        assert_eq!(failure.remedy.as_deref(), Some("enable them"));
        assert!(failure.reason.contains("disabled"));
    }

    #[test]
    fn a_ready_host_has_no_refusal() {
        let checks = vec![Check::holds("nix", "nix (Nix) 2.34.9")];
        assert!(refusal(&checks).is_none());
    }

    #[test]
    fn the_report_marks_each_check_and_its_remedy() {
        let checks = vec![
            Check::holds("nix", "nix (Nix) 2.34.9"),
            Check::missing("flakes", "disabled", "enable them"),
        ];
        let rendered = report(&checks);
        assert!(rendered.contains("ok       nix: nix (Nix) 2.34.9"), "{rendered}");
        assert!(rendered.contains("MISSING  flakes: disabled"), "{rendered}");
        assert!(rendered.contains("remedy: enable them"), "{rendered}");
    }

    #[test]
    fn the_report_is_also_machine_readable() {
        let checks = vec![Check::missing("flakes", "disabled", "enable them")];
        let rendered = report_json(&checks);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("json");
        assert_eq!(parsed[0]["name"], "flakes");
        assert_eq!(parsed[0]["class"], "unavailable");
        assert_eq!(parsed[0]["ok"], false);
        assert_eq!(parsed[0]["remedy"], "enable them");
    }

    #[test]
    fn the_output_path_is_read_from_the_out_variable() {
        let json = br#"{"variables": {"out": {"type": "exported", "value": "/nix/store/abc-dev"}}}"#;
        assert_eq!(output_path(json).as_deref(), Some("/nix/store/abc-dev"));
    }

    #[test]
    fn a_missing_or_non_string_output_path_is_none() {
        assert!(output_path(br#"{"variables": {}}"#).is_none());
        assert!(output_path(br#"{"variables": {"out": {"value": 7}}}"#).is_none());
        assert!(output_path(b"not json").is_none());
    }
}
