fn main() {
    let out = std::process::Command::new("git").args(["describe", "--always", "--dirty"]).output();
    let build = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    // A crate from crates.io has no .git, but cargo package recorded the commit it was made from.
    let packaged = || {
        let info = std::fs::read_to_string(".cargo_vcs_info.json").ok()?;
        let sha = info.split("\"sha1\"").nth(1)?.split('"').nth(1)?;
        Some(sha.get(..7).unwrap_or(sha).to_string())
    };
    println!("cargo:rustc-env=LOW_BUILD={}", build.or_else(packaged).unwrap_or_else(|| "unknown".into()));
    for path in ["src", "build.rs", "Cargo.toml", ".git/HEAD", ".git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
