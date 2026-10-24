//! What this host last knew of a session's copy in the bucket, so a cloud
//! read whose local copy is current transfers nothing (koto#428).
//!
//! Four host-local files in a session's local directory, none of them ever
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
