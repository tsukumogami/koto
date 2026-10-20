//! The locked writer: the one way a line reaches the store.
//!
//! [`Store::append`] validates and assembles the line before it takes the
//! lock, so the lock is held only for the check, the append and the sync.
//! Under the lock it repairs a torn tail on the target file, skips a line
//! already present with identical content (and refuses one present with
//! different content), runs the caller's check closure, and appends what
//! the closure allows.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::reader;
use super::record::{validate_outcome, validate_subject, Decision, Kind};
use super::{
    id_month, redact_decision, time_month, FieldError, Store, StoreError, DECISION_FORMAT_VERSION,
    LOCK_FILE, MAX_BASIS, MAX_EVIDENCE, MAX_LINE_BYTES, REVERSALS_FILE,
};

/// How long [`Store::append`] waits for the lock: the request store's
/// bound.
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Sleep between non-blocking lock attempts.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// The lock wait a debug build reads from the environment in place of
/// [`LOCK_WAIT_TIMEOUT`], in milliseconds, so the binary's tests can hold
/// the lock past the bound without waiting seconds. Release builds don't
/// read it, so it is not a setting.
pub const LOCK_WAIT_TEST_ENV: &str = "KOTO_TEST_DECISION_LOCK_WAIT_MS";

fn lock_wait() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var(LOCK_WAIT_TEST_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return Duration::from_millis(ms);
    }
    LOCK_WAIT_TIMEOUT
}

/// What the check closure decides about a decision under the lock.
#[derive(Debug)]
pub enum Check {
    /// Append the decision.
    Append,
    /// Write nothing: the decision is already accounted for.
    Skip,
    /// Don't append the decision. With a line, append that `refused`
    /// decision instead.
    Refuse(Option<Box<Decision>>),
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The decision was appended.
    Appended { id: String },
    /// The decision was already on the record (identical content), or the
    /// check skipped it; nothing was written.
    Skipped { id: String },
    /// The check refused the decision; `refused_id` names the `refused`
    /// line written in its place, when there was one.
    Refused { refused_id: Option<String> },
}

/// Read access to the store while the writer holds its lock, for check
/// closures that must decide against what is on the record (a reversal
/// race, an import repeat).
pub struct LockedStore<'a> {
    root: &'a Path,
}

impl LockedStore<'_> {
    /// The decision with this identifier, from the month file it names or
    /// the reversal log.
    pub fn find(&self, id: &str) -> Result<Option<Decision>, StoreError> {
        reader::find_decision(self.root, id)
    }

    /// Every line in the reversal log.
    pub fn reversals(&self) -> Result<Vec<Decision>, StoreError> {
        Ok(reader::read_reversals(self.root)?.decisions)
    }

    /// The store directory, for checks that keep their own ledger (the
    /// import ledger) under the same lock.
    pub fn root(&self) -> &Path {
        self.root
    }
}

/// The file a decision belongs in: the reversal log for a `reverse`, the
/// month file of its own time for every other kind.
pub fn target_path(root: &Path, d: &Decision) -> Result<PathBuf, FieldError> {
    if d.kind == Kind::Reverse {
        return Ok(root.join(REVERSALS_FILE));
    }
    let month = time_month(&d.time).ok_or_else(|| {
        FieldError::new(
            "time",
            format!("'{}' is not a UTC time koto writes", d.time),
        )
    })?;
    Ok(root.join(format!("{}.jsonl", month)))
}

/// Check the envelope every line must satisfy, and serialize it.
///
/// Returns the line without its newline.
pub fn assemble_line(d: &Decision) -> Result<String, FieldError> {
    if d.v != DECISION_FORMAT_VERSION {
        return Err(FieldError::new(
            "v",
            format!("unknown decision format version {}", d.v),
        ));
    }
    let id_m = id_month(&d.id).ok_or_else(|| {
        FieldError::new(
            "id",
            format!(
                "'{}' is not a decision identifier (d-YYYYMM-<32 hex>)",
                d.id
            ),
        )
    })?;
    let t_m = time_month(&d.time).ok_or_else(|| {
        FieldError::new(
            "time",
            format!("'{}' is not a UTC time koto writes", d.time),
        )
    })?;
    if id_m != t_m {
        return Err(FieldError::new(
            "id",
            format!(
                "identifier {} names month {} but the decision's time is in {}",
                d.id, id_m, t_m
            ),
        ));
    }
    validate_outcome(d.kind, &d.outcome)?;
    // Links are checked here, not only in the CLI, so every caller of the
    // writer gets the rule.
    if let Some(link) = d.decided_by.link.as_deref() {
        super::validate_link("link", link)?;
    }
    if let Some(link) = d.subject.source_link.as_deref() {
        super::validate_link("source_link", link)?;
    }
    validate_subject(d.kind, &d.subject, &d.evidence)?;
    validate_reversal_fields(d)?;
    if d.basis.len() > MAX_BASIS {
        return Err(FieldError::new(
            "basis",
            format!(
                "{} basis identifiers exceed the maximum of {}",
                d.basis.len(),
                MAX_BASIS
            ),
        ));
    }
    if d.evidence.len() > MAX_EVIDENCE {
        return Err(FieldError::new(
            "evidence",
            format!(
                "{} evidence pointers exceed the maximum of {}",
                d.evidence.len(),
                MAX_EVIDENCE
            ),
        ));
    }
    let line = serde_json::to_string(d)
        .map_err(|e| FieldError::new("decision", format!("serializing: {}", e)))?;
    if line.len() > MAX_LINE_BYTES {
        return Err(FieldError::new(
            "decision",
            format!(
                "the decision line is {} bytes, over the maximum of {}",
                line.len(),
                MAX_LINE_BYTES
            ),
        ));
    }
    Ok(line)
}

/// A `reverse` line names what it reverses twice (the subject's `decision`
/// and `reverses`), carries the copy of the original and its standing; no
/// other kind carries any of these, nor a compensating link or replacement.
fn validate_reversal_fields(d: &Decision) -> Result<(), FieldError> {
    if d.kind == Kind::Reverse {
        if d.reverses.is_none() || d.reverses != d.subject.decision {
            return Err(FieldError::new(
                "reverses",
                "a reverse decision names the decision it reverses in 'reverses' and its subject",
            ));
        }
        if d.original.is_none() {
            return Err(FieldError::new(
                "original",
                "a reverse decision carries a copy of the original it reverses",
            ));
        }
        if !matches!(
            d.authority.standing.as_deref(),
            Some("verified" | "asserted")
        ) {
            return Err(FieldError::new(
                "standing",
                "a reverse decision's authority carries its standing (verified or asserted)",
            ));
        }
        if let Some(link) = d.compensating.as_ref().and_then(|c| c.link.as_deref()) {
            super::validate_link("compensating", link)?;
        }
        return Ok(());
    }
    for (present, field) in [
        (d.reverses.is_some(), "reverses"),
        (d.original.is_some(), "original"),
        (d.authority.standing.is_some(), "standing"),
        (d.compensating.is_some(), "compensating"),
        (d.replacement.is_some(), "replacement"),
    ] {
        if present {
            return Err(FieldError::new(
                field,
                format!("only a reverse decision carries '{}'", field),
            ));
        }
    }
    Ok(())
}

impl Store {
    /// Write one decision through the locked writer.
    ///
    /// The decision is redacted and validated, and its line assembled,
    /// before the lock is taken. Under the lock: a torn tail on the target
    /// file is repaired; a line with the same identifier and identical
    /// content makes the write a skip, and one with different content an
    /// [`StoreError::IdConflict`]; then `check` decides whether to append,
    /// skip, or refuse (optionally appending a `refused` line instead).
    pub fn append<F>(&self, decision: Decision, check: F) -> Result<WriteOutcome, StoreError>
    where
        F: FnOnce(&LockedStore) -> Result<Check, StoreError>,
    {
        let mut decision = decision;
        redact_decision(&mut decision, self.redactor());
        let line = assemble_line(&decision)?;
        let target = target_path(self.root(), &decision)?;

        ensure_dir(self.root())?;
        let _lock = acquire_lock(self.root())?;

        let mut file = open_store_file(&target)?;
        repair_torn_tail(&mut file, &target)?;
        if line_already_present(&mut file, &target, &decision, &line)? {
            return Ok(WriteOutcome::Skipped { id: decision.id });
        }

        let locked = LockedStore { root: self.root() };
        match check(&locked)? {
            Check::Append => {
                append_line(&mut file, &target, &line)?;
                Ok(WriteOutcome::Appended { id: decision.id })
            }
            Check::Skip => Ok(WriteOutcome::Skipped { id: decision.id }),
            Check::Refuse(None) => Ok(WriteOutcome::Refused { refused_id: None }),
            Check::Refuse(Some(refused)) => {
                let mut refused = *refused;
                if refused.kind != Kind::Refused {
                    return Err(FieldError::new(
                        "kind",
                        "a refusal may only append a 'refused' decision",
                    )
                    .into());
                }
                redact_decision(&mut refused, self.redactor());
                let r_line = assemble_line(&refused)?;
                let r_target = target_path(self.root(), &refused)?;
                let mut r_file = open_store_file(&r_target)?;
                repair_torn_tail(&mut r_file, &r_target)?;
                if !line_already_present(&mut r_file, &r_target, &refused, &r_line)? {
                    append_line(&mut r_file, &r_target, &r_line)?;
                }
                Ok(WriteOutcome::Refused {
                    refused_id: Some(refused.id),
                })
            }
        }
    }
}

// ===== The directory =====

/// Create the store directory (mode 0700) if it is missing, and refuse it
/// when it isn't a directory owned by the current user, or is writable by
/// group or others.
#[cfg(unix)]
pub fn ensure_dir(root: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| StoreError::io(format!("creating {}", parent.display()), e))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(StoreError::io(format!("creating {}", root.display()), e)),
    }
    check_dir(root)
}

#[cfg(not(unix))]
pub fn ensure_dir(_root: &Path) -> Result<(), StoreError> {
    Err(StoreError::Unsupported)
}

/// Refuse a store directory that isn't safe to read or write.
#[cfg(unix)]
pub fn check_dir(root: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let unsafe_dir = |reason: &str| StoreError::UnsafeDir {
        path: root.display().to_string(),
        reason: reason.to_string(),
    };
    let meta = std::fs::symlink_metadata(root)
        .map_err(|e| StoreError::io(format!("reading {}", root.display()), e))?;
    if meta.file_type().is_symlink() {
        return Err(unsafe_dir("it is a symlink"));
    }
    if !meta.is_dir() {
        return Err(unsafe_dir("it is not a directory"));
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        return Err(unsafe_dir("it is owned by another user"));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(unsafe_dir("it is writable by group or others"));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn check_dir(_root: &Path) -> Result<(), StoreError> {
    Err(StoreError::Unsupported)
}

// ===== Files =====

/// Open (creating if needed) a store file for reading and appending, with
/// mode 0600 and `O_NOFOLLOW`, and confirm on the descriptor that it is a
/// regular file.
#[cfg(unix)]
pub fn open_store_file(path: &Path) -> Result<File, StoreError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| StoreError::io(format!("opening {}", path.display()), e))?;
    let meta = file
        .metadata()
        .map_err(|e| StoreError::io(format!("reading {}", path.display()), e))?;
    if !meta.is_file() {
        return Err(StoreError::UnsafeDir {
            path: path.display().to_string(),
            reason: "it is not a regular file".to_string(),
        });
    }
    Ok(file)
}

#[cfg(not(unix))]
pub fn open_store_file(_path: &Path) -> Result<File, StoreError> {
    Err(StoreError::Unsupported)
}

/// Guard whose drop releases the store's flock.
pub struct StoreLock {
    _file: File,
}

/// Take the store lock with the request store's bounded wait.
#[cfg(unix)]
pub fn acquire_lock(root: &Path) -> Result<StoreLock, StoreError> {
    use std::os::fd::AsRawFd;
    let path = root.join(LOCK_FILE);
    let file = open_store_file(&path)?;
    let started = Instant::now();
    loop {
        // SAFETY: the descriptor borrows `file`, which outlives the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(StoreLock { _file: file });
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(StoreError::io(format!("locking {}", path.display()), err));
        }
        if started.elapsed() >= lock_wait() {
            return Err(StoreError::LockContention {
                waited_ms: started.elapsed().as_millis() as u64,
            });
        }
        std::thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

#[cfg(not(unix))]
pub fn acquire_lock(_root: &Path) -> Result<StoreLock, StoreError> {
    let _ = (Instant::now(), lock_wait(), LOCK_RETRY_INTERVAL);
    Err(StoreError::Unsupported)
}

fn read_all(file: &mut File, path: &Path) -> Result<Vec<u8>, StoreError> {
    let mut buf = Vec::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_end(&mut buf))
        .map_err(|e| StoreError::io(format!("reading {}", path.display()), e))?;
    Ok(buf)
}

/// Repair a final line with no newline, through the opened descriptor.
///
/// A final line that is complete JSON gets its newline appended; one that
/// doesn't parse is truncated away. Called with the lock held. Returns
/// whether anything changed.
pub fn repair_torn_tail(file: &mut File, path: &Path) -> Result<bool, StoreError> {
    let content = read_all(file, path)?;
    if content.is_empty() || content.ends_with(b"\n") {
        return Ok(false);
    }
    let start = content
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let last = &content[start..];
    let complete = serde_json::from_slice::<serde_json::Value>(last).is_ok();
    if complete {
        file.write_all(b"\n")
            .map_err(|e| StoreError::io(format!("repairing {}", path.display()), e))?;
    } else {
        file.set_len(start as u64)
            .map_err(|e| StoreError::io(format!("truncating {}", path.display()), e))?;
    }
    file.sync_data()
        .map_err(|e| StoreError::io(format!("syncing {}", path.display()), e))?;
    Ok(true)
}

/// Whether a line with `decision`'s identifier is already in `file`.
/// Identical content is a skip; different content is an error.
fn line_already_present(
    file: &mut File,
    path: &Path,
    decision: &Decision,
    line: &str,
) -> Result<bool, StoreError> {
    let content = read_all(file, path)?;
    let needle = format!(
        "\"id\":{}",
        serde_json::to_string(&decision.id).unwrap_or_default()
    );
    let finder = aho_corasick::AhoCorasick::new([needle.as_bytes()])
        .map_err(|e| StoreError::io("building the identifier search", std::io::Error::other(e)))?;
    let ours: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| FieldError::new("decision", format!("re-reading the line: {}", e)))?;
    for existing in content.split(|b| *b == b'\n') {
        if existing.is_empty() || !finder.is_match(existing) {
            continue;
        }
        let Ok(theirs) = serde_json::from_slice::<serde_json::Value>(existing) else {
            continue;
        };
        if theirs.get("id").and_then(|v| v.as_str()) != Some(decision.id.as_str()) {
            continue;
        }
        if theirs == ours {
            return Ok(true);
        }
        return Err(StoreError::IdConflict {
            id: decision.id.clone(),
        });
    }
    Ok(false)
}

fn append_line(file: &mut File, path: &Path, line: &str) -> Result<(), StoreError> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    file.write_all(&bytes)
        .map_err(|e| StoreError::io(format!("appending to {}", path.display()), e))?;
    file.sync_data()
        .map_err(|e| StoreError::io(format!("syncing {}", path.display()), e))?;
    Ok(())
}
