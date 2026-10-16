fn main() {
    let out = std::process::Command::new("git").args(["describe", "--always", "--dirty"]).output();
    let build = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    println!("cargo:rustc-env=LOW_BUILD={}", build.unwrap_or_else(|| "unknown".into()));
    for path in ["src", "build.rs", "Cargo.toml", ".git/HEAD", ".git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
