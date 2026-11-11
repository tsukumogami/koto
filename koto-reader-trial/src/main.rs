//! A second program reading koto sessions through the separated reader.
//!
//! Two modes:
//!
//! - `list --root <name> [--key <ctx key>]`: the sessions under one root
//!   (the root and every descendant by `parent_workflow`), each with its
//!   current state, liveness, and one recorded context value.
//! - `once [--all]`: the same rows `koto dashboard --once` prints for the
//!   local store, columns 1-8, in the same order, so the two can be diffed.
//!
//! `--repeat N` runs the read N times in one process (a fresh tree each
//! time) to show repeated reads change nothing. Nothing here opens a file
//! for writing or makes a network request; the crate graph has no client
//! that could.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use koto_session_reader::cli::dashboard_data::liveness::{idle_for, session_attention};
use koto_session_reader::cli::dashboard_data::{
    classify_liveness, parent_key, refresh, CachedSession, Liveness, SessionTree,
};
use koto_session_reader::session::validate::validate_context_key;
use koto_session_reader::source::LocalDirSource;

struct Args {
    mode: String,
    sessions_dir: PathBuf,
    root: Option<String>,
    key: Option<String>,
    all: bool,
    repeat: usize,
}

fn default_sessions_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("KOTO_HOME") {
        return PathBuf::from(home).join("sessions");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".koto").join("sessions")
}

fn parse_args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mode = it.next().unwrap_or_else(|| "once".to_string());
    let mut args = Args {
        mode,
        sessions_dir: default_sessions_dir(),
        root: None,
        key: None,
        all: false,
        repeat: 1,
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sessions-dir" => args.sessions_dir = PathBuf::from(it.next().unwrap_or_default()),
            "--root" => args.root = it.next(),
            "--key" => args.key = it.next(),
            "--all" => args.all = true,
            "--repeat" => args.repeat = it.next().unwrap_or_default().parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

/// Same tokens as `koto dashboard --once`.
fn liveness_token(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::NeedsYouBlocked => "needs-you-blocked",
        Liveness::NeedsYouFailed => "needs-you-failed",
        Liveness::NeedsYouStalled => "needs-you-stalled",
        Liveness::Active => "active",
        Liveness::Idle => "idle",
        Liveness::Pending => "pending",
        Liveness::Done => "done",
    }
}

/// Same buckets as `koto dashboard --once` (`classify_status`, which lives
/// in the dashboard's entry file rather than its data layer).
fn status_bucket(session: &CachedSession) -> &'static str {
    if session.is_terminal {
        let state = session
            .current_state
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if state.contains("failed") || state.contains("error") {
            "failed"
        } else {
            "done"
        }
    } else if session.is_blocked {
        "blocked"
    } else if session.current_state.is_some() {
        "running"
    } else {
        "unknown"
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn fmt_secs(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{secs}s")
    }
}

/// One recorded context value, read in place: the key grammar koto
/// enforces, no symlink followed, at most 200 bytes shown. Read-only.
fn context_value(session_dir: &Path, key: &str) -> String {
    if validate_context_key(key).is_err() {
        return "<invalid key>".to_string();
    }
    let path = session_dir.join("ctx").join(key);
    match fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return "<not a file>".to_string(),
        Err(_) => return "<absent>".to_string(),
    }
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let first = text.lines().next().unwrap_or("");
            clean(&first.chars().take(200).collect::<String>())
        }
        Err(_) => "<unreadable>".to_string(),
    }
}

fn under_root<'a>(tree: &'a SessionTree, root: &str) -> Vec<(&'a str, &'a CachedSession)> {
    let mut out = Vec::new();
    let mut frontier = vec![root.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(key) = frontier.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some((k, s)) = tree.sessions.get_key_value(&key) {
            out.push((k.as_str(), s));
        }
        for (k, s) in &tree.sessions {
            if parent_key(s).as_deref() == Some(key.as_str()) {
                frontier.push(k.clone());
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let source = LocalDirSource::new(&args.sessions_dir);
    let mut last = Vec::new();
    for _ in 0..args.repeat {
        let mut tree = SessionTree::new();
        refresh(&mut tree, &source)?;
        let now = SystemTime::now();
        let mut lines = Vec::new();
        match args.mode.as_str() {
            "list" => {
                let root = args
                    .root
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("--root is required"))?;
                for (name, s) in under_root(&tree, root) {
                    let value = args
                        .key
                        .as_deref()
                        .map(|k| context_value(&source.base_dir().join(name), k))
                        .unwrap_or_default();
                    let last = s
                        .header
                        .created_at
                        .is_empty()
                        .then(String::new)
                        .unwrap_or_else(|| {
                            s.last_event_at
                                .map(|t| {
                                    fmt_secs(now.duration_since(t).unwrap_or_default().as_secs())
                                })
                                .unwrap_or_default()
                        });
                    lines.push(format!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        clean(name),
                        clean(s.header.parent_workflow.as_deref().unwrap_or("-")),
                        clean(s.current_state.as_deref().unwrap_or("")),
                        liveness_token(classify_liveness(s, now)),
                        last,
                        value
                    ));
                }
            }
            "once" => {
                let mut rows: Vec<(&String, &CachedSession)> = tree
                    .sessions
                    .iter()
                    .filter(|(_, s)| args.all || session_attention(s, now).0 != 3)
                    .collect();
                rows.sort_by(|a, b| {
                    session_attention(a.1, now)
                        .cmp(&session_attention(b.1, now))
                        .then_with(|| a.0.cmp(b.0))
                });
                for (name, s) in rows {
                    let created = koto_session_reader::elapsed_since(&s.header.created_at);
                    lines.push(format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        clean(name),
                        clean(s.current_state.as_deref().unwrap_or("")),
                        fmt_secs(created.as_secs()),
                        status_bucket(s),
                        clean(s.intent.as_deref().unwrap_or("")),
                        clean(s.header.template_name.as_deref().unwrap_or("")),
                        fmt_secs(idle_for(s, now).as_secs()),
                        liveness_token(classify_liveness(s, now)),
                    ));
                }
                if tree.unreadable > 0 {
                    eprintln!("note: {} unreadable session(s) skipped", tree.unreadable);
                }
            }
            other => anyhow::bail!("unknown mode {other}"),
        }
        last = lines;
    }
    for line in last {
        println!("{line}");
    }
    Ok(())
}
