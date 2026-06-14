pub fn read(var_name: &str) -> Option<String> {
    std::env::var(var_name).ok()
}
