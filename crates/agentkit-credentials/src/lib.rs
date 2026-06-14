pub fn read_stdin() -> Result<serde_json::Value, String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str(&input).map_err(|e| format!("failed to parse JSON: {e}"))
}
