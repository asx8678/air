#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn hidden_window_reads_only_its_digits() {
        assert_eq!(window("context length is 32k tokens"), Some(32_000));
        assert_eq!(window("the context window of 200,000"), Some(200_000));
        assert_eq!(window("no size here"), None);
        assert_eq!(window("context length of 4096,max_tokens=10"), Some(4096));
        assert_eq!(window("context length is 1 million tokens"), None);
    }
}
