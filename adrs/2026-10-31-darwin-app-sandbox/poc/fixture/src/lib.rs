pub fn add(left: i32, right: i32) -> i32 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn reads_a_file_from_the_workspace() {
        let path = std::env::current_dir().expect("cwd").join("workspace.txt");
        std::fs::write(&path, "ok").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "ok");
    }
}
