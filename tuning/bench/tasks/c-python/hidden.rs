#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn hidden_picks_the_newest_runnable_python() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("air-hidden-python-{}", std::process::id()));
        let (first, second) = (base.join("a"), base.join("b"));
        for (dir, name, mode) in [
            (&first, "python3.9", 0o755),
            (&first, "python3.12", 0o755),
            (&first, "python3.12-config", 0o755),
            (&first, "python3.13", 0o644),
            (&second, "python3.12", 0o755),
            (&second, "python3", 0o755),
            (&second, "python3.10", 0o755),
        ] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(name), "").unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let found = newest_python([first.clone(), second.clone()].into_iter());
        assert_eq!(found, Some(first.join("python3.12")));
        assert_eq!(newest_python([second.clone(), first.clone()].into_iter()), Some(second.join("python3.12")));
        assert_eq!(newest_python([base.join("missing")].into_iter()), None);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
