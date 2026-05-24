//! Discovery scan — surfaces unassigned-child requests on `koto next`.
//!
//! Walks `~/.koto/sessions/*`, applies the tied-boundary seen-set rule
//! against the per-coord cursor at
//! `~/.koto/coordinators/<coord_id>/scan_cursor.toml`, filters to
//! candidates that name this coordinator and have not been claimed, and
//! returns up to `directive_batch_size` [`UnassignedChild`] entries
//! ordered by header mtime ascending.
//!
//! The cursor closes the Linux jiffy-resolution mtime hole: files
//! written in a tight loop share an identical-to-the-nanosecond mtime,
//! so a strict-greater-than rule silently loses every file but the
//! first. The walk rule
//!
//! ```text
//! mtime > last_max OR (mtime == last_max AND id NOT IN seen_at_boundary)
//! ```
//!
//! surfaces unseen-tied files while still excluding ones already
//! delivered on a prior tick.
//!
//! ## Cursor recovery
//!
//! Cursor reads fall through to a fresh-rescan (`last_max = 0`,
//! `seen_at_boundary = []`) on any of three conditions:
//!
//! - Cursor file is absent (never written, or deleted by GC / operator)
//! - Cursor file fails to parse (malformed TOML)
//! - Cursor `last_scan_at_unix_micros` is older than the configured TTL
//!   (`kt1.coord_cursor_ttl_days`, default 7 days)
//!
//! After a fresh-rescan the next cursor write captures the current scan
//! state and the next tick resumes incremental behavior.
//!
//! ## Atomicity
//!
//! Cursor writes use a temp-file + fsync + atomic rename. A crash
//! mid-write leaves the prior cursor intact; on the next tick the
//! orphan `.tmp` (if any) is silently overwritten.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::next_types::UnassignedChild;
use crate::config::Kt1Config;
use crate::engine::persistence::read_header;
use crate::engine::types::{StateFileHeader, ValidatedCoordId};
use crate::session::{state_file_name, validate::validate_session_id};

/// Per-coordinator scan cursor.
///
/// Persisted as TOML at `<koto_root>/coordinators/<coord_id>/scan_cursor.toml`.
/// All three fields are required; absent or malformed cursors trigger
/// full-rescan recovery.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanCursor {
    /// Wall-clock micros when this cursor was last written. Drives
    /// the TTL recovery path: a cursor older than
    /// `kt1.coord_cursor_ttl_days` is treated as absent.
    #[serde(default)]
    pub last_scan_at_unix_micros: u64,

    /// Highest header mtime observed on the previous scan, in
    /// microseconds since the UNIX epoch. The walk rule's
    /// strict-greater-than half compares against this.
    #[serde(default)]
    pub last_max_header_mtime_unix_micros: u64,

    /// Session ids that surfaced at the previous scan's
    /// `last_max_header_mtime_unix_micros`. The walk rule's tied-mtime
    /// half excludes these so they don't re-surface on the next tick.
    #[serde(default)]
    pub seen_at_boundary: Vec<String>,
}

/// Return the path to the cursor file for the named coordinator under
/// `koto_root`. Does not create the parent directory.
pub fn cursor_path(koto_root: &Path, coord_id: &str) -> PathBuf {
    koto_root
        .join("coordinators")
        .join(coord_id)
        .join("scan_cursor.toml")
}

/// Read the cursor for `coord_id`.
///
/// Returns a fresh-rescan cursor (`Default::default()`) and emits an
/// `eprintln!` diagnostic when the file is absent, malformed, or older
/// than `ttl_days`. Distinguishes the three branches in the diagnostic
/// only — the return shape is identical so callers handle recovery
/// uniformly.
pub fn read_cursor(koto_root: &Path, coord_id: &str, ttl_days: u32) -> ScanCursor {
    let path = cursor_path(koto_root, coord_id);
    let contents = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ScanCursor::default();
        }
        Err(e) => {
            eprintln!(
                "warning: cursor read failed for coord '{}' ({}); treating as full rescan",
                coord_id, e
            );
            return ScanCursor::default();
        }
    };
    let cursor: ScanCursor = match toml::from_str(&contents) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "warning: cursor for coord '{}' is malformed ({}); treating as full rescan",
                coord_id, e
            );
            return ScanCursor::default();
        }
    };
    let now_micros = now_unix_micros();
    let ttl_micros = u64::from(ttl_days)
        .saturating_mul(24)
        .saturating_mul(60)
        .saturating_mul(60)
        .saturating_mul(1_000_000);
    if cursor.last_scan_at_unix_micros.saturating_add(ttl_micros) < now_micros {
        eprintln!(
            "warning: cursor for coord '{}' is older than {}-day TTL; treating as full rescan",
            coord_id, ttl_days
        );
        return ScanCursor::default();
    }
    cursor
}

/// Write the cursor atomically: temp file -> fsync -> rename.
///
/// Creates the parent `coordinators/<coord_id>/` directory as needed.
/// On any error the prior cursor (if any) is left untouched.
pub fn write_cursor_atomic(koto_root: &Path, coord_id: &str, cursor: &ScanCursor) -> Result<()> {
    let path = cursor_path(koto_root, coord_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cursor parent dir {}", parent.display()))?;
    }
    let tmp_path = path.with_extension("toml.tmp");
    let contents = toml::to_string(cursor).context("failed to serialize cursor to TOML")?;
    {
        let mut f = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp_path.display()))?;
    }
    fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Walk `<koto_root>/coordinators/*/scan_cursor.toml` and delete any
/// cursor whose `last_scan_at_unix_micros` is older than
/// `cfg.coord_cursor_ttl_days`. Returns the number of cursors removed.
///
/// Called from `koto workspace prune` and from `koto next` startup so
/// stale coordinator state is reclaimed on both code paths. Missing
/// coordinators directory returns 0 without error.
pub fn gc_stale_cursors(koto_root: &Path, cfg: &Kt1Config) -> Result<usize> {
    let dir = koto_root.join("coordinators");
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", dir.display()));
        }
    };

    let now_micros = now_unix_micros();
    let ttl_micros = u64::from(cfg.coord_cursor_ttl_days)
        .saturating_mul(24)
        .saturating_mul(60)
        .saturating_mul(60)
        .saturating_mul(1_000_000);
    let mut deleted = 0usize;

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let cursor_file = entry.path().join("scan_cursor.toml");
        let contents = match fs::read_to_string(&cursor_file) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                // A malformed-but-present cursor is stale by definition
                // (no usable last_scan_at). Delete it.
                if fs::remove_file(&cursor_file).is_ok() {
                    deleted += 1;
                }
                continue;
            }
        };
        let cursor: ScanCursor = match toml::from_str(&contents) {
            Ok(c) => c,
            Err(_) => {
                if fs::remove_file(&cursor_file).is_ok() {
                    deleted += 1;
                }
                continue;
            }
        };
        if cursor.last_scan_at_unix_micros.saturating_add(ttl_micros) < now_micros
            && fs::remove_file(&cursor_file).is_ok()
        {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Apply the tied-boundary walk rule to a single header observation.
///
/// Pure function; no I/O. Returns `true` when the header's mtime
/// satisfies
///
/// ```text
/// mtime > last_max OR (mtime == last_max AND id NOT IN seen_at_boundary)
/// ```
///
/// The seen-set is consulted only at the tied-boundary; strictly newer
/// mtimes are always admitted regardless of seen-set membership.
pub fn walk_admits(
    mtime_unix_micros: u64,
    session_id: &str,
    cursor: &ScanCursor,
    seen_at_boundary: &HashSet<&str>,
) -> bool {
    if mtime_unix_micros > cursor.last_max_header_mtime_unix_micros {
        return true;
    }
    if mtime_unix_micros == cursor.last_max_header_mtime_unix_micros
        && !seen_at_boundary.contains(session_id)
    {
        return true;
    }
    false
}

/// True when the header's KT1 request-store fields name `coord_id` as
/// the coordinator-of-record AND mark the request as unassigned
/// (`needs_agent == Some(true)` AND `assignment_claim.is_none()`).
fn header_is_candidate(header: &StateFileHeader, coord_id: &str) -> bool {
    header.needs_agent == Some(true)
        && header.assignment_claim.is_none()
        && header
            .coordinator_of_record
            .as_deref()
            .map(|c| c == coord_id)
            .unwrap_or(false)
}

/// Convert a candidate header into the [`UnassignedChild`] shape that
/// the directive return ships. Returns `None` when companion fields
/// required by the dispatch contract (`role`, `template_name`,
/// `requested_by`) are missing — Issue 4 (`--needs-agent` CLI) enforces
/// these at write time but Issue 7 must tolerate hand-crafted headers
/// from tests and from external writers that landed before Issue 4.
fn header_to_unassigned_child(header: &StateFileHeader) -> Option<UnassignedChild> {
    let role = header.role.clone()?;
    let template = header.template_name.clone()?;
    let requested_by = header.requested_by.clone()?;
    Some(UnassignedChild {
        child_session_id: header.workflow.clone(),
        role,
        template,
        inputs: header.inputs.clone(),
        requested_by,
        created_at: header.created_at.clone(),
        dispatch_epoch: header.dispatch_epoch,
    })
}

/// Read the mtime of the state file at `path` in microseconds since
/// the UNIX epoch.
fn header_mtime_unix_micros(path: &Path) -> Result<u64> {
    let meta = fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    let modified = meta
        .modified()
        .with_context(|| format!("filesystem does not report mtime for {}", path.display()))?;
    let dur = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime predates UNIX epoch for {}", path.display()))?;
    Ok(dur.as_micros() as u64)
}

/// Run the discovery scan for `coord_id` against `koto_root`.
///
/// Returns up to `cfg.directive_batch_size` candidate
/// [`UnassignedChild`] entries ordered by header mtime ascending.
/// Advances the per-coord cursor at scan end and writes it atomically.
///
/// Cursor recovery is silent (logged via `eprintln!`); absent /
/// malformed / TTL-exceeded cursors all fall through to a fresh
/// rescan and produce the same candidate set a brand-new coordinator
/// would see.
pub fn scan(
    koto_root: &Path,
    coord_id: &ValidatedCoordId,
    cfg: &Kt1Config,
) -> Result<Vec<UnassignedChild>> {
    let sessions_dir = koto_root.join("sessions");
    let cursor = read_cursor(koto_root, coord_id.as_str(), cfg.coord_cursor_ttl_days);
    let boundary_set: HashSet<&str> = cursor.seen_at_boundary.iter().map(String::as_str).collect();

    // Walk the workspace; collect (mtime, session_id, header) for every
    // session whose mtime is admitted by the walk rule. The candidate
    // filter narrows further; the cursor update needs every admitted
    // entry (not just the candidates).
    let mut admitted: Vec<(u64, String, StateFileHeader)> = Vec::new();
    let entries = match fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Fresh workspace; nothing to walk. Persist a cursor so
            // the next tick has a `last_scan_at` for TTL discipline.
            write_cursor_post_scan(koto_root, coord_id, &[])?;
            return Ok(Vec::new());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", sessions_dir.display()));
        }
    };

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let session_id = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Defensive: validate the session id. A non-conforming dir name
        // here is a workspace foreign-file; skip it without surfacing.
        if validate_session_id(&session_id).is_err() {
            continue;
        }
        let state_path = entry.path().join(state_file_name(&session_id));
        if !state_path.exists() {
            continue;
        }
        let mtime = match header_mtime_unix_micros(&state_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !walk_admits(mtime, &session_id, &cursor, &boundary_set) {
            continue;
        }
        let header = match read_header(&state_path) {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "warning: skipping {} during discovery: {}",
                    state_path.display(),
                    e
                );
                continue;
            }
        };
        admitted.push((mtime, session_id, header));
    }

    // Filter to candidates, sort by mtime ascending, cap at
    // `directive_batch_size`. The cap discipline (Decision 4): excess
    // candidates surface on subsequent ticks rather than overflowing
    // the directive payload. A workspace with 500 eligible candidates
    // produces 50 ten ticks in a row — the cursor advances ONLY to the
    // mtime of the surfaced (post-cap) set, so deferred candidates
    // re-cross the walk-rule boundary on the very next tick.
    let mut candidates: Vec<(u64, String, UnassignedChild)> = admitted
        .iter()
        .filter(|(_, _, h)| header_is_candidate(h, coord_id.as_str()))
        .filter_map(|(m, sid, h)| header_to_unassigned_child(h).map(|c| (*m, sid.clone(), c)))
        .collect();
    candidates.sort_by_key(|(m, _, _)| *m);
    let cap = cfg.directive_batch_size as usize;
    let was_capped = candidates.len() > cap;
    if was_capped {
        candidates.truncate(cap);
    }

    // Cursor advancement set:
    // - When NOT capped: advance the cursor across every admitted
    //   observation (matches the AC 5 "quiet second tick" contract;
    //   nothing surfaced this tick AND no new writes → empty).
    // - When capped: advance only up to the surfaced (post-cap) set so
    //   the deferred candidates re-surface on the next tick. The
    //   non-candidate-filter admitted entries (other coordinator's
    //   children, claimed children) are NOT carried forward in the
    //   advancement set — they are bounded by the parent's mtime and
    //   the candidate filter naturally re-excludes them on the next
    //   tick if their headers haven't moved.
    let cursor_advancement: Vec<(u64, String, StateFileHeader)> = if was_capped {
        // Use the post-cap surfaced subset.
        let surfaced_ids: HashSet<&str> = candidates.iter().map(|(_, s, _)| s.as_str()).collect();
        admitted
            .iter()
            .filter(|(_, sid, _)| surfaced_ids.contains(sid.as_str()))
            .cloned()
            .collect()
    } else {
        admitted.clone()
    };
    write_cursor_post_scan(koto_root, coord_id, &cursor_advancement)?;

    Ok(candidates.into_iter().map(|(_, _, c)| c).collect())
}

/// Compose and atomically write the post-scan cursor from the full
/// admitted-set observation.
///
/// When `admitted` is empty the cursor's mtime/seen fields are
/// preserved (we still bump `last_scan_at` so the TTL clock resets
/// on every successful scan; an empty workspace shouldn't trigger
/// recovery on the next tick). When `admitted` is non-empty the new
/// `last_max_header_mtime_unix_micros` is the max observed mtime and
/// `seen_at_boundary` is the set of session ids that landed on that
/// max-mtime.
fn write_cursor_post_scan(
    koto_root: &Path,
    coord_id: &ValidatedCoordId,
    admitted: &[(u64, String, StateFileHeader)],
) -> Result<()> {
    let prior = read_cursor(koto_root, coord_id.as_str(), u32::MAX);
    let now_micros = now_unix_micros();

    let (last_max, seen) = if admitted.is_empty() {
        (
            prior.last_max_header_mtime_unix_micros,
            prior.seen_at_boundary.clone(),
        )
    } else {
        let max = admitted.iter().map(|(m, _, _)| *m).max().unwrap_or(0);
        let prior_max = prior.last_max_header_mtime_unix_micros;
        let merged_max = max.max(prior_max);
        let mut seen: Vec<String> = admitted
            .iter()
            .filter(|(m, _, _)| *m == merged_max)
            .map(|(_, sid, _)| sid.clone())
            .collect();
        if merged_max == prior_max {
            // Carry forward prior-tick seen ids whose mtime equals the
            // new max — they remain at the boundary even though they
            // weren't admitted this tick.
            for sid in &prior.seen_at_boundary {
                if !seen.contains(sid) {
                    seen.push(sid.clone());
                }
            }
        }
        seen.sort();
        seen.dedup();
        (merged_max, seen)
    };

    let cursor = ScanCursor {
        last_scan_at_unix_micros: now_micros,
        last_max_header_mtime_unix_micros: last_max,
        seen_at_boundary: seen,
    };
    write_cursor_atomic(koto_root, coord_id.as_str(), &cursor)
}

/// Now, in microseconds since the UNIX epoch.
fn now_unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_rule_strict_greater_than_admits() {
        let cursor = ScanCursor {
            last_scan_at_unix_micros: 0,
            last_max_header_mtime_unix_micros: 100,
            seen_at_boundary: vec!["a".into()],
        };
        let boundary: HashSet<&str> = cursor.seen_at_boundary.iter().map(String::as_str).collect();
        assert!(walk_admits(101, "b", &cursor, &boundary));
        assert!(walk_admits(200, "a", &cursor, &boundary));
    }

    #[test]
    fn walk_rule_strict_less_than_rejects() {
        let cursor = ScanCursor {
            last_max_header_mtime_unix_micros: 100,
            ..Default::default()
        };
        let boundary: HashSet<&str> = HashSet::new();
        assert!(!walk_admits(99, "b", &cursor, &boundary));
        assert!(!walk_admits(0, "x", &cursor, &boundary));
    }

    #[test]
    fn walk_rule_tied_boundary_seen_set_excludes_seen() {
        let cursor = ScanCursor {
            last_max_header_mtime_unix_micros: 100,
            seen_at_boundary: vec!["seen-1".into(), "seen-2".into()],
            ..Default::default()
        };
        let boundary: HashSet<&str> = cursor.seen_at_boundary.iter().map(String::as_str).collect();
        // Tied + seen → reject (the load-bearing exclusion).
        assert!(!walk_admits(100, "seen-1", &cursor, &boundary));
        assert!(!walk_admits(100, "seen-2", &cursor, &boundary));
        // Tied + unseen → admit (the load-bearing inclusion).
        assert!(walk_admits(100, "unseen-3", &cursor, &boundary));
        assert!(walk_admits(100, "unseen-4", &cursor, &boundary));
    }

    #[test]
    fn cursor_toml_round_trips() {
        let cursor = ScanCursor {
            last_scan_at_unix_micros: 1_716_580_000_000_000,
            last_max_header_mtime_unix_micros: 1_716_579_999_000_000,
            seen_at_boundary: vec!["alpha".into(), "beta".into()],
        };
        let s = toml::to_string(&cursor).unwrap();
        let parsed: ScanCursor = toml::from_str(&s).unwrap();
        assert_eq!(cursor, parsed);
    }

    #[test]
    fn cursor_path_layout() {
        let root = PathBuf::from("/tmp/.koto");
        let p = cursor_path(&root, "team-lead");
        assert_eq!(
            p,
            PathBuf::from("/tmp/.koto/coordinators/team-lead/scan_cursor.toml")
        );
    }

    #[test]
    fn read_cursor_missing_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cursor = read_cursor(tmp.path(), "no-such-coord", 7);
        assert_eq!(cursor, ScanCursor::default());
    }

    #[test]
    fn read_cursor_malformed_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = cursor_path(tmp.path(), "garbage-coord");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not = valid toml = [[[").unwrap();
        let cursor = read_cursor(tmp.path(), "garbage-coord", 7);
        assert_eq!(cursor, ScanCursor::default());
    }

    #[test]
    fn read_cursor_ttl_exceeded_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let coord = "old-coord";
        // Last scan 100 days ago.
        let hundred_days_micros: u64 = 100 * 24 * 60 * 60 * 1_000_000;
        let stale = ScanCursor {
            last_scan_at_unix_micros: now_unix_micros().saturating_sub(hundred_days_micros),
            last_max_header_mtime_unix_micros: 42,
            seen_at_boundary: vec!["x".into()],
        };
        write_cursor_atomic(tmp.path(), coord, &stale).unwrap();
        // TTL = 7 days; cursor is 100 days old → recovery.
        let recovered = read_cursor(tmp.path(), coord, 7);
        assert_eq!(recovered, ScanCursor::default());
    }

    #[test]
    fn read_cursor_within_ttl_returns_value() {
        let tmp = tempfile::tempdir().unwrap();
        let coord = "fresh-coord";
        let fresh = ScanCursor {
            last_scan_at_unix_micros: now_unix_micros(),
            last_max_header_mtime_unix_micros: 1000,
            seen_at_boundary: vec!["a".into()],
        };
        write_cursor_atomic(tmp.path(), coord, &fresh).unwrap();
        let read_back = read_cursor(tmp.path(), coord, 7);
        assert_eq!(read_back, fresh);
    }

    #[test]
    fn write_cursor_atomic_leaves_no_tmp() {
        let tmp = tempfile::tempdir().unwrap();
        let coord = "atomic-coord";
        let cursor = ScanCursor {
            last_scan_at_unix_micros: 1,
            last_max_header_mtime_unix_micros: 2,
            seen_at_boundary: vec!["a".into()],
        };
        write_cursor_atomic(tmp.path(), coord, &cursor).unwrap();
        let path = cursor_path(tmp.path(), coord);
        assert!(path.exists());
        let tmp_path = path.with_extension("toml.tmp");
        assert!(!tmp_path.exists(), "atomic write must rename the tmp file");
    }

    #[test]
    fn gc_stale_cursors_deletes_stale_preserves_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Kt1Config::default(); // 7-day TTL
        let hundred_days_micros: u64 = 100 * 24 * 60 * 60 * 1_000_000;
        let stale = ScanCursor {
            last_scan_at_unix_micros: now_unix_micros().saturating_sub(hundred_days_micros),
            ..Default::default()
        };
        let fresh = ScanCursor {
            last_scan_at_unix_micros: now_unix_micros(),
            ..Default::default()
        };
        write_cursor_atomic(tmp.path(), "stale-coord", &stale).unwrap();
        write_cursor_atomic(tmp.path(), "fresh-coord", &fresh).unwrap();

        let deleted = gc_stale_cursors(tmp.path(), &cfg).unwrap();
        assert_eq!(deleted, 1);
        assert!(!cursor_path(tmp.path(), "stale-coord").exists());
        assert!(cursor_path(tmp.path(), "fresh-coord").exists());
    }

    #[test]
    fn gc_stale_cursors_handles_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Kt1Config::default();
        let deleted = gc_stale_cursors(tmp.path(), &cfg).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn gc_stale_cursors_deletes_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Kt1Config::default();
        let path = cursor_path(tmp.path(), "garbage-coord");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not toml :::").unwrap();
        let deleted = gc_stale_cursors(tmp.path(), &cfg).unwrap();
        assert_eq!(deleted, 1);
        assert!(!path.exists());
    }
}
