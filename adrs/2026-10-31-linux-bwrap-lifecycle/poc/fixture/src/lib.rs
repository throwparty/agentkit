pub fn value() -> i32 {
    5
}

#[cfg(test)]
mod tests {
    #[test]
    fn value_is_five() {
        assert_eq!(super::value(), 5);
    }
}
