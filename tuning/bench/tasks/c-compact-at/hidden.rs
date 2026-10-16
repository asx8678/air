#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn hidden_profiles_need_a_usable_compact_at() {
        let check = |text: &str| check_profiles(&toml::from_str(text).expect("valid toml"));
        assert!(check("[models.m]\ncompact_at = 200000\ntemperature = 1.0\n").is_ok());
        assert!(check("[models.m]\ncompact_at = 1024\n").is_ok());
        for bad in ["0", "100", "\"200k\"", "2.5e5"] {
            let e = check(&format!("[models.m]\ncompact_at = {bad}\n")).unwrap_err().to_string();
            assert!(e.contains("compact_at"), "{bad}: {e}");
        }
        assert!(check("[models.m]\nturn_tokens = 5\n").is_err());
    }
}
