//! CLI coverage for the opt-in `[decider]` configuration: layer rules,
//! key redaction, `koto config set` refusals, and user-config file
//! permissions.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

const KEY: &str = "sk-cli-DISTINCTIVE-4c3b2a";

const DECIDER_ENV: &[&str] = &[
    "KOTO_DECIDER",
    "KOTO_DECIDER_API_KEY",
    "KOTO_DECIDER_ENDPOINT",
];

struct Dirs {
    _tmp: TempDir,
    home: PathBuf,
    cwd: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cwd = tmp.path().join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    Dirs {
        _tmp: tmp,
        home,
        cwd,
    }
}

/// A `koto` command reading user config from `home` and project config
/// from `cwd`, with every decider env var removed so the file layers are
/// what's under test.
fn koto(d: &Dirs) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(&d.cwd);
    cmd.env("HOME", &d.home);
    cmd.env("KOTO_SESSIONS_BASE", d.cwd.join("sessions"));
    cmd.env_remove("AWS_ACCESS_KEY_ID");
    cmd.env_remove("AWS_SECRET_ACCESS_KEY");
    for k in DECIDER_ENV {
        cmd.env_remove(k);
    }
    cmd
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn user_config(d: &Dirs) -> PathBuf {
    d.home.join(".koto").join("config.toml")
}

fn project_config(d: &Dirs) -> PathBuf {
    d.cwd.join(".koto").join("config.toml")
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(mut cmd: Command, args: &[&str]) -> Out {
    let o = cmd.args(args).output().unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8(o.stdout).unwrap(),
        stderr: String::from_utf8(o.stderr).unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Unchanged output without a [decider] table
// ---------------------------------------------------------------------------

#[test]
fn list_without_decider_has_no_decider_key() {
    let d = dirs();
    write(&user_config(&d), "[session]\nbackend = \"local\"\n");
    let out = run(koto(&d), &["config", "list"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!out.stdout.contains("decider"), "{}", out.stdout);
    let out = run(koto(&d), &["config", "list", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(v.get("decider").is_none(), "{}", out.stdout);
    assert_eq!(
        v.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
        vec!["request_store", "session", "workflows"]
    );
}

// ---------------------------------------------------------------------------
// Project config can't supply credentials
// ---------------------------------------------------------------------------

#[test]
fn project_key_endpoint_timeout_dropped_at_load() {
    let d = dirs();
    write(
        &project_config(&d),
        &format!(
            "[decider]\nmode = \"auto\"\napi_key = \"{KEY}\"\nendpoint = \"https://proj-endpoint.example/d\"\ntimeout_ms = 7777\n"
        ),
    );

    let out = run(koto(&d), &["config", "get", "decider.api_key"]);
    assert_eq!(out.code, 1);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));

    let out = run(koto(&d), &["config", "get", "decider.endpoint"]);
    assert_eq!(out.code, 1);
    let out = run(koto(&d), &["config", "get", "decider.timeout_ms"]);
    assert_eq!(out.code, 1);

    for args in [&["config", "list"][..], &["config", "list", "--json"][..]] {
        let out = run(koto(&d), args);
        assert_eq!(out.code, 0, "{}", out.stderr);
        for bad in [KEY, "proj-endpoint", "7777", "<set>"] {
            assert!(!out.stdout.contains(bad), "{bad} in {}", out.stdout);
        }
    }
}

#[test]
fn project_set_refuses_decider_credentials() {
    let d = dirs();
    let out = run(koto(&d), &["config", "set", "decider.api_key", KEY]);
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("credentials"), "{}", out.stderr);
    assert!(!out.stderr.contains(KEY) && !out.stdout.contains(KEY));

    let out = run(
        koto(&d),
        &["config", "set", "decider.endpoint", "https://x.example/d"],
    );
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("user config"), "{}", out.stderr);
    assert!(
        out.stderr.contains("KOTO_DECIDER_ENDPOINT"),
        "{}",
        out.stderr
    );

    let out = run(koto(&d), &["config", "set", "decider.timeout_ms", "500"]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("not allowed in project config"),
        "{}",
        out.stderr
    );
    assert!(!project_config(&d).exists());

    // decider.mode is the one project-settable decider key.
    let out = run(koto(&d), &["config", "set", "decider.mode", "off"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let body = std::fs::read_to_string(project_config(&d)).unwrap();
    assert!(body.contains("mode = \"off\""), "{body}");
}

// ---------------------------------------------------------------------------
// Key display
// ---------------------------------------------------------------------------

#[test]
fn get_and_list_print_set_for_user_key() {
    let d = dirs();
    let out = run(
        koto(&d),
        &["config", "set", "--user", "decider.api_key", KEY],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));

    let out = run(koto(&d), &["config", "get", "decider.api_key"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.trim(), "<set>");
    assert!(!out.stderr.contains(KEY));

    for args in [&["config", "list"][..], &["config", "list", "--json"][..]] {
        let out = run(koto(&d), args);
        assert_eq!(out.code, 0);
        assert!(out.stdout.contains("<set>"), "{}", out.stdout);
        assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
    }
}

#[test]
fn get_prints_set_for_env_key_and_exits_1_without_one() {
    let d = dirs();
    let out = run(koto(&d), &["config", "get", "decider.api_key"]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty());

    let mut cmd = koto(&d);
    cmd.env("KOTO_DECIDER_API_KEY", KEY);
    let out = run(cmd, &["config", "get", "decider.api_key"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.trim(), "<set>");
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));

    // An empty env value counts as unset.
    let mut cmd = koto(&d);
    cmd.env("KOTO_DECIDER_API_KEY", "");
    let out = run(cmd, &["config", "get", "decider.api_key"]);
    assert_eq!(out.code, 1);
}

// ---------------------------------------------------------------------------
// `koto config set --user` value checks
// ---------------------------------------------------------------------------

#[test]
fn user_set_mode_accepts_only_off_shadow_auto() {
    let d = dirs();
    for ok in ["off", "shadow", "auto"] {
        let out = run(koto(&d), &["config", "set", "--user", "decider.mode", ok]);
        assert_eq!(out.code, 0, "{ok}: {}", out.stderr);
        let out = run(koto(&d), &["config", "get", "decider.mode"]);
        assert_eq!(out.stdout.trim(), ok);
    }
    let out = run(
        koto(&d),
        &["config", "set", "--user", "decider.mode", "never"],
    );
    assert_ne!(out.code, 0);
    assert!(out.stderr.contains("template-only"), "{}", out.stderr);
    let out = run(
        koto(&d),
        &["config", "set", "--user", "decider.mode", "bogus"],
    );
    assert_ne!(out.code, 0);
}

#[test]
fn user_set_timeout_bounds() {
    let d = dirs();
    for ok in ["1", "5000", "10000"] {
        let out = run(
            koto(&d),
            &["config", "set", "--user", "decider.timeout_ms", ok],
        );
        assert_eq!(out.code, 0, "{ok}: {}", out.stderr);
    }
    for bad in ["0", "10001", "fast", "-1"] {
        let out = run(
            koto(&d),
            &["config", "set", "--user", "decider.timeout_ms", bad],
        );
        assert_ne!(out.code, 0, "{bad}");
    }
}

#[test]
fn user_set_endpoint_applies_scheme_loopback_userinfo_rules() {
    let d = dirs();
    for ok in [
        "https://api.example.com/v1/decide",
        "http://127.0.0.1:8080/decide",
        "http://[::1]:8080/decide",
        "http://localhost:8080/decide",
    ] {
        let out = run(
            koto(&d),
            &["config", "set", "--user", "decider.endpoint", ok],
        );
        assert_eq!(out.code, 0, "{ok}: {}", out.stderr);
    }
    for bad in [
        "http://example.com/decide",
        "http://localhost.example.com/decide",
        "http://10.0.0.1/decide",
        "ftp://example.com/decide",
        "not a url",
    ] {
        let out = run(
            koto(&d),
            &["config", "set", "--user", "decider.endpoint", bad],
        );
        assert_ne!(out.code, 0, "{bad}");
    }
    let out = run(
        koto(&d),
        &[
            "config",
            "set",
            "--user",
            "decider.endpoint",
            "https://someuser:hunter2@api.example.com/d",
        ],
    );
    assert_ne!(out.code, 0);
    assert!(!out.stderr.contains("someuser") && !out.stderr.contains("hunter2"));
}

// ---------------------------------------------------------------------------
// Lenient parsing
// ---------------------------------------------------------------------------

#[test]
fn malformed_decider_tables_do_not_break_config() {
    let d = dirs();
    write(
        &user_config(&d),
        "[workflows]\nnative = false\n\n[decider]\nmode = 3\ntimeout_ms = \"fast\"\nunknown = 1\n",
    );
    write(
        &project_config(&d),
        "[session]\nbackend = \"local\"\n\n[decider]\nmode = [1, 2]\napi_key = 5\n",
    );
    let out = run(koto(&d), &["config", "get", "workflows.native"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "false");
    assert!(
        out.stderr.is_empty(),
        "load_config must not print: {}",
        out.stderr
    );

    let out = run(koto(&d), &["config", "list"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
}

// ---------------------------------------------------------------------------
// File permissions
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn user_set_creates_config_0600() {
    let d = dirs();
    let out = run(
        koto(&d),
        &["config", "set", "--user", "decider.api_key", KEY],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(mode_of(&user_config(&d)), 0o600);
}

#[cfg(unix)]
#[test]
fn user_set_and_unset_tighten_existing_0644() {
    use std::os::unix::fs::PermissionsExt;
    let d = dirs();
    let path = user_config(&d);
    write(&path, "[workflows]\nnative = true\n");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(
        koto(&d),
        &["config", "set", "--user", "decider.mode", "shadow"],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(mode_of(&path), 0o600, "set --user");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(koto(&d), &["config", "unset", "--user", "decider.mode"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(mode_of(&path), 0o600, "unset --user");
}
