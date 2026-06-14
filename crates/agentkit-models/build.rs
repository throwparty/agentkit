use std::path::Path;

fn main() {
    let out_dir = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("data");
    std::fs::create_dir_all(&out_dir).unwrap();
    let snapshot_path = out_dir.join("models.dev.json");

    let snapshot = serde_json::json!({
        "models": {},
        "providers": {}
    });

    std::fs::write(&snapshot_path, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
