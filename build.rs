fn main() {
    // Git hash
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=KOTO_GIT_HASH={}", hash);

    // Build date
    let date = std::process::Command::new("date")
        .args(["+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=KOTO_BUILD_DATE={}", date);

    println!("cargo:rerun-if-changed=.git/HEAD");
}
