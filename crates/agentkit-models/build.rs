use serde_json::{json, Map, Value};
use std::{convert::TryFrom, env, error::Error, fs, path::{Path, PathBuf}, time::Duration};

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

    if let Ok(url) = env::var("AGENTKIT_MODELS_DEV_URL") {
        let raw = fetch_snapshot(&url)?;
        match normalize_snapshot(&raw) {
            Ok(snapshot) => return Ok(snapshot),
            Err(err) => {
                eprintln!("agentkit-models: remote snapshot from {url} could not be normalized: {err}");
                return read_checked_in_snapshot(manifest_dir);
            }
        }
    }

    // Deterministic default: use checked-in snapshot. Network fetch only when
    // AGENTKIT_MODELS_DEV_URL or AGENTKIT_MODELS_DEV_SNAPSHOT is explicitly set.
    read_checked_in_snapshot(manifest_dir)
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

    // models.dev catalog.json: provider-agnostic model facts plus per-provider
    // serving details and pricing. Transformed into the ModelSnapshot shape.
    if value.get("models").and_then(|v| v.as_object()).is_some()
        && value.get("providers").and_then(|v| v.as_object()).is_some()
    {
        return normalize_catalog(&value);
    }

    // Legacy models.dev models.json: a `data` array of model records.
    if let Some(data) = value.get("data").and_then(|v| v.as_array()) {
        return normalize_legacy(data);
    }

    Err("models.dev payload has an unrecognized shape (expected a `data` array or `models`/`providers` objects)".into())
}

fn normalize_catalog(value: &Value) -> Result<Value, Box<dyn Error>> {
    let models_in = value
        .get("models")
        .and_then(|v| v.as_object())
        .ok_or("catalog payload is missing a `models` object")?;
    let providers_in = value
        .get("providers")
        .and_then(|v| v.as_object())
        .ok_or("catalog payload is missing a `providers` object")?;

    let mut models = Map::new();
    for (key, entry) in models_in {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let id = key.rsplit('/').next().unwrap_or(key).to_string();
        let mut model_entry = Map::new();
        if let Some(limit) = entry.get("limit").and_then(|v| v.as_object()) {
            if let Some(context) = limit.get("context").and_then(|v| v.as_u64()) {
                if let Ok(context_window) = u32::try_from(context) {
                    model_entry.insert("context_window".to_string(), json!(context_window));
                }
            }
            if let Some(output) = limit.get("output").and_then(|v| v.as_u64()) {
                if let Ok(max_output) = u32::try_from(output) {
                    model_entry.insert("max_output".to_string(), json!(max_output));
                }
            }
        }
        let capabilities = catalog_capabilities(entry);
        if !capabilities.is_empty() {
            model_entry.insert("capabilities".to_string(), Value::Object(capabilities));
        }
        models.insert(id, Value::Object(model_entry));
    }

    let mut providers = Map::new();
    for (prov_id, prov) in providers_in {
        let Some(prov) = prov.as_object() else {
            continue;
        };
        let mut prov_entry = Map::new();
        if let Some(billing) = prov.get("billing").and_then(|v| v.as_str()) {
            prov_entry.insert("billing".to_string(), json!(billing));
        }
        let mut prov_models = Map::new();
        if let Some(entry_models) = prov.get("models").and_then(|v| v.as_object()) {
            for (model_id, model_entry) in entry_models {
                let Some(model_entry) = model_entry.as_object() else {
                    continue;
                };
                if model_entry.get("status").and_then(|v| v.as_str()) == Some("deprecated") {
                    continue;
                }
                let mut pricing = Map::new();
                if let Some(cost) = model_entry.get("cost").and_then(|v| v.as_object()) {
                    insert_cost(&mut pricing, cost, "input", "input_per_mtok");
                    insert_cost(&mut pricing, cost, "output", "output_per_mtok");
                    insert_cost(&mut pricing, cost, "cache_read", "cache_read_per_mtok");
                    insert_cost(&mut pricing, cost, "cache_write", "cache_write_per_mtok");
                    insert_cost(&mut pricing, cost, "reasoning", "reasoning_per_mtok");
                }
                prov_models.insert(model_id.clone(), Value::Object(pricing));
            }
        }
        prov_entry.insert("models".to_string(), Value::Object(prov_models));
        providers.insert(prov_id.clone(), Value::Object(prov_entry));
    }

    Ok(json!({ "models": models, "providers": providers }))
}

fn insert_cost(
    out: &mut Map<String, Value>,
    cost: &Map<String, Value>,
    key: &str,
    out_key: &str,
) {
    if let Some(v) = cost.get(key).and_then(|v| v.as_f64()) {
        out.insert(out_key.to_string(), json!(v));
    }
}

fn catalog_capabilities(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut capabilities = Map::new();
    for (key, out_key) in [
        ("tool_call", "tool_calling"),
        ("reasoning", "reasoning"),
        ("structured_output", "structured_output"),
    ] {
        if let Some(b) = entry.get(key).and_then(|v| v.as_bool()) {
            capabilities.insert(out_key.to_string(), Value::Bool(b));
        }
    }
    capabilities
}

fn normalize_legacy(data: &[Value]) -> Result<Value, Box<dyn Error>> {
    let mut model_map = Map::new();
    for entry in data {
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
