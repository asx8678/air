#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn hidden_an_overflow_carries_its_window() {
        let e = ContextOverflow::new("400: This model's maximum context length is 131,072 tokens".into());
        assert_eq!(e.window, Some(131_072));
        assert!(e.to_string().starts_with("context window exceeded: 400"));
        assert_eq!(ContextOverflow::new("413: request too large".into()).window, None);
        assert_eq!(window("context length is 32k tokens"), Some(32_000));
    }
}
