//! The same-host import takeover: `koto session import <name> --from <A>`
//! run in another workspace on the host the source was created on.
//!
//! Every workspace on a host shares one local store, keyed by name, so the
//! source's own copy already holds `<name>`. When that copy is the source
//! itself (same `session_id` as the source's state file in the bucket) and
//! holds nothing the bucket lacks, the import takes the name over instead of
//! refusing `import_name_taken`. The order is the cloud-session-config
//! design's, chosen so the local store never holds the new session while
//! the source's prefix lacks its marker:
//!
//! 1. check, writing nothing: the source's locks, whether its copy is ahead
//!    of the bucket, its children, this workspace's prefix, the exchange;
//! 2. stage the new session in a private `.import-<name>-<random>` and,
//!    once it is complete, rename it to `<sessions>/.takeover-<name>`, so
//!    that name only ever holds a whole session;
//! 3. push it under this workspace's prefix;
//! 4. re-check under the held locks, then write the marker on the source;
//! 5. exchange `.takeover-<name>` and `<name>` in one call;
//! 6. journal the import, then delete the old copy now in `.takeover-<name>`,
//!    or, if it changed since it was checked, move it aside to
//!    `.takeover-kept-<name>-<time>` so the name is usable.
//!
//! A failure before the marker takes back what was pushed and leaves the
//! source as it was. A failure or a crash after it is completed by running
//! the same command again: the import never deletes anything under the
//! source's prefix, and the marker names the new session's id.

use std::path::Path;

use super::*;

/// The fault-injection seam for the binary's tests, in the pattern of the
/// run journal's `KOTO_TEST_*` hooks: a debug build reads it, a release
/// build never does, so it is not a setting. Unset, nothing changes.
const FAULT_ENV: &str = "KOTO_TEST_IMPORT_FAULT";

fn fault(point: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(FAULT_ENV).as_deref() == Ok(point)
}

/// Stop as a killed process would: no destructor runs, so nothing is
/// cleaned up.
fn crash_at(point: &str) {
    if fault(point) {
        eprintln!("{}: stopping at {}", FAULT_ENV, point);
        std::process::exit(86);
    }
}

/// What the takeover checked about the source's local copy, kept to
/// re-check under the locks and before the old copy is deleted.
pub(super) struct Verified {
    session_id: String,
    last_seq: u64,
    manifest: Option<Vec<u8>>,
}

/// Locks held on the source's local copy from the checks through the
/// exchange. The files stay open, so the locks move with the directory.
struct HeldLocks {
    _state: SessionLock,
    _manifest: Option<std::fs::File>,
}

impl CloudBackend {
    /// Whether `req` is a same-host takeover: no `--as`, a local copy at
    /// the name that is the source's own (same non-empty `session_id`), and
    /// a source workspace other than this one.
    pub(super) fn is_takeover(&self, req: &ImportRequest<'_>, source: &ImportSource) -> bool {
        if req.explicit_as || req.target != source.name {
            return false;
        }
        if source.prefix == self.prefix {
            return false;
        }
        let sid = &source.log.header.session_id;
        !sid.is_empty() && self.local_session_id(req.target).as_deref() == Some(sid.as_str())
    }

    /// The `session_id` in the local copy's header at `name`.
    fn local_session_id(&self, name: &str) -> Option<String> {
        let bytes = std::fs::read(self.local.session_dir(name).join(state_file_name(name))).ok()?;
        header_session_id(&bytes)
    }

    /// Run the takeover from the start (no marker on the source yet), or
    /// from step 2 again when `reuse_session_id` names the id an earlier
    /// run's marker recorded.
    pub(super) fn takeover(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
        reuse_session_id: Option<&str>,
    ) -> Result<ImportOutcome, ImportError> {
        use ImportErrorCode::*;
        let name = req.target;
        let base = self.local.base_dir().to_path_buf();
        let holder = self.local.session_dir(name);
        let pending = takeover_dir(&base, name);

        // 1. Checks. Nothing is written until they all pass.
        // The probe comes first: taking the locks creates `ctx/manifest.lock`,
        // which must never happen through a symlink at the name.
        self.probe_exchange(&base, &holder)?;
        let (manifest, manifest_bytes) = self.read_source_manifest(source)?;
        let locks = self.lock_source_copy(name)?;
        let verified = self.verify_source_copy(name, source, &manifest)?;
        if reuse_session_id.is_none() {
            self.remote_name_free(req, source)?;
        } else {
            self.remote_copy_unmoved(req, source)?;
        }

        // 2. Stage.
        let template = self.import_template(req, source)?;
        let machine_id = import_machine_id()?;
        self.clear_leftover_takeover(&pending, source)?;
        let scratch = base.join(format!(
            ".import-{}-{}",
            name,
            crate::engine::types::generate_session_id()
        ));
        let (mut staging, session_id) = self.stage_import(
            req,
            source,
            &template,
            &manifest,
            manifest_bytes.as_deref(),
            &machine_id,
            Some((&scratch, reuse_session_id)),
        )?;
        crash_at("after-staging");
        crate::engine::atomic_fs::atomic_rename_dir(&staging.dir, &pending).map_err(|e| {
            ImportError::new(
                PushFailed,
                format!(
                    "could not move the staged session to {}: {:?}",
                    pending.display(),
                    e
                ),
            )
        })?;
        staging.dir = pending.clone();

        // 3. Push.
        let pushed = self.push_staged(name, &staging.dir, source, &manifest)?;
        crash_at("after-push");

        // 4. Re-check under the locks, then mark. A definite marker failure
        // still takes everything back; past this point nothing does.
        let again = self.verify_source_copy(name, source, &manifest);
        if let Err(e) = again.and_then(|v| {
            if v.session_id == verified.session_id
                && v.last_seq == verified.last_seq
                && v.manifest == verified.manifest
            {
                Ok(())
            } else {
                Err(ImportError::new(
                    SourceUnpushed,
                    format!("session '{}' changed while the import ran", name),
                ))
            }
        }) {
            // Before the marker, everything is taken back. On a re-run the
            // marker already names this copy, so what was pushed stays for
            // the next run to finish.
            if reuse_session_id.is_none() {
                self.take_back(&pushed);
            }
            return Err(e);
        }
        // From here an outcome that isn't known keeps the staged session:
        // the marker may have landed, and `.takeover-<name>` is what keeps
        // the source's copy refusing until a re-run settles it.
        staging.moved = true;
        if reuse_session_id.is_none() {
            if let Err(e) = self.mark_source(req, source, &session_id, &machine_id) {
                if let Some(stop) = self.after_failed_mark(e, source, &pushed) {
                    return Err(stop);
                }
            }
        }
        crash_at("after-mark");

        // 5. Exchange.
        let exchanged = if fault("exchange-error") {
            Err(std::io::Error::other("injected exchange failure"))
        } else {
            crate::engine::atomic_fs::exchange_dirs(&pending, &holder)
        };
        if let Err(e) = exchanged {
            return Err(ImportError::new(
                PushFailed,
                format!(
                    "the source is marked migrated, but {} could not be exchanged with {}: {}; \
                     run the same import again to finish it",
                    pending.display(),
                    holder.display(),
                    e
                ),
            ));
        }
        drop(locks);
        // The state file was named after the target already; only a
        // finished record of the name would now be stale.
        if let Err(e) = crate::session::finished::remove(&base, name) {
            eprintln!("warning: could not remove the finished record for {name}: {e:#}");
        }
        crash_at("after-exchange");

        // 6. Journal, then delete the old copy.
        self.finish_takeover(name, source, Some(&verified));
        Ok(ImportOutcome {
            name: name.to_string(),
            from_workspace: source.workspace.clone(),
            from_session: source.name.clone(),
            keys: manifest.keys.len(),
            template: if req.trust_template {
                TemplateOrigin::Bucket
            } else {
                TemplateOrigin::LocalCache
            },
        })
    }

    /// Journal the import and delete the old copy left in
    /// `.takeover-<name>` after the exchange. Run by the takeover itself and
    /// by a re-run that finds the exchange done but the directory still
    /// there. The old copy is deleted only when it is still the source's
    /// copy as it was checked (or, on a re-run, as the bucket has it): a
    /// copy that grew is kept, with a warning naming it.
    pub(super) fn finish_takeover(
        &self,
        name: &str,
        source: &ImportSource,
        verified: Option<&Verified>,
    ) {
        if let Ok((header, events)) = self.local.read_events(name) {
            let current = crate::engine::persistence::derive_state_from_log(&events);
            crate::run_journal::imported(
                self.local.journal_root(),
                &self.local,
                name,
                &header,
                current.as_deref(),
                &source.log.header.session_id,
            );
        }
        crash_at("after-journal");

        let pending = takeover_dir(self.local.base_dir(), name);
        let md = match std::fs::symlink_metadata(&pending) {
            Ok(md) => md,
            Err(_) => return,
        };
        if md.file_type().is_symlink() || !md.is_dir() {
            self.keep_old_copy(name, &pending, "is not a directory");
            return;
        }
        let unchanged = match verified {
            Some(v) => {
                let state = std::fs::read(pending.join(state_file_name(name))).ok();
                let manifest = std::fs::read(pending.join("ctx").join("manifest.json")).ok();
                state.as_deref().is_some_and(|bytes| {
                    header_session_id(bytes).as_deref() == Some(v.session_id.as_str())
                        && last_seq_of(bytes) == Some(v.last_seq)
                }) && manifest == v.manifest
            }
            None => self.old_copy_in_bucket(name, &pending, source),
        };
        if unchanged {
            if let Err(e) = std::fs::remove_dir_all(&pending) {
                eprintln!(
                    "warning: could not remove the old copy at {}: {}",
                    pending.display(),
                    e
                );
            }
        } else {
            self.keep_old_copy(name, &pending, "changed after the import checked it");
        }
    }

    /// On a re-run, with nothing recorded from the first check: whether the
    /// old copy in `pending` is still the source and holds nothing the
    /// bucket lacks, events or context. A state file already gone means an
    /// earlier run was deleting it.
    fn old_copy_in_bucket(&self, name: &str, pending: &Path, source: &ImportSource) -> bool {
        let Ok(bytes) = std::fs::read(pending.join(state_file_name(name))) else {
            return true;
        };
        let Ok(old) = parse_source_log(&bytes) else {
            return false;
        };
        if old.header.session_id != source.log.header.session_id {
            return false;
        }
        let Ok((remote_manifest, _)) = self.read_source_manifest(source) else {
            return false;
        };
        let old_manifest: crate::session::context::Manifest =
            std::fs::read(pending.join("ctx").join("manifest.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        local_ahead_of(&old, &source.log, &old_manifest, &remote_manifest).is_none()
    }

    /// Move a `.takeover-<name>` that can't be deleted safely aside to
    /// `.takeover-kept-<name>-<time>`, where it no longer holds the name
    /// refusing, and say where it went.
    fn keep_old_copy(&self, name: &str, pending: &Path, why: &str) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let kept = self
            .local
            .base_dir()
            .join(format!(".takeover-kept-{}-{}", name, stamp));
        match crate::engine::atomic_fs::atomic_rename_dir(pending, &kept) {
            Ok(()) => eprintln!(
                "warning: the old copy of '{}' {}, so it was kept at {}; look at it, then \
                 remove it by hand",
                name,
                why,
                kept.display()
            ),
            Err(e) => eprintln!(
                "warning: the old copy of '{}' {}, and it could not be moved aside from {} \
                 ({:?}); move or remove it by hand",
                name,
                why,
                pending.display(),
                e
            ),
        }
    }

    /// Take the source copy's state-file lock and its manifest lock,
    /// non-blocking. Either held by another process refuses
    /// `import_source_running`.
    fn lock_source_copy(&self, name: &str) -> Result<HeldLocks, ImportError> {
        use ImportErrorCode::*;
        let running = || {
            ImportError::new(
                SourceRunning,
                format!(
                    "another process holds session '{}''s lock on this host, so the source is \
                     still running; stop it, then import again",
                    name
                ),
            )
        };
        let state = match self.local.lock_state_file(name) {
            Ok(lock) => lock,
            Err(SessionError::Locked { .. }) => return Err(running()),
            Err(e) => {
                return Err(ImportError::new(
                    PushFailed,
                    format!("could not lock session '{}': {}", name, e),
                ))
            }
        };
        let manifest_lock = self
            .local
            .session_dir(name)
            .join("ctx")
            .join("manifest.lock");
        let manifest = match try_flock(&manifest_lock) {
            Ok(file) => file,
            Err(TryLock::Held) => return Err(running()),
            Err(TryLock::Io(e)) => {
                return Err(ImportError::new(
                    PushFailed,
                    format!("could not lock {}: {}", manifest_lock.display(), e),
                ))
            }
        };
        Ok(HeldLocks {
            _state: state,
            _manifest: manifest,
        })
    }

    /// Check the source's local copy against the bucket's: still the
    /// source, not ahead of it, and no local child. Refusals are
    /// `import_source_unpushed` and `import_source_has_children`.
    fn verify_source_copy(
        &self,
        name: &str,
        source: &ImportSource,
        manifest: &crate::session::context::Manifest,
    ) -> Result<Verified, ImportError> {
        use ImportErrorCode::*;
        let dir = self.local.session_dir(name);
        let bytes = std::fs::read(dir.join(state_file_name(name))).map_err(|e| {
            ImportError::new(
                PushFailed,
                format!("could not read the local copy of '{}': {}", name, e),
            )
        })?;
        let local = parse_source_log(&bytes).map_err(|e| {
            ImportError::new(
                SourceUnpushed,
                format!(
                    "the local copy of session '{}' can't be read ({}); push it with `koto \
                     session resolve {} --keep local` from the source workspace first",
                    name, e, name
                ),
            )
        })?;
        if local.header.session_id != source.log.header.session_id {
            return Err(ImportError::new(
                NameTaken,
                format!(
                    "the session named '{}' on this machine is no longer the source; pass --as \
                     <new-name> to import it under another name",
                    name
                ),
            ));
        }
        let manifest_path = dir.join("ctx").join("manifest.json");
        let manifest_bytes = std::fs::read(&manifest_path).ok();
        let local_manifest: crate::session::context::Manifest = manifest_bytes
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default();
        if let Some(why) = local_ahead_of(&local, &source.log, &local_manifest, manifest) {
            return Err(ImportError::new(
                SourceUnpushed,
                format!(
                    "the local copy of session '{}' has {} that the bucket lacks; push it with \
                     `koto session resolve {} --keep local` from the source workspace, then \
                     import again",
                    name, why, name
                ),
            ));
        }
        if let Some(child) = self.local_child_of(name) {
            return Err(ImportError::new(
                SourceHasChildren,
                format!(
                    "session '{}' names '{}' as its parent on this machine; an import carries no \
                     children, and the new session would take them over by name. Finish or \
                     clean up '{}' first",
                    child, name, child
                ),
            ));
        }
        Ok(Verified {
            session_id: local.header.session_id,
            last_seq: local.last_seq,
            manifest: manifest_bytes,
        })
    }

    /// A session in the local store whose header names `name` as parent.
    fn local_child_of(&self, name: &str) -> Option<String> {
        let sessions = self.local.list().ok()?;
        sessions
            .into_iter()
            .find(|s| s.parent_workflow.as_deref() == Some(name))
            .map(|s| s.id)
    }

    /// Check that the name and its store can take an exchange, before
    /// anything is written: a real directory, on the store's device, in a
    /// store this user owns whose filesystem accepts the exchange.
    fn probe_exchange(&self, base: &Path, holder: &Path) -> Result<(), ImportError> {
        let refuse = |why: &str| {
            ImportError::new(
                ImportErrorCode::PushFailed,
                format!(
                    "the session can't take over its own name on this host: {}; import it \
                     under another name with --as <new-name>",
                    why
                ),
            )
        };
        let md = std::fs::symlink_metadata(holder)
            .map_err(|e| refuse(&format!("{} can't be read ({})", holder.display(), e)))?;
        if md.file_type().is_symlink() || !md.is_dir() {
            return Err(refuse(&format!(
                "{} is not a plain directory",
                holder.display()
            )));
        }
        let same_device = if fault("name-other-device") {
            false
        } else {
            same_device(base, holder)
        };
        if !same_device {
            return Err(refuse(&format!(
                "{} is on another device than the session store",
                holder.display()
            )));
        }
        let supported =
            !fault("exchange-unsupported") && crate::engine::atomic_fs::exchange_supported(base);
        if !supported {
            return Err(refuse(
                "the session store's filesystem doesn't support an atomic directory exchange",
            ));
        }
        Ok(())
    }

    /// A `.takeover-<name>` left by an earlier run of this import (staged,
    /// never exchanged) is removed; anything else there refuses, naming it.
    fn clear_leftover_takeover(
        &self,
        pending: &Path,
        source: &ImportSource,
    ) -> Result<(), ImportError> {
        if std::fs::symlink_metadata(pending).is_err() {
            return Ok(());
        }
        let name = &source.name;
        // A plain directory with no state file is one an earlier run was
        // deleting: staging only ever renames a complete session here.
        let plain_dir = std::fs::symlink_metadata(pending)
            .is_ok_and(|md| md.is_dir() && !md.file_type().is_symlink());
        let ours = match std::fs::read(pending.join(state_file_name(name))) {
            Ok(b) => ImportedAs::read(&b).is_some_and(|i| i.is_of(source)),
            Err(e) => plain_dir && e.kind() == std::io::ErrorKind::NotFound,
        };
        if !ours {
            return Err(ImportError::new(
                ImportErrorCode::PushFailed,
                format!(
                    "{} is in the way and isn't this import's; look at it, and remove it by hand \
                     if it isn't wanted",
                    pending.display()
                ),
            ));
        }
        std::fs::remove_dir_all(pending).map_err(|e| {
            ImportError::new(
                ImportErrorCode::PushFailed,
                format!("could not remove {}: {}", pending.display(), e),
            )
        })
    }

    /// The marker step failed: find out whether the marker landed. Landed,
    /// `None`, and the import goes on to the exchange; definitely absent,
    /// everything is taken back and the run stops at `import_push_failed`
    /// with the source unchanged; unknown, everything is kept and the run
    /// stops at `import_unmarked`, which a re-run finishes.
    fn after_failed_mark(
        &self,
        err: ImportError,
        source: &ImportSource,
        pushed: &[String],
    ) -> Option<ImportError> {
        let pending = takeover_dir(self.local.base_dir(), &source.name);
        if err.code != ImportErrorCode::Unmarked {
            // Another import marked the source meanwhile.
            self.take_back(pushed);
            let _ = std::fs::remove_dir_all(&pending);
            return Some(err);
        }
        let listed = if fault("marker-unknown") {
            Err(anyhow::anyhow!("injected unknown marker outcome"))
        } else {
            self.object_listed(&source.key(MIGRATED_MARKER))
        };
        match listed {
            Ok(true) => None,
            Ok(false) => {
                self.take_back(pushed);
                let _ = std::fs::remove_dir_all(&pending);
                Some(ImportError::new(
                    ImportErrorCode::PushFailed,
                    format!(
                        "the marker on the source could not be written, so nothing changed: {}",
                        err.message
                    ),
                ))
            }
            Err(_) => Some(err),
        }
    }

    /// For a re-run after the marker: this workspace's copy of the name in
    /// the bucket must still be the import as pushed, with nothing appended
    /// after its `session_imported` event and the source's events before
    /// it; otherwise it moved on, and nothing is overwritten.
    fn remote_copy_unmoved(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
    ) -> Result<(), ImportError> {
        let key = self.state_key(req.target);
        let bytes = match self.get_object_if_present(&key) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(()),
            Err(e) => {
                return Err(ImportError::new(
                    ImportErrorCode::PushFailed,
                    format!(
                        "could not read this workspace's copy of '{}': {}",
                        req.target, e
                    ),
                ))
            }
        };
        let moved = || {
            ImportError::new(
                ImportErrorCode::SourceMigrated,
                format!(
                    "the source is marked migrated to '{}' here, and this workspace's copy in \
                     the bucket has moved on since; nothing was overwritten. Continue that copy, \
                     or remove it and import again",
                    req.target
                ),
            )
        };
        let Ok(log) = parse_source_log(&bytes) else {
            return Err(moved());
        };
        let Some(last) = log.event_lines.last() else {
            return Err(moved());
        };
        let ends_with_import = serde_json::from_str::<crate::engine::types::Event>(last)
            .map(|e| e.event_type == "session_imported")
            .unwrap_or(false);
        let before = &log.event_lines[..log.event_lines.len() - 1];
        let prefix_of_source = before.len() <= source.log.event_lines.len()
            && before
                .iter()
                .zip(&source.log.event_lines)
                .all(|(a, b)| a == b);
        if ends_with_import && prefix_of_source {
            Ok(())
        } else {
            Err(moved())
        }
    }
}

/// Why `local` holds something the bucket's `remote` lacks, or `None`.
///
/// Ahead means the local event lines aren't a prefix of the bucket's (the
/// local log is longer, or differs at a `seq` both hold), or the local
/// manifest holds a key the bucket's lacks, or one whose hash differs and
/// whose `created_at` is later.
fn local_ahead_of(
    local: &SourceLog,
    remote: &SourceLog,
    local_manifest: &crate::session::context::Manifest,
    remote_manifest: &crate::session::context::Manifest,
) -> Option<String> {
    if local.event_lines.len() > remote.event_lines.len() {
        return Some(format!(
            "{} event(s)",
            local.event_lines.len() - remote.event_lines.len()
        ));
    }
    if let Some(i) = local
        .event_lines
        .iter()
        .zip(&remote.event_lines)
        .position(|(a, b)| a != b)
    {
        return Some(format!("a different event at seq {}", i + 1));
    }
    for (key, meta) in &local_manifest.keys {
        match remote_manifest.keys.get(key) {
            None => return Some(format!("context key {:?}", key)),
            Some(r) if r.hash != meta.hash && meta.created_at > r.created_at => {
                return Some(format!("a newer version of context key {:?}", key))
            }
            _ => {}
        }
    }
    None
}

/// The last event's `seq` in a state log's bytes.
fn last_seq_of(bytes: &[u8]) -> Option<u64> {
    parse_source_log(bytes).ok().map(|l| l.last_seq)
}

/// Whether `a` and `b` sit on one device.
fn same_device(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        true
    }
}

enum TryLock {
    Held,
    Io(std::io::Error),
}

/// Take an exclusive flock on `path` without waiting, creating the file
/// so a writer that arrives later waits on it too. A missing directory
/// means no one holds it: `Ok(None)`.
fn try_flock(path: &Path) -> Result<Option<std::fs::File>, TryLock> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(TryLock::Io(e)),
    };
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the fd belongs to `file`, which outlives the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(TryLock::Held);
            }
            return Err(TryLock::Io(err));
        }
    }
    Ok(Some(file))
}
