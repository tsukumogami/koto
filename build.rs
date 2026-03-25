fn main() {
    // Version from git tags.
    // On an exact tag (v0.2.0): "0.2.0"
    // Ahead of a tag (3 commits after v0.2.0): "0.2.0-dev+abc1234"
    // No tags at all: "dev+abc1234"
    let version = git_version();
    println!("cargo:rustc-env=KOTO_VERSION={}", version);

    // Git hash (short, for the commit field)
    let hash = run_git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=KOTO_GIT_HASH={}", hash);

    // Build date
    let date = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=KOTO_BUILD_DATE={}", date);

    // Rebuild when the git state changes (new commits, new tags, branch switch).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}

/// Derive a version string from git tags.
fn git_version() -> String {
    let hash = run_git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());

    // Try git describe with tags.
    let describe = run_git(&["describe", "--tags", "--always"]);

    match describe {
        Some(desc) => {
            if let Some(tag) = desc.strip_prefix('v') {
                // Exact tag match: "v0.2.0" -> "0.2.0"
                if !tag.contains('-') {
                    return tag.to_string();
                }
                // Ahead of tag: "v0.2.0-3-gabc1234" -> "0.2.0-dev+abc1234"
                if let Some(base) = tag.split('-').next() {
                    return format!("{}-dev+{}", base, hash);
                }
            }
            // Fallback: no recognized tag pattern
            format!("dev+{}", hash)
        }
        None => format!("dev+{}", hash),
    }
}

/// Run a git command and return trimmed stdout, or None on failure.
fn run_git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(o.stdout)
            } else {
                None
            }
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
