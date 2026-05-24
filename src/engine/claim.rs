//! Assignment-claim sidecar atomicity and drift-recovery (Issue 11).
//!
//! The sidecar at `<session-dir>/claim.lock` is the exactly-one-winner
//! primitive every other multi-coordinator safety property leans on.
//! It is created via
//!
//! ```text
//! open(O_CREAT | O_EXCL | O_WRONLY, mode=0600)
//! ```
//!
//! with a TOML payload `{coord_id, claimed_at}`. After the write
//! succeeds the writer issues `fsync(fd)` then `close(fd)`. The sidecar
//! IS truth — the best-effort `assignment_claim` header field is
//! operator-visibility only and may lag the sidecar. PRD R5
//! (exactly-one-winner) is enforced by this primitive.
//!
//! ## Drift recovery
//!
//! Coordinator crashes leave one of four observable on-disk patterns
//! that [`recover_orphaned_sidecar`] resolves:
//!
//! - **Case 3a — terminal child, stale sidecar.** L1 unlink missed
//!   between terminal-evidence fsync and process exit. Unlinks the
//!   sidecar; no audit event, no epoch bump.
//! - **Case 3b — orphan (no header claim, sidecar present).** Coord
//!   crashed AFTER taking the sidecar but BEFORE writing
//!   `assignment_claim` to the header. Emits `ChildRedelegated` then
//!   unlinks the sidecar and bumps `dispatch_epoch` (the next
//!   coordinator can claim).
//! - **Case 3c — header claim set, sidecar present, no agent
//!   dispatched.** Coord wrote header then crashed before substrate
//!   spawn. Emits `ChildRedelegated`, unlinks sidecar, clears the
//!   header claim, bumps `dispatch_epoch`.
//! - **Malformed sidecar fallthrough.** Unparseable/truncated sidecar
//!   contents synthesize a maximally-stale claim record so cases
//!   3b/3c fire — the design forbids skip-on-malformed because the
//!   safe default is to redelegate, not stall.
//!
//! Cases 3b/3c respect the `redelegation_cap` (default 3 per Issue 18).
//! On cap-exceedance the recovery flips to [`RecoveryAction::Abandon`]
//! which the caller drives through the standard `koto next --with-data`
//! terminal-evidence path (this module never writes terminal evidence
//! directly — the abandon decision is reported back to the caller for
//! routing).
//!
//! ## Security
//!
//! - Sidecar reads use `O_NOFOLLOW`: a symlink at the sidecar path is
//!   refused (Security touch-up #4 — refuses sidecar-substitute
//!   attacks where a foothold attacker plants a symlink to a sensitive
//!   target).
//! - `O_EXCL` writes refuse symlinks by POSIX (a symlink at the final
//!   path component causes `EEXIST` or `ELOOP`).
//! - Sidecar mode 0600 is set explicitly, not via umask.
//!
//! ## Race window
//!
//! The unlink-then-recreate gap (one coord unlinks the sidecar in L3
//! just before another coord's `O_EXCL` writes a fresh one) is
//! documented as **benign**: the dispatch-epoch bump in cases 3b/3c
//! plus the R43 epoch fence (Issue 13) catches any stale-epoch write
//! from the dead coordinator's still-running agent. The sidecar and
//! the epoch fence are independent mechanisms enforcing the same
//! invariant from two directions.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::audit;
use crate::engine::errors::EngineError;
use crate::engine::persistence::{append_event, read_header};
use crate::engine::types::{
    AssignmentClaim, EventPayload, StateFileHeader, ValidatedCoordId, ValidatedSessionId,
};

/// Filename for the claim sidecar inside a session directory.
pub const SIDECAR_FILENAME: &str = "claim.lock";

/// Marker used as the synthesized `coord_id` when a sidecar's contents
/// fail to parse. Threaded through to the audit event so operators can
/// see why a redelegation fired.
pub const MALFORMED_COORD_MARKER: &str = "<unparseable-sidecar>";

/// On-disk shape of the claim sidecar.
///
/// Serialized as TOML; one field per line. The `claimed_at` timestamp
/// uses RFC 3339 with millisecond precision (`YYYY-MM-DDTHH:MM:SS.sssZ`)
/// so cross-coordinator timestamp comparisons remain stable even on
/// hosts whose clock resolution is jiffy-class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarContents {
    /// Coordinator id that owns this claim.
    pub coord_id: String,
    /// RFC 3339 UTC timestamp with millisecond precision at the moment
    /// the sidecar was created.
    pub claimed_at: String,
}

/// Outcome of attempting to acquire the sidecar.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// This caller won the race; the sidecar now carries the caller's
    /// `coord_id` and `claimed_at`.
    Acquired,
    /// Another coordinator already holds the sidecar (`O_EXCL` returned
    /// `EEXIST`). The caller should treat the child as not-its-business
    /// for this tick.
    Contended,
}

/// Result of inspecting an orphan sidecar against the
/// `recover_orphaned_sidecar` state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No sidecar present; nothing to do.
    None,
    /// Sidecar present but its `claimed_at` is younger than the
    /// configured `stale_claim_timeout`. The owning coordinator is
    /// presumed live; the caller MUST leave the sidecar alone.
    Skip,
    /// Case 3a: terminal child with stale sidecar. The recovery
    /// unlinks the sidecar; no audit event, no epoch bump. The caller
    /// SHOULD also append a terminal-index entry via Issue 8's writer
    /// once that lands.
    CleanupOnly,
    /// Cases 3b/3c: orphaned non-terminal child. The recovery emits
    /// `ChildRedelegated` on the coordinator's log, unlinks the
    /// sidecar, clears (3c) or leaves alone (3b) the header
    /// `assignment_claim`, and bumps `dispatch_epoch` so the next
    /// claim from any coord receives the fresh epoch.
    Redelegate {
        /// New `dispatch_epoch` value after the bump.
        new_dispatch_epoch: u32,
        /// `true` when the recovery had to clear a populated header
        /// `assignment_claim` (case 3c); `false` when the header was
        /// already absent (case 3b).
        cleared_header_claim: bool,
    },
    /// Redelegation cap exceeded. The caller is responsible for
    /// driving the standard `koto next --with-data` terminal-evidence
    /// path (which fires Issue 8's terminal-index append AND the L1
    /// sidecar unlink in the right order). This module does NOT write
    /// terminal evidence — it only reports the decision.
    Abandon {
        /// `dispatch_epoch` value the caller observed; the
        /// terminal-evidence path will see the same value when it
        /// reads the header.
        observed_dispatch_epoch: u32,
    },
}

impl RecoveryAction {
    /// Convenience: did this action mutate on-disk state? Useful for
    /// test assertions.
    pub fn mutated(&self) -> bool {
        matches!(
            self,
            RecoveryAction::CleanupOnly | RecoveryAction::Redelegate { .. }
        )
    }
}

/// Inputs the `SubstrateSpawner` carries from coordinator into the
/// substrate's Task-spawn primitive. Captured here so tests can assert
/// the dispatch contract without depending on a concrete substrate
/// implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// Child session id rendered into both the on-disk path and the
    /// substrate's `team_name`.
    pub child_session_id: ValidatedSessionId,
    /// Substrate role marker (`scrutineer`, `reviewer`, …). Read from
    /// the child's header.
    pub role: String,
    /// Template path/name the agent will execute. Read from the child's
    /// header.
    pub template_name: String,
    /// Optional input bag passed to the spawned agent. Read from the
    /// child's header. JSON-encoded to keep the trait substrate-neutral.
    pub inputs: Option<serde_json::Value>,
    /// Coordinator's `coord_id`, used to scope the `SubagentStop` hook
    /// command's reporting back to the right coord.
    pub coord_id: ValidatedCoordId,
    /// Current dispatch-epoch value baked into the `SubagentStop` hook
    /// so the epoch fence in Issue 13 can reject stale-agent reports.
    pub dispatch_epoch: u32,
    /// `team_name` used for SendMessage addressability. Defaults to
    /// the coordinator's `coord_id` when no explicit team is set.
    pub team_name: String,
}

/// Pluggable substrate primitive abstraction.
///
/// The concrete implementation (Claude Code Task tool, bunki BK2 hosted
/// dispatch, etc.) lives outside this module. Tests use a mock that
/// records every spawn request so the dispatch contract can be asserted
/// without touching a real substrate.
pub trait SubstrateSpawner {
    /// Invoke the substrate's Task-spawn primitive. Returning `Err`
    /// from this call is **not** a claim-rollback signal: the sidecar
    /// stays in place and a future drift-recovery sweep treats the
    /// dispatch as crashed (case 3c, since the header claim was already
    /// written before this call). The caller surfaces the error to the
    /// operator but does NOT undo the claim.
    fn spawn(&self, request: &SpawnRequest) -> Result<(), EngineError>;
}

/// Outcome of [`claim_and_dispatch`].
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Sidecar acquired, header updated, audit event emitted, substrate
    /// spawn requested successfully.
    Dispatched,
    /// Another coordinator held the sidecar; nothing was written.
    Contended,
}

// ============================================================
// Sidecar primitives (acquire / read / release)
// ============================================================

/// Compute the sidecar path for a session directory.
pub fn sidecar_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SIDECAR_FILENAME)
}

/// Attempt to acquire the claim sidecar at `<session_dir>/claim.lock`.
///
/// Uses `open(O_CREAT | O_EXCL | O_WRONLY, mode=0600)` for exactly-one-
/// winner semantics. `fsync` follows the write before `close` so the
/// claim is durable before the caller proceeds. `EEXIST` is the only
/// expected non-error path; every other I/O error bubbles up.
///
/// On success the sidecar carries `{coord_id, claimed_at}` as TOML.
pub fn acquire_sidecar(
    session_dir: &Path,
    coord_id: &ValidatedCoordId,
    now: SystemTime,
) -> Result<AcquireOutcome> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("failed to ensure session dir {}", session_dir.display()))?;

    let path = sidecar_path(session_dir);
    let claimed_at = format_rfc3339_millis(now);
    let contents = SidecarContents {
        coord_id: coord_id.as_str().to_string(),
        claimed_at,
    };
    let body = toml::to_string(&contents).context("failed to serialize sidecar contents")?;

    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = match opts.open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(AcquireOutcome::Contended);
        }
        Err(e) => {
            return Err(anyhow!(
                "failed to open sidecar {} with O_EXCL: {}",
                path.display(),
                e
            ));
        }
    };

    file.write_all(body.as_bytes())
        .with_context(|| format!("failed to write sidecar contents to {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to fsync sidecar {}", path.display()))?;
    drop(file);

    Ok(AcquireOutcome::Acquired)
}

/// Read the sidecar at `<session_dir>/claim.lock` using `O_NOFOLLOW`.
///
/// A symlink at the sidecar path causes the open to fail with an error
/// matching the kernel's `ELOOP` (the underlying `OpenOptions::open`
/// surfaces it as `Other` or `InvalidInput` depending on libc version).
/// Returns `Ok(None)` when the sidecar is absent; `Ok(Some(Err(_)))` for
/// a present-but-malformed sidecar so the caller can synthesize the
/// stale-claim fallthrough.
pub fn read_sidecar(
    session_dir: &Path,
) -> Result<Option<std::result::Result<SidecarContents, String>>> {
    let path = sidecar_path(session_dir);
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // libc::O_NOFOLLOW refuses the open when the final path
        // component is a symlink (Security touch-up #4). On Linux the
        // open returns ELOOP; on macOS it also returns ELOOP. Other
        // open(2) flags (read-only here) are untouched.
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    let file = match opts.open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(e) => {
            // Surface symlink-refusal and other open errors to the
            // caller. The recovery state machine treats this as the
            // malformed-sidecar fallthrough rather than crashing the
            // discovery sweep — a foothold attacker should not be able
            // to wedge the protocol by planting a symlink at the
            // sidecar path.
            return Ok(Some(Err(format!("sidecar open refused: {}", e))));
        }
    };

    let mut buf = String::new();
    let mut reader = std::io::BufReader::new(file);
    use std::io::Read;
    if let Err(e) = reader.read_to_string(&mut buf) {
        return Ok(Some(Err(format!("sidecar read failed: {}", e))));
    }
    match toml::from_str::<SidecarContents>(&buf) {
        Ok(c) => Ok(Some(Ok(c))),
        Err(e) => Ok(Some(Err(format!("sidecar parse failed: {}", e)))),
    }
}

/// Unlink the claim sidecar. Used for L1 (terminal evidence) and L3
/// (redelegation) unlinks.
///
/// Returns `Ok(())` whether or not the sidecar was present. The
/// idempotent-recovery AC depends on this: re-running recovery after a
/// successful unlink must not error.
pub fn release_sidecar(session_dir: &Path) -> Result<()> {
    let path = sidecar_path(session_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!(
            "failed to unlink sidecar {}: {}",
            path.display(),
            e
        )),
    }
}

// ============================================================
// Header mutation helpers (best-effort, temp+rename)
// ============================================================

/// Rewrite the state file at `path`, replacing the header line in
/// place. Uses temp+rename so a mid-write crash leaves the prior file
/// intact. The event-log tail is preserved verbatim.
///
/// `mutate` runs against the current header and returns the new shape.
/// `mutate` returning the input unchanged is fine; the rewrite still
/// happens (cheap relative to the safety guarantee).
pub fn rewrite_header_atomically<F>(path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(StateFileHeader) -> StateFileHeader,
{
    let original = fs::read_to_string(path)
        .with_context(|| format!("failed to read state file {}", path.display()))?;
    let mut lines = original.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| anyhow!("state file {} is empty", path.display()))?;
    let header: StateFileHeader = serde_json::from_str(header_line)
        .with_context(|| format!("failed to parse header from {}", path.display()))?;
    let new_header = mutate(header);
    let new_header_line = serde_json::to_string(&new_header).context("serialize header")?;

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("state file path has no parent: {}", path.display()))?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("state"),
        std::process::id()
    ));

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("failed to open temp {}", tmp.display()))?;
        writeln!(file, "{}", new_header_line)
            .with_context(|| format!("failed to write header to {}", tmp.display()))?;
        for tail in lines {
            // Preserve original trailing newline shape — `.lines()`
            // strips terminators but the file is well-formed when every
            // line (including the last) ends in `\n`.
            writeln!(file, "{}", tail)
                .with_context(|| format!("failed to write event line to {}", tmp.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    }

    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Best-effort write of `assignment_claim` to the header. Failure is
/// logged but does NOT undo the sidecar claim (the sidecar IS truth).
pub fn write_header_assignment_claim_best_effort(
    state_file: &Path,
    coord_id: &ValidatedCoordId,
    claimed_at: &str,
) {
    let coord_id_str = coord_id.as_str().to_string();
    let claimed_at = claimed_at.to_string();
    let result = rewrite_header_atomically(state_file, move |mut h| {
        h.assignment_claim = Some(AssignmentClaim {
            coord_id: coord_id_str,
            claimed_at,
        });
        h
    });
    if let Err(e) = result {
        eprintln!(
            "warning: best-effort assignment_claim write failed for {}: {}",
            state_file.display(),
            e
        );
    }
}

// ============================================================
// Drift recovery state machine
// ============================================================

/// Inputs to [`recover_orphaned_sidecar`]. Carrying these through a
/// struct (vs a wide function signature) keeps the call sites tidy as
/// new policy knobs land in later issues.
#[derive(Debug)]
pub struct RecoveryInputs<'a> {
    /// Path to the child session directory containing the sidecar.
    pub session_dir: &'a Path,
    /// Path to the child's state file (header + event log).
    pub state_file: &'a Path,
    /// Coordinator id performing the recovery (recorded in audit events).
    pub coord_id: &'a ValidatedCoordId,
    /// Child session id (for audit event field).
    pub child_session_id: &'a ValidatedSessionId,
    /// Whether the child has reached a terminal state. Determined by
    /// the caller (which has the compiled template + event log
    /// context); passed in as a flag rather than re-derived here so
    /// `claim.rs` stays template-free.
    pub child_is_terminal: bool,
    /// Sidecar staleness threshold — sidecars younger than this are
    /// treated as live and skipped. Default 600 s per Issue 18.
    pub stale_claim_timeout: Duration,
    /// Redelegation cap — when `header.dispatch_epoch >= cap`, the
    /// recovery flips to `Abandon` rather than `Redelegate`.
    pub redelegation_cap: u32,
    /// Wall-clock used for staleness comparison. Threaded through as a
    /// parameter so tests can pin a synthetic clock.
    pub now: SystemTime,
}

/// Recover an orphan sidecar against the cases-3a/3b/3c state machine.
///
/// Reads the sidecar with `O_NOFOLLOW`, branches on
/// `(header.assignment_claim, child_is_terminal, dispatch_epoch)`, and
/// either unlinks the sidecar (3a), emits `ChildRedelegated` + unlinks
/// + clears header + bumps epoch (3b/3c), or reports
///   `RecoveryAction::Abandon` for the caller to drive through the
///   standard terminal-evidence path.
///
/// **Idempotent.** A second invocation against a recovered state
/// observes `RecoveryAction::None` (sidecar absent) or
/// `RecoveryAction::Skip` (sidecar present but fresh) and makes no
/// further changes.
pub fn recover_orphaned_sidecar(inputs: &RecoveryInputs<'_>) -> Result<RecoveryAction> {
    let sidecar = match read_sidecar(inputs.session_dir)? {
        None => return Ok(RecoveryAction::None),
        Some(s) => s,
    };

    // Malformed sidecar → synthesize a maximally-stale claim record so
    // the case 3b/3c branch always fires. Per the design we explicitly
    // do NOT skip-on-malformed.
    let parsed = sidecar.unwrap_or_else(|err| {
        eprintln!(
            "warning: sidecar at {} is malformed ({}); treating as orphaned",
            sidecar_path(inputs.session_dir).display(),
            err
        );
        SidecarContents {
            coord_id: MALFORMED_COORD_MARKER.to_string(),
            // 2x the stale-claim timeout in the past so the freshness
            // check below always classifies as stale.
            claimed_at: format_rfc3339_millis(inputs.now - inputs.stale_claim_timeout * 2),
        }
    });

    let claimed_at_system = parse_rfc3339_to_system(&parsed.claimed_at).unwrap_or_else(|_| {
        // Malformed timestamp also routes to redelegation (treat as
        // maximally stale).
        inputs.now - inputs.stale_claim_timeout * 2
    });
    let age = inputs
        .now
        .duration_since(claimed_at_system)
        .unwrap_or(Duration::ZERO);

    if age < inputs.stale_claim_timeout {
        return Ok(RecoveryAction::Skip);
    }

    let header = read_header(inputs.state_file)
        .with_context(|| format!("read header for recovery {}", inputs.state_file.display()))?;

    if inputs.child_is_terminal {
        // Case 3a: terminal child whose L1 unlink was missed.
        release_sidecar(inputs.session_dir)?;
        return Ok(RecoveryAction::CleanupOnly);
    }

    // Cases 3b/3c: orphan or stuck-after-header-write. Check the
    // redelegation cap first; cap-exceedance reroutes the recovery to
    // `Abandon`.
    if header.dispatch_epoch >= inputs.redelegation_cap {
        return Ok(RecoveryAction::Abandon {
            observed_dispatch_epoch: header.dispatch_epoch,
        });
    }

    let cleared_header_claim = header.assignment_claim.is_some();
    let new_epoch = header.dispatch_epoch + 1;

    // Emit ChildRedelegated audit event on the coordinator's log
    // BEFORE the sidecar unlink. The order matters: a crash between
    // the audit append and the sidecar unlink leaves the audit event
    // visible (the next recovery pass will re-fire the unlink), but a
    // crash between the unlink and the audit append would lose
    // provenance.
    let respawn_generation = new_epoch; // 1-to-1 mapping for V1; see Issue 16 follow-up.
    let fields = audit::child_redelegated_fields(
        inputs.child_session_id,
        inputs.coord_id.as_str(),
        new_epoch,
        respawn_generation,
    );
    let coord_state_file = coord_state_file_for(inputs);
    append_redelegated_audit(&coord_state_file, fields, &inputs.now)?;

    // Update header: clear claim (3c) or leave None (3b), bump epoch.
    rewrite_header_atomically(inputs.state_file, move |mut h| {
        h.assignment_claim = None;
        h.dispatch_epoch = new_epoch;
        h
    })?;

    release_sidecar(inputs.session_dir)?;

    Ok(RecoveryAction::Redelegate {
        new_dispatch_epoch: new_epoch,
        cleared_header_claim,
    })
}

/// Resolve the coordinator's own state-file path so the audit event
/// lands on the right log. The coordinator's state file is derived
/// from the session dir's parent: `<koto_root>/sessions/<coord_id>/<coord_id>.state.jsonl`.
///
/// Best-effort: when the coordinator's state file isn't on disk yet
/// (synthetic tests, fresh coord), we fall back to writing the audit
/// event into the child's directory so the test harness can still
/// observe the call. Production code SHOULD set up the coordinator's
/// state file before invoking recovery.
fn coord_state_file_for(inputs: &RecoveryInputs<'_>) -> PathBuf {
    // <koto_root>/sessions/<child>/<child>.state.jsonl → walk up.
    let sessions_dir = inputs
        .session_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| inputs.session_dir.to_path_buf());
    let coord_dir = sessions_dir.join(inputs.coord_id.as_str());
    let coord_state = coord_dir.join(format!("{}.state.jsonl", inputs.coord_id.as_str()));
    if coord_state.exists() {
        coord_state
    } else {
        // Fallback: write to <session_dir>/<coord_id>.audit.jsonl so
        // tests can still assert the call.
        inputs
            .session_dir
            .join(format!("{}.audit.jsonl", inputs.coord_id.as_str()))
    }
}

fn append_redelegated_audit(
    coord_log: &Path,
    fields: HashMap<String, serde_json::Value>,
    now: &SystemTime,
) -> Result<()> {
    let payload = EventPayload::EvidenceSubmitted {
        state: "request_store.redelegation".to_string(),
        fields,
        submitter_cwd: None,
    };
    if let Some(parent) = coord_log.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to ensure coord log parent {}", parent.display()))?;
    }
    append_event(coord_log, &payload, &format_rfc3339_millis(*now))
        .map(|_| ())
        .with_context(|| format!("append ChildRedelegated to {}", coord_log.display()))
}

// ============================================================
// Happy-path claim + dispatch
// ============================================================

/// Acquire the claim sidecar, write the best-effort header field, emit
/// `ChildDispatched` on the coordinator's log, and invoke the
/// substrate's Task-spawn primitive.
///
/// Returns [`DispatchOutcome::Contended`] without writing anything when
/// another coordinator already holds the sidecar — the caller should
/// move on to the next unassigned child.
pub fn claim_and_dispatch(
    session_dir: &Path,
    state_file: &Path,
    coord_state_file: &Path,
    coord_id: &ValidatedCoordId,
    child_session_id: &ValidatedSessionId,
    spawner: &dyn SubstrateSpawner,
    now: SystemTime,
) -> Result<DispatchOutcome> {
    match acquire_sidecar(session_dir, coord_id, now)? {
        AcquireOutcome::Contended => return Ok(DispatchOutcome::Contended),
        AcquireOutcome::Acquired => {}
    }

    let claimed_at = format_rfc3339_millis(now);
    write_header_assignment_claim_best_effort(state_file, coord_id, &claimed_at);

    let header = read_header(state_file)
        .with_context(|| format!("read header for dispatch {}", state_file.display()))?;

    // ChildDispatched MUST land on the coord's log BEFORE substrate
    // spawn. Issue 15's wake-candidates pass walks the coord log for
    // ChildDispatched-without-RequesterWoken events; an out-of-order
    // emit would silently break wake recovery.
    let fields =
        audit::child_dispatched_fields(child_session_id, coord_id.as_str(), header.dispatch_epoch);
    let payload = EventPayload::EvidenceSubmitted {
        state: "request_store.dispatch".to_string(),
        fields,
        submitter_cwd: None,
    };
    if let Some(parent) = coord_state_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to ensure coord log parent {}", parent.display()))?;
    }
    append_event(coord_state_file, &payload, &claimed_at)
        .with_context(|| format!("append ChildDispatched to {}", coord_state_file.display()))?;

    let request = SpawnRequest {
        child_session_id: child_session_id.clone(),
        role: header.role.clone().unwrap_or_default(),
        template_name: header.template_name.clone().unwrap_or_default(),
        inputs: header.inputs.clone(),
        coord_id: coord_id.clone(),
        dispatch_epoch: header.dispatch_epoch,
        team_name: coord_id.as_str().to_string(),
    };
    spawner
        .spawn(&request)
        .map_err(|e| anyhow!(e.to_string()))?;

    Ok(DispatchOutcome::Dispatched)
}

// ============================================================
// Timestamp helpers
// ============================================================

/// Format a `SystemTime` as RFC 3339 UTC with millisecond precision.
/// Pure function; no I/O.
pub fn format_rfc3339_millis(t: SystemTime) -> String {
    let dur = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let secs = dur.as_secs() as i64;
    let millis = (dur.subsec_millis()) as i64;
    // Decompose seconds into civil time without pulling in chrono.
    // We rely on the standard year/month/day arithmetic that avoids a
    // heavyweight dependency for what is ultimately a 24-byte string.
    let (y, m, d, hh, mm, ss) = civil_from_unix_seconds(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hh, mm, ss, millis
    )
}

/// Parse a sidecar's RFC 3339 timestamp back into a `SystemTime`.
fn parse_rfc3339_to_system(s: &str) -> std::result::Result<SystemTime, String> {
    // Minimal RFC 3339 parser: `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
    if s.len() < 20 {
        return Err(format!("timestamp too short: {}", s));
    }
    let year: i64 = s[0..4].parse().map_err(|_| "bad year".to_string())?;
    let month: u32 = s[5..7].parse().map_err(|_| "bad month".to_string())?;
    let day: u32 = s[8..10].parse().map_err(|_| "bad day".to_string())?;
    let hour: u32 = s[11..13].parse().map_err(|_| "bad hour".to_string())?;
    let minute: u32 = s[14..16].parse().map_err(|_| "bad minute".to_string())?;
    let second: u32 = s[17..19].parse().map_err(|_| "bad second".to_string())?;
    // Optional millis between `.` and `Z`.
    let millis: u32 = if s.len() > 20 && s.as_bytes()[19] == b'.' {
        let end = s.find('Z').unwrap_or(s.len());
        s[20..end].parse().map_err(|_| "bad millis".to_string())?
    } else {
        0
    };
    let secs = unix_seconds_from_civil(year, month, day, hour, minute, second);
    if secs < 0 {
        return Err("timestamp predates UNIX epoch".to_string());
    }
    Ok(UNIX_EPOCH + Duration::from_secs(secs as u64) + Duration::from_millis(millis as u64))
}

/// Convert UNIX seconds-since-epoch to civil (Y, M, D, h, m, s).
///
/// Uses Howard Hinnant's days_from_civil/civil_from_days algorithm.
/// Suitable for tests and human-readable sidecar timestamps; we don't
/// take a chrono dependency just for this.
fn civil_from_unix_seconds(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = y + if m <= 2 { 1 } else { 0 };
    let hh = rem / 3_600;
    let mm = (rem % 3_600) / 60;
    let ss = rem % 60;
    (y, m, d, hh, mm, ss)
}

/// Inverse of `civil_from_unix_seconds`.
fn unix_seconds_from_civil(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let d = d as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe as i64 - 719_468;
    days * 86_400 + (hh as i64) * 3_600 + (mm as i64) * 60 + (ss as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc3339_round_trip_millis() {
        let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let s = format_rfc3339_millis(now);
        let back = parse_rfc3339_to_system(&s).expect("parse");
        let s2 = format_rfc3339_millis(back);
        assert_eq!(s, s2);
    }

    #[test]
    fn rfc3339_format_shape() {
        let t = UNIX_EPOCH + Duration::from_millis(1_700_000_000_500);
        let s = format_rfc3339_millis(t);
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 24);
        assert_eq!(&s[10..11], "T");
        assert_eq!(&s[19..20], ".");
    }

    #[test]
    fn sidecar_filename_constant() {
        assert_eq!(SIDECAR_FILENAME, "claim.lock");
    }

    #[test]
    fn recovery_action_mutated_flag() {
        assert!(!RecoveryAction::None.mutated());
        assert!(!RecoveryAction::Skip.mutated());
        assert!(RecoveryAction::CleanupOnly.mutated());
        assert!(RecoveryAction::Redelegate {
            new_dispatch_epoch: 1,
            cleared_header_claim: false,
        }
        .mutated());
        assert!(!RecoveryAction::Abandon {
            observed_dispatch_epoch: 3,
        }
        .mutated());
    }
}
