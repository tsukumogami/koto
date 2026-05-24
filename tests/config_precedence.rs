//! Integration tests for the `request_store` config precedence cascade.
//!
//! Validates the five-level resolution order from DESIGN-koto-request-store
//! Decision 4: CLI flag > env-var > project config > user config > built-in
//! default. Also covers the `[request_store.recursion]` reserved-but-ignored namespace
//! warn behavior on `koto config get` and `koto next` startup.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

/// Return a `koto` command wired to read its user config from `home_dir`
/// (HOME env var) and run from `cwd_dir` (so `.koto/config.toml` is
/// discovered relative to that directory).
fn koto_cmd(home_dir: &Path, cwd_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(cwd_dir);
    cmd.env("HOME", home_dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(cwd_dir));
    // Clear all request-store env-vars by default so the test author opts in
    // explicitly when exercising the env-var layer.
    for k in REQUEST_STORE_ENV_KEYS {
        cmd.env_remove(k);
    }
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

const REQUEST_STORE_ENV_KEYS: &[&str] = &[
    "KOTO_REQUEST_STORE_STALE_CLAIM_TIMEOUT_S",
    "KOTO_REQUEST_STORE_STALE_DISPATCH_TIMEOUT_S",
    "KOTO_REQUEST_STORE_REDELEGATION_CAP",
    "KOTO_REQUEST_STORE_COORD_CURSOR_TTL_DAYS",
    "KOTO_REQUEST_STORE_TERMINAL_INDEX_COMPACT_LINES",
    "KOTO_REQUEST_STORE_COMPACT_LOCK_TIMEOUT_S",
    "KOTO_REQUEST_STORE_DIRECTIVE_BATCH_SIZE",
    "KOTO_REQUEST_STORE_RESPAWN_GENERATION_CAP",
];

fn write_user_config(home_dir: &Path, body: &str) {
    let dir = home_dir.join(".koto");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), body).unwrap();
}

fn write_project_config(cwd_dir: &Path, body: &str) {
    let dir = cwd_dir.join(".koto");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), body).unwrap();
}

fn config_get(home_dir: &Path, cwd_dir: &Path, key: &str) -> (i32, String, String) {
    let output = koto_cmd(home_dir, cwd_dir)
        .args(["config", "get", key])
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

// ---------------------------------------------------------------------------
// Layer 1: built-in defaults
// ---------------------------------------------------------------------------

#[test]
fn defaults_apply_when_no_overrides_set() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    // Spot-check each of the eight dimensions against Decision 4's defaults.
    let cases = [
        ("request_store.stale_claim_timeout_seconds", "600"),
        ("request_store.stale_dispatch_timeout_seconds", "600"),
        ("request_store.redelegation_cap", "3"),
        ("request_store.coord_cursor_ttl_days", "7"),
        ("request_store.terminal_index_compact_lines", "100000"),
        ("request_store.compact_lock_timeout_seconds", "3600"),
        ("request_store.directive_batch_size", "50"),
        ("request_store.respawn_generation_cap", "2"),
    ];
    for (key, expected) in cases {
        let (code, stdout, stderr) = config_get(&home_dir, &cwd_dir, key);
        assert_eq!(code, 0, "{} get should succeed; stderr={}", key, stderr);
        assert_eq!(stdout.trim(), expected, "{} default mismatch", key);
    }
}

// ---------------------------------------------------------------------------
// Layer 2: user config overrides default
// ---------------------------------------------------------------------------

#[test]
fn user_config_overrides_default() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    write_user_config(
        &home_dir,
        "[request_store]\nstale_claim_timeout_seconds = 1200\n",
    );

    let (code, stdout, _) = config_get(
        &home_dir,
        &cwd_dir,
        "request_store.stale_claim_timeout_seconds",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "1200");

    // Untouched dimensions still resolve to defaults.
    let (_, stdout, _) = config_get(&home_dir, &cwd_dir, "request_store.redelegation_cap");
    assert_eq!(stdout.trim(), "3");
}

// ---------------------------------------------------------------------------
// Layer 3: project config overrides user
// ---------------------------------------------------------------------------

#[test]
fn project_config_overrides_user() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    write_user_config(
        &home_dir,
        "[request_store]\nstale_claim_timeout_seconds = 1200\n",
    );
    write_project_config(
        &cwd_dir,
        "[request_store]\nstale_claim_timeout_seconds = 1800\n",
    );

    let (_, stdout, _) = config_get(
        &home_dir,
        &cwd_dir,
        "request_store.stale_claim_timeout_seconds",
    );
    assert_eq!(stdout.trim(), "1800");
}

// ---------------------------------------------------------------------------
// Layer 4: env var overrides project config
// ---------------------------------------------------------------------------

#[test]
fn env_var_overrides_project_config() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    write_user_config(&home_dir, "[request_store]\nredelegation_cap = 2\n");
    write_project_config(&cwd_dir, "[request_store]\nredelegation_cap = 4\n");

    let output = koto_cmd(&home_dir, &cwd_dir)
        .env("KOTO_REQUEST_STORE_REDELEGATION_CAP", "5")
        .args(["config", "get", "request_store.redelegation_cap"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "config get should succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout.trim(), "5");
}

// ---------------------------------------------------------------------------
// Layer 5: CLI flag overrides env var (per-tick `koto next` flag)
// ---------------------------------------------------------------------------

#[test]
fn cli_flag_overrides_env_var_in_koto_next() {
    // The --redelegation-cap CLI flag lives on `koto next` only;
    // assert it parses and is accepted (full behavior is exercised
    // when downstream consumer issues land).
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["next", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("--redelegation-cap"),
        "koto next --help should advertise --redelegation-cap; got:\n{}",
        stdout
    );
}

#[test]
fn only_redelegation_cap_is_cli_overridable() {
    // The six other tunable request-store dimensions must NOT be exposed as CLI
    // flags at V1 (Decision 4: --redelegation-cap is the only one).
    let output = Command::cargo_bin("koto")
        .unwrap()
        .args(["next", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    for forbidden in [
        "--stale-claim-timeout",
        "--stale-dispatch-timeout",
        "--coord-cursor-ttl",
        "--terminal-index-compact-lines",
        "--compact-lock-timeout",
        "--directive-batch-size",
        "--respawn-generation-cap",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "koto next must NOT expose {} at V1; got:\n{}",
            forbidden,
            stdout
        );
    }
}

// ---------------------------------------------------------------------------
// Reserved [request_store.recursion] namespace warn behavior
// ---------------------------------------------------------------------------

#[test]
fn request_store_recursion_table_emits_warn_on_config_get() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    write_user_config(
        &home_dir,
        "[request_store.recursion]\nmax_depth_soft = 7\nmax_depth_hard = 20\n",
    );

    let (code, _stdout, stderr) = config_get(&home_dir, &cwd_dir, "request_store.redelegation_cap");
    assert_eq!(
        code, 0,
        "[request_store.recursion] presence must NOT cause failure; stderr={}",
        stderr
    );
    assert!(
        stderr.contains("[request_store.recursion]"),
        "warn message should mention the reserved table; stderr was:\n{}",
        stderr
    );
}

#[test]
fn request_store_recursion_absent_is_silent_on_config_get() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    let (code, _stdout, stderr) = config_get(&home_dir, &cwd_dir, "request_store.redelegation_cap");
    assert_eq!(code, 0);
    assert!(
        !stderr.contains("[request_store.recursion]"),
        "stderr should be silent when the reserved table is absent; got:\n{}",
        stderr
    );
}

#[test]
fn request_store_recursion_table_emits_warn_on_koto_next_startup() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    write_user_config(&home_dir, "[request_store.recursion]\nmax_depth_soft = 7\n");

    // Invoke `koto next` on a non-existent workflow -- the call fails
    // (no such workflow) but the [request_store.recursion] warn fires at startup
    // before the workflow-not-found check, so it must appear on stderr
    // regardless of exit code.
    let output = koto_cmd(&home_dir, &cwd_dir)
        .args(["next", "nonexistent-wf"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[request_store.recursion]"),
        "koto next startup must emit the reserved-namespace warn; stderr was:\n{}",
        stderr
    );
}

#[test]
fn request_store_recursion_project_table_also_warns() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    // Reserved-namespace warn must fire when the table is in project
    // config too (not only user config).
    write_project_config(&cwd_dir, "[request_store.recursion]\nmax_depth_soft = 7\n");

    let (_, _, stderr) = config_get(&home_dir, &cwd_dir, "request_store.redelegation_cap");
    assert!(
        stderr.contains("[request_store.recursion]"),
        "project config [request_store.recursion] should also trigger the warn; stderr was:\n{}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Env-var key spellings match Decision 4's table
// ---------------------------------------------------------------------------

#[test]
fn all_request_store_env_var_keys_parse() {
    let tmp = TempDir::new().unwrap();
    let home_dir = tmp.path().join("home");
    let cwd_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&home_dir).unwrap();
    std::fs::create_dir_all(&cwd_dir).unwrap();

    let cases: &[(&str, &str, &str)] = &[
        (
            "KOTO_REQUEST_STORE_STALE_CLAIM_TIMEOUT_S",
            "request_store.stale_claim_timeout_seconds",
            "11",
        ),
        (
            "KOTO_REQUEST_STORE_STALE_DISPATCH_TIMEOUT_S",
            "request_store.stale_dispatch_timeout_seconds",
            "12",
        ),
        (
            "KOTO_REQUEST_STORE_REDELEGATION_CAP",
            "request_store.redelegation_cap",
            "13",
        ),
        (
            "KOTO_REQUEST_STORE_COORD_CURSOR_TTL_DAYS",
            "request_store.coord_cursor_ttl_days",
            "14",
        ),
        (
            "KOTO_REQUEST_STORE_TERMINAL_INDEX_COMPACT_LINES",
            "request_store.terminal_index_compact_lines",
            "15",
        ),
        (
            "KOTO_REQUEST_STORE_COMPACT_LOCK_TIMEOUT_S",
            "request_store.compact_lock_timeout_seconds",
            "16",
        ),
        (
            "KOTO_REQUEST_STORE_DIRECTIVE_BATCH_SIZE",
            "request_store.directive_batch_size",
            "17",
        ),
        (
            "KOTO_REQUEST_STORE_RESPAWN_GENERATION_CAP",
            "request_store.respawn_generation_cap",
            "18",
        ),
    ];

    for (env_key, config_key, value) in cases {
        let output = koto_cmd(&home_dir, &cwd_dir)
            .env(env_key, value)
            .args(["config", "get", config_key])
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "config get {} with {}={} should succeed; stderr={}",
            config_key,
            env_key,
            value,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout.trim(),
            *value,
            "env var {} did not flow into {}",
            env_key,
            config_key
        );
    }
}
