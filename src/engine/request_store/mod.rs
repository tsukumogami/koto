//! The request store: one append-only log per request, at a
//! workspace path of its own.
//!
//! ```text
//! ~/.koto/requests/<request_id>/
//!   request.jsonl     header line + every event for this request
//!   request.lock      flock target for the validate-and-append critical section
//! ```
//!
//! # Why a second log family
//!
//! [`crate::engine::persistence`] is genuinely path-parameterized, so
//! `EventPayload` is koto's *event-log* format rather than its
//! *session* format. A request log at a workspace path is therefore
//! the same format, the same crash-safety machinery, and the same enum
//! — and because nothing in koto walks `~/.koto/` outside `sessions/`
//! and `coordinators/`, a request record survives session cleanup by
//! construction rather than by every deletion site remembering to
//! spare it (DESIGN-request-lifecycle.md Decision 1).
//!
//! # The three invariants this module defends
//!
//! A request log is multi-writer and immortal where a session log is
//! neither, and all three defenses follow from that.
//!
//! *Every writer takes the lock.* `append_event` derives its sequence
//! number from an unlocked read of the last one, and the reader
//! hard-errors on a non-consecutive sequence — so two unlocked
//! concurrent appends computing the same next sequence would brick the
//! request permanently. The structural defense is that the log path is
//! private to this module: no public accessor hands a caller a path it
//! could append to outside [`validate_and_append`].
//!
//! *A torn tail is repaired before appending, not after.* An
//! unbuffered `writeln!` can issue the payload and the newline as
//! separate writes, so a crash can leave a line with no terminator.
//! The reader recovers that only while it is the final line; the next
//! append concatenates onto it, making it non-final and permanently
//! fatal. Since the lock is already held, [`repair_torn_tail`] runs
//! inside it.
//!
//! *Reads are quiet.* A lock-free read racing a concurrent append hits
//! the truncated-final-line path legitimately, and `wait` polls every
//! two seconds, so [`read_view`] uses
//! [`crate::engine::persistence::read_log_quiet`] rather than
//! streaming warnings to stderr during normal operation.
//!
//! # Platform and replication limits
//!
//! flock is host-local, so the write path requires a local filesystem:
//! two hosts appending over a network filesystem would collide on
//! sequence assignment and the reader hard-errors on the resulting
//! gap. Request records also do not replicate under the cloud backend.
//! Both are documented limitations rather than silent gaps.

pub mod view;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::RequestStoreConfig;
use crate::engine::atomic_fs::{atomic_create_rename, AtomicCreateError};
use crate::engine::name_grammar::{name_regex, validate_member_name, MemberNameError};
use crate::engine::persistence::{
    self, append_event_idempotent, idempotency_hash, AppendOutcome, LogHeader,
};
use crate::engine::types::{
    CloseDisposition, Event, EventPayload, LegDeclaration, LegDisposition, LegResultSource,
    RequestState, WorkflowResult,
};

pub use view::{LegCounts, LegView, ProgressEntry, RequestView};

// ===== Layout =====

/// Workspace-relative directory holding every request record.
const REQUESTS_DIR: &str = "requests";
/// The log file inside one request's directory.
const LOG_FILE: &str = "request.jsonl";
/// The flock target inside one request's directory. A separate file
/// from the log so acquiring the lock never truncates or extends it.
const LOCK_FILE: &str = "request.lock";

/// Schema version of the request log family.
///
/// Deliberately independent of the session store's
/// `CURRENT_SCHEMA_VERSION`: the two families evolve separately, and
/// coupling them would mean a session-format change made every request
/// log unreadable to the older build.
pub const REQUEST_SCHEMA_VERSION: u32 = 1;

/// How long [`validate_and_append`] waits for the lock before giving up.
///
/// flock is not first-in-first-out, so a writer can be starved past
/// its deadline under sustained contention; that surfaces as
/// [`RequestStoreError::LockContention`], which is a transient-class
/// error the caller may retry.
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Sleep between non-blocking lock attempts. koto has no bounded-wait
/// flock primitive, so the bounded acquisition is
/// non-blocking-plus-retry rather than a blocking `LOCK_EX`.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);

// ===== Bounds (Decision 9) =====

/// Bytes accepted in one progress append's serialized content.
pub const MAX_APPEND_BYTES: usize = 16 * 1024;

/// Bytes accepted in any JSON payload the caller supplies.
///
/// Same ceiling as the CLI's `--inputs` and `--with-data` guards. The
/// number is restated rather than imported because those live in the
/// CLI layer and the engine must not depend on it; if one moves, both
/// move.
pub const MAX_JSON_PAYLOAD_BYTES: usize = 1024 * 1024;

/// Nesting depth accepted in any JSON payload the caller supplies.
/// Pairs with [`MAX_JSON_PAYLOAD_BYTES`] against a parser bomb that
/// stays under the size cap by recursing.
pub const MAX_JSON_DEPTH: usize = 128;

/// Bytes accepted in an abandonment rationale.
///
/// Much tighter than koto's existing 1 MiB rationale precedent because
/// this text is prepended to a delegate's directive on every tick
/// until it terminates, which makes a large one a context-exhaustion
/// problem rather than merely a large string.
pub const MAX_RATIONALE_BYTES: usize = 4 * 1024;

/// The two operator-tunable bounds, resolved once per call site.
///
/// The other three (per-append bytes, JSON payload size and depth,
/// rationale bytes) are fixed: they defend the format and the
/// delegate's context window rather than expressing an operator
/// preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestBounds {
    /// Progress appends accepted on one leg.
    pub progress_appends_per_leg: usize,
    /// Legs accepted on one request, checked at creation.
    pub legs_per_request: usize,
}

impl Default for RequestBounds {
    fn default() -> Self {
        Self {
            progress_appends_per_leg: 256,
            legs_per_request: 256,
        }
    }
}

impl RequestBounds {
    /// Read both bounds off the resolved `[request_store]` table.
    pub fn from_config(config: &RequestStoreConfig) -> Self {
        Self {
            progress_appends_per_leg: config.request_leg_append_cap as usize,
            legs_per_request: config.request_leg_cap as usize,
        }
    }
}

// ===== Errors =====

/// Every way a request-store call can fail.
///
/// The variants are deliberately fine-grained: the CLI maps them onto
/// exit classes and error codes, and a caller that has to string-match
/// a message to tell "this leg already has a result" from "this
/// request is closed" has no contract at all.
#[derive(Debug, Error)]
pub enum RequestStoreError {
    /// No request record at this identifier.
    #[error("request not found: {request_id}")]
    NotFound { request_id: String },

    /// The caller-supplied identifier failed the grammar. Rejected
    /// before any path is joined, so a traversal attempt never reaches
    /// the filesystem.
    #[error("invalid request id ({reason}): {input_preview}")]
    InvalidRequestId {
        reason: String,
        input_preview: String,
    },

    /// A leg name failed the shared member-name grammar.
    #[error("invalid leg name ({reason}): {input_preview}")]
    InvalidLegName {
        reason: String,
        input_preview: String,
    },

    /// Two legs were declared with the same name.
    #[error("duplicate leg name in request creation: {leg_name}")]
    DuplicateLegName { leg_name: String },

    /// The request has no leg by that name.
    #[error("request {request_id} has no leg named {leg_name}")]
    LegNotFound {
        request_id: String,
        leg_name: String,
    },

    /// A second result on a leg that already has one.
    #[error("leg {leg_name} of request {request_id} already has a result")]
    LegAlreadyResolved {
        request_id: String,
        leg_name: String,
    },

    /// An explicit resolve on a leg that has a child working it.
    ///
    /// A bound leg resolves by promotion from its child's terminal tick;
    /// accepting an explicit result here would block the real one.
    #[error(
        "leg {leg_name} of request {request_id} is bound to child {child_session_id} \
         and resolves when that child completes"
    )]
    LegBoundToChild {
        request_id: String,
        leg_name: String,
        child_session_id: String,
    },

    /// A mutation on a leg the requester stopped waiting on.
    #[error("leg {leg_name} of request {request_id} was abandoned")]
    LegAbandoned {
        request_id: String,
        leg_name: String,
    },

    /// A rebind that would point an already-bound leg somewhere else.
    /// Rebinding to the same child is a no-op success, not this.
    #[error(
        "leg {leg_name} of request {request_id} is bound to {bound_child}; \
         cannot rebind to {requested_child}"
    )]
    LegBoundToDifferentChild {
        request_id: String,
        leg_name: String,
        bound_child: String,
        requested_child: String,
    },

    /// Any leg mutation, or a second close, on a closed request.
    #[error("request {request_id} is closed")]
    RequestClosed { request_id: String },

    /// The atomic create found a record already at this identifier.
    #[error("request {request_id} already exists")]
    RequestIdCollision { request_id: String },

    /// One of the five bounds rejected the call.
    #[error("{dimension} bound exceeded: {observed} exceeds the limit of {limit}")]
    BoundExceeded {
        /// Which bound, as a stable slug the CLI can map to a code.
        dimension: &'static str,
        limit: usize,
        observed: usize,
    },

    /// The lock was not acquired before the deadline. Transient: the
    /// caller may retry.
    #[error("timed out after {waited_ms}ms waiting for the lock on request {request_id}")]
    LockContention { request_id: String, waited_ms: u64 },

    /// A retry presented a known idempotency hash with a different
    /// payload, so it is not the same logical write.
    #[error(
        "idempotency hash collision on request {request_id}: payload differs from the recorded one"
    )]
    IdempotencyConflict { request_id: String },

    /// The log disagrees with itself in a way no projection can
    /// resolve. Distinct from a truncated final line, which recovers.
    #[error("request {request_id} log is corrupted: {reason}")]
    Corrupt { request_id: String, reason: String },

    /// Anything the filesystem refused.
    #[error("request store IO failure ({context}): {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// A read or append failed below the typed layer.
    #[error("request store failure: {0}")]
    Other(String),
}

impl RequestStoreError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        RequestStoreError::Io {
            context: context.into(),
            source,
        }
    }
}

// ===== The validated identifier =====

/// A request id that has passed the grammar and may be used as a path
/// component.
///
/// Construct via [`ValidatedRequestId::new`] or
/// [`ValidatedRequestId::generate`] — there is no other way in, so the
/// read and write entry points literally cannot be called with an
/// unvalidated string. Validation becomes a property the type system
/// holds rather than a discipline each call site re-keeps
/// (DESIGN-request-lifecycle.md Security Considerations).
///
/// Two rules on top of the shared member-name grammar:
///
/// - **Lowercase only.** Two ids differing in case would collide to
///   one directory on a case-insensitive filesystem, interleaving two
///   logs into one file and failing the sequence check permanently.
///   Generated ids are lowercase, and the constructor refuses anything
///   else rather than trusting that.
/// - The grammar's own character class admits no `/`, no `.`, and no
///   NUL, so neither `..` nor an absolute path can escape
///   `~/.koto/requests/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValidatedRequestId(String);

impl ValidatedRequestId {
    /// Validate caller-controlled input as a request id.
    pub fn new(input: &str) -> Result<Self, RequestStoreError> {
        let re = name_regex();
        validate_member_name(input, &re).map_err(|e| RequestStoreError::InvalidRequestId {
            reason: describe_name_error(&e),
            input_preview: preview(input),
        })?;
        if input.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(RequestStoreError::InvalidRequestId {
                reason: "request ids are lowercase-only so two cannot collide to one directory \
                         on a case-insensitive filesystem"
                    .to_string(),
                input_preview: preview(input),
            });
        }
        Ok(Self(input.to_string()))
    }

    /// Mint a fresh, opaque, lowercase identifier.
    ///
    /// Opaque on purpose: it embeds no coordinator identity, so
    /// holding one reveals nothing about who asked. That prevents
    /// enumeration by guessing and nothing more — anything at the same
    /// uid can read any request it can name.
    pub fn generate() -> Self {
        // `generate_session_id` is a v4 UUID from `/dev/urandom`,
        // rendered lowercase-hex with hyphens — inside the shared
        // grammar's character class and well under its length band.
        let id = format!("req-{}", crate::engine::types::generate_session_id());
        debug_assert!(
            Self::new(&id).is_ok(),
            "generated request ids must satisfy their own constructor"
        );
        Self(id)
    }

    /// Borrow the inner string without re-validation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ValidatedRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ValidatedRequestId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Validate a leg name against the shared grammar.
///
/// Leg names are map keys inside the log rather than path components,
/// so this guards the wire format and the CLI rather than the
/// filesystem — including the grammar's leading-hyphen rejection,
/// because a leg is a positional shell argument in every mutating verb
/// and a leg named `--rationale` is an argument-parsing hazard.
pub fn validate_leg_name(name: &str) -> Result<(), RequestStoreError> {
    let re = name_regex();
    validate_member_name(name, &re).map_err(|e| RequestStoreError::InvalidLegName {
        reason: describe_name_error(&e),
        input_preview: preview(name),
    })
}

fn describe_name_error(e: &MemberNameError) -> String {
    match e {
        MemberNameError::LengthOutOfRange(n) => format!("length {n} is outside 1..=64 bytes"),
        MemberNameError::RegexMismatch => "contains a character outside [A-Za-z0-9_-]".to_string(),
        MemberNameError::LeadingHyphen => "starts with a hyphen".to_string(),
    }
}

/// Truncate rejected input for an error message so an overlong string
/// cannot spam a log through the rejection path.
fn preview(input: &str) -> String {
    const MAX: usize = 64;
    if input.len() <= MAX {
        return input.escape_debug().to_string();
    }
    let mut cut = MAX;
    while !input.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    format!("{}...", input[..cut].escape_debug())
}

// ===== The header =====

/// The first line of every request log.
///
/// Carries its own `schema_version` rather than borrowing the session
/// store's, so the two families version independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestHeader {
    pub schema_version: u32,
    /// Opaque; embeds no coordinator identity.
    pub request_id: String,
    /// RFC 3339 UTC.
    pub created_at: String,
    /// Audit attribution for who asked. Not an access check.
    pub requested_by: String,
    /// The coordinator answerable for the request, which may outlive
    /// the session that created it.
    pub coordinator_of_record: String,
}

impl LogHeader for RequestHeader {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }
    fn max_supported_schema_version() -> u32 {
        REQUEST_SCHEMA_VERSION
    }
}

// ===== Paths (module-private on purpose) =====

fn requests_root(root: &Path) -> PathBuf {
    root.join(REQUESTS_DIR)
}

fn request_dir(root: &Path, id: &ValidatedRequestId) -> PathBuf {
    requests_root(root).join(id.as_str())
}

/// The one place the log path is computed. Private: handing a caller
/// this path would let it append outside the lock, and two unlocked
/// appends assigning the same sequence number brick the request.
fn log_path(root: &Path, id: &ValidatedRequestId) -> PathBuf {
    request_dir(root, id).join(LOG_FILE)
}

fn lock_path(root: &Path, id: &ValidatedRequestId) -> PathBuf {
    request_dir(root, id).join(LOCK_FILE)
}

/// Create a directory with mode 0700, rather than relying on the home
/// directory's mode having been set correctly once.
fn create_dir_0700(path: &Path) -> Result<(), RequestStoreError> {
    reject_if_symlink(path)?;
    if path.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(path)
        .map_err(|e| RequestStoreError::io(format!("creating {}", path.display()), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| RequestStoreError::io(format!("chmod 0700 {}", path.display()), e))?;
    }
    Ok(())
}

/// Refuse to read or write through a symlink.
///
/// A planted symlink at the log path would let a foothold redirect an
/// append into a file the operator never meant to write, matching how
/// the claim sidecar is opened and how workspace prune refuses a
/// symlinked root. The lock file gets the same treatment via
/// `O_NOFOLLOW`.
fn reject_if_symlink(path: &Path) -> Result<(), RequestStoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => Err(RequestStoreError::Other(format!(
            "refusing to follow the symlink at {}",
            path.display()
        ))),
        _ => Ok(()),
    }
}

/// Best-effort fsync of a directory so a fresh entry survives a crash.
/// Tolerates the filesystems that reject an `O_RDONLY` directory
/// fsync, matching the pattern in `persistence.rs` and `discovery.rs`.
fn fsync_dir(path: &Path) {
    if let Ok(dir) = std::fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

// ===== Payload guards =====

/// Reject a caller-supplied JSON value that is too large or too deeply
/// nested, before it is written anywhere.
fn guard_json_payload(
    dimension: &'static str,
    value: &serde_json::Value,
) -> Result<(), RequestStoreError> {
    let encoded = serde_json::to_string(value)
        .map_err(|e| RequestStoreError::Other(format!("payload is not serializable: {e}")))?;
    if encoded.len() > MAX_JSON_PAYLOAD_BYTES {
        return Err(RequestStoreError::BoundExceeded {
            dimension,
            limit: MAX_JSON_PAYLOAD_BYTES,
            observed: encoded.len(),
        });
    }
    let depth = json_max_depth(value);
    if depth > MAX_JSON_DEPTH {
        return Err(RequestStoreError::BoundExceeded {
            dimension: "json_depth",
            limit: MAX_JSON_DEPTH,
            observed: depth,
        });
    }
    Ok(())
}

/// Maximum nesting depth of a parsed value (an empty leaf is depth 1).
///
/// Iterative so measuring a deeply nested input cannot blow our own
/// stack while we are trying to reject it.
fn json_max_depth(value: &serde_json::Value) -> usize {
    let mut max_depth = 1usize;
    let mut stack: Vec<(&serde_json::Value, usize)> = vec![(value, 1)];
    while let Some((v, d)) = stack.pop() {
        if d > max_depth {
            max_depth = d;
        }
        match v {
            serde_json::Value::Array(items) => {
                for item in items {
                    stack.push((item, d + 1));
                }
            }
            serde_json::Value::Object(map) => {
                for (_, item) in map {
                    stack.push((item, d + 1));
                }
            }
            _ => {}
        }
    }
    max_depth
}

/// Reject an overlong rationale, then strip what a delegate's
/// directive must never carry.
///
/// Control characters go because this text reaches a terminal and a
/// log; newlines collapse to spaces because the notice is one line.
/// Rejecting comes first so the caller learns its text was too long
/// rather than silently getting a shortened one.
fn sanitize_rationale(rationale: &str) -> Result<String, RequestStoreError> {
    if rationale.len() > MAX_RATIONALE_BYTES {
        return Err(RequestStoreError::BoundExceeded {
            dimension: "rationale_bytes",
            limit: MAX_RATIONALE_BYTES,
            observed: rationale.len(),
        });
    }
    Ok(rationale
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

// ===== Creation =====

/// One leg as declared at creation time.
///
/// A `Vec` of these rather than a map is what makes the
/// duplicate-name rejection possible: the shared grammar validates one
/// name at a time, and two legs sharing a name would silently collapse
/// in a map before anyone could notice.
#[derive(Debug, Clone, PartialEq)]
pub struct LegSpec {
    pub name: String,
    pub declaration: LegDeclaration,
}

/// Everything a request needs at creation.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRequest {
    pub requested_by: String,
    pub coordinator_of_record: String,
    pub legs: Vec<LegSpec>,
    /// Shared context for the whole request; per-leg asks live on each
    /// declaration.
    pub inputs: Option<serde_json::Value>,
    /// RFC 3339 UTC creation timestamp, supplied by the caller so the
    /// engine stays free of a clock dependency.
    pub created_at: String,
}

/// Create a request and return its generated identifier.
///
/// **One atomic write, not two.** A header write followed by an event
/// append would be two fsyncs, and a crash between them would leave a
/// request whose header parses and whose log is empty — listable, and
/// projecting to a request with zero legs, because the leg
/// declarations live in the creation event. So the header line and the
/// `request.created` event are buffered together, written to a
/// tempfile in the target directory, fsynced, and renamed into place
/// with no-replace semantics. The rename also refuses a colliding
/// identifier for free.
pub fn create_request(
    root: &Path,
    spec: &NewRequest,
    bounds: &RequestBounds,
) -> Result<ValidatedRequestId, RequestStoreError> {
    create_request_with_id(root, &ValidatedRequestId::generate(), spec, bounds)
}

/// [`create_request`] with the identifier supplied rather than
/// generated. Private: the collision refusal is only meaningful
/// because callers cannot choose an id.
fn create_request_with_id(
    root: &Path,
    request_id: &ValidatedRequestId,
    spec: &NewRequest,
    bounds: &RequestBounds,
) -> Result<ValidatedRequestId, RequestStoreError> {
    if spec.legs.len() > bounds.legs_per_request {
        return Err(RequestStoreError::BoundExceeded {
            dimension: "legs_per_request",
            limit: bounds.legs_per_request,
            observed: spec.legs.len(),
        });
    }

    // Validate every name and payload before touching the filesystem,
    // so a rejected creation leaves no directory behind.
    let mut legs: BTreeMap<String, LegDeclaration> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for leg in &spec.legs {
        validate_leg_name(&leg.name)?;
        if !seen.insert(leg.name.as_str()) {
            return Err(RequestStoreError::DuplicateLegName {
                leg_name: leg.name.clone(),
            });
        }
        guard_json_payload("leg_inputs_bytes", &leg.declaration.inputs)?;
        legs.insert(leg.name.clone(), leg.declaration.clone());
    }
    if let Some(inputs) = &spec.inputs {
        guard_json_payload("request_inputs_bytes", inputs)?;
    }

    let header = RequestHeader {
        schema_version: REQUEST_SCHEMA_VERSION,
        request_id: request_id.as_str().to_string(),
        created_at: spec.created_at.clone(),
        requested_by: spec.requested_by.clone(),
        coordinator_of_record: spec.coordinator_of_record.clone(),
    };
    let created = Event {
        seq: 1,
        timestamp: spec.created_at.clone(),
        event_type: "request.created".to_string(),
        payload: EventPayload::RequestCreated {
            request_id: request_id.as_str().to_string(),
            requested_by: spec.requested_by.clone(),
            coordinator_of_record: spec.coordinator_of_record.clone(),
            legs,
            inputs: spec.inputs.clone(),
        },
        idempotency_hash: None,
    };

    let mut buf = persistence::serialize_header_line(&header)
        .map_err(|e| RequestStoreError::Other(e.to_string()))?;
    buf.push('\n');
    buf.push_str(
        &serde_json::to_string(&created)
            .map_err(|e| RequestStoreError::Other(format!("failed to serialize event: {e}")))?,
    );
    buf.push('\n');

    let dir = request_dir(root, request_id);
    create_dir_0700(&requests_root(root))?;
    create_dir_0700(&dir)?;

    // The tempfile lives in the target directory so the rename is
    // same-filesystem.
    let tmp = tempfile::Builder::new()
        .prefix(".request-init-")
        .suffix(".tmp")
        .tempfile_in(&dir)
        .map_err(|e| RequestStoreError::io("creating the creation tempfile", e))?;
    {
        let mut file = tmp.as_file();
        file.write_all(buf.as_bytes())
            .map_err(|e| RequestStoreError::io("writing the creation tempfile", e))?;
        file.sync_data()
            .map_err(|e| RequestStoreError::io("fsyncing the creation tempfile", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
                .map_err(|e| RequestStoreError::io("chmod 0600 on the creation tempfile", e))?;
        }
    }

    // Detach so the tempfile is not deleted on drop; we drive the
    // rename ourselves and unlink explicitly on failure.
    let (_file, tmp_path) = tmp
        .keep()
        .map_err(|e| RequestStoreError::io("detaching the creation tempfile", e.error))?;

    let target = log_path(root, request_id);
    match atomic_create_rename(&tmp_path, &target) {
        Ok(()) => {
            fsync_dir(&dir);
            Ok(request_id.clone())
        }
        Err(e) => {
            // Every failure path leaves the tempfile at `tmp_path`
            // (renameat2/link/rename leave the source untouched on
            // failure), so unconditional removal is safe.
            let _ = std::fs::remove_file(&tmp_path);
            Err(match e {
                AtomicCreateError::Collision => RequestStoreError::RequestIdCollision {
                    request_id: request_id.as_str().to_string(),
                },
                AtomicCreateError::Io(e) => {
                    RequestStoreError::io("renaming the creation tempfile into place", e)
                }
            })
        }
    }
}

// ===== Reading =====

/// Replay a request log into its view.
///
/// Takes no lock and writes nothing. A concurrent append can leave a
/// torn final line, which the quiet reader recovers silently — see the
/// module comment on why the request path must not warn.
pub fn read_view(
    root: &Path,
    request_id: &ValidatedRequestId,
) -> Result<RequestView, RequestStoreError> {
    let path = log_path(root, request_id);
    read_view_at(&path, request_id)
}

fn read_view_at(
    path: &Path,
    request_id: &ValidatedRequestId,
) -> Result<RequestView, RequestStoreError> {
    reject_if_symlink(path)?;
    if !path.exists() {
        return Err(RequestStoreError::NotFound {
            request_id: request_id.as_str().to_string(),
        });
    }
    let (header, events) = persistence::read_log_quiet::<RequestHeader>(path)
        .map_err(|e| map_read_error(request_id, e))?;
    // The directory name is the authority on which request this is, so
    // a header naming a different one means the record was moved or
    // two were interleaved — either way its events cannot be trusted
    // to belong here.
    if header.request_id != request_id.as_str() {
        return Err(RequestStoreError::Corrupt {
            request_id: request_id.as_str().to_string(),
            reason: format!("header names request '{}'", header.request_id),
        });
    }
    view::project(header, &events)
}

/// Route a read failure into the typed vocabulary, keeping "the record
/// is not there" distinguishable from "the record is broken".
fn map_read_error(request_id: &ValidatedRequestId, e: anyhow::Error) -> RequestStoreError {
    if let Some(io) = e.downcast_ref::<std::io::Error>() {
        if io.kind() == std::io::ErrorKind::NotFound {
            return RequestStoreError::NotFound {
                request_id: request_id.as_str().to_string(),
            };
        }
    }
    RequestStoreError::Corrupt {
        request_id: request_id.as_str().to_string(),
        reason: e.to_string(),
    }
}

// ===== Listing =====

/// One row of a listing.
///
/// Field names mirror `UnassignedChild` for the concepts the two share
/// — `requested_by`, `coordinator_of_record`, `created_at` — so an
/// operator reading both surfaces learns one vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestSummary {
    pub request_id: String,
    pub requested_by: String,
    pub coordinator_of_record: String,
    pub created_at: String,
    pub request_state: RequestState,
    pub leg_counts: LegCounts,
}

/// What a listing keeps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListFilter {
    pub requested_by: Option<String>,
    pub coordinator_of_record: Option<String>,
    pub state: Option<RequestState>,
    /// Keep only requests with at least one leg still open.
    pub unresolved_legs_only: bool,
}

/// Walk `~/.koto/requests/` and summarize what matches.
///
/// Cursor-free and write-free: this is a read of the request store and
/// nothing else, so it neither advances a coordinator cursor nor
/// touches the dispatch protocol's state.
///
/// The identity filters are answered from the header line alone, so a
/// request excluded by requester or coordinator never costs more than
/// one line. The rows that survive that filter do need a replay:
/// `request_state` and the leg counts are derived from events, so
/// "parse only the header" and "filter by open-or-closed" cannot both
/// hold. The header-only read is kept where it can pay off rather than
/// dropped.
///
/// A request directory that fails to parse is skipped with a warning
/// rather than failing the whole call: one corrupt record must not
/// make the workspace unlistable.
pub fn list_requests(
    root: &Path,
    filter: &ListFilter,
) -> Result<Vec<RequestSummary>, RequestStoreError> {
    let dir = requests_root(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // No requests directory means no requests, not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(RequestStoreError::io(
                format!("reading {}", dir.display()),
                e,
            ))
        }
    };

    let mut summaries = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!(
                    "warning: skipping unreadable entry under {}: {e}",
                    dir.display()
                );
                continue;
            }
        };
        // `symlink_metadata` rather than `metadata`: a symlinked
        // request directory could point outside the workspace, and the
        // store refuses to follow one.
        match std::fs::symlink_metadata(entry.path()) {
            Ok(md) if md.is_dir() => {}
            Ok(_) => continue,
            Err(e) => {
                eprintln!(
                    "warning: skipping {} (stat failed: {e})",
                    entry.path().display()
                );
                continue;
            }
        }

        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            eprintln!(
                "warning: skipping {} (name is not UTF-8)",
                entry.path().display()
            );
            continue;
        };
        let request_id = match ValidatedRequestId::new(name) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("warning: skipping request directory {name}: {e}");
                continue;
            }
        };

        match summarize(root, &request_id, filter) {
            Ok(Some(summary)) => summaries.push(summary),
            Ok(None) => {}
            Err(e) => {
                eprintln!("warning: skipping request {request_id}: {e}");
            }
        }
    }

    // Sorted so two listings of an unchanged workspace are byte-equal;
    // `read_dir` order is filesystem-dependent.
    summaries.sort_by(|a, b| a.request_id.cmp(&b.request_id));
    Ok(summaries)
}

/// Summarize one request, or `Ok(None)` when a filter excludes it.
fn summarize(
    root: &Path,
    request_id: &ValidatedRequestId,
    filter: &ListFilter,
) -> Result<Option<RequestSummary>, RequestStoreError> {
    let path = log_path(root, request_id);
    let header = persistence::read_header_only::<RequestHeader>(&path)
        .map_err(|e| map_read_error(request_id, e))?;

    // The identity filters come off the header, so an excluded request
    // costs one line rather than a replay.
    if let Some(requested_by) = &filter.requested_by {
        if &header.requested_by != requested_by {
            return Ok(None);
        }
    }
    if let Some(coordinator) = &filter.coordinator_of_record {
        if &header.coordinator_of_record != coordinator {
            return Ok(None);
        }
    }

    let view = read_view_at(&path, request_id)?;
    if let Some(state) = filter.state {
        if view.request_state != state {
            return Ok(None);
        }
    }
    let leg_counts = view.leg_counts();
    if filter.unresolved_legs_only && leg_counts.open == 0 {
        return Ok(None);
    }

    Ok(Some(RequestSummary {
        request_id: view.header.request_id.clone(),
        requested_by: view.header.requested_by.clone(),
        coordinator_of_record: view.header.coordinator_of_record.clone(),
        created_at: view.header.created_at.clone(),
        request_state: view.request_state,
        leg_counts,
    }))
}

// ===== The lock =====

/// Guard whose drop releases the flock.
///
/// An flock rather than an exclusive-create lease file: the kernel
/// releases it when the holding process dies, so a killed writer
/// cannot strand it. The lease-file pattern elsewhere in koto needs
/// stale recovery and an age heuristic precisely because those files
/// do strand.
struct RequestLock {
    /// Held only for its `Drop`: closing the descriptor releases the
    /// flock. Never read.
    _file: std::fs::File,
}

/// Acquire the exclusive lock with a bounded wait.
///
/// Non-blocking plus deadline retry rather than a blocking `LOCK_EX`,
/// because koto has no bounded-wait flock primitive and an unbounded
/// wait would turn contention into a hang. flock is not
/// first-in-first-out, so sustained contention can starve a writer
/// past its deadline; that is reported as
/// [`RequestStoreError::LockContention`] for the caller to retry.
#[cfg(unix)]
fn acquire_request_lock(
    root: &Path,
    request_id: &ValidatedRequestId,
    timeout: Duration,
) -> Result<RequestLock, RequestStoreError> {
    use std::os::fd::AsRawFd;

    let path = lock_path(root, request_id);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).read(true).write(true).truncate(false);
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
        // Refuse a symlink at the lock path: a planted symlink would
        // otherwise let a foothold move the lock to an inode two
        // writers do not share.
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts
        .open(&path)
        .map_err(|e| RequestStoreError::io(format!("opening the lock {}", path.display()), e))?;

    let started = Instant::now();
    loop {
        // SAFETY: `fd` borrows `file`, which outlives the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(RequestLock { _file: file });
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(RequestStoreError::io(
                format!("locking {}", path.display()),
                err,
            ));
        }
        if started.elapsed() >= timeout {
            return Err(RequestStoreError::LockContention {
                request_id: request_id.as_str().to_string(),
                waited_ms: started.elapsed().as_millis() as u64,
            });
        }
        std::thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

#[cfg(not(unix))]
fn acquire_request_lock(
    _root: &Path,
    request_id: &ValidatedRequestId,
    _timeout: Duration,
) -> Result<RequestLock, RequestStoreError> {
    // The write path is unix-only, matching koto's platform support:
    // without flock there is no way to make check-then-write atomic
    // across processes, and appending anyway would risk two writers
    // assigning the same sequence number.
    Err(RequestStoreError::Other(format!(
        "the request store write path requires flock, which this platform does not provide \
         (request {request_id})"
    )))
}

// ===== Torn-tail repair =====

/// Truncate a partial final line so a later append cannot concatenate
/// onto it.
///
/// An unbuffered `writeln!` can issue the payload and the newline as
/// separate writes, so a crash mid-append leaves a line with no
/// terminator. The reader recovers that only while it is the *final*
/// line — once an append lands after it, it becomes a non-final
/// malformed line and the log is permanently unreadable. Called with
/// the lock held, which is what makes the read-check-truncate sequence
/// safe.
///
/// Returns whether anything was truncated.
fn repair_torn_tail(path: &Path) -> Result<bool, RequestStoreError> {
    let content = std::fs::read(path)
        .map_err(|e| RequestStoreError::io(format!("reading {}", path.display()), e))?;
    if content.is_empty() {
        return Ok(false);
    }

    let segments: Vec<&[u8]> = content.split_inclusive(|b| *b == b'\n').collect();
    // A one-segment file is the header alone. There is nothing to
    // repair there: truncating it would destroy the record, and no
    // append has happened yet for a torn line to poison.
    if segments.len() < 2 {
        return Ok(false);
    }

    let last = segments[segments.len() - 1];
    let terminated = last.ends_with(b"\n");
    let parses = std::str::from_utf8(last)
        .ok()
        .map(str::trim)
        .map(|s| s.is_empty() || serde_json::from_str::<Event>(s).is_ok())
        .unwrap_or(false);
    if terminated && parses {
        return Ok(false);
    }

    let keep = content.len() - last.len();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| RequestStoreError::io(format!("opening {} to repair", path.display()), e))?;
    file.set_len(keep as u64)
        .map_err(|e| RequestStoreError::io(format!("truncating {}", path.display()), e))?;
    file.sync_data()
        .map_err(|e| RequestStoreError::io(format!("fsyncing {}", path.display()), e))?;
    Ok(true)
}

// ===== The write path =====

/// What an append produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendResult {
    /// The request's revision after the call — the sequence number of
    /// the last event on the log.
    pub revision: u64,
    /// Whether this call wrote an event. `false` for a no-op success:
    /// a rebind to the child a leg is already bound to, a re-abandon,
    /// or a retry the idempotency hash recognized.
    pub written: bool,
}

/// An append the precondition approved, ready to be written.
struct PendingAppend {
    payload: EventPayload,
    timestamp: String,
    /// Hash stored on the event so a later retry can recognize it;
    /// `None` leaves the event un-hashed.
    hash: Option<String>,
}

/// A retry probe, checked before the precondition runs.
///
/// Order matters here. If the precondition ran first, a retried
/// resolve would be rejected as a second result on an already-resolved
/// leg — turning the documented advice to retry on the transient class
/// into a correctness hazard. So a recognized retry short-circuits
/// before any state check gets to object to it
/// (DESIGN-request-lifecycle.md Security Considerations, "retry
/// safety").
struct IdempotencyProbe {
    hash: String,
    payload: EventPayload,
}

/// The one write path.
///
/// Acquires the exclusive lock, repairs a torn tail, re-reads the
/// view, runs `precondition`, appends only on success, and releases.
/// The lock covering check and write together is what makes
/// at-most-one-result, idempotent rebinding, and exactly-one-winner
/// enforceable at all — four requirements from one mechanism
/// (DESIGN-request-lifecycle.md Decision 1).
///
/// Returns the request's new revision.
pub fn validate_and_append<F>(
    root: &Path,
    request_id: &ValidatedRequestId,
    payload: EventPayload,
    timestamp: &str,
    precondition: F,
) -> Result<u64, RequestStoreError>
where
    F: FnOnce(&RequestView) -> Result<(), RequestStoreError>,
{
    let timestamp = timestamp.to_string();
    let result = append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, None, move |view| {
        precondition(view)?;
        Ok(Some(PendingAppend {
            payload,
            timestamp,
            hash: None,
        }))
    })?;
    Ok(result.revision)
}

/// The lock-held body every mutating entry point funnels through.
///
/// `decide` is handed the freshly re-read view and returns the event
/// to append, or `None` for a no-op success. Deriving the payload
/// inside the lock rather than accepting it from outside is what lets
/// `close` compute its disposition from state that cannot change under
/// it.
fn append_under_lock<F>(
    root: &Path,
    request_id: &ValidatedRequestId,
    lock_timeout: Duration,
    probe: Option<IdempotencyProbe>,
    decide: F,
) -> Result<AppendResult, RequestStoreError>
where
    F: FnOnce(&RequestView) -> Result<Option<PendingAppend>, RequestStoreError>,
{
    let path = log_path(root, request_id);
    reject_if_symlink(&path)?;
    if !path.exists() {
        return Err(RequestStoreError::NotFound {
            request_id: request_id.as_str().to_string(),
        });
    }

    let _guard = acquire_request_lock(root, request_id, lock_timeout)?;

    // Repair before reading: a torn tail the reader would silently
    // recover becomes permanently fatal the moment this append
    // concatenates onto it.
    repair_torn_tail(&path)?;

    let view = read_view_at(&path, request_id)?;

    // Retry recognition comes before the precondition: see
    // [`IdempotencyProbe`].
    if let Some(probe) = &probe {
        if find_by_hash(&path, &probe.hash, &probe.payload, request_id)?.is_some() {
            return Ok(AppendResult {
                revision: view.revision,
                written: false,
            });
        }
    }

    let Some(pending) = decide(&view)? else {
        return Ok(AppendResult {
            revision: view.revision,
            written: false,
        });
    };

    let outcome = append_event_idempotent(
        &path,
        &pending.payload,
        &pending.timestamp,
        pending.payload.type_name(),
        pending.hash.as_deref(),
    )
    .map_err(|e| RequestStoreError::Other(format!("append failed: {e}")))?;

    let revision = match outcome {
        AppendOutcome::Written { seq } => seq,
        AppendOutcome::Idempotent { seq } => seq,
    };
    Ok(AppendResult {
        revision,
        written: matches!(outcome, AppendOutcome::Written { .. }),
    })
}

/// Look for a prior event carrying `hash`.
///
/// Returns its sequence number when the payload matches — the retry
/// case — and rejects when the payload differs, because the same hash
/// over different content means the hash domain is not what the caller
/// thinks it is.
fn find_by_hash(
    path: &Path,
    hash: &str,
    payload: &EventPayload,
    request_id: &ValidatedRequestId,
) -> Result<Option<u64>, RequestStoreError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| RequestStoreError::io(format!("reading {}", path.display()), e))?;
    for line in content.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(trimmed) else {
            continue;
        };
        if event.idempotency_hash.as_deref() != Some(hash) {
            continue;
        }
        let recorded = serde_json::to_value(&event.payload)
            .map_err(|e| RequestStoreError::Other(e.to_string()))?;
        let incoming =
            serde_json::to_value(payload).map_err(|e| RequestStoreError::Other(e.to_string()))?;
        if recorded == incoming {
            return Ok(Some(event.seq));
        }
        return Err(RequestStoreError::IdempotencyConflict {
            request_id: request_id.as_str().to_string(),
        });
    }
    Ok(None)
}

/// Reject any leg mutation on a closed request.
fn require_open(view: &RequestView) -> Result<(), RequestStoreError> {
    if view.request_state == RequestState::Closed {
        return Err(RequestStoreError::RequestClosed {
            request_id: view.header.request_id.clone(),
        });
    }
    Ok(())
}

// ===== Typed operations =====

/// Bind a leg to the child session that will fulfil it.
#[derive(Debug, Clone, PartialEq)]
pub struct BindLeg {
    pub leg_name: String,
    pub child_session_id: String,
    /// The child's dispatch epoch at bind time. Recorded on the event
    /// so the leg's mutating paths can fence against it after the
    /// child's session directory is gone.
    pub dispatch_epoch: Option<u32>,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Bind, or confirm an existing identical binding.
///
/// Rebinding a leg to the child it is already bound to is a no-op
/// success — the caller asked for a state that already holds — while
/// rebinding to a different child is rejected, because two children
/// answering one leg is exactly the ambiguity the leg exists to
/// prevent.
pub fn bind_leg(
    root: &Path,
    request_id: &ValidatedRequestId,
    bind: &BindLeg,
) -> Result<AppendResult, RequestStoreError> {
    validate_leg_name(&bind.leg_name)?;
    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, None, |view| {
        require_open(view)?;
        let leg = view.leg(&bind.leg_name)?;
        reject_closed_leg(view, leg)?;
        if let Some(bound) = &leg.bound_child {
            if bound == &bind.child_session_id {
                return Ok(None);
            }
            return Err(RequestStoreError::LegBoundToDifferentChild {
                request_id: view.header.request_id.clone(),
                leg_name: bind.leg_name.clone(),
                bound_child: bound.clone(),
                requested_child: bind.child_session_id.clone(),
            });
        }
        Ok(Some(PendingAppend {
            payload: EventPayload::RequestLegBound {
                request_id: view.header.request_id.clone(),
                leg_name: bind.leg_name.clone(),
                child_session_id: bind.child_session_id.clone(),
                dispatch_epoch: bind.dispatch_epoch,
                issued_by: bind.issued_by.clone(),
            },
            timestamp: bind.timestamp.clone(),
            hash: None,
        }))
    })
}

/// A mid-flight progress append.
#[derive(Debug, Clone, PartialEq)]
pub struct LegProgress {
    pub leg_name: String,
    pub content: BTreeMap<String, serde_json::Value>,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Append progress to an open leg.
///
/// Carries an idempotency hash derived from the payload, so a retry
/// after an ambiguous failure collapses rather than double-appending.
/// The consequence is that two *deliberately* identical appends on one
/// leg also collapse — the same trade koto already makes for evidence
/// submission.
pub fn append_progress(
    root: &Path,
    request_id: &ValidatedRequestId,
    progress: &LegProgress,
    bounds: &RequestBounds,
) -> Result<AppendResult, RequestStoreError> {
    validate_leg_name(&progress.leg_name)?;
    let payload = EventPayload::RequestLegProgress {
        request_id: request_id.as_str().to_string(),
        leg_name: progress.leg_name.clone(),
        content: progress.content.clone(),
        issued_by: progress.issued_by.clone(),
    };
    let hash = idempotency_hash(&progress.leg_name, &payload);
    let probe = IdempotencyProbe {
        hash: hash.clone(),
        payload: payload.clone(),
    };

    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, Some(probe), |view| {
        require_open(view)?;
        let leg = view.leg(&progress.leg_name)?;
        reject_closed_leg(view, leg)?;

        // Both bounds are checked here, inside the lock, so neither
        // can be raced past by two writers reading the same count.
        if leg.progress.len() >= bounds.progress_appends_per_leg {
            return Err(RequestStoreError::BoundExceeded {
                dimension: "progress_appends_per_leg",
                limit: bounds.progress_appends_per_leg,
                observed: leg.progress.len() + 1,
            });
        }
        let content = serde_json::to_value(&progress.content)
            .map_err(|e| RequestStoreError::Other(e.to_string()))?;
        let encoded =
            serde_json::to_string(&content).map_err(|e| RequestStoreError::Other(e.to_string()))?;
        if encoded.len() > MAX_APPEND_BYTES {
            return Err(RequestStoreError::BoundExceeded {
                dimension: "append_bytes",
                limit: MAX_APPEND_BYTES,
                observed: encoded.len(),
            });
        }
        guard_json_payload("progress_content_bytes", &content)?;

        Ok(Some(PendingAppend {
            payload,
            timestamp: progress.timestamp.clone(),
            hash: Some(hash),
        }))
    })
}

/// A leg's terminal answer.
#[derive(Debug, Clone, PartialEq)]
pub struct LegResult {
    pub leg_name: String,
    pub result: WorkflowResult,
    /// Whether the result was promoted from a bound child's terminal
    /// tick or recorded explicitly.
    pub source: LegResultSource,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Record a leg's result.
///
/// At most one per leg: a second is rejected rather than overwriting,
/// because a leg that answered twice has no defensible answer. A
/// result on an abandoned leg is rejected separately, so the caller
/// can tell "someone beat you to it" from "nobody is waiting any
/// more".
pub fn record_result(
    root: &Path,
    request_id: &ValidatedRequestId,
    result: &LegResult,
) -> Result<AppendResult, RequestStoreError> {
    validate_leg_name(&result.leg_name)?;
    if let Some(payload) = &result.result.payload {
        guard_json_payload("result_payload_bytes", payload)?;
    }
    let payload = EventPayload::RequestLegResult {
        request_id: request_id.as_str().to_string(),
        leg_name: result.leg_name.clone(),
        result: result.result.clone(),
        source: result.source,
        issued_by: result.issued_by.clone(),
    };
    let hash = idempotency_hash(&result.leg_name, &payload);
    let probe = IdempotencyProbe {
        hash: hash.clone(),
        payload: payload.clone(),
    };

    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, Some(probe), |view| {
        require_open(view)?;
        let leg = view.leg(&result.leg_name)?;
        match leg.disposition {
            LegDisposition::Resolved => {
                return Err(RequestStoreError::LegAlreadyResolved {
                    request_id: view.header.request_id.clone(),
                    leg_name: result.leg_name.clone(),
                })
            }
            LegDisposition::Abandoned => {
                return Err(RequestStoreError::LegAbandoned {
                    request_id: view.header.request_id.clone(),
                    leg_name: result.leg_name.clone(),
                })
            }
            LegDisposition::Open => {}
        }
        // An explicit resolve is for legs nobody else is working on. A
        // bound leg resolves by promotion from its child's terminal
        // tick, so accepting one here would let a coordinator record an
        // answer on a delegate's behalf and permanently block the real
        // one — the promotion would then hit a leg that already has a
        // result and be dropped.
        //
        // This check lives inside the lock rather than as a pre-read in
        // the caller because the discriminator is a leg's binding, and a
        // bind landing between an unlocked read and this append would
        // slip through.
        if result.source == LegResultSource::Explicit && leg.bound_child.is_some() {
            return Err(RequestStoreError::LegBoundToChild {
                request_id: view.header.request_id.clone(),
                leg_name: result.leg_name.clone(),
                child_session_id: leg
                    .bound_child
                    .clone()
                    .expect("bound_child is Some in this arm"),
            });
        }
        Ok(Some(PendingAppend {
            payload,
            timestamp: result.timestamp.clone(),
            hash: Some(hash),
        }))
    })
}

/// Stop waiting on one leg.
#[derive(Debug, Clone, PartialEq)]
pub struct AbandonLeg {
    pub leg_name: String,
    pub rationale: String,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Abandon one leg, leaving the request's other legs open.
///
/// Abandoning an already-abandoned leg is a no-op success: abandonment
/// is idempotent by nature, and the request-scoped abandon walks every
/// open leg without wanting to care which ones a concurrent caller got
/// to first.
pub fn abandon_leg(
    root: &Path,
    request_id: &ValidatedRequestId,
    abandon: &AbandonLeg,
) -> Result<AppendResult, RequestStoreError> {
    validate_leg_name(&abandon.leg_name)?;
    let rationale = sanitize_rationale(&abandon.rationale)?;
    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, None, |view| {
        require_open(view)?;
        let leg = view.leg(&abandon.leg_name)?;
        match leg.disposition {
            LegDisposition::Abandoned => return Ok(None),
            LegDisposition::Resolved => {
                return Err(RequestStoreError::LegAlreadyResolved {
                    request_id: view.header.request_id.clone(),
                    leg_name: abandon.leg_name.clone(),
                })
            }
            LegDisposition::Open => {}
        }
        Ok(Some(PendingAppend {
            payload: EventPayload::RequestLegAbandoned {
                request_id: view.header.request_id.clone(),
                leg_name: abandon.leg_name.clone(),
                rationale: rationale.clone(),
                issued_by: abandon.issued_by.clone(),
            },
            timestamp: abandon.timestamp.clone(),
            hash: None,
        }))
    })
}

/// Close a request.
#[derive(Debug, Clone, PartialEq)]
pub struct CloseRequest {
    /// Supply a disposition to force one — the request-scoped abandon
    /// does. Leave it `None` and the disposition is derived from the
    /// legs' state inside the lock, so it cannot describe a request
    /// that changed while the caller was deciding.
    pub disposition: Option<CloseDisposition>,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Close a request, rejecting a second close.
///
/// Closing twice is rejected rather than ignored: the second caller
/// believes it is recording a disposition, and silently keeping the
/// first one would tell it something false.
pub fn close_request(
    root: &Path,
    request_id: &ValidatedRequestId,
    close: &CloseRequest,
) -> Result<AppendResult, RequestStoreError> {
    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, None, |view| {
        if view.request_state == RequestState::Closed {
            return Err(RequestStoreError::RequestClosed {
                request_id: view.header.request_id.clone(),
            });
        }
        let disposition = close
            .disposition
            .unwrap_or_else(|| derive_disposition(view));
        Ok(Some(PendingAppend {
            payload: EventPayload::RequestClosed {
                request_id: view.header.request_id.clone(),
                disposition,
                issued_by: close.issued_by.clone(),
            },
            timestamp: close.timestamp.clone(),
            hash: None,
        }))
    })
}

/// Derive a close disposition from the legs.
///
/// `RequestAbandoned` is never derived — it means the requester gave
/// up on the whole request, which only the caller can assert.
fn derive_disposition(view: &RequestView) -> CloseDisposition {
    let counts = view.leg_counts();
    if counts.abandoned > 0 || counts.open > 0 {
        CloseDisposition::ClosedWithAbandonedLegs
    } else {
        CloseDisposition::AllResolved
    }
}

/// Reject a mutation on a leg that has already reached a terminal
/// disposition.
fn reject_closed_leg(view: &RequestView, leg: &LegView) -> Result<(), RequestStoreError> {
    match leg.disposition {
        LegDisposition::Open => Ok(()),
        LegDisposition::Resolved => Err(RequestStoreError::LegAlreadyResolved {
            request_id: view.header.request_id.clone(),
            leg_name: leg.name.clone(),
        }),
        LegDisposition::Abandoned => Err(RequestStoreError::LegAbandoned {
            request_id: view.header.request_id.clone(),
            leg_name: leg.name.clone(),
        }),
    }
}

#[cfg(test)]
mod tests;
