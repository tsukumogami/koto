//! Retention for koto's entries in a Claude Code `/workflows` directory.
//!
//! koto writes one `koto-<session-id>.json` per session into a directory it
//! shares with Claude Code's own `wf_*.json` files, and no koto command ever
//! learns that directory again once the session is gone. So the only place
//! that can bound it is the write itself: [`apply`] runs at the end of every
//! entry write (`try_materialize`) and sweeps the directory it just wrote
//! into.
//!
//! The sweep keeps at most [`KEEP`] koto-owned entries. It removes finished
//! entries (a top-level `status` other than `running`) oldest `startTime`
//! first, and never removes a live entry or the entry just written, so a
//! directory with more than [`KEEP`] live sessions holds more until they
//! finish. A file is koto's only when its name is `koto-<id>.json` with a
//! session-id-shaped id and its content carries koto's marker (see [`owned`]);
//! anything else is left exactly as it was and doesn't count toward the
//! bound, since a false removal in a shared directory can't be undone.
//!
//! Every step is best-effort: the sweep returns counts, never an error, so it
//! can't change the outcome of the commit that triggered it.

use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// How many koto-owned entries a `/workflows` directory keeps.
///
/// Ten koto rows beside Claude Code's own entries fit one terminal screen,
/// and the full history stays in `koto dashboard` and `koto session list`.
/// It is a constant rather than a setting; [`Limits`] carries it so tests can
/// run the sweep with a smaller bound.
pub const KEEP: usize = 10;

/// The most bytes the sweep reads from one file. An entry koto writes is a
/// few kilobytes; a larger file is not one of koto's.
const READ_CAP: u64 = 1024 * 1024;

/// The limits a sweep runs under. [`Limits::default`] is what every koto
/// write uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Owned entries the directory keeps, the entry just written included.
    pub keep: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { keep: KEEP }
    }
}

/// What one sweep did, for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Regular files whose name matches `koto-<id>.json`, the entry just
    /// written included.
    pub matched: usize,
    /// Files the sweep opened and read. Zero on the fast path.
    pub read: usize,
    /// Owned entries before removal, the entry just written included.
    /// Zero on the fast path, where nothing is read.
    pub owned: usize,
    /// Entries removed.
    pub removed: usize,
}

/// One owned entry other than the one just written.
struct Candidate {
    name: String,
    /// The ordering key: `startTime` when the entry has a numeric one, else
    /// the file's modification time, both in epoch milliseconds.
    key: u64,
}

/// Sweep `dir` after koto wrote `written` into it.
///
/// Lists `dir` once. When at most `limits.keep` files match koto's entry
/// name, returns having read nothing. Otherwise reads each name-matching file
/// except `written`, keeps the owned ones, and removes finished owned entries
/// from the oldest end (`startTime`, else modification time, then filename in
/// ascending byte order) until `limits.keep` owned entries remain or no
/// finished one is left. A removal that finds the file already gone counts as
/// done; any other failure skips that file.
pub fn apply(dir: &Path, written: &Path, limits: &Limits) -> Sweep {
    let mut sweep = Sweep::default();
    let Ok(listing) = std::fs::read_dir(dir) else {
        return sweep;
    };
    let written_name = written.file_name();

    let mut others = Vec::new();
    for entry in listing.flatten() {
        // `DirEntry::file_type` doesn't follow links, so a symlink or a
        // directory named like an entry is never matched.
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(id) = entry_id(name) else {
            continue;
        };
        sweep.matched += 1;
        if Some(file_name.as_os_str()) == written_name {
            continue;
        }
        others.push((name.to_string(), id.to_string()));
    }

    if sweep.matched <= limits.keep {
        return sweep;
    }

    // The entry just written is owned: koto wrote it a moment ago.
    sweep.owned = 1;
    let mut candidates = Vec::new();
    for (name, id) in others {
        sweep.read += 1;
        let Some(entry) = owned(&dir.join(&name), &id) else {
            continue;
        };
        sweep.owned += 1;
        if entry.status != "running" {
            candidates.push(Candidate {
                name,
                key: entry.start_time.unwrap_or(entry.modified_ms),
            });
        }
    }

    // Oldest first; equal keys in ascending filename order. The names are
    // ASCII (they passed the session-id check), so `str` order is byte order.
    candidates.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.name.cmp(&b.name)));

    let mut remaining = sweep.owned;
    for candidate in candidates {
        if remaining <= limits.keep {
            break;
        }
        match std::fs::remove_file(dir.join(&candidate.name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "warning: /workflows retention left {}: {:?}",
                    candidate.name,
                    e.kind()
                );
                continue;
            }
        }
        sweep.removed += 1;
        remaining -= 1;
    }
    sweep
}

/// The session id in an entry's file name, when the name is
/// `koto-<id>.json` and `<id>` uses only the characters a session id can
/// hold (letters, digits, `.`, `_`, `~`, `-`), which admit no path
/// separator.
///
/// The id is the session's init-time UUID, which can start with a digit, so
/// this is the character allowlist of `validate_session_id` without its
/// leading-letter rule (a session *name* must start with a letter; the UUID
/// in the file name needn't).
fn entry_id(name: &str) -> Option<&str> {
    let id = name.strip_prefix("koto-")?.strip_suffix(".json")?;
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-');
    if id.is_empty() || id == "." || id == ".." || !id.chars().all(allowed) {
        return None;
    }
    Some(id)
}

/// What the sweep needs from an owned entry.
struct OwnedEntry {
    status: String,
    start_time: Option<u64>,
    modified_ms: u64,
}

/// Read `path` and return it when it is koto's entry for session `id`.
///
/// The marker every contract version koto has written carries: a JSON object
/// whose `id` is `koto-<id>`, whose `koto.sessionId` is `<id>`, whose
/// `koto.contractVersion` is an unsigned integer and whose top-level
/// `status` is a string. A file that can't be read, is larger than
/// [`READ_CAP`], or lacks any part of the marker is not owned.
fn owned(path: &Path, id: &str) -> Option<OwnedEntry> {
    let file = std::fs::File::open(path).ok()?;
    let modified_ms = file
        .metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut bytes = Vec::new();
    file.take(READ_CAP + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > READ_CAP {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let koto = value.get("koto")?;
    let marked = value.get("id")?.as_str()? == format!("koto-{id}")
        && koto.get("sessionId")?.as_str()? == id
        && koto.get("contractVersion")?.as_u64().is_some();
    if !marked {
        return None;
    }
    Some(OwnedEntry {
        status: value.get("status")?.as_str()?.to_string(),
        start_time: value.get("startTime").and_then(serde_json::Value::as_u64),
        modified_ms,
    })
}
