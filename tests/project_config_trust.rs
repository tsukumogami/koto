//! Project config trust, end to end against the built binary.
//!
//! A project's `.koto/config.toml` applies only after `koto config trust`
//! in its directory, and only as long as the file is unchanged. A file the
//! user creates with `koto config set` is trusted as written. These tests
//! run each command with a `HOME` of their own and the project in a
//! separate directory, so the user config and the project config are two
//! files.

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Env {
    fn new() -> Env {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        Env {
            _tmp: tmp,
            home,
            project,
        }
    }

    fn cmd_in(&self, dir: &Path) -> Command {
        let mut cmd = koto_command::koto();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        cmd.env_remove("KOTO_HOME");
        cmd.env_remove("AWS_ACCESS_KEY_ID");
        cmd.env_remove("AWS_SECRET_ACCESS_KEY");
        cmd.current_dir(dir);
        cmd.env("HOME", &self.home);
        cmd
    }

    /// Run `koto <args>` in the project; return (exit code, stdout, stderr).
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.cmd_in(&self.project).args(args).output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (code, stdout, stderr) = self.run(args);
        assert_eq!(code, 0, "koto {args:?} failed: {stderr}");
        stdout
    }

    fn get(&self, key: &str) -> String {
        self.ok(&["config", "get", key]).trim().to_string()
    }

    fn project_file(&self) -> PathBuf {
        self.project.join(".koto").join("config.toml")
    }

    fn write_project_file(&self, body: &str) {
        std::fs::create_dir_all(self.project.join(".koto")).unwrap();
        std::fs::write(self.project_file(), body).unwrap();
    }
}

/// User and project config read back, project over
/// user, and a credential key refused in project config.
#[test]
fn user_and_project_config_read_back_and_credentials_are_refused() {
    let env = Env::new();
    env.ok(&[
        "config",
        "set",
        "--user",
        "session.cloud.bucket",
        "user-bucket",
    ]);
    assert_eq!(env.get("session.cloud.bucket"), "user-bucket");

    // A project file created with `koto config set` is trusted as written.
    env.ok(&["config", "set", "session.cloud.bucket", "project-bucket"]);
    let (code, stdout, stderr) = env.run(&["config", "get", "session.cloud.bucket"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.trim(), "project-bucket");
    assert!(stderr.is_empty(), "no note for a trusted file: {stderr}");

    // Editing it with `koto config set` keeps it trusted.
    env.ok(&["config", "set", "session.cloud.region", "eu-west-1"]);
    assert_eq!(env.get("session.cloud.region"), "eu-west-1");
    env.ok(&["config", "unset", "session.cloud.region"]);
    assert_eq!(env.get("session.cloud.bucket"), "project-bucket");

    for key in ["session.cloud.access_key", "session.cloud.secret_key"] {
        let (code, _, stderr) = env.run(&["config", "set", key, "value"]);
        assert_ne!(code, 0, "{key} was accepted in project config");
        assert!(stderr.contains("credentials"), "{stderr}");
    }
    let (code, _, stderr) = env.run(&[
        "config",
        "set",
        "session.cloud.endpoint",
        "https://user:pass@store.example",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("credentials"), "{stderr}");
}

/// A cloned repository's file doesn't apply before
/// the user opts in, and does after.
#[test]
fn a_cloned_project_file_applies_only_after_trust() {
    let env = Env::new();
    env.ok(&[
        "config",
        "set",
        "--user",
        "session.cloud.bucket",
        "user-bucket",
    ]);
    env.write_project_file("[session.cloud]\nbucket = \"repo-bucket\"\n");

    let (code, stdout, stderr) = env.run(&["config", "get", "session.cloud.bucket"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.trim(), "user-bucket");
    assert!(stderr.contains("isn't trusted"), "{stderr}");
    assert!(stderr.contains("koto config trust"), "{stderr}");

    let report = env.ok(&["config", "trust"]);
    assert!(
        report.contains("session.cloud.bucket = repo-bucket"),
        "{report}"
    );
    let (_, stdout, stderr) = env.run(&["config", "get", "session.cloud.bucket"]);
    assert_eq!(stdout.trim(), "repo-bucket");
    assert!(stderr.is_empty(), "{stderr}");

    // A later change, as a pull would bring, lapses the grant.
    env.write_project_file("[session.cloud]\nbucket = \"other-bucket\"\n");
    let (_, stdout, stderr) = env.run(&["config", "get", "session.cloud.bucket"]);
    assert_eq!(stdout.trim(), "user-bucket");
    assert!(stderr.contains("changed since it was trusted"), "{stderr}");

    // Trusting again shows what changed, then applies it.
    let report = env.ok(&["config", "trust"]);
    assert!(
        report.contains("session.cloud.bucket: repo-bucket -> other-bucket"),
        "{report}"
    );
    assert_eq!(env.get("session.cloud.bucket"), "other-bucket");

    env.ok(&["config", "untrust"]);
    assert_eq!(env.get("session.cloud.bucket"), "user-bucket");
}

#[test]
fn the_grant_is_kept_outside_the_project() {
    let env = Env::new();
    env.write_project_file("[session.cloud]\nbucket = \"repo-bucket\"\n");
    env.ok(&["config", "trust"]);
    let store = env.home.join(".koto").join("trust.toml");
    let text = std::fs::read_to_string(&store).unwrap();
    assert!(text.contains("sha256"), "{text}");
    assert!(!env.project.join(".koto").join("trust.toml").exists());

    // A grant for one directory doesn't cover another with the same file.
    let other = env.project.parent().unwrap().join("other");
    std::fs::create_dir_all(other.join(".koto")).unwrap();
    std::fs::copy(env.project_file(), other.join(".koto").join("config.toml")).unwrap();
    let out = env
        .cmd_in(&other)
        .args(["config", "get", "session.cloud.bucket"])
        .output()
        .unwrap();
    assert_ne!(String::from_utf8_lossy(&out.stdout).trim(), "repo-bucket");
}

#[test]
fn set_and_unset_refuse_a_file_that_is_not_trusted_as_it_stands() {
    let env = Env::new();
    env.write_project_file("[session.cloud]\nbucket = \"repo-bucket\"\n");
    for args in [
        &["config", "set", "session.cloud.region", "r"][..],
        &["config", "unset", "session.cloud.bucket"][..],
    ] {
        let (code, _, stderr) = env.run(args);
        assert_ne!(code, 0, "koto {args:?} edited an untrusted file");
        assert!(stderr.contains("isn't trusted"), "{stderr}");
    }
    assert_eq!(
        std::fs::read_to_string(env.project_file()).unwrap(),
        "[session.cloud]\nbucket = \"repo-bucket\"\n"
    );

    env.ok(&["config", "trust"]);
    env.write_project_file("[session.cloud]\nbucket = \"changed\"\n");
    let (code, _, stderr) = env.run(&["config", "set", "session.cloud.region", "r"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("changed since it was trusted"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_project_file_is_neither_trusted_nor_written() {
    let env = Env::new();
    let elsewhere = env.project.parent().unwrap().join("elsewhere.toml");
    std::fs::write(&elsewhere, "[session.cloud]\nbucket = \"linked\"\n").unwrap();
    std::fs::create_dir_all(env.project.join(".koto")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, env.project_file()).unwrap();

    let (code, _, stderr) = env.run(&["config", "trust"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("symlink"), "{stderr}");
    let (code, _, stderr) = env.run(&["config", "set", "session.cloud.region", "r"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("symlink"), "{stderr}");
    let (_, stdout, _) = env.run(&["config", "get", "session.cloud.bucket"]);
    assert_ne!(stdout.trim(), "linked");
}

/// A relative `KOTO_HOME` resolves against the working directory, so its
/// grants are never used: in a project it could name a root the project
/// ships.
#[test]
fn no_grant_is_used_under_a_relative_koto_home() {
    let env = Env::new();
    env.write_project_file("[session.cloud]\nbucket = \"repo-bucket\"\n");
    let root = env.project.join(".koto-root");
    std::fs::create_dir_all(&root).unwrap();
    let file = std::fs::canonicalize(env.project_file()).unwrap();
    let dir = std::fs::canonicalize(&env.project).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    use sha2::Digest;
    std::fs::write(
        root.join("trust.toml"),
        format!(
            "version = 1\n[[grant]]\ndir = {:?}\nfile = {:?}\nsha256 = {:?}\n",
            dir.to_string_lossy(),
            file.to_string_lossy(),
            hex::encode(sha2::Sha256::digest(&bytes))
        ),
    )
    .unwrap();

    let out = env
        .cmd_in(&env.project)
        .env("KOTO_HOME", ".koto-root")
        .args(["config", "get", "session.cloud.bucket"])
        .output()
        .unwrap();
    assert_ne!(String::from_utf8_lossy(&out.stdout).trim(), "repo-bucket");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("relative"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = env
        .cmd_in(&env.project)
        .env("KOTO_HOME", ".koto-root")
        .args(["config", "trust"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn trust_refuses_a_file_over_the_size_cap() {
    let env = Env::new();
    let mut body = String::from("[session.cloud]\nbucket = \"repo-bucket\"\n");
    body.push_str(&"#".repeat(64 * 1024));
    env.write_project_file(&body);
    let (code, _, stderr) = env.run(&["config", "trust"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("larger than 64 KiB"), "{stderr}");
    assert!(!env.home.join(".koto").join("trust.toml").exists());
}

#[test]
fn a_malformed_untrusted_file_does_not_break_commands() {
    let env = Env::new();
    env.write_project_file("this = = is not [ toml");
    let (code, _, stderr) = env.run(&["config", "list", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let (code, _, stderr) = env.run(&["config", "trust"]);
    assert_ne!(code, 0, "a malformed file was trusted");
    assert!(stderr.contains("not valid TOML"), "{stderr}");
}

#[test]
fn a_trusted_file_sets_only_what_project_config_may_set() {
    let env = Env::new();
    env.write_project_file(
        "[session.cloud]\nbucket = \"repo-bucket\"\naccess_key = \"repo-access\"\n\
         [request_store]\nredelegation_cap = 9\n",
    );
    let report = env.ok(&["config", "trust"]);
    assert!(report.contains("session.cloud.access_key"), "{report}");
    assert!(
        !report.contains("repo-access"),
        "a credential was echoed: {report}"
    );

    let (_, stdout, stderr) = env.run(&["config", "get", "request_store.redelegation_cap"]);
    assert_eq!(stdout.trim(), "3");
    assert!(
        stderr.contains("request_store.redelegation_cap is ignored in project config"),
        "{stderr}"
    );
    let (code, _, _) = env.run(&["config", "get", "session.cloud.access_key"]);
    assert_ne!(code, 0, "a project file supplied a credential");

    let list = env.ok(&["config", "list", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(json["project_config"]["status"], "trusted");
    let dropped = json["project_config"]["dropped"].as_array().unwrap();
    assert!(dropped.iter().any(|k| k == "session.cloud.access_key"));
    assert!(!list.contains("repo-access"));
}

#[test]
fn session_dir_reports_the_project_config_state() {
    let env = Env::new();
    env.write_project_file("[session.cloud]\nbucket = \"repo-bucket\"\n");
    let out = env.ok(&["session", "dir", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["project_config"]["status"], "untrusted");
    assert_eq!(json["project_config"]["path"], ".koto/config.toml");
}

#[test]
fn trust_refuses_where_there_is_nothing_to_trust() {
    let env = Env::new();
    let (code, _, stderr) = env.run(&["config", "trust"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("no project config"), "{stderr}");

    // From the home directory the project file is the user file.
    std::fs::create_dir_all(env.home.join(".koto")).unwrap();
    std::fs::write(env.home.join(".koto").join("config.toml"), "").unwrap();
    let out = env
        .cmd_in(&env.home)
        .args(["config", "trust"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("user config"));
}

/// Every reader of the project config file goes through the config module's
/// trust check, so no code elsewhere builds the file's path itself.
#[test]
fn no_code_outside_the_config_module_builds_the_project_config_path() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path != src.join("config") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let joins = line.contains("\".koto\"") && line.contains("config.toml");
                if joins || line.contains("\".koto/config.toml\"") {
                    offenders.push(format!("{}:{}", path.display(), n + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "read the project config through config::trust instead: {offenders:?}"
    );
}
