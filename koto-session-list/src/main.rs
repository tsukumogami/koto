//! `koto-session-list`: koto's sessions, read through `koto-session-reader`
//! alone.
//!
//! A program other than koto that reads koto's session store the way
//! `koto dashboard` does, with nothing but the reader crate: no koto binary,
//! no lock, no write, no network. It has two modes.
//!
//! ```text
//! koto-session-list once [--all]
//! koto-session-list list --root <name> --key <key>
//! ```
//!
//! `once` prints the first eight columns of `koto dashboard --once`, one
//! session per line, tab-separated: id, current state, elapsed, status,
//! intent, template, idle and liveness. Lines come in the dashboard's order
//! (attention, then id), and the receded sessions (done, or silent past the
//! abandoned threshold) are left out unless `--all` is given. A store holding
//! sessions whose state file can't be read prints the dashboard's note on
//! stderr. Columns 9 and 10 of the dashboard (machine and workspace) aren't
//! printed: they come from the cloud backend's bucket, which this program
//! never reads.
//!
//! `list` prints the session `--root` names and every session nested under
//! it, the root first and each session's children after it in id order, one
//! line each: id, parent, current state, liveness, and the value of context
//! key `--key` as `koto context get` would print it, with tabs, carriage
//! returns and newlines read as spaces. The value field is empty when the
//! session has no value under the key; a value the reader refuses to read is
//! empty too, with a note on stderr naming the session and the reason.
//!
//! The store is the sessions directory koto uses, found by koto's own rule
//! (`koto_session_reader::source::sessions_dir`): `KOTO_SESSIONS_BASE` when
//! it is set, else `<koto root>/sessions`, which is the cloud backend's
//! local mirror too. A store whose layout marker this reader doesn't read is
//! refused.
//!
//! The column rules below copy the ones koto's dashboard applies in its own
//! `src/cli/dashboard.rs`, where they are private. koto's
//! `tests/koto_session_list_test.rs` runs both programs on the same store and
//! fails when their rows differ.
//!
//! Exit status: 0 on success, 1 when the store can't be found or read, and 2
//! for a usage error, an unknown root or a key that can't name a context
//! value.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::io::Write;
use std::process::ExitCode;
use std::time::SystemTime;

use koto_session_reader::dashboard::liveness::{
    classify_liveness, idle_for, session_attention, Liveness,
};
use koto_session_reader::dashboard::{compute_elapsed_since, refresh_scan, CachedSession};
use koto_session_reader::source::{
    read_context, read_session_events, sessions_dir, ContextRead, LocalDirSource, SessionSource,
};

/// The program has no bucket overlay, so its tree never records one's error.
type SessionTree = koto_session_reader::dashboard::SessionTree<Infallible>;

const USAGE: &str = "usage: koto-session-list once [--all]\n       koto-session-list list --root <name> --key <key>";

/// The attention band `session_attention` gives a receded session, which
/// the dashboard's `--once` leaves out unless `--all` is given.
const RECEDED_BAND: u8 = 3;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Once { all: bool },
    List { root: String, key: String },
}

/// Parses the arguments after the program name. The error is the message a
/// usage failure prints before the usage lines.
fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some((mode, rest)) = args.split_first() else {
        return Err("no mode given".to_string());
    };
    match mode.as_str() {
        "-h" | "--help" | "help" if rest.is_empty() => Ok(Command::Help),
        "once" => match rest {
            [] => Ok(Command::Once { all: false }),
            [flag] if flag == "--all" => Ok(Command::Once { all: true }),
            [other, ..] if other != "--all" => Err(format!("once: unknown argument {other:?}")),
            _ => Err("once: --all given more than once, or followed by another argument".into()),
        },
        "list" => {
            let mut root = None;
            let mut key = None;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let slot = match flag.as_str() {
                    "--root" => &mut root,
                    "--key" => &mut key,
                    other => return Err(format!("list: unknown argument {other:?}")),
                };
                let Some(value) = it.next() else {
                    return Err(format!("list: {flag} needs a value"));
                };
                if slot.replace(value.clone()).is_some() {
                    return Err(format!("list: {flag} given more than once"));
                }
            }
            match (root, key) {
                (Some(root), Some(key)) => Ok(Command::List { root, key }),
                (None, _) => Err("list: --root is required".to_string()),
                (_, None) => Err("list: --key is required".to_string()),
            }
        }
        other => Err(format!("unknown mode {other:?}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("error: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if command == Command::Help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let (source, tree) = match open_store() {
        Ok(opened) => opened,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };

    let (lines, notes, code) = match command {
        Command::Help => unreachable!("handled above"),
        Command::Once { all } => {
            let (lines, notes) = once(&tree, all, SystemTime::now());
            (lines, notes, ExitCode::SUCCESS)
        }
        Command::List { root, key } => match list(&source, &tree, &root, &key, SystemTime::now()) {
            Ok((lines, notes)) => (lines, notes, ExitCode::SUCCESS),
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(2);
            }
        },
    };

    let mut stdout = std::io::stdout().lock();
    for line in &lines {
        if writeln!(stdout, "{line}").is_err() {
            // A closed pipe (`| head`) ends the listing quietly.
            return code;
        }
    }
    for note in &notes {
        eprintln!("{note}");
    }
    code
}

/// Finds koto's sessions directory, checks its layout marker and reads
/// every session in it once.
fn open_store() -> Result<(LocalDirSource, SessionTree), String> {
    let dir = sessions_dir(None).map_err(|e| e.to_string())?;
    let source = LocalDirSource::new(dir).map_err(|e| e.to_string())?;
    let mut tree = SessionTree::new();
    refresh_scan(&mut tree, &source).map_err(|e| e.to_string())?;
    Ok((source, tree))
}

/// The `once` lines, in the dashboard's order, and the notes for stderr.
fn once(tree: &SessionTree, all: bool, now: SystemTime) -> (Vec<String>, Vec<String>) {
    let mut rows: Vec<(&str, &CachedSession)> = tree
        .sessions
        .iter()
        .filter(|(_, session)| all || session_attention(session, now).0 != RECEDED_BAND)
        .map(|(id, session)| (id.as_str(), session))
        .collect();
    rows.sort_by(|a, b| {
        session_attention(a.1, now)
            .cmp(&session_attention(b.1, now))
            .then_with(|| a.0.cmp(b.0))
    });

    let lines = rows
        .into_iter()
        .map(|(id, session)| once_line(id, session, now))
        .collect();
    let mut notes = Vec::new();
    if tree.unreadable > 0 {
        notes.push(format!(
            "note: {} unreadable session(s) skipped",
            tree.unreadable
        ));
    }
    (lines, notes)
}

/// One `once` line: the dashboard's columns 1 to 8.
fn once_line(id: &str, session: &CachedSession, now: SystemTime) -> String {
    let elapsed = compute_elapsed_since(&session.header.created_at).as_secs();
    let idle = idle_for(session, now);
    [
        sanitize_field(id),
        sanitize_field(session.current_state.as_deref().unwrap_or("")),
        format_elapsed(elapsed),
        classify_status(session).to_string(),
        sanitize_field(session.intent.as_deref().unwrap_or("")),
        sanitize_field(session.header.template_name.as_deref().unwrap_or("")),
        format_elapsed(idle.as_secs()),
        liveness_token(classify_liveness(session, now)).to_string(),
    ]
    .join("\t")
}

/// The `list` lines for `root` and the sessions nested under it, and the
/// notes for stderr. An unknown root or an unusable key is an error.
fn list(
    source: &dyn SessionSource,
    tree: &SessionTree,
    root: &str,
    key: &str,
    now: SystemTime,
) -> Result<(Vec<String>, Vec<String>), String> {
    if !tree.sessions.contains_key(root) {
        return Err(format!("no readable session named {root:?}"));
    }

    // Children by the parent the dashboard nests each row under, each list
    // in id order (the map's keys are, too).
    let mut children: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (id, session) in &tree.sessions {
        if let Some(parent) = tree.parent_of(id, session) {
            children.entry(parent).or_default().push(id.as_str());
        }
    }
    for kids in children.values_mut() {
        kids.sort_unstable();
    }

    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        order.push(id);
        if let Some(kids) = children.get(id) {
            stack.extend(kids.iter().rev());
        }
    }

    let mut lines = Vec::new();
    let mut notes = Vec::new();
    for id in order {
        let session = &tree.sessions[id];
        let value = match read_session_events(source, id) {
            Ok((_, events)) => match read_context(&source.session_dir(id), key, Some(&events)) {
                ContextRead::Present { bytes, .. } => {
                    sanitize_field(&String::from_utf8_lossy(&bytes))
                }
                ContextRead::Absent => String::new(),
                ContextRead::Unreadable(reason) => {
                    notes.push(format!("note: {id}: {reason}"));
                    String::new()
                }
                // The key itself can't name a context value (it fails the
                // key grammar or names one of koto's own files), so it is
                // the caller's error, exit 2, not the store's.
                ContextRead::Unusable(reason) => return Err(reason),
            },
            Err(e) => {
                notes.push(format!("note: {id}: {e}"));
                String::new()
            }
        };
        lines.push(
            [
                sanitize_field(id),
                sanitize_field(&tree.parent_of(id, session).unwrap_or_default()),
                sanitize_field(session.current_state.as_deref().unwrap_or("")),
                liveness_token(classify_liveness(session, now)).to_string(),
                value,
            ]
            .join("\t"),
        );
    }
    Ok((lines, notes))
}

// ---------------------------------------------------------------------------
// The dashboard's column rules, as koto's src/cli/dashboard.rs applies them
// ---------------------------------------------------------------------------

/// Column 4: one of five status buckets.
fn classify_status(session: &CachedSession) -> &'static str {
    status_bucket(
        session.is_terminal,
        session.is_blocked,
        session.current_state.as_deref(),
    )
}

/// [`classify_status`] over the facts it reads.
fn status_bucket(is_terminal: bool, is_blocked: bool, current_state: Option<&str>) -> &'static str {
    if is_terminal {
        let state = current_state.unwrap_or("").to_lowercase();
        if state.contains("failed") || state.contains("error") {
            "failed"
        } else {
            "done"
        }
    } else if is_blocked {
        "blocked"
    } else if current_state.is_some() {
        "running"
    } else {
        "unknown"
    }
}

/// Column 8: the liveness class as a kebab-case token. The match has no
/// catch-all, so a class the reader adds fails this program's build.
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

/// Tabs, carriage returns and newlines read as spaces, so a field can't
/// break the tab-separated line.
fn sanitize_field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Columns 3 and 7: `0s`, `{s}s`, `{m}m{s}s` or `{h}h{m}m`.
fn format_elapsed(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let s = secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m{s}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn once_takes_an_optional_all() {
        assert_eq!(
            parse_args(&args(&["once"])),
            Ok(Command::Once { all: false })
        );
        assert_eq!(
            parse_args(&args(&["once", "--all"])),
            Ok(Command::Once { all: true })
        );
    }

    #[test]
    fn list_takes_root_and_key_in_either_order() {
        let want = Ok(Command::List {
            root: "r".into(),
            key: "k/v".into(),
        });
        assert_eq!(
            parse_args(&args(&["list", "--root", "r", "--key", "k/v"])),
            want
        );
        assert_eq!(
            parse_args(&args(&["list", "--key", "k/v", "--root", "r"])),
            want
        );
    }

    #[test]
    fn usage_errors_name_what_is_wrong() {
        for (given, says) in [
            (&[][..], "no mode"),
            (&["status"][..], "unknown mode"),
            (&["once", "--root"][..], "unknown argument"),
            (&["once", "--all", "--all"][..], "more than once"),
            (&["list", "--key", "k"][..], "--root is required"),
            (&["list", "--root", "r"][..], "--key is required"),
            (&["list", "--root"][..], "needs a value"),
            (
                &["list", "--root", "r", "--root", "s", "--key", "k"][..],
                "more than once",
            ),
            (
                &["list", "--all", "--root", "r", "--key", "k"][..],
                "unknown argument",
            ),
        ] {
            let err = parse_args(&args(given)).expect_err(&format!("{given:?} parses"));
            assert!(err.contains(says), "{given:?}: {err:?} lacks {says:?}");
        }
    }

    #[test]
    fn help_is_its_own_command() {
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert!(parse_args(&args(&["--help", "once"])).is_err());
    }

    #[test]
    fn status_buckets_follow_the_dashboard() {
        assert_eq!(status_bucket(true, false, Some("done")), "done");
        assert_eq!(status_bucket(true, false, Some("Build_FAILED")), "failed");
        assert_eq!(status_bucket(true, false, Some("error_exit")), "failed");
        assert_eq!(status_bucket(true, true, Some("done")), "done");
        assert_eq!(status_bucket(false, true, Some("review")), "blocked");
        assert_eq!(status_bucket(false, false, Some("work")), "running");
        assert_eq!(status_bucket(false, false, None), "unknown");
    }

    #[test]
    fn liveness_tokens_are_kebab_case() {
        assert_eq!(
            liveness_token(Liveness::NeedsYouBlocked),
            "needs-you-blocked"
        );
        assert_eq!(liveness_token(Liveness::NeedsYouFailed), "needs-you-failed");
        assert_eq!(
            liveness_token(Liveness::NeedsYouStalled),
            "needs-you-stalled"
        );
        assert_eq!(liveness_token(Liveness::Active), "active");
        assert_eq!(liveness_token(Liveness::Idle), "idle");
        assert_eq!(liveness_token(Liveness::Pending), "pending");
        assert_eq!(liveness_token(Liveness::Done), "done");
    }

    #[test]
    fn elapsed_is_compact() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(61), "1m1s");
        assert_eq!(format_elapsed(3600), "1h0m");
        assert_eq!(format_elapsed(90061), "25h1m");
    }

    #[test]
    fn fields_lose_tabs_and_line_breaks() {
        assert_eq!(sanitize_field("a\tb\nc\r\nd"), "a b c  d");
        assert_eq!(sanitize_field("plain"), "plain");
    }
}
