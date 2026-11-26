//! Terminal detection reads a projection of the compiled template, and every
//! template read is the hardened one.
//!
//! - A session koto drives to a terminal state stays finished when its
//!   compiled template is rewritten with fields and a gate type this koto
//!   doesn't know: the full `CompiledTemplate` parse fails, the projection
//!   still answers, and `koto dashboard --once` prints the same rows.
//! - On every template a current koto parses (the sources under `plugins/`,
//!   `test/` and `tests/fixtures/`, compiled, and the compiled JSON
//!   fixtures), `is_terminal_state` gives the answer the full parse gave.
//! - With a session's template replaced by a FIFO, a directory or a sparse
//!   file one byte over the cap, the reads refuse without blocking, and a
//!   symlink to a template outside the store reads as its target.
//! - A refusal's reason is a fixed category and the path, never the file's
//!   bytes.
//!
//! Dashboard rows are compared as `tests/dashboard_golden_test.rs` compares
//! them: cut to columns 1, 2, 4, 5, 6 and 8, leaving out the two that move
//! with the clock.

#![cfg(unix)]

#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use koto_session_reader::dashboard::{read_detail, DetailData};
use koto_session_reader::engine::persistence::{
    is_terminal_state, read_terminal_state, TemplateReadCategory, TemplateReadError,
};
use koto_session_reader::source::TEMPLATE_READ_CAP;
use koto_session_reader::template::types::CompiledTemplate;
use serde_json::{json, Value};
use tempfile::TempDir;

/// How long one `koto` run may take before the test calls it blocked.
const BINARY_DEADLINE: Duration = Duration::from_secs(60);

/// How long one in-process read may take before the test calls it blocked.
const IN_PROCESS_DEADLINE: Duration = Duration::from_secs(10);

/// The golden session in a terminal state whose template the terminal plants
/// replace. Only `--once --all` prints it, with the status `done`.
const DONE_SESSION: &str = "golden-done";

/// The golden session whose detail the detail plants read: it sits in
/// `review` after a failed gate, so its detail has a directive, a gate
/// condition and remaining states.
const DETAIL_SESSION: &str = "golden-blocked";

/// A byte string that must never reach a reason's `Display` or `Debug`.
const PLANTED: &str = "PLANTED-7f3a9c-template-bytes";

const VIEWS: [&[&str]; 2] = [&["--once"], &["--once", "--all"]];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Runs `f` on a thread, failing the test if it hasn't returned within
/// [`IN_PROCESS_DEADLINE`].
fn within_deadline<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(IN_PROCESS_DEADLINE)
        .unwrap_or_else(|_| panic!("{what} didn't return within {IN_PROCESS_DEADLINE:?}"))
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// A `--once` line cut to columns 1, 2, 4, 5, 6 and 8.
fn project(line: &str) -> String {
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(
        fields.len(),
        10,
        "a --once line has ten tab-separated columns: {line:?}"
    );
    [1, 2, 4, 5, 6, 8]
        .iter()
        .map(|n| fields[n - 1])
        .collect::<Vec<_>>()
        .join("\t")
}

/// A projected row's id and status columns.
fn id_and_status(row: &str) -> (&str, &str) {
    let fields: Vec<&str> = row.split('\t').collect();
    (fields[0], fields[2])
}

/// Runs `koto <args>` in `dir` with its sessions in `sessions`, failing the
/// test if it hasn't exited within [`BINARY_DEADLINE`]. Returns the exit
/// code, stdout and stderr.
fn run_koto(dir: &Path, sessions: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(dir)
        .env("KOTO_SESSIONS_BASE", sessions)
        .env("HOME", dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .args(args)
        .timeout(BINARY_DEADLINE);
    let started = Instant::now();
    let output = cmd.output().unwrap();
    assert!(
        started.elapsed() < BINARY_DEADLINE,
        "koto {} didn't exit within {BINARY_DEADLINE:?}",
        args.join(" ")
    );
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The projected rows of `koto dashboard <view>`, which must exit 0.
fn dashboard_rows(dir: &Path, sessions: &Path, view: &[&str]) -> Vec<String> {
    let mut args = vec!["dashboard"];
    args.extend_from_slice(view);
    let (code, stdout, stderr) = run_koto(dir, sessions, &args);
    assert_eq!(
        code,
        Some(0),
        "koto dashboard {} exited {code:?}; stderr:\n{stderr}",
        view.join(" ")
    );
    stdout.lines().map(project).collect()
}

/// What `koto dashboard` prints in every view.
fn all_views(dir: &Path, sessions: &Path) -> Vec<Vec<String>> {
    VIEWS
        .iter()
        .map(|view| dashboard_rows(dir, sessions, view))
        .collect()
}

/// The full-parse rule `is_terminal_state` followed before the projection:
/// parse the whole `CompiledTemplate`, then read the state's `terminal`,
/// `false` on any error.
fn full_parse_rule(path: &Path, state: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(compiled) = serde_json::from_slice::<CompiledTemplate>(&bytes) else {
        return false;
    };
    compiled
        .states
        .get(state)
        .map(|s| s.terminal)
        .unwrap_or(false)
}

/// The golden store in `<tmp>/sessions`, with `<tmp>/outside` beside it.
struct Golden {
    tmp: TempDir,
    sessions: PathBuf,
    outside: PathBuf,
}

impl Golden {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let sessions = tmp.path().join("sessions");
        golden_store::build(&sessions, SystemTime::now());
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        Golden {
            tmp,
            sessions,
            outside,
        }
    }

    fn state_path(&self, id: &str) -> PathBuf {
        self.sessions
            .join(id)
            .join(format!("koto-{id}.state.jsonl"))
    }

    /// The compiled template in a golden session's directory, the file its
    /// log's relative `template_path` names.
    fn template_path(&self, id: &str) -> PathBuf {
        let found: Vec<PathBuf> = std::fs::read_dir(self.sessions.join(id))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        assert_eq!(found.len(), 1, "{id} holds one compiled template");
        found.into_iter().next().unwrap()
    }

    fn views(&self) -> Vec<Vec<String>> {
        all_views(self.tmp.path(), &self.sessions)
    }
}

/// Something a template path can be replaced by.
#[derive(Debug, Clone, Copy)]
enum Plant {
    Fifo,
    Directory,
    OverCap,
}

fn plant(path: &Path, what: Plant) {
    std::fs::remove_file(path).unwrap();
    match what {
        Plant::Fifo => mkfifo(path),
        Plant::Directory => std::fs::create_dir(path).unwrap(),
        Plant::OverCap => {
            use std::io::Write;
            let mut f = std::fs::File::create(path).unwrap();
            f.write_all(PLANTED.as_bytes()).unwrap();
            f.set_len(TEMPLATE_READ_CAP + 1).unwrap();
        }
    }
}

fn expected_category(what: Plant) -> TemplateReadCategory {
    match what {
        Plant::Fifo | Plant::Directory => TemplateReadCategory::NotRegular,
        Plant::OverCap => TemplateReadCategory::TooLarge,
    }
}

/// `read_terminal_state` on `path` under the in-process deadline.
fn terminal_read(path: &Path, state: &str) -> Result<bool, TemplateReadError> {
    let p = path.to_str().unwrap().to_string();
    let s = state.to_string();
    within_deadline("read_terminal_state", move || read_terminal_state(&p, &s))
}

/// `is_terminal_state` on `path` under the in-process deadline.
fn terminal_flag(path: &Path, state: &str) -> bool {
    let p = path.to_str().unwrap().to_string();
    let s = state.to_string();
    within_deadline("is_terminal_state", move || is_terminal_state(&p, &s))
}

/// `read_detail` on a golden session under the in-process deadline.
fn detail(store: &Golden, id: &str) -> DetailData {
    let path = store.state_path(id);
    let id = id.to_string();
    within_deadline("read_detail", move || read_detail(&path, &id))
        .expect("the session's detail reads")
}

fn has_gate_condition(d: &DetailData) -> bool {
    d.history.iter().any(|h| h.gate_condition.is_some())
}

// ---------------------------------------------------------------------------
// A session koto finished stays finished on a newer template
// ---------------------------------------------------------------------------

const TWO_STATE_TEMPLATE: &str = r#"---
name: projection-flow
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Do the work.

## done

Done.
"#;

/// The compiled template path a koto-written log names, resolved against
/// the session directory when it is relative.
fn logged_template_path(sessions: &Path, id: &str) -> PathBuf {
    let dir = sessions.join(id);
    let log = std::fs::read_to_string(dir.join(format!("koto-{id}.state.jsonl"))).unwrap();
    let template = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "workflow_initialized")
        .and_then(|v| v["payload"]["template_path"].as_str().map(str::to_string))
        .expect("the log has a workflow_initialized event naming its template");
    let p = PathBuf::from(template);
    if p.is_absolute() {
        p
    } else {
        dir.join(p)
    }
}

#[test]
fn a_finished_session_stays_finished_when_its_template_gains_unknown_fields() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let sessions = dir.join("sessions");
    let source = dir.join("projection-flow.md");
    std::fs::write(&source, TWO_STATE_TEMPLATE).unwrap();

    let id = "projection-done";
    let (code, _, stderr) = run_koto(
        dir,
        &sessions,
        &["init", id, "--template", source.to_str().unwrap()],
    );
    assert_eq!(code, Some(0), "koto init: {stderr}");
    let (code, stdout, stderr) = run_koto(
        dir,
        &sessions,
        &[
            "next",
            id,
            "--with-data",
            r#"{"marker":"done"}"#,
            "--no-cleanup",
        ],
    );
    assert_eq!(code, Some(0), "koto next: {stdout}\n{stderr}");

    let template = logged_template_path(&sessions, id);
    assert!(is_terminal_state(template.to_str().unwrap(), "done"));
    let before = all_views(dir, &sessions);
    let done_row = before[1]
        .iter()
        .find(|r| id_and_status(r).0 == id)
        .unwrap_or_else(|| panic!("--once --all prints {id}: {:?}", before[1]));
    assert_eq!(id_and_status(done_row).1, "done", "{done_row:?}");

    // Rewrite the compiled template the way a newer koto might write it.
    let mut compiled: Value = serde_json::from_slice(&std::fs::read(&template).unwrap()).unwrap();
    compiled["planted_top_level_field"] = json!({"from": "a newer koto"});
    compiled["states"]["done"]["planted_state_field"] = json!(["anything"]);
    compiled["review_vocabulary"] = json!({"seats": ["clarity"], "planted_vocab_field": true});
    compiled["states"]["work"]["gates"] = json!({
        "planted-gate": {"type": "planted-future-gate-type", "planted_gate_field": 1}
    });
    std::fs::write(&template, serde_json::to_vec_pretty(&compiled).unwrap()).unwrap();

    let bytes = std::fs::read(&template).unwrap();
    assert!(
        serde_json::from_slice::<CompiledTemplate>(&bytes).is_err(),
        "the full CompiledTemplate parse refuses the rewritten template"
    );
    let path = template.to_str().unwrap();
    assert!(is_terminal_state(path, "done"), "the terminal state");
    assert!(!is_terminal_state(path, "work"), "a non-terminal state");
    assert!(!is_terminal_state(path, "undeclared"), "an undeclared name");
    assert_eq!(read_terminal_state(path, "done"), Ok(true));
    assert_eq!(read_terminal_state(path, "undeclared"), Ok(false));

    let after = all_views(dir, &sessions);
    assert_eq!(after, before, "every view prints the same rows");
}

// ---------------------------------------------------------------------------
// The projection agrees with the full parse on every template koto parses
// ---------------------------------------------------------------------------

fn collect_files(root: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        let meta = std::fs::symlink_metadata(&p).unwrap();
        if meta.is_dir() {
            collect_files(&p, ext, out);
        } else if meta.is_file() && p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// One template the corpus compares, with where it came from.
struct Corpus {
    /// `plugins`, `test` or `tests/fixtures`.
    origin: &'static str,
    /// The repository path it was compiled or read from.
    from: PathBuf,
    /// The compiled JSON, written to a file of its own.
    compiled: PathBuf,
}

#[test]
fn terminal_detection_agrees_with_the_full_parse_on_every_template_koto_parses() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = TempDir::new().unwrap();
    let mut corpus: Vec<Corpus> = Vec::new();
    let mut refused_sources = 0usize;
    let mut skipped_json = 0usize;

    for origin in ["plugins", "test", "tests/fixtures"] {
        let mut sources = Vec::new();
        collect_files(&repo.join(origin), "md", &mut sources);
        for src in sources {
            match koto::template::compile::compile(&src, false) {
                Ok(compiled) => {
                    let out = scratch.path().join(format!("{}.json", corpus.len()));
                    std::fs::write(&out, serde_json::to_vec(&compiled).unwrap()).unwrap();
                    corpus.push(Corpus {
                        origin,
                        from: src,
                        compiled: out,
                    });
                }
                Err(_) => refused_sources += 1,
            }
        }
    }

    // Compiled JSON fixtures, read as they are. A JSON file that isn't a
    // compiled template, or that the full parse refuses, is not a template
    // a current koto parses, and is counted apart.
    let mut fixtures = Vec::new();
    collect_files(&repo.join("tests/fixtures"), "json", &mut fixtures);
    for f in fixtures {
        let bytes = std::fs::read(&f).unwrap();
        let is_compiled = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_some_and(|v| v.get("format_version").is_some() && v["states"].is_object());
        if is_compiled && serde_json::from_slice::<CompiledTemplate>(&bytes).is_ok() {
            corpus.push(Corpus {
                origin: "tests/fixtures",
                from: f.clone(),
                compiled: f,
            });
        } else {
            skipped_json += 1;
        }
    }

    let mut states_compared = 0usize;
    let mut terminal_seen = 0usize;
    let mut mismatches = Vec::new();
    for t in &corpus {
        let compiled: CompiledTemplate =
            serde_json::from_slice(&std::fs::read(&t.compiled).unwrap()).unwrap();
        let mut names: Vec<String> = compiled.states.keys().cloned().collect();
        names.push("state-no-template-declares".to_string());
        let path = t.compiled.to_str().unwrap();
        for name in &names {
            let want = full_parse_rule(&t.compiled, name);
            let got = is_terminal_state(path, name);
            states_compared += 1;
            if want {
                terminal_seen += 1;
            }
            if got != want {
                mismatches.push(format!(
                    "{} state {name}: projection {got}, full parse {want}",
                    t.from.display()
                ));
            }
        }
    }

    let from_plugins = corpus.iter().filter(|t| t.origin == "plugins").count();
    let from_fixtures = corpus.len() - from_plugins;
    println!(
        "compared {} templates ({from_plugins} from plugins/, {from_fixtures} from test/ and \
         tests/fixtures/) and {states_compared} state names, {terminal_seen} terminal; the \
         compiler refused {refused_sources} sources; {skipped_json} JSON fixtures are not \
         compiled templates koto parses",
        corpus.len()
    );
    assert!(
        mismatches.is_empty(),
        "mismatches:\n{}",
        mismatches.join("\n")
    );
    assert!(from_plugins > 0, "no template from plugins/ was compared");
    assert!(
        from_fixtures > 0,
        "no template from test/ or tests/fixtures/ was compared"
    );
    assert!(terminal_seen > 0, "no terminal state was compared");
}

// ---------------------------------------------------------------------------
// The template read is hardened for every caller
// ---------------------------------------------------------------------------

fn check_terminal_plant(what: Plant) {
    let store = Golden::new();
    let template = store.template_path(DONE_SESSION);
    assert!(terminal_flag(&template, "done"), "{what:?}: unplanted");
    let before = store.views();

    plant(&template, what);

    let refused = terminal_read(&template, "done");
    match refused {
        Err(e) => {
            assert_eq!(e.category, expected_category(what), "{what:?}");
            assert_eq!(e.path, template, "{what:?}");
        }
        Ok(v) => panic!("{what:?}: read_terminal_state answered {v}"),
    }
    assert!(!terminal_flag(&template, "done"), "{what:?}: not terminal");

    let after = store.views();
    for (view, (b, a)) in VIEWS.iter().zip(before.iter().zip(after.iter())) {
        let others = |rows: &[String]| -> Vec<String> {
            rows.iter()
                .filter(|r| id_and_status(r).0 != DONE_SESSION)
                .cloned()
                .collect()
        };
        assert_eq!(
            others(a),
            others(b),
            "{what:?}: every other row of {} stays",
            view.join(" ")
        );
        if let Some(row) = a.iter().find(|r| id_and_status(r).0 == DONE_SESSION) {
            assert_ne!(
                id_and_status(row).1,
                "done",
                "{what:?}: {DONE_SESSION} isn't done in {}",
                view.join(" ")
            );
        }
    }
}

fn check_detail_plant(what: Plant) {
    let store = Golden::new();
    let plain = detail(&store, DETAIL_SESSION);
    assert!(
        plain.directive.is_some(),
        "the unplanted detail's directive"
    );
    assert!(
        !plain.remaining.is_empty(),
        "the unplanted detail's remaining"
    );
    assert!(has_gate_condition(&plain), "the unplanted detail's gate");

    plant(&store.template_path(DETAIL_SESSION), what);

    let planted = detail(&store, DETAIL_SESSION);
    assert_eq!(planted.directive, None, "{what:?}: no directive");
    assert!(planted.remaining.is_empty(), "{what:?}: no remaining state");
    assert!(!has_gate_condition(&planted), "{what:?}: no gate condition");
    assert_eq!(planted.current_state, plain.current_state, "{what:?}");
}

#[test]
fn a_fifo_template_is_refused_without_blocking() {
    check_terminal_plant(Plant::Fifo);
    check_detail_plant(Plant::Fifo);
}

#[test]
fn a_directory_template_is_refused() {
    check_terminal_plant(Plant::Directory);
    check_detail_plant(Plant::Directory);
}

#[test]
fn an_over_cap_template_is_refused() {
    check_terminal_plant(Plant::OverCap);
    check_detail_plant(Plant::OverCap);
}

#[test]
fn a_symlinked_template_outside_the_store_reads_as_its_target() {
    let store = Golden::new();
    let plain = detail(&store, DETAIL_SESSION);
    let before = store.views();

    for id in [DONE_SESSION, DETAIL_SESSION] {
        let template = store.template_path(id);
        let target = store.outside.join(format!("{id}.json"));
        std::fs::rename(&template, &target).unwrap();
        std::os::unix::fs::symlink(&target, &template).unwrap();
    }

    let done = store.template_path(DONE_SESSION);
    assert_eq!(terminal_read(&done, "done"), Ok(true));
    assert!(terminal_flag(&done, "done"));
    assert!(!terminal_flag(&done, "work"));

    let linked = detail(&store, DETAIL_SESSION);
    assert_eq!(linked.directive, plain.directive);
    assert_eq!(linked.remaining, plain.remaining);
    assert!(has_gate_condition(&linked));

    assert_eq!(store.views(), before, "every view prints the same rows");
}

// ---------------------------------------------------------------------------
// Reasons carry no file bytes
// ---------------------------------------------------------------------------

fn assert_no_planted_bytes(err: &TemplateReadError, path: &Path, category: TemplateReadCategory) {
    let display = err.to_string();
    let debug = format!("{err:?}");
    assert_eq!(err.category, category, "{display}");
    assert_eq!(display, format!("{category}: {}", path.display()));
    assert!(
        !display.contains(PLANTED),
        "Display carries file bytes: {display}"
    );
    assert!(
        !debug.contains(PLANTED),
        "Debug carries file bytes: {debug}"
    );
}

#[test]
fn a_reason_is_a_fixed_category_and_the_path() {
    let tmp = TempDir::new().unwrap();
    let state = "work";

    let not_json = tmp.path().join("not-json.json");
    std::fs::write(&not_json, format!("{PLANTED} is not JSON")).unwrap();
    let malformed = tmp.path().join("malformed-state.json");
    std::fs::write(
        &malformed,
        format!(r#"{{"states": {{"work": {{"terminal": "{PLANTED}"}}}}}}"#),
    )
    .unwrap();
    let no_states = tmp.path().join("no-states.json");
    std::fs::write(&no_states, format!(r#"{{"name": "{PLANTED}"}}"#)).unwrap();
    let fifo = tmp.path().join("fifo.json");
    mkfifo(&fifo);
    let over_cap = tmp.path().join("over-cap.json");
    plant_new_over_cap(&over_cap);

    for (path, category) in [
        (&not_json, TemplateReadCategory::Malformed),
        (&malformed, TemplateReadCategory::Malformed),
        (&no_states, TemplateReadCategory::Malformed),
        (&fifo, TemplateReadCategory::NotRegular),
        (&over_cap, TemplateReadCategory::TooLarge),
    ] {
        let err = terminal_read(path, state).expect_err("the read refuses");
        assert_no_planted_bytes(&err, path, category);
        assert!(!terminal_flag(path, state));
    }

    let missing = tmp.path().join("missing.json");
    let err = terminal_read(&missing, state).expect_err("a missing template refuses");
    assert_eq!(err.category, TemplateReadCategory::NotFound);
    assert_eq!(
        err.to_string(),
        format!("template not found: {}", missing.display())
    );
}

fn plant_new_over_cap(path: &Path) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(PLANTED.as_bytes()).unwrap();
    f.set_len(TEMPLATE_READ_CAP + 1).unwrap();
}
