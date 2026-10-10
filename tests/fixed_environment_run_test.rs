//! What a session's commands run with once the record exists
//! (DESIGN-koto-fixed-environment.md): the reproductions from the issue, the
//! names that reach a command, and the notes a stale record produces.
//!
//! Every `koto` here runs with a cleared environment plus what the test sets.
//! Commands under test report variable names, or compare a value to an
//! expected one; none prints an environment's values.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A system `PATH` that finds the coreutils and git these templates use.
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

struct Env {
    home: TempDir,
    cwd: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let cwd = home.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(home.path().join("sessions")).unwrap();
        std::fs::create_dir_all(home.path().join("out")).unwrap();
        Env { home, cwd }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn sessions(&self) -> PathBuf {
        self.home().join("sessions")
    }

    /// Where the commands under test write what they observed.
    fn out(&self) -> PathBuf {
        self.home().join("out")
    }

    fn read_out(&self, file: &str) -> String {
        std::fs::read_to_string(self.out().join(file)).unwrap_or_default()
    }

    /// A `koto` with nothing inherited from the test process: `PATH`, `HOME`
    /// and the session store are set here, then `extra`, which may replace
    /// any of them.
    fn koto(&self, extra: &[(&str, &str)]) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID");
        cmd.env_clear();
        cmd.current_dir(&self.cwd);
        cmd.env("PATH", SYSTEM_PATH);
        cmd.env("HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, extra: &[(&str, &str)], args: &[&str]) -> Run {
        Run::from(self.koto(extra).args(args).output().unwrap())
    }

    fn run_stdin(&self, extra: &[(&str, &str)], args: &[&str], stdin: &str) -> Run {
        let output = self
            .koto(extra)
            .args(args)
            .write_stdin(stdin.to_string())
            .output()
            .unwrap();
        Run::from(output)
    }

    /// Write a template, replacing `OUT` with the observation directory.
    fn template(&self, name: &str, body: &str) -> String {
        let path = self.home().join(name);
        std::fs::write(&path, body.replace("OUT", self.out().to_str().unwrap())).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn init(&self, extra: &[(&str, &str)], name: &str, tpl: &str) {
        let r = self.run(extra, &["init", name, "--template", tpl]);
        assert!(r.success, "init {name}: {}", r.stderr);
    }

    /// `koto next`, keeping a finished session on disk so its state and
    /// files can be read afterwards.
    fn next(&self, extra: &[(&str, &str)], name: &str) -> Run {
        self.run(extra, &["next", name, "--no-cleanup"])
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    fn events(&self, name: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap()
            .lines()
            .skip(1)
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn current_state(&self, name: &str) -> String {
        let r = self.run(&[], &["status", name]);
        r.json["current_state"].as_str().unwrap_or("").to_string()
    }

    /// Every file under a session's directory, read as bytes.
    fn session_files(&self, name: &str) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.sessions().join(name), &mut out);
        out
    }

    /// A directory holding one executable script, `name`, running `body`.
    fn bin_dir(&self, dir: &str, name: &str, body: &str) -> PathBuf {
        let d = self.home().join(dir);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        d
    }
}

struct Run {
    success: bool,
    stdout: String,
    stderr: String,
    json: serde_json::Value,
}

impl From<std::process::Output> for Run {
    fn from(output: std::process::Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
        Run {
            success: output.status.success(),
            stdout,
            stderr,
            json,
        }
    }
}

impl Run {
    fn directive(&self) -> String {
        self.json["directive"].as_str().unwrap_or("").to_string()
    }
}

fn path_with(dir: &Path) -> String {
    format!("{}:{SYSTEM_PATH}", dir.display())
}

fn koto_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_koto"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// One state whose single command gate must pass before `done`.
fn gate_template(gate: &str, command: &str) -> String {
    format!(
        r#"---
name: gated
version: "1.0"
initial_state: check
states:
  check:
    gates:
      {gate}:
        type: command
        command: '{command}'
        overridable: false
    transitions:
      - target: done
  done:
    terminal: true
---

## check

Check.

## done

Done.
"#
    )
}

// ---------------------------------------------------------------------------
// the issue's reproductions
// ---------------------------------------------------------------------------

/// A non-overridable gate asking whether `whoami` answers `impostor`. With a
/// fake `whoami` first on the ticking shell's `PATH`, it passed before this
/// change; the recorded `PATH` finds the real one.
#[test]
fn a_path_prefix_on_the_tick_does_not_reach_a_gate() {
    let env = Env::new();
    let fake = env.bin_dir("fakebin", "whoami", "echo impostor");
    let tpl = env.template(
        "whoami.md",
        &gate_template("identity", r#"test "$(whoami)" = impostor"#),
    );
    env.init(&[], "wf", &tpl);

    let r = env.next(&[("PATH", &path_with(&fake))], "wf");
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.current_state("wf"), "check");

    // The same tick on a legacy session, which keeps the behaviour before
    // this change, reaches the terminal state.
    let r = env.run(
        &[],
        &["init", "old", "--template", &tpl, "--legacy-environment"],
    );
    assert!(r.success, "{}", r.stderr);
    env.next(&[("PATH", &path_with(&fake))], "old");
    assert_eq!(env.current_state("old"), "done");
}

/// The same gate with an exported `whoami` function on the tick. Only a
/// bash `/bin/sh` imports exported functions, so the test needs one: CI runs
/// it in a job that links `/bin/sh` to bash, and locally it runs in a
/// container. It checks the vector is live on the host first, so a pass
/// can't come from a shell that ignores the function anyway.
#[test]
#[ignore = "needs /bin/sh to be bash; run with --ignored where it is"]
fn an_exported_function_on_the_tick_does_not_reach_a_gate() {
    let function = "() {  echo impostor\n}";
    let control = std::process::Command::new("/bin/sh")
        .args(["-c", "whoami"])
        .env("BASH_FUNC_whoami%%", function)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&control.stdout).trim(),
        "impostor",
        "/bin/sh on this host doesn't import exported functions; this test needs a bash /bin/sh"
    );

    let env = Env::new();
    let tpl = env.template(
        "whoami.md",
        &gate_template("identity", r#"test "$(whoami)" = impostor"#),
    );
    env.init(&[], "wf", &tpl);

    let r = env.next(&[("BASH_FUNC_whoami%%", function)], "wf");
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.current_state("wf"), "check");

    let r = env.run(
        &[],
        &["init", "old", "--template", &tpl, "--legacy-environment"],
    );
    assert!(r.success, "{}", r.stderr);
    env.next(&[("BASH_FUNC_whoami%%", function)], "old");
    assert_eq!(env.current_state("old"), "done");
}

/// A gate calling a git alias that only crafted configuration defines. The
/// tick points `HOME`, then `XDG_CONFIG_HOME`, at that configuration; the
/// recorded values don't have it, so the gate fails.
#[test]
fn git_config_under_a_ticks_home_or_xdg_does_not_reach_a_gate() {
    let env = Env::new();
    let crafted = env.home().join("crafted");
    std::fs::create_dir_all(crafted.join("git")).unwrap();
    let alias = "[alias]\n\tkoto-probe = !true\n";
    std::fs::write(crafted.join(".gitconfig"), alias).unwrap();
    std::fs::write(crafted.join("git").join("config"), alias).unwrap();

    // The alias works when the configuration is reachable.
    let control = std::process::Command::new("git")
        .arg("koto-probe")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .env("HOME", &crafted)
        .output()
        .unwrap();
    assert!(control.status.success(), "the crafted alias must work");

    let tpl = env.template("git.md", &gate_template("aliased", "git koto-probe"));
    let xdg = env.home().join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    for (name, var, value) in [
        ("via-home", "HOME", crafted.clone()),
        ("via-xdg", "XDG_CONFIG_HOME", crafted.clone()),
    ] {
        env.init(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())], name, &tpl);
        let r = env.next(&[(var, value.to_str().unwrap())], name);
        assert!(r.success, "{}", r.stderr);
        assert_eq!(env.current_state(name), "check", "{var}");
    }
}

/// A stand-in `gh` on the creating shell's `PATH` is what the session's
/// gates find, whichever shell ticks; one only on a ticking shell's `PATH`
/// isn't.
#[test]
fn an_accidental_shim_follows_the_record_not_the_tick() {
    let env = Env::new();
    let shim = env.bin_dir("shim", "gh", "echo shim");
    let tpl = env.template(
        "gh.md",
        &gate_template("uses_shim", r#"test "$(gh 2>/dev/null)" = shim"#),
    );

    env.init(&[("PATH", &path_with(&shim))], "with-shim", &tpl);
    assert!(env.next(&[], "with-shim").success);
    assert_eq!(env.current_state("with-shim"), "done");

    env.init(&[], "without-shim", &tpl);
    assert!(
        env.next(&[("PATH", &path_with(&shim))], "without-shim")
            .success
    );
    assert_eq!(env.current_state("without-shim"), "check");
}

// ---------------------------------------------------------------------------
// what reaches a command
// ---------------------------------------------------------------------------

/// Names only: each command writes the sorted names it can see.
const NAMES: &str = "env | cut -d= -f1 | sort -u";

const THREE_KINDS: &str = r#"---
name: kinds
version: "1.0"
initial_state: one
pass_env:
  - KOTO_DECLARED
states:
  one:
    default_action:
      command: 'NAMES > OUT/action.txt'
    gates:
      probe:
        type: command
        command: 'NAMES > OUT/gate.txt'
    transitions:
      - target: two
  two:
    default_action:
      command: 'NAMES > OUT/polled.txt'
      polling:
        interval_secs: 1
        timeout_secs: 10
    gates:
      polled_ran:
        type: command
        command: 'test -s OUT/polled.txt'
    transitions:
      - target: done
  done:
    terminal: true
---

## one

One.

## two

Two.

## done

Done.
"#;

/// The names a shell adds for itself, which aren't koto's to control.
const SHELL_OWN: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

fn names(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::to_string)
        .filter(|n| !n.is_empty() && !SHELL_OWN.contains(&n.as_str()))
        .collect()
}

#[test]
fn only_listed_names_reach_a_gate_an_action_and_a_polled_action() {
    let env = Env::new();
    let tpl = env.template("kinds.md", &THREE_KINDS.replace("NAMES", NAMES));
    let xdg = env.home().join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    env.init(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())], "wf", &tpl);

    let tick = [
        ("TMPDIR", "/tmp"),
        ("KOTO_DECLARED", "declared"),
        ("KOTO_UNLISTED", "unlisted"),
        ("BASH_ENV", "/dev/null"),
        ("ENV", "/dev/null"),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_SSH_COMMAND", "ssh"),
        ("BASH_FUNC_probe%%", "() {  true\n}"),
    ];
    let r = env.next(&tick, "wf");
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.current_state("wf"), "done", "{}", r.stdout);

    let expected: BTreeSet<String> = [
        "PATH",
        "HOME",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "KOTO_DECLARED",
        "KOTO_TICK_SESSION",
        "KOTO_SESSIONS_BASE",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for file in ["gate.txt", "action.txt", "polled.txt"] {
        assert_eq!(names(&env.read_out(file)), expected, "{file}");
    }
}

#[test]
fn a_declared_name_and_tmpdir_are_read_live_on_every_tick() {
    let env = Env::new();
    let tpl = env.template(
        "live.md",
        &gate_template(
            "record_values",
            r#"printf "%s|%s" "$KOTO_DECLARED" "$TMPDIR" > OUT/live.txt; exit 1"#,
        )
        .replace(
            "initial_state: check\n",
            "initial_state: check\npass_env:\n  - KOTO_DECLARED\n",
        ),
    );
    env.init(&[], "wf", &tpl);

    env.next(&[("KOTO_DECLARED", "first"), ("TMPDIR", "/tmp/a")], "wf");
    assert_eq!(env.read_out("live.txt"), "first|/tmp/a");
    env.next(&[("KOTO_DECLARED", "second"), ("TMPDIR", "/tmp/b")], "wf");
    assert_eq!(env.read_out("live.txt"), "second|/tmp/b");
}

/// Credential-shaped values set at init and on ticks that run a gate, an
/// action and a polled action appear in no file under the session.
#[test]
fn no_credential_value_is_written_under_the_session_after_ticks() {
    let env = Env::new();
    let tpl = env.template("kinds.md", &THREE_KINDS.replace("NAMES", NAMES));
    let at_init = [
        ("GH_TOKEN", "marker-init-gh-7f3a"),
        ("GITHUB_TOKEN", "marker-init-github-7f3a"),
        (
            "HTTPS_PROXY",
            "http://user:marker-init-proxy-7f3a@proxy.invalid:3128",
        ),
    ];
    env.init(&at_init, "wf", &tpl);
    let on_tick = [
        ("GH_TOKEN", "marker-tick-gh-9c1e"),
        ("GITHUB_TOKEN", "marker-tick-github-9c1e"),
        (
            "HTTPS_PROXY",
            "http://user:marker-tick-proxy-9c1e@proxy.invalid:3128",
        ),
    ];
    let r = env.next(&on_tick, "wf");
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.current_state("wf"), "done");

    for (path, bytes) in env.session_files("wf") {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("marker-"),
            "a credential marker is in {}",
            path.display()
        );
    }
}

/// A gate sees the tick's session name, a nested `koto next` from it is
/// refused, and a nested `koto context get` reaches the same store.
#[test]
fn nested_koto_in_a_gate_sees_the_tick_and_the_store() {
    let env = Env::new();
    let tpl = env.template(
        "nested.md",
        &gate_template(
            "nested",
            r#"printf "%s" "$KOTO_TICK_SESSION" > OUT/tick.txt; koto next wf > OUT/nested.txt 2>&1; koto context get wf note > OUT/ctx.txt; exit 1"#,
        ),
    );
    env.init(&[("PATH", &path_with(&koto_dir()))], "wf", &tpl);
    let note = env.home().join("note.txt");
    std::fs::write(&note, "from the store").unwrap();
    assert!(
        env.run(
            &[],
            &[
                "context",
                "add",
                "wf",
                "note",
                "--from-file",
                note.to_str().unwrap()
            ]
        )
        .success
    );

    assert!(env.next(&[], "wf").success);
    assert_eq!(env.read_out("tick.txt"), "wf");
    assert!(
        env.read_out("nested.txt").contains("nested_invocation"),
        "{}",
        env.read_out("nested.txt")
    );
    assert_eq!(env.read_out("ctx.txt"), "from the store");
}

#[test]
fn a_gate_reads_end_of_file_on_standard_input() {
    let env = Env::new();
    let tpl = env.template(
        "stdin.md",
        &gate_template(
            "reads",
            r#"if read -r line; then echo got > OUT/stdin.txt; else echo eof > OUT/stdin.txt; fi"#,
        ),
    );
    env.init(&[], "wf", &tpl);
    let r = env.run_stdin(&[], &["next", "wf"], "piped input\n");
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.read_out("stdin.txt").trim(), "eof");
}

/// The shell is `/bin/sh` whatever the record's `PATH` holds: a decoy `sh`
/// first on it isn't run, and a `PATH` with no `sh` still runs commands.
#[test]
fn commands_run_under_bin_sh_whatever_the_path() {
    let env = Env::new();
    let marker = env.out().join("decoy.txt");
    let decoy = env.bin_dir("decoy", "sh", &format!("echo decoy > {}", marker.display()));
    let tpl = env.template("sh.md", &gate_template("runs", "echo ran >> OUT/ran.txt"));

    env.init(&[("PATH", &path_with(&decoy))], "decoyed", &tpl);
    assert!(env.next(&[], "decoyed").success);
    env.init(&[("PATH", "/koto-nonexistent-dir")], "no-sh", &tpl);
    assert!(env.next(&[], "no-sh").success);

    assert_eq!(env.read_out("ran.txt"), "ran\nran\n");
    assert_eq!(env.read_out("decoy.txt"), "");
}

// ---------------------------------------------------------------------------
// stale records and missing commands
// ---------------------------------------------------------------------------

fn blocking_output(r: &Run, gate: &str) -> serde_json::Value {
    r.json["blocking_conditions"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|c| c["name"] == gate)
        .map(|c| c["output"].clone())
        .unwrap_or(serde_json::Value::Null)
}

#[test]
fn a_missing_tool_names_the_gate_the_recorded_path_and_the_remedy() {
    let env = Env::new();
    for (name, command, exit) in [
        ("direct", "koto-no-such-tool-xyz", 127),
        ("wrapped", "koto-no-such-tool-xyz; exit 3", 3),
    ] {
        let tpl = env.template(&format!("{name}.md"), &gate_template("needs_tool", command));
        env.init(&[], name, &tpl);
        let r = env.next(&[], name);
        assert!(r.success, "{}", r.stderr);
        let directive = r.directive();
        assert!(directive.contains("gate 'needs_tool'"), "{directive}");
        assert!(
            directive.contains(&format!("recorded PATH={SYSTEM_PATH}")),
            "{directive}"
        );
        assert!(
            directive.contains(&format!("koto cancel --cleanup {name}")),
            "{directive}"
        );
        // The evidence is what it always was.
        assert_eq!(
            blocking_output(&r, "needs_tool"),
            serde_json::json!({"exit_code": exit, "error": ""})
        );
    }
}

#[test]
fn a_plain_failure_with_a_current_record_carries_no_note() {
    let env = Env::new();
    let tpl = env.template("fails.md", &gate_template("fails", "exit 1"));
    env.init(&[], "wf", &tpl);
    let r = env.next(&[], "wf");
    assert!(!r.directive().contains("[koto]"), "{}", r.directive());
}

#[test]
fn a_recorded_directory_removed_after_init_is_named() {
    let env = Env::new();
    let tools = env.bin_dir("tools", "koto-probe-tool", "exit 0");

    // A notice with nothing failing: a gate that blocks without a command.
    let wait = env.template(
        "wait.md",
        r#"---
name: wait
version: "1.0"
initial_state: wait
states:
  wait:
    accepts:
      go:
        type: string
        required: true
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#,
    );
    env.init(&[("PATH", &path_with(&tools))], "quiet", &wait);
    let failing = env.template("tool.md", &gate_template("uses_tool", "koto-probe-tool"));
    env.init(&[("PATH", &path_with(&tools))], "loud", &failing);

    std::fs::remove_dir_all(&tools).unwrap();
    let tools_text = tools.to_str().unwrap();

    let quiet = env.next(&[], "quiet");
    let notice = quiet.directive();
    assert!(notice.contains("no longer exist"), "{notice}");
    assert!(notice.contains(&format!("PATH {tools_text}")), "{notice}");

    let loud = env.next(&[], "loud");
    let note = loud.directive();
    assert!(note.contains("gate 'uses_tool'"), "{note}");
    assert!(note.contains(&format!("PATH {tools_text}")), "{note}");
}

#[test]
fn a_path_entry_missing_at_init_is_not_stale() {
    let env = Env::new();
    let never = env.home().join("never-created");
    let tpl = env.template("fails.md", &gate_template("fails", "exit 1"));
    env.init(&[("PATH", &path_with(&never))], "wf", &tpl);
    let r = env.next(&[], "wf");
    assert!(!r.directive().contains("[koto]"), "{}", r.directive());
}

#[test]
fn a_recorded_home_removed_after_init_is_named() {
    let env = Env::new();
    let home = env.home().join("session-home");
    std::fs::create_dir_all(&home).unwrap();
    let tpl = env.template("fails.md", &gate_template("fails", "exit 1"));
    // The compiled template is cached under the creating process's cache
    // directory, which defaults to one under HOME; it goes elsewhere so only
    // the recorded HOME disappears.
    let cache = env.home().join("cache");
    env.init(
        &[
            ("HOME", home.to_str().unwrap()),
            ("XDG_CACHE_HOME", cache.to_str().unwrap()),
        ],
        "wf",
        &tpl,
    );
    std::fs::remove_dir_all(&home).unwrap();
    let r = env.next(&[], "wf");
    let note = r.directive();
    assert!(note.contains("gate 'fails'"), "{}", r.stdout);
    assert!(note.contains(&format!("HOME {}", home.display())), "{note}");
}

/// A `HOME` that didn't exist when the session was created is never called
/// stale: a new session would record the same value, so the remedy would loop.
#[test]
fn a_home_missing_at_creation_is_not_reported_stale() {
    let env = Env::new();
    let never = env.home().join("never-created-home");
    let cache = env.home().join("cache");
    let tpl = env.template("fails.md", &gate_template("fails", "exit 1"));
    env.init(
        &[
            ("HOME", never.to_str().unwrap()),
            ("XDG_CACHE_HOME", cache.to_str().unwrap()),
        ],
        "wf",
        &tpl,
    );
    let r = env.next(&[], "wf");
    assert!(r.success, "{}", r.stderr);
    assert!(!r.directive().contains("[koto]"), "{}", r.directive());
}

#[test]
fn no_note_text_reaches_an_actions_captured_output() {
    let env = Env::new();
    let tpl = env.template(
        "action.md",
        r#"---
name: action
version: "1.0"
initial_state: run
states:
  run:
    default_action:
      command: 'koto-no-such-tool-xyz'
    transitions:
      - target: done
  done:
    terminal: true
---

## run

Run it.

## done

Done.
"#,
    );
    env.init(&[], "wf", &tpl);
    let r = env.next(&[], "wf");
    assert!(
        r.directive().contains("the default action of state 'run'"),
        "{}",
        r.stdout
    );
    let executed: Vec<_> = env
        .events("wf")
        .into_iter()
        .filter(|e| e["type"] == "default_action_executed")
        .collect();
    assert!(!executed.is_empty());
    for event in executed {
        let payload = event["payload"].to_string();
        assert!(!payload.contains("[koto]"), "{payload}");
    }
}

#[test]
fn a_legacy_session_gets_no_note() {
    let env = Env::new();
    let tpl = env.template(
        "legacy.md",
        &gate_template("needs_tool", "koto-no-such-tool-xyz"),
    );
    let r = env.run(
        &[],
        &["init", "wf", "--template", &tpl, "--legacy-environment"],
    );
    assert!(r.success, "{}", r.stderr);
    let r = env.next(&[], "wf");
    assert!(r.success, "{}", r.stderr);
    assert!(!r.directive().contains("[koto]"), "{}", r.directive());
}

#[test]
fn a_recorded_xdg_config_home_removed_after_init_is_named() {
    let env = Env::new();
    let xdg = env.home().join("session-xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let tpl = env.template("fails.md", &gate_template("fails", "exit 1"));
    env.init(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())], "wf", &tpl);
    std::fs::remove_dir_all(&xdg).unwrap();
    let r = env.next(&[], "wf");
    let note = r.directive();
    assert!(note.contains("gate 'fails'"), "{}", r.stdout);
    assert!(
        note.contains(&format!("XDG_CONFIG_HOME {}", xdg.display())),
        "{note}"
    );
}

/// `koto next --to` refused by a non-overridable guard gate that couldn't
/// find its command names it in the refusal, the only response it gives.
#[test]
fn a_directed_transition_refused_by_a_missing_tool_names_it() {
    let env = Env::new();
    let tpl = env.template(
        "guard.md",
        &gate_template("needs_tool", "koto-no-such-tool-xyz"),
    );
    env.init(&[], "wf", &tpl);
    let r = env.run(&[], &["next", "wf", "--to", "done", "--no-cleanup"]);
    assert!(!r.success, "{}", r.stdout);
    let message = r.json["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("gate 'needs_tool'"), "{}", r.stdout);
    assert!(message.contains("koto cancel --cleanup wf"), "{message}");
}
