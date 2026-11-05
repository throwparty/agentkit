//! Mapping a contract failure onto the MCP wire error.
//!
//! rmcp has three codes where the contract distinguishes seven failures, so the
//! finer class and the provisioning fields travel in the data payload.

use rmcp::ErrorData as McpError;

use crate::config::ConfigError;
use crate::sandbox::contract::Error;
use crate::sandbox::mapping::SurfaceClass;

/// Map a configuration-loading failure, keeping a provisioning issue structured.
pub(super) fn map_config(error: ConfigError) -> McpError {
    match error {
        ConfigError::Provisioning(issue) => {
            let mut data = serde_json::json!({
                "class": "misconfigured",
                "reason": issue.reason,
                "remedy": issue.remedy,
            });
            if let Some(backend) = issue.backend {
                data["backend"] = serde_json::Value::String(backend);
            }
            if let Some(step) = issue.step {
                data["step"] = serde_json::Value::String(step);
            }
            McpError::internal_error(
                format!(
                    "Provisioning configuration: {}",
                    data["reason"].as_str().unwrap_or("")
                ),
                Some(data),
            )
        }
        other => {
            let data = serde_json::json!({
                "class": "misconfigured",
                "reason": other.to_string(),
            });
            McpError::internal_error(other.to_string(), Some(data))
        }
    }
}

pub(super) fn map_contract(error: Error) -> McpError {
    let mut data = serde_json::json!({ "class": error.class().as_str() });
    if let Some(failure) = error.provisioning_failure() {
        if let Some(backend) = &failure.backend {
            data["backend"] = serde_json::Value::String(backend.clone());
        }
        if let Some(step) = &failure.step {
            data["step"] = serde_json::Value::String(step.clone());
        }
        if let Some(kind) = &failure.kind {
            data["kind"] = serde_json::Value::String(kind.clone());
        }
        data["reason"] = serde_json::Value::String(failure.reason.clone());
        if !failure.remedy.is_empty() {
            data["remedy"] = serde_json::Value::String(failure.remedy.clone());
        }
        if let Some(exit_code) = failure.exit_code {
            data["exit_code"] = serde_json::Value::from(exit_code);
        }
        if let Some(output) = &failure.output {
            // Step output is project-controlled. Escape it and mark it so an
            // agent does not treat it as orchestrator-authored text.
            let escaped: String = output.chars().flat_map(char::escape_default).collect();
            data["output"] = serde_json::Value::String(escaped);
            data["output_untrusted"] = serde_json::Value::Bool(true);
        }
    }
    match error.surface_class() {
        SurfaceClass::InvalidParams => McpError::invalid_params(error.to_string(), Some(data)),
        SurfaceClass::InvalidRequest => McpError::invalid_request(error.to_string(), Some(data)),
        SurfaceClass::Internal => McpError::internal_error(error.to_string(), Some(data)),
    }
}

#[cfg(test)]
mod tests {
    use super::map_config;
    use crate::config::{ConfigError, ProvisioningIssue};

    #[test]
    fn a_config_load_provisioning_issue_keeps_its_fields() {
        let error = map_config(ConfigError::Provisioning(ProvisioningIssue {
            backend: Some("docker".to_string()),
            step: Some("fetch".to_string()),
            reason: "the step is not defined".to_string(),
            remedy: "define it".to_string(),
        }));
        let data = error.data.as_ref().expect("data");
        assert_eq!(data["class"], "misconfigured");
        assert_eq!(data["backend"], "docker");
        assert_eq!(data["step"], "fetch");
        assert_eq!(data["reason"], "the step is not defined");
        assert_eq!(data["remedy"], "define it");
    }
}
