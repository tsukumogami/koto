use thiserror::Error;

/// Typed engine errors for use at the CLI boundary.
///
/// Persistence functions return `anyhow::Result` for internal flexibility.
/// CLI commands should convert `anyhow::Error` to an `EngineError`
/// variant when they need to present a specific user-facing message.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("state not found: {0}")]
    StateNotFound(String),

    #[error("empty event log")]
    EmptyLog,

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("state file corrupted: {0}")]
    StateFileCorrupted(String),

    #[error("incompatible schema version: found {found}, max supported {max_supported}")]
    IncompatibleSchemaVersion { found: u32, max_supported: u32 },

    /// Per-write epoch fence rejected a dispatch attempt because the
    /// presented dispatch epoch did not match the value recorded on
    /// the child's [`crate::engine::types::StateFileHeader`].
    ///
    /// Maps to BSD sysexit code `EX_DATAERR` (65) at the CLI boundary —
    /// the input data (the presented epoch) is invalid; the caller
    /// should not retry without re-reading the header. Drives PRD R43.
    #[error("epoch fence violation for {child_session_id}: expected dispatch_epoch={expected}, presented={presented}")]
    EpochFenceViolation {
        /// Child session id whose header carries the authoritative
        /// `dispatch_epoch`.
        child_session_id: String,
        /// The `dispatch_epoch` value currently on the child's header
        /// (the authoritative epoch the writer was expected to fence
        /// against).
        expected: u32,
        /// The `dispatch_epoch` value the writer presented.
        presented: u32,
    },

    /// Redelegation cap reached for a child — the substrate refuses
    /// to spawn another generation because the configured limit on
    /// re-dispatches has been exhausted for this child.
    ///
    /// Maps to BSD sysexit code `EX_TEMPFAIL` (75) at the CLI
    /// boundary — the failure is operator-tunable (raise the cap or
    /// reset the child) rather than a permanent data error. Drives
    /// PRD R29.
    #[error("redelegation cap exceeded for {child_session_id}: cap={cap}")]
    RedelegationCapExceeded {
        /// Child session id whose respawn generation has reached the
        /// configured cap.
        child_session_id: String,
        /// The cap value that was exceeded.
        cap: u32,
    },

    /// Caller-supplied session id failed the validation regex used
    /// by [`crate::engine::types::ValidatedSessionId`].
    ///
    /// The `input_preview` is truncated to 64 characters to keep log
    /// output bounded; overlong inputs should not be able to spam
    /// audit logs through this error.
    #[error("invalid session id ({reason}): {input_preview}")]
    InvalidSessionId {
        /// Human-readable rejection reason (e.g. `"empty input"`,
        /// `"leading dot"`, `"contains shell metacharacters"`).
        reason: String,
        /// First up-to-64 characters of the rejected input, with the
        /// rest replaced by `...` when truncated.
        input_preview: String,
    },

    /// Caller-supplied coordinator id failed the validation regex
    /// used by [`crate::engine::types::ValidatedCoordId`].
    ///
    /// The `input_preview` is truncated to 64 characters to keep log
    /// output bounded; overlong inputs should not be able to spam
    /// audit logs through this error.
    #[error("invalid coord id ({reason}): {input_preview}")]
    InvalidCoordId {
        /// Human-readable rejection reason.
        reason: String,
        /// First up-to-64 characters of the rejected input, with the
        /// rest replaced by `...` when truncated.
        input_preview: String,
    },

    /// `koto next --with-data` carried a `fields.kind` value that
    /// collides with the KT1 audit family — either one of the four
    /// reserved literal names ([`crate::engine::audit::RESERVED_KINDS`])
    /// or anything starting with the [`crate::engine::audit::KT1_PREFIX`]
    /// prefix. Rejected at parse time before any disk write so
    /// template authors cannot shadow a reserved kind and corrupt
    /// bunki BK2's dispatch_invalidation read path. See Decision 6
    /// in DESIGN-koto-request-store.
    #[error("reserved audit-event kind: {offending_kind}")]
    ReservedKindCollision {
        /// The kind value the operator submitted; preserved verbatim
        /// in the Display message for diagnostics.
        offending_kind: String,
    },

    /// Recursion cap exceeded on a `koto session start --needs-agent`
    /// invocation. The caller's spawn request would push one of the
    /// three dimensions (depth, fanout, total-unassigned) past its
    /// hard-reject threshold. The caps are hard-coded constants at V1
    /// (Decision 4) so this rejection is structural — the operator
    /// has no override surface; the calling agent must restructure
    /// its dispatch fanout.
    ///
    /// Maps to BSD sysexit code `EX_USAGE` (64) at the CLI boundary —
    /// the caller's request is invalid; no retry will help until the
    /// dispatch shape changes. Drives PRD R29.
    #[error(
        "recursion cap exceeded ({dimension}): observed {observed}, hard reject at {threshold}"
    )]
    RecursionCapExceeded {
        /// Which dimension fired: `"depth"`, `"fanout"`, or
        /// `"total_unassigned"`. The string is part of the typed
        /// error so operators reading logs can attribute the
        /// rejection without parsing the message body.
        dimension: String,
        /// The hard-reject threshold for this dimension.
        threshold: u32,
        /// The observed count that triggered the rejection. Strictly
        /// greater than or equal to `threshold` for the variant to
        /// be valid.
        observed: u32,
    },

    /// A retry presented the same `idempotency_hash` as a prior event
    /// AND a divergent payload at the same `state_name`. The
    /// short-circuit cannot apply (the payloads differ) and the
    /// rewrite cannot apply (the hash already exists), so the writer
    /// must back off and re-read the event log before deciding what
    /// to do.
    ///
    /// Maps to BSD sysexit code `EX_TEMPFAIL` (75) at the CLI
    /// boundary — the conflict is transient under the caller's
    /// control. Drives PRD R17 (idempotent retries) and OQ8
    /// (idempotency-hash domain). See Issue 12 of
    /// PLAN-koto-request-store.
    #[error("concurrent submission conflict for session '{session_id}' at state '{state_name}'")]
    ConcurrentSubmissionConflict {
        /// Session id (workflow name) whose log carries the
        /// conflicting prior event.
        session_id: String,
        /// Template state name where the conflict surfaced. The hash
        /// domain is `(state_name, payload)`; conflicts can only fire
        /// when both inputs and a prior hash match.
        state_name: String,
    },
}

impl EngineError {
    /// Return the BSD sysexit-style exit code associated with this
    /// error variant. The CLI boundary calls this from
    /// `exit_code_for_engine_error` when dispatching exit codes for
    /// engine errors.
    ///
    /// Mapping:
    /// - `EpochFenceViolation` → 65 (`EX_DATAERR`)
    /// - `RedelegationCapExceeded` → 75 (`EX_TEMPFAIL`)
    /// - `RecursionCapExceeded` → 64 (`EX_USAGE`)
    /// - `ConcurrentSubmissionConflict` → 75 (`EX_TEMPFAIL`)
    /// - `StateFileCorrupted` → 3 (legacy `EXIT_INFRASTRUCTURE`)
    /// - other variants → 1 (generic error)
    ///
    /// Variants without a documented sysexit code intentionally fall
    /// back to 1; only the variants the design pins to specific codes
    /// are special-cased here.
    pub fn exit_code(&self) -> i32 {
        match self {
            EngineError::EpochFenceViolation { .. } => 65,
            EngineError::RedelegationCapExceeded { .. } => 75,
            EngineError::RecursionCapExceeded { .. } => 64,
            EngineError::ConcurrentSubmissionConflict { .. } => 75,
            EngineError::StateFileCorrupted(_) => 3,
            _ => 1,
        }
    }
}
