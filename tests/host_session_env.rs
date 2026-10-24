//! Every test that runs koto's library in-process includes
//! `tests/support/host_session.rs`, which removes the hosting Claude Code
//! session's variables from the test process as it loads. Without it, a
//! suite run inside Claude Code reads that session's id, and every event a
//! test appends through `LocalBackend` writes a workflow file into that
//! session's `/workflows` directory.
//!
//! The first test reads every test crate, a `tests/*.rs` file or a
//! `tests/<dir>/main.rs` with every module file it loads, and fails on one
//! that names the `koto` crate without loading the module. The second reads
//! every call to `host_env` in `src/` and fails on a variable it reads that
//! `HOST_SESSION_ENV` doesn't list. Both read Rust tokens, so a comment
//! neither names the crate nor includes a module. The others run this test
//! binary again with the variables set and `HOME` in a temporary directory
//! that holds a transcript for the session id, so a leak lands there and
//! nowhere else, and check what an in-process append wrote. The control
//! runs set each variable again after the binary loaded and show that the
//! same append does write there, so an empty directory is the module's
//! doing.

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use host_session::HOST_SESSION_ENV;
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Set on a re-run of this binary to the run's root, so the probe runs
/// only there.
const PROBE: &str = "KOTO_HOST_SESSION_PROBE";

/// On a control run, the probe sets the variable named after this prefix
/// to its value, after the binary loaded.
const LEAK_PREFIX: &str = "KOTO_HOST_SESSION_PROBE_LEAK_";

/// An id-shaped Claude Code session id.
const SESSION_ID: &str = "4a6f2c1e-0433-4c4b-9d2e-5f0a1b2c3d4e";

const WORKFLOW: &str = "probe";

const TEMPLATE: &str = r#"---
name: host-session-probe
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        values: ["yes"]
        required: true
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Submit go.

## done

Done.
"#;

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// A token of Rust source, as far as the guards need one. Comments are
/// dropped, so a commented-out include or `koto::` counts for nothing, and a
/// string literal keeps its text, so a `#[path]` value can be read while a
/// `koto::` inside a string names nothing.
#[derive(Debug, PartialEq)]
enum Token {
    Ident(String),
    Punct(char),
    Str(String),
    Other,
}

fn ident(word: &str) -> Token {
    Token::Ident(word.to_string())
}

/// The body of a string literal whose opening quote is just before `i`, and
/// the index after its closing quote.
fn string_body(s: &[char], mut i: usize) -> (String, usize) {
    let mut text = String::new();
    while i < s.len() && s[i] != '"' {
        if s[i] == '\\' {
            i += 1;
        }
        if let Some(&c) = s.get(i) {
            text.push(c);
        }
        i += 1;
    }
    (text, i + 1)
}

/// The body of a raw string literal whose `r` is just before `i`, and the
/// index after it, or `None` when what follows isn't a raw string.
fn raw_string_body(s: &[char], mut i: usize) -> Option<(String, usize)> {
    let hashes = s[i..].iter().take_while(|&&c| c == '#').count();
    i += hashes;
    if s.get(i) != Some(&'"') {
        return None;
    }
    let start = i + 1;
    let end = (start..s.len()).find(|&j| {
        s[j] == '"' && s.len() - j > hashes && s[j + 1..=j + hashes].iter().all(|&c| c == '#')
    })?;
    Some((s[start..end].iter().collect(), end + 1 + hashes))
}

fn tokens(src: &str) -> Vec<Token> {
    let s: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let next = s.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < s.len() && s[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < s.len() {
                if s[i] == '/' && s.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if s[i] == '*' && s.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            let (text, end) = string_body(&s, i + 1);
            out.push(Token::Str(text));
            i = end;
        } else if c == '\'' {
            // A character literal, or the quote of a lifetime, whose name
            // comes next as an identifier.
            if next == Some('\\') {
                i += 3;
                while i < s.len() && s[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if s.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
            out.push(Token::Other);
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < s.len() && (s[i].is_alphanumeric() || s[i] == '_') {
                i += 1;
            }
            let word: String = s[start..i].iter().collect();
            let raw = match word.as_str() {
                "r" | "br" | "cr" => raw_string_body(&s, i),
                "b" | "c" if s.get(i) == Some(&'"') => Some(string_body(&s, i + 1)),
                _ => None,
            };
            if let Some((text, end)) = raw {
                out.push(Token::Str(text));
                i = end;
            } else if word == "r" && s.get(i) == Some(&'#') {
                // A raw identifier, `r#name`: the name comes next.
                i += 1;
            } else if c.is_ascii_digit() {
                out.push(Token::Other);
            } else {
                out.push(Token::Ident(word));
            }
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

fn path_separator_at(toks: &[Token], i: usize) -> bool {
    toks.get(i) == Some(&Token::Punct(':')) && toks.get(i + 1) == Some(&Token::Punct(':'))
}

/// Whether the tokens name the `koto` crate: a path that starts with `koto`
/// (`koto::x`, `::koto::x`), `use koto;`, or `extern crate koto`. A `koto`
/// inside another path (`a::koto`) or after a `.` is something else.
fn names_koto(toks: &[Token]) -> bool {
    (0..toks.len()).any(|i| {
        if toks[i] != ident("koto") {
            return false;
        }
        let before = |back: usize| i.checked_sub(back).map(|j| &toks[j]);
        if before(1) == Some(&Token::Punct('.')) {
            return false;
        }
        let mut start = i;
        if i >= 2 && path_separator_at(toks, i - 2) {
            match before(3) {
                Some(Token::Ident(w)) if w != "use" => return false,
                Some(Token::Punct('>')) => return false,
                _ => start = i - 2,
            }
        }
        let prev = start.checked_sub(1).map(|j| &toks[j]);
        path_separator_at(toks, i + 1)
            || prev == Some(&ident("use"))
            || (prev == Some(&ident("crate"))
                && start
                    .checked_sub(2)
                    .is_some_and(|j| toks[j] == ident("extern")))
    })
}

/// The `mod name;` items in a file, each with the value of its `#[path]`
/// attribute, if it has one. Inline `mod name { .. }` blocks are read as
/// part of the file.
fn module_items(toks: &[Token]) -> Vec<(String, Option<String>)> {
    let mut items = Vec::new();
    let mut path = None;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Token::Punct('#') => {
                let mut open = i + 1;
                if toks.get(open) == Some(&Token::Punct('!')) {
                    open += 1;
                }
                if toks.get(open) == Some(&Token::Punct('[')) {
                    let mut depth = 0;
                    let mut close = open;
                    while close < toks.len() {
                        match toks[close] {
                            Token::Punct('[') => depth += 1,
                            Token::Punct(']') => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            break;
                        }
                        close += 1;
                    }
                    if let [Token::Ident(name), Token::Punct('='), Token::Str(value)] =
                        &toks[open + 1..close.min(toks.len())]
                    {
                        if name == "path" {
                            path = Some(value.clone());
                        }
                    }
                    i = close + 1;
                    continue;
                }
            }
            Token::Ident(w) if w == "pub" => {
                i += 1;
                if toks.get(i) == Some(&Token::Punct('(')) {
                    while i < toks.len() && toks[i] != Token::Punct(')') {
                        i += 1;
                    }
                    i += 1;
                }
                continue;
            }
            Token::Ident(w) if w == "mod" => {
                if let (Some(Token::Ident(name)), Some(Token::Punct(';'))) =
                    (toks.get(i + 1), toks.get(i + 2))
                {
                    items.push((name.clone(), path.take()));
                    i += 3;
                    continue;
                }
            }
            _ => {}
        }
        path = None;
        i += 1;
    }
    items
}

/// The files of the test crate whose root is `root`, each with its tokens:
/// the root and every module file a `mod` item loads, however deep. As
/// rustc does, a `#[path]` is read from the declaring file's directory, and
/// a module without one sits beside a crate root, a `mod.rs` or a file
/// loaded through `#[path]`, and in a directory named after any other file.
fn crate_files(root: &Path) -> Vec<(PathBuf, Vec<Token>)> {
    let mut files: Vec<(PathBuf, Vec<Token>)> = Vec::new();
    let mut todo = vec![(root.to_path_buf(), true)];
    while let Some((file, owns_dir)) = todo.pop() {
        let file = std::fs::canonicalize(&file)
            .unwrap_or_else(|e| panic!("can't read {}: {e}", file.display()));
        if files.iter().any(|(f, _)| *f == file) {
            continue;
        }
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("can't read {}: {e}", file.display()));
        let toks = tokens(&text);
        let parent = file.parent().unwrap();
        for (name, path) in module_items(&toks) {
            let child = match &path {
                Some(path) => parent.join(path),
                None => {
                    let dir = if owns_dir {
                        parent.to_path_buf()
                    } else {
                        parent.join(file.file_stem().unwrap())
                    };
                    [
                        dir.join(format!("{name}.rs")),
                        dir.join(name.as_str()).join("mod.rs"),
                    ]
                    .into_iter()
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| {
                        panic!("{} declares mod {name}, found in no file", file.display())
                    })
                }
            };
            let owns = path.is_some() || child.file_name().is_some_and(|n| n == "mod.rs");
            todo.push((child, owns));
        }
        files.push((file, toks));
    }
    files
}

/// The roots of the integration test crates Cargo builds: `tests/*.rs` and
/// `tests/<dir>/main.rs`.
fn test_crates(tests: &Path) -> Vec<PathBuf> {
    let mut crates: Vec<PathBuf> = std::fs::read_dir(tests)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter_map(|p| {
            if p.is_dir() {
                Some(p.join("main.rs")).filter(|m| m.is_file())
            } else {
                Some(p).filter(|p| p.extension().is_some_and(|e| e == "rs"))
            }
        })
        .collect();
    crates.sort();
    crates
}

#[test]
fn every_test_that_names_koto_includes_the_host_session_module() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tests = manifest.join("tests");
    let module = std::fs::canonicalize(tests.join("support/host_session.rs")).unwrap();

    let mut linked = 0;
    let mut found = Vec::new();
    for root in test_crates(&tests) {
        let files = crate_files(&root);
        if !files.iter().any(|(_, toks)| names_koto(toks)) {
            continue;
        }
        linked += 1;
        if !files.iter().any(|(file, _)| *file == module) {
            found.push(
                root.strip_prefix(manifest)
                    .unwrap_or(&root)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        linked > 1,
        "found no test file under {} that names the koto crate",
        tests.display()
    );
    assert!(
        found.is_empty(),
        "these test files run koto's library in-process without \
         tests/support/host_session.rs, so a suite run inside Claude Code reads \
         the hosting session's CLAUDE_CODE_SESSION_ID and writes into its \
         /workflows directory; add\n\n#[path = \"support/host_session.rs\"]\n\
         mod host_session;\n\nto each of (with the path from its own directory):\n  {}",
        found.join("\n  ")
    );
}

/// `src/host_env.rs` reads every host variable through `host_env`, and
/// `HOST_SESSION_ENV` must name each one, or an in-process test reads the
/// new one from the hosting session. A call is read from its argument: a
/// string literal, or a `&str` constant declared anywhere in `src/`.
#[test]
fn every_variable_read_through_host_env_is_in_host_session_env() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    files_under(&manifest.join("src"), &mut sources);
    sources.retain(|p| p.extension().is_some_and(|e| e == "rs"));
    sources.sort();
    let sources: Vec<(PathBuf, Vec<Token>)> = sources
        .into_iter()
        .map(|p| {
            let toks = tokens(&std::fs::read_to_string(&p).unwrap());
            (p, toks)
        })
        .collect();

    // `const NAME: &str = "value";`, and `&'static str` alike.
    let mut consts: HashMap<&str, Vec<&str>> = HashMap::new();
    for (_, toks) in &sources {
        for i in 0..toks.len() {
            let (Token::Ident(kw), Some(Token::Ident(name))) = (&toks[i], toks.get(i + 1)) else {
                continue;
            };
            if kw != "const" || toks.get(i + 2) != Some(&Token::Punct(':')) {
                continue;
            }
            let Some(eq) = (i + 3..toks.len()).find(|&j| toks[j] == Token::Punct('=')) else {
                continue;
            };
            if !toks[i + 3..eq].contains(&ident("str")) {
                continue;
            }
            if let (Some(Token::Str(value)), Some(Token::Punct(';'))) =
                (toks.get(eq + 1), toks.get(eq + 2))
            {
                consts
                    .entry(name.as_str())
                    .or_default()
                    .push(value.as_str());
            }
        }
    }

    let mut reads = Vec::new();
    for (file, toks) in &sources {
        let file = file.strip_prefix(manifest).unwrap_or(file).display();
        for i in 0..toks.len() {
            if toks[i] != ident("host_env")
                || toks.get(i + 1) != Some(&Token::Punct('('))
                || (i > 0 && toks[i - 1] == ident("fn"))
            {
                continue;
            }
            let close = (i + 2..toks.len())
                .find(|&j| toks[j] == Token::Punct(')'))
                .unwrap_or(toks.len());
            let arg = &toks[i + 2..close];
            let names: Vec<&str> = match arg.last() {
                Some(Token::Str(value)) if arg.len() == 1 => vec![value.as_str()],
                Some(Token::Ident(name))
                    if arg[..arg.len() - 1]
                        .iter()
                        .all(|t| matches!(t, Token::Ident(_) | Token::Punct(':'))) =>
                {
                    consts.get(name.as_str()).cloned().unwrap_or_else(|| {
                        panic!("{file}: host_env reads {name}, a constant this test can't find")
                    })
                }
                _ => panic!(
                    "{file}: host_env reads {arg:?}, which this test can't resolve to a \
                     variable name; pass a string literal or a &str constant"
                ),
            };
            reads.extend(names.into_iter().map(|n| (n, file.to_string())));
        }
    }

    assert!(
        reads.len() > 1,
        "found no call to host_env in src/, so this check checks nothing"
    );
    let missing: Vec<String> = reads
        .iter()
        .filter(|(name, _)| !HOST_SESSION_ENV.contains(name))
        .map(|(name, file)| format!("{name} (read in {file})"))
        .collect();
    assert!(
        missing.is_empty(),
        "src/ reads these host variables through host_env, but HOST_SESSION_ENV in \
         tests/support/host_session.rs doesn't list them, so an in-process test reads \
         them from the hosting session; add each to the list:\n  {}",
        missing.join("\n  ")
    );
}

/// The probe the tests below run in a child of this binary: it appends
/// through `LocalBackend` in-process, as the session update the hosting
/// session's id once leaked into did. Elsewhere it does nothing.
#[test]
fn probe_appends_in_process_under_a_hosting_session() {
    let Some(root) = std::env::var_os(PROBE) else {
        return;
    };
    let root = PathBuf::from(root);
    let sessions = root.join("sessions");
    let mut init = koto_command::koto();
    for name in HOST_SESSION_ENV {
        init.env_remove(name);
    }
    init.current_dir(&root)
        .env("HOME", root.join("home"))
        .env("KOTO_SESSIONS_BASE", &sessions)
        .args(["init", WORKFLOW, "--template", "probe.md"])
        .assert()
        .success();

    for name in HOST_SESSION_ENV {
        match std::env::var_os(format!("{LEAK_PREFIX}{name}")) {
            Some(value) => std::env::set_var(name, value),
            None => assert!(
                std::env::var_os(name).is_none(),
                "{name} is still set after the binary loaded"
            ),
        }
    }
    let backend = LocalBackend::with_base_dir(sessions);
    koto::cli::session::handle_update(&backend, WORKFLOW, "probe intent").unwrap();
}

/// One run of the probe in a child of this binary, with the hosting
/// session's variables set in its environment.
struct Probe {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Probe {
    /// Runs the probe, setting `leak` again after load when given.
    fn run(leak: Option<&str>) -> Probe {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("probe.md"), TEMPLATE).unwrap();
        let project = root.join("home/.claude/projects/probe-project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{SESSION_ID}.jsonl")), "").unwrap();
        let probe = Probe { _dir: dir, root };

        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        koto_command::scrub(&mut cmd)
            .current_dir(&probe.root)
            .env("HOME", probe.root.join("home"))
            .env(PROBE, &probe.root)
            .env("CLAUDE_CODE_SESSION_ID", SESSION_ID)
            .env("KOTO_WORKFLOWS_DIR", probe.handoff())
            .args([
                "--exact",
                "probe_appends_in_process_under_a_hosting_session",
                "--test-threads=1",
            ]);
        if let Some(name) = leak {
            let value = match name {
                "CLAUDE_CODE_SESSION_ID" => SESSION_ID.into(),
                "KOTO_WORKFLOWS_DIR" => probe.handoff(),
                other => panic!("no control value for {other}"),
            };
            cmd.env(format!("{LEAK_PREFIX}{name}"), value);
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "the probe failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the probe didn't run:\n{stdout}"
        );
        probe
    }

    /// The directory handed over in `KOTO_WORKFLOWS_DIR`.
    fn handoff(&self) -> PathBuf {
        self.root.join("handoff")
    }

    /// The hosting session's `/workflows` directory, beside its transcript.
    fn session_workflows(&self) -> PathBuf {
        self.root
            .join("home/.claude/projects/probe-project")
            .join(SESSION_ID)
            .join("workflows")
    }

    fn files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        files_under(dir, &mut out);
        out
    }
}

#[test]
fn an_in_process_append_leaves_the_hosting_session_alone() {
    let probe = Probe::run(None);

    // `koto init` caches the compiled template under the home's `.cache`,
    // so only Claude Code's directory is checked.
    let claude = probe.files(&probe.root.join("home/.claude"));
    assert_eq!(
        claude,
        vec![probe
            .root
            .join("home/.claude/projects/probe-project")
            .join(format!("{SESSION_ID}.jsonl"))],
        "the in-process append wrote into the hosting session's directory"
    );
    assert!(
        !probe.handoff().exists(),
        "the in-process append wrote into KOTO_WORKFLOWS_DIR: {:?}",
        probe.files(&probe.handoff())
    );

    let sessions = probe.root.join("sessions");
    for file in probe.files(&sessions) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SESSION_ID),
            "{} records the hosting session's id",
            file.display()
        );
    }
    let (_, events) = LocalBackend::with_base_dir(sessions)
        .read_events(WORKFLOW)
        .unwrap();
    assert_eq!(
        events.last().map(|e| e.event_type.as_str()),
        Some("intent_updated")
    );
}

#[test]
fn control_a_session_id_set_after_load_writes_into_its_workflows() {
    let probe = Probe::run(Some("CLAUDE_CODE_SESSION_ID"));
    assert!(
        !probe.files(&probe.session_workflows()).is_empty(),
        "the control append wrote no workflow file under {}, so an empty one \
         proves nothing",
        probe.session_workflows().display()
    );
}

#[test]
fn control_a_workflows_dir_set_after_load_is_written() {
    let probe = Probe::run(Some("KOTO_WORKFLOWS_DIR"));
    assert!(
        !probe.files(&probe.handoff()).is_empty(),
        "the control append wrote no workflow file under {}, so an empty one \
         proves nothing",
        probe.handoff().display()
    );
}
