use serde_json::{json, Map, Value};
use std::{convert::TryFrom, env, error::Error, fs, path::{Path, PathBuf}, time::Duration};

const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/models.json";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let snapshot_path = out_dir.join("models.dev.json");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/models.dev.json");
    println!("cargo:rerun-if-env-changed=AGENTKIT_MODELS_DEV_SNAPSHOT");
    println!("cargo:rerun-if-env-changed=AGENTKIT_MODELS_DEV_URL");

    let snapshot = load_snapshot(&manifest_dir).unwrap_or_else(|err| {
        eprintln!("agentkit-models: {err}; falling back to checked-in snapshot");
        read_checked_in_snapshot(&manifest_dir)
            .unwrap_or_else(|fallback_err| panic!("failed to load fallback snapshot: {fallback_err}"))
    });

    fs::create_dir_all(&out_dir).unwrap();
    fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
    println!("cargo:rustc-env=AGENTKIT_MODELS_DEV_JSON={}", snapshot_path.display());
}

fn load_snapshot(manifest_dir: &Path) -> Result<Value, Box<dyn Error>> {
    if let Ok(path) = env::var("AGENTKIT_MODELS_DEV_SNAPSHOT") {
        return normalize_snapshot(&fs::read_to_string(path)?);
    }

    let url = env::var("AGENTKIT_MODELS_DEV_URL").unwrap_or_else(|_| DEFAULT_MODELS_DEV_URL.to_string());
    let raw = fetch_snapshot(&url)?;
    match normalize_snapshot(&raw) {
        Ok(snapshot) => Ok(snapshot),
        Err(err) => {
            eprintln!("agentkit-models: remote snapshot could not be normalized: {err}");
            read_checked_in_snapshot(manifest_dir)
        }
    }
}

fn fetch_snapshot(url: &str) -> Result<String, Box<dyn Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let response = client.get(url).send()?.error_for_status()?;
    Ok(response.text()?)
}

fn read_checked_in_snapshot(manifest_dir: &Path) -> Result<Value, Box<dyn Error>> {
    normalize_snapshot(&fs::read_to_string(manifest_dir.join("data/models.dev.json"))?)
}

fn normalize_snapshot(raw: &str) -> Result<Value, Box<dyn Error>> {
    let value: Value = serde_json::from_str(raw)?;
    if value.get("models").and_then(|v| v.as_object()).is_some() {
        return Ok(value);
    }

    let models = value
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("models.dev payload is missing a `data` array")?;

    let mut model_map = Map::new();
    for entry in models {
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };

        let mut model_entry = Map::new();
        if let Some(context_length) = entry.get("context_length").and_then(|v| v.as_u64()) {
            if let Ok(context_window) = u32::try_from(context_length) {
                model_entry.insert("context_window".to_string(), json!(context_window));
            }
        }

        if let Some(max_output) = entry
            .get("top_provider")
            .and_then(|v| v.get("max_completion_tokens"))
            .and_then(|v| v.as_u64())
        {
            if let Ok(max_output) = u32::try_from(max_output) {
                model_entry.insert("max_output".to_string(), json!(max_output));
            }
        }

        let capabilities = infer_capabilities(entry);
        if !capabilities.is_empty() {
            model_entry.insert("capabilities".to_string(), Value::Object(capabilities));
        }

        model_map.insert(id.to_string(), Value::Object(model_entry));
    }

    Ok(json!({
        "models": model_map,
        "providers": {},
    }))
}

fn infer_capabilities(entry: &Value) -> Map<String, Value> {
    let mut capabilities = Map::new();
    let supported_parameters = entry
        .get("supported_parameters")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let has_param = |needle: &str| {
        supported_parameters.iter().any(|value| value.as_str() == Some(needle))
    };

    if has_param("tools") || has_param("tool_choice") {
        capabilities.insert("tool_calling".to_string(), Value::Bool(true));
    }

    if has_param("reasoning") || has_param("include_reasoning") {
        capabilities.insert("reasoning".to_string(), Value::Bool(true));
    }

    if has_param("structured_outputs") || has_param("response_format") {
        capabilities.insert("structured_output".to_string(), Value::Bool(true));
    }

    capabilities
}
