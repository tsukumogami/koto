//! Run journal rotation, driven through the real `koto` binary in a scratch
//! HOME. Rotation is off until `run_journal.rotate_bytes` is set in user
//! config; these tests set small bounds so ordinary commands cross them.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const JOURNAL: &str = "_run_journal.jsonl";

/// `work` loops on `again: yes`, so every tick enters a state and writes a
/// journal line.
const TEMPLATE: &str = r#"---
name: rotation-loop
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      again:
        type: enum
        required: true
        values: ["yes", "no"]
    transitions:
      - target: work
        when:
          again: "yes"
      - target: done
        when:
          again: "no"
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    work: PathBuf,
    template: PathBuf,
    /// `KOTO_SESSIONS_BASE`, when the store is redirected.
    base: Option<PathBuf>,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = work.join("rotation-loop.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            work,
            template,
            base: None,
        }
    }

    fn redirected(mut self) -> Self {
        let base = self.home.parent().unwrap().join("store");
        std::fs::create_dir_all(&base).unwrap();
        self.base = Some(base);
        self
    }

    /// Where this env's journal lives.
    fn root(&self) -> PathBuf {
        self.base.clone().unwrap_or_else(|| self.home.join(".koto"))
    }

    fn cmd(&self) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        match &self.base {
            Some(b) => cmd.env("KOTO_SESSIONS_BASE", b),
            None => cmd.env_remove("KOTO_SESSIONS_BASE"),
        };
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn set_bound(&self, bytes: u64) {
        self.ok(&[
            "config",
            "set",
            "run_journal.rotate_bytes",
            &bytes.to_string(),
            "--user",
        ]);
    }

    fn start(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
    }

    /// One `koto next` round of the loop. Every journal write it makes,
    /// its `response` record included, waits up to five seconds for the
    /// lock, far longer than these writers ever hold it, so it never prints
    /// a journal warning, however many writers contend.
    fn tick(&self, name: &str) {
        let out = self.run(&[
            "next",
            name,
            "--with-data",
            r#"{"again": "yes"}"#,
            "--no-cleanup",
        ]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "koto next {name}: {stderr}");
        assert!(
            !stderr.contains("run journal"),
            "koto next {name}: {stderr}"
        );
    }

    /// Rotated files, oldest first, by the naming rule.
    fn rotated(&self) -> Vec<PathBuf> {
        let mut names: Vec<String> = std::fs::read_dir(self.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| is_rotated_name(n))
            .collect();
        names.sort();
        names.into_iter().map(|n| self.root().join(n)).collect()
    }

    /// Every journal file, oldest first, the active one last.
    fn all_files(&self) -> Vec<PathBuf> {
        let mut files = self.rotated();
        let active = self.root().join(JOURNAL);
        if active.exists() {
            files.push(active);
        }
        files
    }

    /// Every line across every journal file, in order.
    fn all_lines(&self) -> Vec<String> {
        self.all_files()
            .iter()
            .flat_map(|f| {
                std::fs::read_to_string(f)
                    .unwrap()
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// `_run_journal.` + ten digits + `.jsonl`.
fn is_rotated_name(name: &str) -> bool {
    name.strip_prefix("_run_journal.")
        .and_then(|r| r.strip_suffix(".jsonl"))
        .is_some_and(|d| d.len() == 10 && d.bytes().all(|b| b.is_ascii_digit()))
}

fn session_of(line: &str) -> String {
    let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("torn line ({e}): {line}"));
    v["session"].as_str().unwrap().to_string()
}

/// Whether `line` was written in the same batch as `prev`, the line before
/// it. One command's commit is one batch: `koto init` writes
/// `session_started` and the initial `state_entered`; a `driver_seen` rides
/// with the record that follows it; a check event's records all carry its
/// `koto.seq`, and so do a closing visit's `context_reads`, which ride with
/// the `state_entered` or `cancelled` that follows them.
fn same_batch(prev: &str, line: &str) -> bool {
    let p: Value = serde_json::from_str(prev).unwrap();
    let l: Value = serde_json::from_str(line).unwrap();
    if p["session"] != l["session"] {
        return false;
    }
    (p["kind"] == "session_started" && l["kind"] == "state_entered")
        || p["kind"] == "driver_seen"
        || (p["kind"] == "context_reads"
            && (l["kind"] == "state_entered" || l["kind"] == "cancelled"))
        || (!l["koto.seq"].is_null() && p["koto.seq"] == l["koto.seq"])
}

/// Every rotated file was moved only once it reached the bound, and nothing
/// was written to it after: all of it but its last batch is under the
/// bound. And no batch was split: the first line of each later file starts
/// a batch of its own.
fn assert_rotated_at_the_bound(files: &[PathBuf], bound: u64) {
    let mut prev_last: Option<String> = None;
    for f in files {
        let text = std::fs::read_to_string(f).unwrap();
        let size = text.len() as u64;
        let lines: Vec<&str> = text.lines().collect();
        if let Some(prev) = &prev_last {
            assert!(
                !same_batch(prev, lines[0]),
                "a batch was split across files at {}",
                f.display()
            );
        }
        let mut start = lines.len() - 1;
        while start > 0 && same_batch(lines[start - 1], lines[start]) {
            start -= 1;
        }
        let last_batch: u64 = lines[start..].iter().map(|l| l.len() as u64 + 1).sum();
        let before_last = size - last_batch;
        assert!(
            size >= bound,
            "{} is {size} bytes, under {bound}",
            f.display()
        );
        assert!(
            before_last < bound,
            "{} grew past the bound before its last batch ({before_last} >= {bound})",
            f.display()
        );
        prev_last = Some(lines[lines.len() - 1].to_string());
    }
}

#[test]
fn an_unconfigured_journal_never_rotates() {
    let env = Env::new();
    assert_eq!(
        env.ok(&["config", "get", "run_journal.rotate_bytes"])
            .trim(),
        "0"
    );
    assert_eq!(
        env.ok(&["config", "get", "run_journal.retain_days"]).trim(),
        "30"
    );
    assert_eq!(
        env.ok(&["config", "get", "run_journal.reader_stale_days"])
            .trim(),
        "90"
    );
    env.start("plain");
    for _ in 0..40 {
        env.tick("plain");
    }
    assert!(env.rotated().is_empty());
    assert!(std::fs::metadata(env.root().join(JOURNAL)).unwrap().len() > 4000);
}

#[test]
fn a_configured_bound_rotates_with_ordered_names() {
    let env = Env::new();
    let bound = 300;
    env.set_bound(bound);
    env.start("rot");
    for _ in 0..40 {
        env.tick("rot");
    }
    let rotated = env.rotated();
    assert!(rotated.len() >= 10, "only {} rotations", rotated.len());
    let names: Vec<String> = rotated
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for (i, name) in names.iter().enumerate() {
        assert_eq!(name, &format!("_run_journal.{:010}.jsonl", i + 1));
    }
    assert_rotated_at_the_bound(&rotated, bound);
    assert!(env.root().join("_run_journal.readers").is_dir());
    // Every line parses and belongs to the one session.
    for line in env.all_lines() {
        assert_eq!(session_of(&line), "rot");
    }
}

#[test]
fn eight_concurrent_writers_across_rotations_lose_and_repeat_nothing() {
    const WRITERS: usize = 8;
    const TICKS: usize = 15;
    let bound = 1000;

    // The same run without rotation, for the expected count per session.
    let control = Env::new();
    control.start("control");
    for _ in 0..TICKS {
        control.tick("control");
    }
    let per_session = control.all_lines().len();

    let env = Env::new();
    env.set_bound(bound);
    for w in 0..WRITERS {
        env.start(&format!("w{w}"));
    }
    std::thread::scope(|s| {
        for w in 0..WRITERS {
            let env = &env;
            s.spawn(move || {
                for _ in 0..TICKS {
                    env.tick(&format!("w{w}"));
                }
            });
        }
    });

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for line in env.all_lines() {
        *counts.entry(session_of(&line)).or_default() += 1;
        assert!(seen.insert(line.clone()), "line written twice: {line}");
    }
    assert_eq!(counts.len(), WRITERS);
    for (session, n) in counts {
        assert_eq!(
            n, per_session,
            "{session} has {n} lines, expected {per_session}"
        );
    }
    let rotated = env.rotated();
    assert!(rotated.len() >= 2, "only {} rotations", rotated.len());
    assert_rotated_at_the_bound(&rotated, bound);
}

/// `read` checks two context keys and moves on to `rest`, which loops back
/// on `again: yes`. Each tick past the first enters `read`, whose gates read
/// both keys and pass, so the tick then leaves `read`: that commit writes
/// the visit's two `context_reads` and the `state_entered` for `rest` as one
/// batch.
const CONTEXT_TEMPLATE: &str = r#"---
name: rotation-context
version: "1.0"
initial_state: read
states:
  read:
    gates:
      has_spec:
        type: context-exists
        key: spec
      note_ok:
        type: context-matches
        key: note
        pattern: "^ok"
    transitions:
      - target: rest
  rest:
    accepts:
      again:
        type: enum
        required: true
        values: ["yes", "no"]
    transitions:
      - target: read
        when:
          again: "yes"
      - target: done
        when:
          again: "no"
  done:
    terminal: true
---

## read

Read.

## rest

Rest.

## done

Done.
"#;

/// Four writers each end eleven visits that read context, under a bound
/// smaller than one closing batch, so nearly every closing commit finds the
/// journal over the bound. Each visit's two `context_reads` land in the
/// same file as the `state_entered` that ends the visit, right before it,
/// whatever the other writers do, and every session has exactly the lines
/// its visits call for.
#[test]
fn a_closing_batch_with_context_reads_lands_whole_in_one_file() {
    const WRITERS: usize = 4;
    const TICKS: usize = 10;
    let bound = 700;

    let env = Env::new();
    env.set_bound(bound);
    let template = env.work.join("rotation-context.md");
    std::fs::write(&template, CONTEXT_TEMPLATE).unwrap();
    let content = env.work.join("context-input.txt");
    std::fs::write(&content, "ok, the note").unwrap();
    for w in 0..WRITERS {
        let name = format!("c{w}");
        env.ok(&["init", &name, "--template", template.to_str().unwrap()]);
        for key in ["spec", "note"] {
            env.ok(&[
                "context",
                "add",
                &name,
                key,
                "--from-file",
                content.to_str().unwrap(),
            ]);
        }
    }
    std::thread::scope(|s| {
        for w in 0..WRITERS {
            let env = &env;
            s.spawn(move || {
                let name = format!("c{w}");
                // The first tick evaluates `read`'s gates and leaves it.
                let out = env.run(&["next", &name, "--no-cleanup"]);
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(out.status.success(), "koto next {name}: {stderr}");
                assert!(!stderr.contains("run journal"), "{stderr}");
                for _ in 0..TICKS {
                    env.tick(&name);
                }
            });
        }
    });

    // Every line, with the index of the file it is in.
    let mut lines: Vec<(usize, Value)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, f) in env.all_files().iter().enumerate() {
        for line in std::fs::read_to_string(f).unwrap().lines() {
            assert!(seen.insert(line.to_string()), "line written twice: {line}");
            let v: Value =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("torn line ({e}): {line}"));
            lines.push((i, v));
        }
    }

    // Walk each run of one session's `context_reads` sharing a `koto.seq`.
    let mut runs: BTreeMap<String, usize> = BTreeMap::new();
    let mut i = 0;
    while i < lines.len() {
        let (file, first) = &lines[i];
        if first["kind"] != "context_reads" {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len()
            && lines[j].1["kind"] == "context_reads"
            && lines[j].1["session"] == first["session"]
            && lines[j].1["koto.seq"] == first["koto.seq"]
        {
            assert_eq!(lines[j].0, *file, "a visit's reads span two files");
            j += 1;
        }
        let keys: Vec<&str> = lines[i..j]
            .iter()
            .map(|(_, r)| r["koto.context.key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, vec!["spec", "note"], "{first}");
        let (end_file, ender) = lines
            .get(j)
            .unwrap_or_else(|| panic!("the journal ends after a visit's reads: {first}"));
        assert_eq!(
            *end_file, *file,
            "the reads and their state entry split: {ender}"
        );
        assert_eq!(ender["kind"], "state_entered", "{ender}");
        assert_eq!(ender["session"], first["session"], "{ender}");
        assert_eq!(ender["koto.state"], "rest", "{ender}");
        *runs
            .entry(first["session"].as_str().unwrap().to_string())
            .or_default() += 1;
        i = j + 1;
    }
    for w in 0..WRITERS {
        let name = format!("c{w}");
        assert_eq!(runs.get(&name), Some(&(TICKS + 1)), "{name}: {runs:?}");
        let reads = lines
            .iter()
            .filter(|(_, r)| r["session"] == name.as_str() && r["kind"] == "context_reads")
            .count();
        assert_eq!(reads, 2 * (TICKS + 1), "{name}");
    }
    let rotated = env.rotated();
    assert!(rotated.len() >= 4, "only {} rotations", rotated.len());
    assert_rotated_at_the_bound(&rotated, bound);
}

/// The documented offset reader: keep `(dev, inode, offset)`; on each pass
/// open the active journal and `fstat` it; on a different identity, finish
/// the file with the saved identity from the saved offset, read each newer
/// rotated file from 0, then the active one from 0.
struct OffsetReader {
    dev: u64,
    ino: u64,
    offset: u64,
}

impl OffsetReader {
    fn pass(&mut self, env: &Env) -> String {
        let mut out = String::new();
        let mut active = std::fs::File::open(env.root().join(JOURNAL)).unwrap();
        let meta = active.metadata().unwrap();
        if (meta.dev(), meta.ino()) != (self.dev, self.ino) {
            let rotated = env.rotated();
            let start = rotated
                .iter()
                .position(|p| {
                    let m = std::fs::metadata(p).unwrap();
                    (m.dev(), m.ino()) == (self.dev, self.ino)
                })
                .expect("the file the reader was in is gone");
            for (i, path) in rotated.iter().enumerate().skip(start) {
                let mut f = std::fs::File::open(path).unwrap();
                let m = f.metadata().unwrap();
                if (m.dev(), m.ino()) == (meta.dev(), meta.ino()) {
                    continue; // rotated during this pass; read last as active
                }
                if i == start {
                    f.seek(SeekFrom::Start(self.offset)).unwrap();
                }
                f.read_to_string(&mut out).unwrap();
            }
            self.dev = meta.dev();
            self.ino = meta.ino();
            self.offset = 0;
        }
        active.seek(SeekFrom::Start(self.offset)).unwrap();
        let mut rest = String::new();
        active.read_to_string(&mut rest).unwrap();
        self.offset += rest.len() as u64;
        out.push_str(&rest);
        out
    }
}

#[test]
fn an_offset_reader_stopped_mid_file_neither_loses_nor_repeats_a_line() {
    let env = Env::new();
    env.set_bound(600);
    env.start("followed");
    // Tick until the active file holds at least two lines (a tick writes a
    // `state_entered` and then a `response`, and either may rotate first).
    let path = env.root().join(JOURNAL);
    let mut text = String::new();
    for _ in 0..5 {
        env.tick("followed");
        text = std::fs::read_to_string(&path).unwrap();
        if text.lines().count() >= 2 {
            break;
        }
    }

    // The reader has read everything rotated so far and stops part-way
    // through the active file, at a line boundary.
    let mut seen: String = env
        .rotated()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let cut = text.find('\n').unwrap() as u64 + 1;
    assert!(cut < text.len() as u64, "the active file has one line");
    let meta = std::fs::metadata(&path).unwrap();
    let mut reader = OffsetReader {
        dev: meta.dev(),
        ino: meta.ino(),
        offset: cut,
    };
    seen.push_str(&text[..cut as usize]);

    // Ordinary commands rotate the file under it, more than once.
    for _ in 0..20 {
        env.tick("followed");
    }
    assert!(env.rotated().len() >= 2);
    seen.push_str(&reader.pass(&env));
    // And once more after a quiet pass.
    seen.push_str(&reader.pass(&env));
    env.tick("followed");
    seen.push_str(&reader.pass(&env));

    let all: String = env
        .all_files()
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    assert_eq!(seen, all);
}

#[test]
fn a_redirected_store_rotates_its_own_journal_and_listings_ignore_the_files() {
    let env = Env::new().redirected();
    env.set_bound(300);
    env.start("redirected");
    for _ in 0..10 {
        env.tick("redirected");
    }
    assert!(!env.rotated().is_empty());
    assert!(env.root().join("_run_journal.readers").is_dir());
    assert!(!env.home.join(".koto").join(JOURNAL).exists());

    let listed: Value = serde_json::from_str(&env.ok(&["session", "list"])).unwrap();
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["redirected"]);
    let workflows: Value = serde_json::from_str(&env.ok(&["workflows"])).unwrap();
    assert_eq!(workflows.as_array().unwrap().len(), 1, "{workflows}");
}

#[test]
fn a_project_config_cannot_turn_rotation_on() {
    let env = Env::new();
    // `koto config set` without --user refuses the key...
    let out = env.run(&["config", "set", "run_journal.rotate_bytes", "100"]);
    assert!(!out.status.success());
    // ...and a hand-written project file is ignored for it.
    std::fs::create_dir_all(env.work.join(".koto")).unwrap();
    std::fs::write(
        env.work.join(".koto/config.toml"),
        "[run_journal]\nrotate_bytes = 100\n",
    )
    .unwrap();
    assert_eq!(
        env.ok(&["config", "get", "run_journal.rotate_bytes"])
            .trim(),
        "0"
    );
    env.start("project");
    for _ in 0..10 {
        env.tick("project");
    }
    assert!(env.rotated().is_empty());
}

#[test]
fn retention_reads_positions_from_the_readers_directory() {
    let env = Env::new();
    env.set_bound(300);
    env.start("kept");
    for _ in 0..4 {
        env.tick("kept");
    }
    // A reader parked at the start of the oldest rotated file keeps it and
    // everything newer, even once the retention bound drops to zero days.
    let oldest = env.rotated().first().cloned().expect("a rotation");
    env.ok(&["config", "set", "run_journal.retain_days", "0", "--user"]);
    let m = std::fs::metadata(&oldest).unwrap();
    std::fs::write(
        env.root().join("_run_journal.readers/parked.json"),
        format!(
            "{{\"v\":1,\"dev\":{},\"inode\":{},\"offset\":0}}",
            m.dev(),
            m.ino()
        ),
    )
    .unwrap();
    let before = env.rotated().len();
    for _ in 0..6 {
        env.tick("kept");
    }
    assert!(oldest.exists());
    assert!(env.rotated().len() > before);

    // Without the position, a zero-day bound lets the next rotation remove
    // every rotated file but the newest, which is always kept so numbering
    // never restarts.
    let highest = env.rotated().last().cloned().unwrap();
    std::fs::remove_file(env.root().join("_run_journal.readers/parked.json")).unwrap();
    for _ in 0..4 {
        env.tick("kept");
    }
    assert!(!oldest.exists());
    let left = env.rotated();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(
        left[0] > highest,
        "{left:?} should be numbered past {highest:?}"
    );
}
