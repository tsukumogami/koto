//! What this host last knew of a session's copy in the bucket, so a cloud
//! read whose local copy is current transfers nothing (koto#428).
//!
//! Host-local files in a session's local directory, none of them ever
//! uploaded:
//!
//! - `cloud-sync.json` ([`Stamp`]): the bucket's ETag for the state file and
//!   the length and SHA-256 of the bytes this host last pulled or pushed
//!   under it, the ETag of the remote manifest cached beside it, and the
//!   tick that last checked the session.
//! - `cloud-manifest.json`: the remote manifest as the bucket last served
//!   it, valid while the bucket's manifest ETag equals the stamped one.
//! - `cloud-tick.id`: in a session's own directory, the id of the tick now
//!   running it, written when the tick starts. A command the tick runs
//!   reads it through the tick marker it inherits, so every command of one
//!   tick sees the same id without a new variable in its environment.
//! - `cloud-check.json`: the last listing this host made of the session,
//!   with when it was made, which every koto process on the host reuses for
//!   `session.cloud.check_window_secs` (koto#439). A push from this host
//!   removes it.
//! - `cloud-ctx-synced.json` ([`SyncedKeys`]): for each context key, the
//!   hash this host last knew the session's home to hold, so which of two
//!   differing copies moved is read from hashes, not host clocks (koto#524).
//! - `cloud-summary-owed` ([`SUMMARY_OWED_FILE`]): left by a failed
//!   `summary.json` PUT, so the next flush writes the summary for the
//!   stamped bytes, and removed by a PUT that succeeds.
//!
//! A read probes the bucket once per session per process: one listing of
//! the session's prefix says whether the migration marker is there and
//! gives the state file's and the manifest's ETags. A conditional GET
//! (`If-None-Match`) would do it in one request too, but rust-s3, built with
//! `fail-on-err`, turns the 304 into an error and retries it after a
//! one-second sleep. The state file is fetched only when its ETag moved
//! since the stamp, or the local file is no longer the stamped bytes; the
//! manifest only when its ETag moved. And a session the current tick has
//! already checked is not probed again in that tick: the tick is the
//! freshness window a command can rely on, and that holds for the tick's own
//! session too while the only change to its local copy is the tick's open
//! batch. Outside a tick, a listing another process on this host made within
//! the check window stands in for a new one.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// File name of the stamp inside a session's local directory.
pub const STAMP_FILE: &str = "cloud-sync.json";

/// File name of the cached remote manifest.
pub const MANIFEST_FILE: &str = "cloud-manifest.json";

/// File name of the running tick's id, in the ticked session's directory.
pub const TICK_FILE: &str = "cloud-tick.id";

/// File name of the last listing this host made of a session.
pub const CHECK_FILE: &str = "cloud-check.json";

/// File name of the context hashes this host last synced with a session's
/// home.
pub const SYNCED_FILE: &str = "cloud-ctx-synced.json";

/// File name of the mark a failed `summary.json` PUT leaves: the next flush
/// writes the summary even when the state didn't change, and a PUT that
/// succeeds removes the mark.
pub const SUMMARY_OWED_FILE: &str = "cloud-summary-owed";

/// The hash of each context key this host last knew the session's home to
/// hold: the copy it pushed there, pulled from there, or found there equal
/// to its own. Kept for one home, so a record left from before a rebind
/// moved the home says nothing about the new one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedKeys {
    /// The workspace prefix the hashes were synced with.
    pub home: String,
    /// Key to lowercase hex SHA-256, as the manifest writes it.
    #[serde(default)]
    pub keys: std::collections::BTreeMap<String, String>,
}

impl SyncedKeys {
    /// The record in `dir` for `home`, or an empty one for it when there is
    /// none, it doesn't parse, or it is for another home.
    pub fn load(dir: &Path, home: &str) -> Self {
        std::fs::read(dir.join(SYNCED_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|synced| synced.home == home)
            .unwrap_or_else(|| SyncedKeys {
                home: home.to_string(),
                keys: Default::default(),
            })
    }

    /// The hash of `key` this host last synced with the home.
    pub fn hash(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }
}

/// Record that `home` holds `hash` for each of `keys` in the session whose
/// directory is `dir`, replacing a record for any other home. Best-effort:
/// a record that can't be written only leaves the next read to compare
/// stamps, as before.
pub fn note_synced<'a>(dir: &Path, home: &str, keys: impl IntoIterator<Item = (&'a str, &'a str)>) {
    let mut synced = SyncedKeys::load(dir, home);
    let before = synced.clone();
    for (key, hash) in keys {
        synced.keys.insert(key.to_string(), hash.to_string());
    }
    if synced == before {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(&synced) {
        write_replacing(dir, SYNCED_FILE, &bytes);
    }
}

/// Drop `key` from the record in `dir`, after this host removed it.
pub fn forget_synced(dir: &Path, key: &str) {
    let Some(mut synced) = std::fs::read(dir.join(SYNCED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SyncedKeys>(&bytes).ok())
    else {
        return;
    };
    if synced.keys.remove(key).is_some() {
        if let Ok(bytes) = serde_json::to_vec(&synced) {
            write_replacing(dir, SYNCED_FILE, &bytes);
        }
    }
}

/// What this host last knew of one session in the bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The bucket key of the state file the stamp is for, so a stamp left in
    /// a directory that moved to another name is never taken for the new
    /// name's.
    pub state_key: String,
    /// The bucket's ETag for the state file, as of the stamped bytes.
    #[serde(default)]
    pub state_etag: Option<String>,
    /// Length of the stamped bytes.
    #[serde(default)]
    pub state_len: u64,
    /// Lowercase hex SHA-256 of the stamped bytes.
    #[serde(default)]
    pub state_sha256: String,
    /// The bucket's ETag for the manifest cached in [`MANIFEST_FILE`].
    #[serde(default)]
    pub manifest_etag: Option<String>,
    /// The tick, by its [`TICK_FILE`] id, in which this host last knew the
    /// bucket to hold the stamped bytes: after a probe found them current,
    /// a pull fetched them, or this host's own push sent them.
    #[serde(default)]
    pub verified_tick: Option<String>,
}

impl Stamp {
    /// The stamp in `dir`, when there is one that parses.
    pub fn load(dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join(STAMP_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write the stamp into `dir`, replacing it whole. Best-effort: a stamp
    /// that can't be written only costs the next read a transfer.
    pub fn save(&self, dir: &Path) {
        let Ok(bytes) = serde_json::to_vec(self) else {
            return;
        };
        write_replacing(dir, STAMP_FILE, &bytes);
    }

    /// Whether `bytes` are exactly the stamped bytes.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.state_len && sha256_hex(bytes) == self.state_sha256
    }

    /// Whether `bytes` are the stamped bytes with more appended: a local log
    /// that has written events since this host last synced it (koto#477).
    pub fn extended_by(&self, bytes: &[u8]) -> bool {
        let len = self.state_len as usize;
        bytes.len() > len && sha256_hex(&bytes[..len]) == self.state_sha256
    }

    /// A stamp for `bytes`, stored under `state_key` with `etag`, keeping the
    /// cached manifest's ETag when the key is the same.
    pub fn for_bytes(previous: Option<&Stamp>, state_key: &str, etag: &str, bytes: &[u8]) -> Self {
        Stamp {
            state_key: state_key.to_string(),
            state_etag: Some(etag.to_string()),
            state_len: bytes.len() as u64,
            state_sha256: sha256_hex(bytes),
            manifest_etag: previous
                .filter(|p| p.state_key == state_key)
                .and_then(|p| p.manifest_etag.clone()),
            verified_tick: None,
        }
    }
}

/// What one listing of a session's prefix says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// The session carries a migration marker.
    pub marker: bool,
    /// The bucket's ETag for the state file; `None` when it has none.
    pub state_etag: Option<String>,
    /// The bucket's ETag for the manifest; `None` when it has none.
    pub manifest_etag: Option<String>,
}

/// One listing of a session, as this host last made it, which other koto
/// processes on the host reuse for the check window (koto#439).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SharedCheck {
    /// The bucket the listing was made in.
    bucket: String,
    /// The state file's bucket key, as the stamp keeps it, so a check left
    /// in a directory that moved to another name is never reused for it.
    state_key: String,
    /// When the listing was made, in milliseconds since the Unix epoch.
    checked_at_ms: u64,
    marker: bool,
    state_etag: Option<String>,
    manifest_etag: Option<String>,
}

fn now_ms(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The listing of the session in `dir` that this host made within `window`
/// before `now`, in `bucket`, for `state_key`. A check stamped after `now`
/// (the clock moved back) is not reused: the window can't be told from it.
pub fn load_check(
    dir: &Path,
    bucket: &str,
    state_key: &str,
    window: std::time::Duration,
    now: std::time::SystemTime,
) -> Option<Probe> {
    if window.is_zero() {
        return None;
    }
    let bytes = std::fs::read(dir.join(CHECK_FILE)).ok()?;
    let check: SharedCheck = serde_json::from_slice(&bytes).ok()?;
    let now = now_ms(now);
    if check.bucket != bucket
        || check.state_key != state_key
        || check.checked_at_ms > now
        || now - check.checked_at_ms >= window.as_millis() as u64
    {
        return None;
    }
    Some(Probe {
        marker: check.marker,
        state_etag: check.state_etag,
        manifest_etag: check.manifest_etag,
    })
}

/// Keep `probe`, the listing just made at `now`, for this host's other
/// processes. Best-effort.
pub fn save_check(
    dir: &Path,
    bucket: &str,
    state_key: &str,
    probe: &Probe,
    now: std::time::SystemTime,
) {
    let check = SharedCheck {
        bucket: bucket.to_string(),
        state_key: state_key.to_string(),
        checked_at_ms: now_ms(now),
        marker: probe.marker,
        state_etag: probe.state_etag.clone(),
        manifest_etag: probe.manifest_etag.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&check) {
        write_replacing(dir, CHECK_FILE, &bytes);
    }
}

/// Drop the shared check of the session in `dir`, after this host changed
/// what the bucket holds for it.
pub fn forget_check(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(CHECK_FILE));
}

/// Mark the session in `dir` as owing its summary ([`SUMMARY_OWED_FILE`]).
/// Best-effort.
pub fn mark_summary_owed(dir: &Path) {
    if dir.is_dir() {
        let _ = std::fs::write(dir.join(SUMMARY_OWED_FILE), b"");
    }
}

/// Whether the session in `dir` owes its summary.
pub fn summary_owed(dir: &Path) -> bool {
    dir.join(SUMMARY_OWED_FILE).exists()
}

/// Clear the mark [`mark_summary_owed`] left in `dir`, if any.
pub fn clear_summary_owed(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(SUMMARY_OWED_FILE));
}

/// An ETag as a header or a listing gives it, without its quotes.
pub fn normalize_etag(etag: &str) -> String {
    etag.trim().trim_matches('"').to_string()
}

/// The cached remote manifest's bytes in `dir`.
pub fn load_manifest(dir: &Path) -> Option<Vec<u8>> {
    std::fs::read(dir.join(MANIFEST_FILE)).ok()
}

/// Cache the remote manifest's bytes in `dir`. Best-effort.
pub fn save_manifest(dir: &Path, bytes: &[u8]) {
    write_replacing(dir, MANIFEST_FILE, bytes);
}

/// Start a tick of the session whose directory is `dir`: write a new id for
/// it, which every command the tick runs reads. Best-effort: with no id, a
/// command probes as it would outside a tick.
pub fn begin_tick(dir: &Path) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let id = format!("{}-{}", std::process::id(), nanos);
    write_replacing(dir, TICK_FILE, id.as_bytes());
}

/// The id of the tick running the session whose directory is `dir`.
pub fn tick_id(dir: &Path) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(TICK_FILE)).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Write `name` in `dir` through a temporary file renamed into place, so a
/// reader never sees it half written. Failures are ignored.
fn write_replacing(dir: &Path, name: &str, bytes: &[u8]) {
    if !dir.is_dir() {
        return;
    }
    let tmp = dir.join(format!(".{}.{}.tmp", name, std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, dir.join(name)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_owed_mark_is_left_and_cleared_only_in_an_existing_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(!summary_owed(dir.path()));
        mark_summary_owed(dir.path());
        assert!(summary_owed(dir.path()));
        assert!(dir.path().join(SUMMARY_OWED_FILE).exists());
        clear_summary_owed(dir.path());
        assert!(!summary_owed(dir.path()));
        clear_summary_owed(dir.path());

        let gone = dir.path().join("gone");
        mark_summary_owed(&gone);
        assert!(!gone.exists(), "no directory is made for the mark");
    }

    #[test]
    fn a_stamp_round_trips_and_matches_only_its_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let stamp = Stamp::for_bytes(None, "p/wf/koto-wf.state.jsonl", "abc", b"hello");
        stamp.save(dir.path());
        let loaded = Stamp::load(dir.path()).unwrap();
        assert_eq!(loaded, stamp);
        assert!(loaded.matches(b"hello"));
        assert!(!loaded.matches(b"hellO"));
        assert!(!loaded.matches(b"hello!"));
    }

    #[test]
    fn a_new_stamp_keeps_the_manifest_etag_only_for_the_same_key() {
        let mut old = Stamp::for_bytes(None, "k", "e1", b"a");
        old.manifest_etag = Some("m1".to_string());
        assert_eq!(
            Stamp::for_bytes(Some(&old), "k", "e2", b"b").manifest_etag,
            Some("m1".to_string())
        );
        assert_eq!(
            Stamp::for_bytes(Some(&old), "other", "e2", b"b").manifest_etag,
            None
        );
    }

    #[test]
    fn a_synced_record_is_kept_per_home_and_drops_a_removed_key() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(SyncedKeys::load(dir.path(), "p1").hash("k"), None);
        note_synced(dir.path(), "p1", [("k", "h1"), ("j", "h2")]);
        assert_eq!(SyncedKeys::load(dir.path(), "p1").hash("k"), Some("h1"));
        // Another home's record says nothing of this one.
        assert_eq!(SyncedKeys::load(dir.path(), "p2").hash("k"), None);
        forget_synced(dir.path(), "k");
        let synced = SyncedKeys::load(dir.path(), "p1");
        assert_eq!(synced.hash("k"), None);
        assert_eq!(synced.hash("j"), Some("h2"));
        // A sync with a new home replaces the old home's record.
        note_synced(dir.path(), "p2", [("j", "h3")]);
        assert_eq!(SyncedKeys::load(dir.path(), "p1").hash("j"), None);
        assert_eq!(SyncedKeys::load(dir.path(), "p2").hash("j"), Some("h3"));
    }

    #[test]
    fn etags_lose_their_quotes() {
        assert_eq!(normalize_etag("\"abc\""), "abc");
        assert_eq!(normalize_etag(" abc "), "abc");
    }

    #[test]
    fn a_tick_id_is_new_per_tick_and_absent_before_one() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(tick_id(dir.path()), None);
        begin_tick(dir.path());
        let first = tick_id(dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        begin_tick(dir.path());
        assert_ne!(tick_id(dir.path()).unwrap(), first);
    }

    #[test]
    fn nothing_is_written_into_a_missing_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let gone = dir.path().join("gone");
        Stamp::default().save(&gone);
        begin_tick(&gone);
        assert!(!gone.exists());
    }

    #[test]
    fn a_shared_check_is_reused_only_inside_its_window_for_its_session() {
        use std::time::{Duration, SystemTime};
        let dir = tempfile::TempDir::new().unwrap();
        let probe = Probe {
            marker: false,
            state_etag: Some("s1".into()),
            manifest_etag: Some("m1".into()),
        };
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let window = Duration::from_secs(5);
        save_check(
            dir.path(),
            "https://b",
            "p/wf/koto-wf.state.jsonl",
            &probe,
            at,
        );
        let load = |bucket: &str, key: &str, window: Duration, now: SystemTime| {
            load_check(dir.path(), bucket, key, window, now)
        };
        let key = "p/wf/koto-wf.state.jsonl";
        let later = |ms: u64| at + Duration::from_millis(ms);
        assert_eq!(
            load("https://b", key, window, later(4_999)),
            Some(probe.clone())
        );
        assert_eq!(
            load("https://b", key, window, later(5_000)),
            None,
            "the window passed"
        );
        assert_eq!(
            load("https://b", key, Duration::ZERO, later(0)),
            None,
            "a window of 0"
        );
        assert_eq!(
            load("https://other", key, window, later(0)),
            None,
            "another bucket"
        );
        assert_eq!(
            load("https://b", "p/x/koto-x.state.jsonl", window, later(0)),
            None
        );
        assert_eq!(
            load("https://b", key, window, at - Duration::from_secs(1)),
            None,
            "a check stamped after now"
        );
        forget_check(dir.path());
        assert_eq!(load("https://b", key, window, later(0)), None, "forgotten");
    }
}
