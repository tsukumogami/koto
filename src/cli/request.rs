//! `koto request` — the CLI noun group over the request store.
//!
//! Ten subcommands, one response envelope, four exit classes
//! (DESIGN-request-lifecycle.md Decision 5). This module is a thin
//! shell over [`crate::engine::request_store`]: it parses flags,
//! validates identifiers, maps the store's typed errors onto the exit
//! classes, and prints JSON. Every precondition, bound, and
//! concurrency guarantee lives in the store, where the lock is.
//!
//! # Why the envelope looks the way it does
//!
//! The discriminated state field is `request_state`, not `action` and
//! not `state`: `action` is imperative and would invite treating `get`
//! as an advancing poll, and `state` already means a template state
//! name everywhere else in koto. `cli_contract` is two integers rather
//! than a `"1.0"` string so no consumer can compare it
//! lexicographically and be wrong.
//!
//! Output is JSON unconditionally with no format flag, matching `koto
//! next` and `koto status`. Two `get` calls on an unchanged request
//! produce byte-equal stdout, which is what makes a shell wrapper's
//! change detection a string comparison rather than a parse.
//!
//! # Why readiness is its own verb
//!
//! `get` exits zero for an open request, a partially resolved one, and
//! a closed one alike — a read that succeeded is a success. Waiting is
//! [`RequestCommand::Wait`], which polls the same read path and writes
//! nothing. A wait that reaches its deadline exits in the transient
//! class; a predicate that can never hold exits in the caller-error
//! class, because telling a shell loop to retry forever on a condition
//! that will never become true is worse than failing it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::cli::next_types::ErrorDetail;
use crate::engine::request_store::{
    self, AbandonLeg, BindLeg, CloseRequest, LegCounts, LegProgress, LegResult, LegSpec, LegView,
    ListFilter, NewRequest, RequestBounds, RequestStoreError, RequestSummary, RequestView,
    ValidatedRequestId,
};
use crate::engine::types::{
    now_iso8601, CloseDisposition, LegDeclaration, LegDisposition, LegResultSource, RequestState,
    ValidatedCoordId, ValidatedSessionId, WorkflowResult,
};

// ===== The contract =====

/// Major of the contract this build implements. A caller pinning a
/// different major is refused: a major bump means a field this caller
/// depends on moved or changed meaning.
pub const CLI_CONTRACT_MAJOR: u32 = 1;

/// Minor of the contract this build implements. Minors are additive,
/// so a caller pinning an *older* minor is served — it will simply see
/// fields it does not read. A caller pinning a *newer* minor is
/// refused, because it was written against fields this build does not
/// emit.
pub const CLI_CONTRACT_MINOR: u32 = 0;

/// The contract version, emitted on every response.
///
/// Two integers rather than a string: a consumer comparing `"1.10"`
/// against `"1.9"` lexicographically gets the wrong answer, and this
/// shape makes that mistake unavailable
/// (DESIGN-request-lifecycle.md Decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CliContract {
    pub major: u32,
    pub minor: u32,
}

impl CliContract {
    fn current() -> Self {
        Self {
            major: CLI_CONTRACT_MAJOR,
            minor: CLI_CONTRACT_MINOR,
        }
    }
}

// ===== Bounds owned by the CLI layer =====

/// Bytes accepted in any JSON flag value before it is parsed.
///
/// Same ceiling as `koto session start --inputs` and `koto next
/// --with-data`. Checked on the raw string so the parser is never
/// handed more than this.
const MAX_JSON_FLAG_BYTES: usize = 1024 * 1024;

/// Nesting depth accepted in any JSON flag value. Pairs with
/// [`MAX_JSON_FLAG_BYTES`] against a payload that stays under the size
/// cap by recursing.
const MAX_JSON_FLAG_DEPTH: usize = 128;

/// Default poll interval for `wait`, in seconds.
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 2;

/// Floor for `--interval-secs`. Zero would spin, burning a core to
/// observe a filesystem that changes on a human timescale
/// (DESIGN-request-lifecycle.md Security Considerations, denial of
/// service).
const MIN_WAIT_INTERVAL_SECS: u64 = 1;

/// Granularity the wait sleeps at, so a signal is noticed promptly
/// rather than after a whole interval.
const WAIT_SLICE: Duration = Duration::from_millis(100);

// ===== Errors =====

/// The closed code set for `koto request`.
///
/// Closed on purpose: a consumer that has to string-match a message to
/// tell "this leg already has a result" from "this request is closed"
/// has no contract at all. Each code binds to exactly one exit class
/// via [`RequestErrorCode::exit_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestErrorCode {
    // -- caller error (2) --
    /// No request record at this identifier.
    RequestNotFound,
    /// The request has no leg by that name.
    LegNotFound,
    /// A request id, leg name, or session id failed its grammar.
    ///
    /// This group carries its own invalid-identifier code rather than
    /// bubbling `EngineError::InvalidSessionId`, which falls through
    /// to the transient class elsewhere in the crate — a malformed
    /// identifier is never worth retrying
    /// (DESIGN-request-lifecycle.md Decision 5).
    InvalidIdentifier,
    /// A flag payload was malformed, or the flag combination was.
    InvalidSubmission,
    /// `--cli-contract` named a contract this build does not serve.
    ContractMismatch,
    /// A leg mutation, or a second close, on a closed request.
    RequestClosed,
    /// A second result, or a mutation, on a leg that already answered.
    LegAlreadyResolved,
    /// A mutation on a leg the requester stopped waiting on.
    LegAbandoned,
    /// A rebind that would point an already-bound leg somewhere else.
    LegBoundToDifferentChild,
    /// `bind` named a child whose session could not be read.
    ChildNotFound,
    /// `bind` named a child whose header does not satisfy the
    /// dispatch-fence predicate, so the leg could never be fenced.
    ChildNotFenceable,
    /// `bind` named a child that already carries a pointer at a
    /// different request-and-leg pair.
    ChildBoundToDifferentLeg,
    /// An explicit `resolve` on a leg whose result will be promoted
    /// from its bound child's terminal tick.
    ExplicitResolveOnBoundLeg,
    /// The generated identifier collided with an existing record.
    RequestIdCollision,
    /// A retry presented a known idempotency hash with a different
    /// payload, so it is not the same logical write.
    IdempotencyConflict,
    /// One of the five bounds rejected the call.
    BoundExceeded,
    /// The presented `--dispatch-epoch` does not match the epoch
    /// recorded on the leg's bind event.
    EpochFenceViolation,
    /// The predicate could never hold, detected before polling began.
    PredicateImpossible,
    /// The predicate became unsatisfiable while the wait was running,
    /// through abandonment or close. Distinct from a timeout so a
    /// consumer can tell "not yet" from "never".
    PredicateBecameImpossible,

    // -- transient (1) --
    /// The predicate was still unsatisfied at the deadline.
    WaitTimeout,
    /// A signal arrived while waiting.
    WaitInterrupted,
    /// The write lock was not acquired before its deadline.
    LockContention,

    // -- infrastructure (3) --
    /// The filesystem refused, or the log disagrees with itself.
    PersistenceError,
}

impl RequestErrorCode {
    /// Map each code to its process exit code.
    ///
    /// Only 0, 1, 2 and 3 are used, so none collides with the
    /// sysexits values (64, 65, 66, 75) returned elsewhere in the
    /// crate (DESIGN-request-lifecycle.md Decision 5).
    pub fn exit_code(&self) -> i32 {
        match self {
            RequestErrorCode::WaitTimeout
            | RequestErrorCode::WaitInterrupted
            | RequestErrorCode::LockContention => 1,

            RequestErrorCode::RequestNotFound
            | RequestErrorCode::LegNotFound
            | RequestErrorCode::InvalidIdentifier
            | RequestErrorCode::InvalidSubmission
            | RequestErrorCode::ContractMismatch
            | RequestErrorCode::RequestClosed
            | RequestErrorCode::LegAlreadyResolved
            | RequestErrorCode::LegAbandoned
            | RequestErrorCode::LegBoundToDifferentChild
            | RequestErrorCode::ChildNotFound
            | RequestErrorCode::ChildNotFenceable
            | RequestErrorCode::ChildBoundToDifferentLeg
            | RequestErrorCode::ExplicitResolveOnBoundLeg
            | RequestErrorCode::RequestIdCollision
            | RequestErrorCode::IdempotencyConflict
            | RequestErrorCode::BoundExceeded
            | RequestErrorCode::EpochFenceViolation
            | RequestErrorCode::PredicateImpossible
            | RequestErrorCode::PredicateBecameImpossible => 2,

            RequestErrorCode::PersistenceError => 3,
        }
    }
}

/// A rejection, in the structured nested envelope shape `koto next`
/// established: `{"error": {"code", "message", "details"}}`.
///
/// The flat `{"error": "...", "command": "..."}` legacy shape is
/// deliberately not used here — this group is new surface, so there is
/// no consumer to keep compatible with it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestError {
    pub code: RequestErrorCode,
    pub message: String,
    pub details: Vec<ErrorDetail>,
}

impl RequestError {
    fn new(code: RequestErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Vec::new(),
        }
    }

    fn with_detail(mut self, field: impl Into<String>, reason: impl Into<String>) -> Self {
        self.details.push(ErrorDetail {
            field: field.into(),
            reason: reason.into(),
        });
        self
    }

    /// Print the envelope on stdout and exit in this code's class.
    fn emit(&self) -> ! {
        let json = serde_json::json!({ "error": self });
        crate::cli::exit_with_error_code(json, self.code.exit_code());
    }
}

/// Route a store rejection onto the CLI's code set.
///
/// Every store variant is mapped explicitly rather than through a
/// catch-all, so adding a variant to [`RequestStoreError`] fails this
/// match rather than silently landing in the infrastructure class.
fn map_store_error(e: RequestStoreError) -> RequestError {
    let message = e.to_string();
    match e {
        RequestStoreError::NotFound { .. } => {
            RequestError::new(RequestErrorCode::RequestNotFound, message)
        }
        RequestStoreError::InvalidRequestId { .. } | RequestStoreError::InvalidLegName { .. } => {
            RequestError::new(RequestErrorCode::InvalidIdentifier, message)
        }
        RequestStoreError::DuplicateLegName { .. } => {
            RequestError::new(RequestErrorCode::InvalidSubmission, message)
        }
        RequestStoreError::LegNotFound { .. } => {
            RequestError::new(RequestErrorCode::LegNotFound, message)
        }
        RequestStoreError::LegAlreadyResolved { .. } => {
            RequestError::new(RequestErrorCode::LegAlreadyResolved, message)
        }
        RequestStoreError::LegAbandoned { .. } => {
            RequestError::new(RequestErrorCode::LegAbandoned, message)
        }
        RequestStoreError::LegBoundToChild { .. } => {
            RequestError::new(RequestErrorCode::ExplicitResolveOnBoundLeg, message)
        }
        RequestStoreError::LegBoundToDifferentChild { .. } => {
            RequestError::new(RequestErrorCode::LegBoundToDifferentChild, message)
        }
        RequestStoreError::RequestClosed { .. } => {
            RequestError::new(RequestErrorCode::RequestClosed, message)
        }
        RequestStoreError::RequestIdCollision { .. } => {
            RequestError::new(RequestErrorCode::RequestIdCollision, message)
        }
        RequestStoreError::BoundExceeded {
            dimension,
            limit,
            observed,
        } => RequestError::new(RequestErrorCode::BoundExceeded, message)
            .with_detail(dimension, format!("limit {limit}, observed {observed}")),
        RequestStoreError::LockContention { .. } => {
            RequestError::new(RequestErrorCode::LockContention, message)
        }
        RequestStoreError::IdempotencyConflict { .. } => {
            RequestError::new(RequestErrorCode::IdempotencyConflict, message)
        }
        RequestStoreError::Corrupt { .. }
        | RequestStoreError::Io { .. }
        | RequestStoreError::Other(_) => {
            RequestError::new(RequestErrorCode::PersistenceError, message)
        }
    }
}

// ===== The command surface =====

/// `--cli-contract MAJOR.MINOR`, declared once and marked global so it
/// is accepted on every subcommand in the group without ten copies of
/// the flag.
#[derive(Args, Debug, Clone)]
pub struct RequestGroupArgs {
    /// Pin the response contract this caller was written against, as
    /// MAJOR.MINOR. Validated before any IO; a mismatch exits in the
    /// caller-error class without reading or writing anything.
    #[arg(long, value_name = "MAJOR.MINOR", global = true)]
    pub cli_contract: Option<String>,
}

/// `--state open|closed` on `list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateFilter {
    Open,
    Closed,
}

impl From<StateFilter> for RequestState {
    fn from(value: StateFilter) -> Self {
        match value {
            StateFilter::Open => RequestState::Open,
            StateFilter::Closed => RequestState::Closed,
        }
    }
}

/// The ten subcommands under `koto request`.
///
/// Leg-scoped `abandon` and request-scoped `abandon-request` are
/// separate subcommands rather than one subcommand with an optional
/// positional: an unset shell variable then fails argument parsing
/// instead of escalating a leg abandonment into abandoning the whole
/// request (DESIGN-request-lifecycle.md Decision 5).
///
/// `--issued-by` is on the six mutating verbs and on none of the
/// reads. `create` carries `--requested-by` instead, which is the same
/// attribution recorded in the header.
#[derive(Subcommand, Debug)]
pub enum RequestCommand {
    /// Create a request and print its generated identifier.
    Create {
        /// The full form: `{"legs":[{"name","role","template",
        /// "inputs"}],"inputs":{...}}`. Mutually exclusive with the
        /// flat triple.
        #[arg(long, value_name = "JSON")]
        with_data: Option<String>,

        /// One-leg shorthand: the role the single leg asks for. The
        /// leg is named after the role.
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// One-leg shorthand: the template the leg's child is
        /// materialized from.
        #[arg(long, value_name = "TEMPLATE")]
        template: Option<String>,

        /// One-leg shorthand: the leg's own ask, as JSON.
        #[arg(long, value_name = "JSON")]
        inputs: Option<String>,

        /// Who asked. Audit attribution, not authorization.
        #[arg(long, value_name = "ID")]
        requested_by: String,

        /// The coordinator answerable for the request, which may
        /// outlive the session that created it.
        #[arg(long, value_name = "ID")]
        coordinator_of_record: String,
    },

    /// Bind a leg to the child session that will fulfil it.
    Bind {
        request_id: String,
        leg: String,
        /// The child session that will answer this leg.
        #[arg(long, value_name = "SESSION_ID")]
        child: String,
        /// The child's dispatch epoch at bind time, recorded on the
        /// bind event so the leg's mutating paths can fence against it
        /// after the child's session directory is gone.
        #[arg(long, value_name = "N")]
        dispatch_epoch: Option<u32>,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },

    /// Read a request. Exits zero for open, partially resolved, and
    /// closed requests alike.
    Get { request_id: String },

    /// Poll until a predicate holds, the deadline passes, or the
    /// predicate becomes impossible.
    #[command(group = clap::ArgGroup::new("predicate").required(true).multiple(false))]
    Wait {
        request_id: String,

        /// Wait for this leg to resolve.
        #[arg(long, value_name = "NAME", group = "predicate")]
        leg: Option<String>,

        /// Wait for every leg to resolve.
        #[arg(long, group = "predicate")]
        all_legs: bool,

        /// Wait for the request to close.
        #[arg(long, group = "predicate")]
        closed: bool,

        /// Wait until at least N legs have resolved. Satisfied by a
        /// partial fan-out; it does not require every leg.
        #[arg(long, value_name = "N", group = "predicate")]
        resolved_count: Option<usize>,

        /// Absolute budget for the wait, in seconds. Required: a wait
        /// with no deadline is a hang.
        #[arg(long, value_name = "N")]
        timeout_secs: u64,

        /// Seconds between polls. Defaults to 2, clamped to a floor of
        /// 1 so zero cannot spin.
        #[arg(long, value_name = "N")]
        interval_secs: Option<u64>,
    },

    /// List requests. Cursor-free: advances no coordinator cursor and
    /// writes nothing.
    List {
        /// Keep only requests asked for by this principal.
        #[arg(long, value_name = "ID", conflicts_with = "coordinator_of_record")]
        requested_by: Option<String>,

        /// Keep only requests this coordinator is answerable for.
        #[arg(long, value_name = "ID")]
        coordinator_of_record: Option<String>,

        /// Keep only open, or only closed, requests.
        #[arg(long, value_enum, value_name = "STATE")]
        state: Option<StateFilter>,

        /// Keep only requests with at least one leg still open.
        #[arg(long)]
        unresolved_legs: bool,
    },

    /// Append mid-flight progress to an open leg.
    Progress {
        request_id: String,
        leg: String,
        /// A JSON object of caller-supplied content.
        #[arg(long, value_name = "JSON")]
        with_data: String,
        /// The epoch this writer was dispatched with. Required when
        /// the leg is bound.
        #[arg(long, value_name = "N")]
        dispatch_epoch: Option<u32>,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },

    /// Record a leg's result explicitly. Only for an unbound leg: a
    /// bound leg's result is promoted from its child's terminal tick.
    Resolve {
        request_id: String,
        leg: String,
        /// The result envelope:
        /// `{"status":"success|failure|skipped","summary":"...",
        /// "payload":{...}}`.
        #[arg(long, value_name = "JSON")]
        with_data: String,
        #[arg(long, value_name = "N")]
        dispatch_epoch: Option<u32>,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },

    /// Stop waiting on one leg, leaving the request's other legs open.
    Abandon {
        request_id: String,
        leg: String,
        /// Why. Required, capped at 4 KiB, control characters
        /// stripped: this text reaches a delegate's directive.
        #[arg(long, value_name = "TEXT")]
        rationale: String,
        #[arg(long, value_name = "N")]
        dispatch_epoch: Option<u32>,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },

    /// Abandon every open leg and close the request.
    AbandonRequest {
        request_id: String,
        #[arg(long, value_name = "TEXT")]
        rationale: String,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },

    /// Close a request, recording a disposition derived from its legs.
    Close {
        request_id: String,
        #[arg(long, value_name = "ID")]
        issued_by: Option<String>,
    },
}

// ===== The response envelope =====

/// Every response this group prints, for every verb.
///
/// One shape rather than one per verb: a consumer reads
/// `request_state`, `leg_counts` and `revision` the same way after a
/// write as after a read, and `revision` after a write is what lets it
/// tell its own append from a concurrent one.
#[derive(Debug, Serialize)]
struct RequestEnvelope<'a> {
    request_id: &'a str,
    request_state: RequestState,
    /// Always present, `null` while the request is open, so a consumer
    /// never has to distinguish "absent" from "not yet closed".
    close_disposition: Option<CloseDisposition>,
    leg_counts: LegCounts,
    revision: u64,
    requested_by: &'a str,
    coordinator_of_record: &'a str,
    created_at: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<&'a serde_json::Value>,
    /// Legs in name order, which is what makes two reads of an
    /// unchanged request byte-equal.
    legs: &'a BTreeMap<String, LegView>,
    /// Whether this call appended an event. Present on the mutating
    /// verbs only, so a caller can tell a real bind from an idempotent
    /// rebind. Absent on reads, which keeps `get` byte-stable.
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<bool>,
    cli_contract: CliContract,
}

/// The `list` response. A listing has no single request state, so it
/// carries per-row summaries plus the contract rather than the
/// per-request envelope.
#[derive(Debug, Serialize)]
struct ListEnvelope {
    requests: Vec<RequestSummary>,
    cli_contract: CliContract,
}

/// Render a view as the response envelope.
fn render(view: &RequestView, written: Option<bool>) -> Result<String, RequestError> {
    let envelope = RequestEnvelope {
        request_id: &view.header.request_id,
        request_state: view.request_state,
        close_disposition: view.close_disposition,
        leg_counts: view.leg_counts(),
        revision: view.revision,
        requested_by: &view.header.requested_by,
        coordinator_of_record: &view.header.coordinator_of_record,
        created_at: &view.header.created_at,
        inputs: view.inputs.as_ref(),
        legs: &view.legs,
        written,
        cli_contract: CliContract::current(),
    };
    serde_json::to_string(&envelope).map_err(|e| {
        RequestError::new(
            RequestErrorCode::PersistenceError,
            format!("failed to serialize the response: {e}"),
        )
    })
}

// ===== Shared helpers =====

/// Validate `--cli-contract` before any IO.
///
/// Modelled on `koto next --dispatch-epoch`: present-and-compare
/// before the first write, so a caller pinned to a contract this build
/// does not serve learns that instead of acting on a response it
/// cannot read.
fn validate_contract(pin: Option<&str>) -> Result<(), RequestError> {
    let Some(pin) = pin else {
        return Ok(());
    };
    let malformed = || {
        RequestError::new(
            RequestErrorCode::ContractMismatch,
            format!(
                "--cli-contract must be MAJOR.MINOR, got '{}'",
                pin.escape_debug()
            ),
        )
    };
    let (major, minor) = pin.split_once('.').ok_or_else(malformed)?;
    let major: u32 = major.parse().map_err(|_| malformed())?;
    let minor: u32 = minor.parse().map_err(|_| malformed())?;

    // A major bump means a field the caller depends on moved or
    // changed meaning. A newer minor means the caller expects fields
    // this build does not emit. An older minor is served: minors are
    // additive, so the caller simply sees fields it does not read.
    if major != CLI_CONTRACT_MAJOR || minor > CLI_CONTRACT_MINOR {
        return Err(RequestError::new(
            RequestErrorCode::ContractMismatch,
            format!(
                "this build serves cli_contract {CLI_CONTRACT_MAJOR}.{CLI_CONTRACT_MINOR}, \
                 caller pinned {major}.{minor}"
            ),
        ));
    }
    Ok(())
}

/// The workspace root the request store lives under.
///
/// `~/.koto`, matching every other workspace-scoped path in the crate
/// (the terminal index, coordinator cursors, workspace prune).
fn koto_root() -> Result<PathBuf, RequestError> {
    dirs::home_dir().map(|h| h.join(".koto")).ok_or_else(|| {
        RequestError::new(
            RequestErrorCode::PersistenceError,
            "could not determine the home directory",
        )
    })
}

/// Read the two operator-tunable bounds off the resolved config.
///
/// A config that fails to load falls back to the defaults rather than
/// failing the call: the bounds are a ceiling, and refusing to serve a
/// request because a config file has a typo would be a worse outcome
/// than serving it against the shipped defaults.
fn bounds() -> RequestBounds {
    match crate::config::resolve::load_config() {
        Ok(config) => RequestBounds::from_config(&config.request_store),
        Err(e) => {
            eprintln!("warning: falling back to default request bounds: {e}");
            RequestBounds::default()
        }
    }
}

/// Validate a caller-supplied request id.
fn request_id(raw: &str) -> Result<ValidatedRequestId, RequestError> {
    ValidatedRequestId::new(raw).map_err(map_store_error)
}

/// Validate a caller-supplied session or principal identifier.
///
/// Mapped to [`RequestErrorCode::InvalidIdentifier`] rather than
/// bubbling the engine's own error, which falls through to the
/// transient class — a malformed identifier is never worth retrying
/// (DESIGN-request-lifecycle.md Decision 5).
fn session_identifier(flag: &str, raw: &str) -> Result<String, RequestError> {
    ValidatedSessionId::new(raw)
        .map(|id| id.as_str().to_string())
        .map_err(|e| {
            RequestError::new(RequestErrorCode::InvalidIdentifier, e.to_string())
                .with_detail(flag, "not a valid session identifier")
        })
}

/// Validate a caller-supplied coordinator identifier.
fn coordinator_identifier(flag: &str, raw: &str) -> Result<String, RequestError> {
    ValidatedCoordId::new(raw)
        .map(|id| id.as_str().to_string())
        .map_err(|e| {
            RequestError::new(RequestErrorCode::InvalidIdentifier, e.to_string())
                .with_detail(flag, "not a valid coordinator identifier")
        })
}

/// Same, for the optional `--issued-by` attribution.
fn optional_identifier(flag: &str, raw: Option<&str>) -> Result<Option<String>, RequestError> {
    match raw {
        Some(raw) => session_identifier(flag, raw).map(Some),
        None => Ok(None),
    }
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

/// Parse a JSON flag value under the size and depth caps.
///
/// The size check is on the raw string so the parser is never handed
/// more than the cap; the depth check runs on the parsed value, which
/// the size cap has already bounded.
fn parse_json_flag(flag: &str, raw: &str) -> Result<serde_json::Value, RequestError> {
    if raw.len() > MAX_JSON_FLAG_BYTES {
        return Err(RequestError::new(
            RequestErrorCode::BoundExceeded,
            format!(
                "{flag} payload is {} bytes, over the {MAX_JSON_FLAG_BYTES}-byte limit",
                raw.len()
            ),
        )
        .with_detail(flag, "payload too large"));
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        RequestError::new(
            RequestErrorCode::InvalidSubmission,
            format!("{flag} is not valid JSON: {e}"),
        )
        .with_detail(flag, "not valid JSON")
    })?;
    let depth = json_max_depth(&value);
    if depth > MAX_JSON_FLAG_DEPTH {
        return Err(RequestError::new(
            RequestErrorCode::BoundExceeded,
            format!("{flag} nests {depth} levels deep, over the {MAX_JSON_FLAG_DEPTH} limit"),
        )
        .with_detail(flag, "nested too deeply"));
    }
    Ok(value)
}

/// Parse a JSON flag that must be an object, into the store's content
/// map. A [`BTreeMap`] rather than the parsed value's own map so the
/// serialized form is byte-stable across reads.
fn parse_object_flag(
    flag: &str,
    raw: &str,
) -> Result<BTreeMap<String, serde_json::Value>, RequestError> {
    let value = parse_json_flag(flag, raw)?;
    let serde_json::Value::Object(map) = value else {
        return Err(RequestError::new(
            RequestErrorCode::InvalidSubmission,
            format!("{flag} must be a JSON object"),
        )
        .with_detail(flag, "expected an object"));
    };
    Ok(map.into_iter().collect())
}

/// Compare a presented `--dispatch-epoch` against the epoch recorded
/// on the leg's bind event.
///
/// **Against the bind event, not the child's header.** The header
/// disappears on the child's terminal tick while the request record
/// outlives it, so comparing against the header would leave the leg
/// permanently unfenceable during exactly the window a displaced agent
/// may still be alive (DESIGN-request-lifecycle.md Decision 3).
///
/// An unbound leg has no epoch and is unfenceable by construction.
/// That is correct rather than a gap: its creator is the only party
/// that can mutate it. The flag is accepted and ignored there rather
/// than rejected, so a wrapper that always passes it keeps working
/// against a leg that has not been bound yet.
///
/// Strict equality, matching [`crate::engine::epoch::validate_epoch`]:
/// a future epoch rejects alongside a stale one, which catches a
/// spawner that bakes the wrong value.
fn fence(leg: &LegView, presented: Option<u32>) -> Result<(), RequestError> {
    let Some(expected) = leg.bound_epoch else {
        return Ok(());
    };
    match presented {
        Some(p) if p == expected => Ok(()),
        Some(p) => Err(RequestError::new(
            RequestErrorCode::EpochFenceViolation,
            format!(
                "leg '{}' was bound at dispatch epoch {expected}; this writer presented {p}",
                leg.name
            ),
        )
        .with_detail("--dispatch-epoch", format!("expected {expected}, got {p}"))),
        None => Err(RequestError::new(
            RequestErrorCode::EpochFenceViolation,
            format!(
                "leg '{}' is bound, so --dispatch-epoch is required and must be {expected}",
                leg.name
            ),
        )
        .with_detail("--dispatch-epoch", "required on a bound leg")),
    }
}

/// Read a leg's view, for the checks that run before a write.
///
/// The store re-reads under the lock before it appends, so this read
/// is advisory: it exists to reject with a precise code rather than to
/// carry a guarantee.
fn read_leg(view: &RequestView, leg_name: &str) -> Result<LegView, RequestError> {
    view.leg(leg_name).cloned().map_err(map_store_error)
}

// ===== Entry point =====

/// Dispatch one `koto request` invocation.
///
/// Never returns: every path either prints an envelope and exits zero
/// or prints an error envelope and exits in its class. That keeps the
/// exit mapping in one place instead of spread across ten call sites.
pub fn handle(args: RequestGroupArgs, command: RequestCommand) -> ! {
    // Before any IO, per Decision 5: a caller pinned to a contract
    // this build does not serve must not observe a side effect.
    if let Err(e) = validate_contract(args.cli_contract.as_deref()) {
        e.emit();
    }

    match run(command) {
        Ok(rendered) => {
            println!("{rendered}");
            std::process::exit(0);
        }
        Err(e) => e.emit(),
    }
}

/// The verb dispatch, returning the rendered envelope.
fn run(command: RequestCommand) -> Result<String, RequestError> {
    match command {
        RequestCommand::Create {
            with_data,
            role,
            template,
            inputs,
            requested_by,
            coordinator_of_record,
        } => create(
            with_data.as_deref(),
            role.as_deref(),
            template.as_deref(),
            inputs.as_deref(),
            &requested_by,
            &coordinator_of_record,
        ),
        RequestCommand::Bind {
            request_id: id,
            leg,
            child,
            dispatch_epoch,
            issued_by,
        } => bind(&id, &leg, &child, dispatch_epoch, issued_by.as_deref()),
        RequestCommand::Get { request_id: id } => get(&id),
        RequestCommand::Wait {
            request_id: id,
            leg,
            all_legs,
            closed,
            resolved_count,
            timeout_secs,
            interval_secs,
        } => {
            let predicate = Predicate::from_flags(leg, all_legs, closed, resolved_count)?;
            wait(&id, predicate, timeout_secs, interval_secs)
        }
        RequestCommand::List {
            requested_by,
            coordinator_of_record,
            state,
            unresolved_legs,
        } => list(
            requested_by.as_deref(),
            coordinator_of_record.as_deref(),
            state,
            unresolved_legs,
        ),
        RequestCommand::Progress {
            request_id: id,
            leg,
            with_data,
            dispatch_epoch,
            issued_by,
        } => progress(&id, &leg, &with_data, dispatch_epoch, issued_by.as_deref()),
        RequestCommand::Resolve {
            request_id: id,
            leg,
            with_data,
            dispatch_epoch,
            issued_by,
        } => resolve(&id, &leg, &with_data, dispatch_epoch, issued_by.as_deref()),
        RequestCommand::Abandon {
            request_id: id,
            leg,
            rationale,
            dispatch_epoch,
            issued_by,
        } => abandon(&id, &leg, &rationale, dispatch_epoch, issued_by.as_deref()),
        RequestCommand::AbandonRequest {
            request_id: id,
            rationale,
            issued_by,
        } => abandon_request(&id, &rationale, issued_by.as_deref()),
        RequestCommand::Close {
            request_id: id,
            issued_by,
        } => close(&id, issued_by.as_deref()),
    }
}

// ===== create =====

/// One leg in the `--with-data` creation form.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegSpecInput {
    name: String,
    role: String,
    template: String,
    /// The leg's own ask. Absent means no ask, which is legitimate for
    /// a leg whose whole brief is in the request-level `inputs`.
    #[serde(default)]
    inputs: serde_json::Value,
}

/// The `--with-data` creation form.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    legs: Vec<LegSpecInput>,
    /// Shared context for the whole request; per-leg asks live on each
    /// declaration.
    #[serde(default)]
    inputs: Option<serde_json::Value>,
}

fn create(
    with_data: Option<&str>,
    role: Option<&str>,
    template: Option<&str>,
    inputs: Option<&str>,
    requested_by: &str,
    coordinator_of_record: &str,
) -> Result<String, RequestError> {
    let flat_present = role.is_some() || template.is_some() || inputs.is_some();
    let (legs, shared_inputs) =
        match (with_data, flat_present) {
            (Some(_), true) => return Err(RequestError::new(
                RequestErrorCode::InvalidSubmission,
                "--with-data and the --role / --template / --inputs triple are mutually exclusive",
            )),
            (Some(raw), false) => parse_create_with_data(raw)?,
            (None, true) => (parse_create_flat(role, template, inputs)?, None),
            (None, false) => return Err(RequestError::new(
                RequestErrorCode::InvalidSubmission,
                "create requires either --with-data or the --role / --template / --inputs triple",
            )),
        };

    let requested_by = session_identifier("--requested-by", requested_by)?;
    let coordinator_of_record =
        coordinator_identifier("--coordinator-of-record", coordinator_of_record)?;

    let root = koto_root()?;
    let spec = NewRequest {
        requested_by,
        coordinator_of_record,
        legs,
        inputs: shared_inputs,
        created_at: now_iso8601(),
    };
    let id = request_store::create_request(&root, &spec, &bounds()).map_err(map_store_error)?;
    // Read back rather than projecting the spec: the envelope must
    // describe what is on disk, including the revision the creation
    // event landed at.
    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(true))
}

/// Parse the full creation form.
fn parse_create_with_data(
    raw: &str,
) -> Result<(Vec<LegSpec>, Option<serde_json::Value>), RequestError> {
    let value = parse_json_flag("--with-data", raw)?;
    let parsed: CreateInput = serde_json::from_value(value).map_err(|e| {
        RequestError::new(
            RequestErrorCode::InvalidSubmission,
            format!("--with-data is not a request creation payload: {e}"),
        )
        .with_detail("--with-data", "expected {\"legs\":[...],\"inputs\":{...}}")
    })?;
    if parsed.legs.is_empty() {
        return Err(RequestError::new(
            RequestErrorCode::InvalidSubmission,
            "a request must declare at least one leg",
        )
        .with_detail("legs", "empty"));
    }
    let legs = parsed
        .legs
        .into_iter()
        .map(|leg| LegSpec {
            name: leg.name,
            declaration: LegDeclaration {
                role: leg.role,
                template: leg.template,
                inputs: leg.inputs,
            },
        })
        .collect();
    Ok((legs, parsed.inputs))
}

/// Parse the one-leg shorthand.
///
/// The leg is named after the role, which is the only identifier the
/// shorthand carries — inventing a fixed name would make every
/// one-leg request's leg indistinguishable in `get` output. A role
/// that does not satisfy the leg-name grammar is rejected with a
/// pointer at the full form rather than silently mangled.
fn parse_create_flat(
    role: Option<&str>,
    template: Option<&str>,
    inputs: Option<&str>,
) -> Result<Vec<LegSpec>, RequestError> {
    let mut missing: Vec<&'static str> = Vec::new();
    if role.is_none() {
        missing.push("--role");
    }
    if template.is_none() {
        missing.push("--template");
    }
    if inputs.is_none() {
        missing.push("--inputs");
    }
    if !missing.is_empty() {
        return Err(RequestError::new(
            RequestErrorCode::InvalidSubmission,
            format!(
                "the one-leg form requires --role, --template and --inputs (missing: {})",
                missing.join(", ")
            ),
        ));
    }
    let role = role.unwrap_or_default();
    let template = template.unwrap_or_default();
    let parsed_inputs = parse_json_flag("--inputs", inputs.unwrap_or_default())?;

    request_store::validate_leg_name(role).map_err(|e| {
        map_store_error(e).with_detail(
            "--role",
            "the one-leg form names the leg after the role; use --with-data to name it separately",
        )
    })?;

    Ok(vec![LegSpec {
        name: role.to_string(),
        declaration: LegDeclaration {
            role: role.to_string(),
            template: template.to_string(),
            inputs: parsed_inputs,
        },
    }])
}

// ===== reads =====

fn get(id: &str) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let root = koto_root()?;
    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, None)
}

fn list(
    requested_by: Option<&str>,
    coordinator_of_record: Option<&str>,
    state: Option<StateFilter>,
    unresolved_legs: bool,
) -> Result<String, RequestError> {
    let filter = ListFilter {
        requested_by: match requested_by {
            Some(raw) => Some(session_identifier("--requested-by", raw)?),
            None => None,
        },
        coordinator_of_record: match coordinator_of_record {
            Some(raw) => Some(coordinator_identifier("--coordinator-of-record", raw)?),
            None => None,
        },
        state: state.map(RequestState::from),
        unresolved_legs_only: unresolved_legs,
    };
    let root = koto_root()?;
    let requests = request_store::list_requests(&root, &filter).map_err(map_store_error)?;
    let envelope = ListEnvelope {
        requests,
        cli_contract: CliContract::current(),
    };
    serde_json::to_string(&envelope).map_err(|e| {
        RequestError::new(
            RequestErrorCode::PersistenceError,
            format!("failed to serialize the response: {e}"),
        )
    })
}

// ===== leg mutations =====

/// Bind a leg to the child session that will fulfil it.
///
/// Three checks run against the child's own session before the append,
/// and the pointer that lets the child read its leg back is written
/// after it (DESIGN-request-lifecycle.md Decision 3):
///
/// 1. The child's session must be readable. Binding a leg to a session
///    that does not exist would produce a leg nobody can fulfil and a
///    pointer nobody can read.
/// 2. The child's header must satisfy the dispatch-fence predicate, so
///    a leg cannot be bound to something that can never be fenced. The
///    predicate excludes top-level sessions and batch-spawned children,
///    both of which would otherwise bind happily and then ignore
///    `--dispatch-epoch` forever after.
/// 3. The child must not already carry a pointer at a *different*
///    request-and-leg pair. The store's lock is per-request, so two
///    binds in different requests targeting one child do not serialize
///    at all — this is the only place the one-leg-per-child half of the
///    invariant can be enforced.
///
/// The epoch recorded on the event is read from the child's header
/// rather than taken from the flag, so the value the fence later
/// compares against cannot be a caller's guess. A presented
/// `--dispatch-epoch` that disagrees with the header is rejected rather
/// than silently overridden.
fn bind(
    id: &str,
    leg: &str,
    child: &str,
    dispatch_epoch: Option<u32>,
    issued_by: Option<&str>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let child = session_identifier("--child", child)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let root = koto_root()?;

    let session_dir = child_session_dir(&child)?;
    let header = read_child_header(&child, &session_dir)?;

    if !crate::engine::epoch::fence_applies_to(&header) {
        return Err(RequestError::new(
            RequestErrorCode::ChildNotFenceable,
            format!(
                "child '{child}' is not a dispatched request-store child, so a leg bound to it \
                 could never be fenced"
            ),
        )
        .with_detail(
            "--child",
            "the header needs a parent workflow and needs_agent: true",
        ));
    }

    // Presented before the header is read, so the comparison is against
    // the value the fence will actually use.
    if let Some(presented) = dispatch_epoch {
        if presented != header.dispatch_epoch {
            return Err(RequestError::new(
                RequestErrorCode::EpochFenceViolation,
                format!(
                    "child '{child}' is at dispatch epoch {}; this caller presented {presented}",
                    header.dispatch_epoch
                ),
            )
            .with_detail(
                "--dispatch-epoch",
                format!("expected {}, got {presented}", header.dispatch_epoch),
            ));
        }
    }

    let existing = crate::engine::leg_pointer::read_pointer(&session_dir).map_err(|e| {
        RequestError::new(
            RequestErrorCode::PersistenceError,
            format!("could not read child '{child}'s leg pointer: {e}"),
        )
    })?;
    if let Some(existing) = &existing {
        if existing.names_a_different_leg(id.as_str(), leg) {
            return Err(RequestError::new(
                RequestErrorCode::ChildBoundToDifferentLeg,
                format!(
                    "child '{child}' already fulfils leg '{}' of request '{}'; a child fulfils at \
                     most one leg",
                    existing.leg_name, existing.request_id
                ),
            )
            .with_detail("--child", "already bound elsewhere"));
        }
    }

    let timestamp = now_iso8601();
    let outcome = request_store::bind_leg(
        &root,
        &id,
        &BindLeg {
            leg_name: leg.to_string(),
            child_session_id: child.clone(),
            dispatch_epoch: Some(header.dispatch_epoch),
            issued_by,
            timestamp: timestamp.clone(),
        },
    )
    .map_err(map_store_error)?;

    // After the event is durable and after the store released the
    // request lock, so there is no lock-ordering cycle against the
    // terminal tick's promotion. The event is authoritative; a lost
    // pointer leaves a correctly-bound leg whose delegate reads no leg,
    // which is degraded capability rather than corruption.
    if let Err(e) = crate::engine::leg_pointer::write_pointer(
        &session_dir,
        &crate::engine::leg_pointer::LegPointer {
            request_id: id.as_str().to_string(),
            leg_name: leg.to_string(),
            bound_at: timestamp,
        },
    ) {
        eprintln!(
            "warning: the leg is bound but child '{child}' could not be told which leg it \
             fulfils; re-run bind to repair: {e}"
        );
    }

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

/// Locate a child's session directory, honoring the same backend
/// resolution `koto next` uses so a test workspace and a real one agree.
fn child_session_dir(child: &str) -> Result<PathBuf, RequestError> {
    let backend = crate::cli::build_local_backend().map_err(|e| {
        RequestError::new(
            RequestErrorCode::PersistenceError,
            format!("could not resolve the session store: {e}"),
        )
    })?;
    use crate::session::SessionBackend;
    Ok(backend.session_dir(child))
}

/// Read a child's state-file header, or report the child unreachable.
fn read_child_header(
    child: &str,
    session_dir: &std::path::Path,
) -> Result<crate::engine::types::StateFileHeader, RequestError> {
    let state_path = session_dir.join(crate::session::state_file_name(child));
    crate::engine::persistence::read_header(&state_path).map_err(|e| {
        RequestError::new(
            RequestErrorCode::ChildNotFound,
            format!("could not read child '{child}'s session: {e}"),
        )
        .with_detail("--child", "no readable session at this identifier")
    })
}

fn progress(
    id: &str,
    leg: &str,
    with_data: &str,
    dispatch_epoch: Option<u32>,
    issued_by: Option<&str>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let content = parse_object_flag("--with-data", with_data)?;
    let root = koto_root()?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    fence(&read_leg(&view, leg)?, dispatch_epoch)?;

    let outcome = request_store::append_progress(
        &root,
        &id,
        &LegProgress {
            leg_name: leg.to_string(),
            content,
            issued_by,
            timestamp: now_iso8601(),
        },
        &bounds(),
    )
    .map_err(map_store_error)?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

fn resolve(
    id: &str,
    leg: &str,
    with_data: &str,
    dispatch_epoch: Option<u32>,
    issued_by: Option<&str>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let value = parse_json_flag("--with-data", with_data)?;
    let result: WorkflowResult = serde_json::from_value(value).map_err(|e| {
        RequestError::new(
            RequestErrorCode::InvalidSubmission,
            format!("--with-data is not a result envelope: {e}"),
        )
        .with_detail(
            "--with-data",
            "expected {\"status\":\"success|failure|skipped\",\"summary\":\"...\"}",
        )
    })?;
    let root = koto_root()?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    let leg_view = read_leg(&view, leg)?;
    fence(&leg_view, dispatch_epoch)?;

    // An explicit resolve on a bound leg is rejected by the store,
    // inside the same lock as the append: the discriminator is the
    // leg's binding, so a bind landing between an unlocked read here
    // and the append would slip past a check made at this layer
    // (DESIGN-request-lifecycle.md Decision 6).

    let outcome = request_store::record_result(
        &root,
        &id,
        &LegResult {
            leg_name: leg.to_string(),
            result,
            source: LegResultSource::Explicit,
            issued_by,
            timestamp: now_iso8601(),
        },
    )
    .map_err(map_store_error)?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

fn abandon(
    id: &str,
    leg: &str,
    rationale: &str,
    dispatch_epoch: Option<u32>,
    issued_by: Option<&str>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let root = koto_root()?;

    // Leg-scoped abandon is fenced alongside progress and resolve
    // because abandonment is what triggers the directive splice: an
    // unfenced abandon would let any party holding a leg identity
    // place text in a sibling delegate's authoritative field
    // (DESIGN-request-lifecycle.md Decision 3).
    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    fence(&read_leg(&view, leg)?, dispatch_epoch)?;

    let outcome = request_store::abandon_leg(
        &root,
        &id,
        &AbandonLeg {
            leg_name: leg.to_string(),
            rationale: rationale.to_string(),
            issued_by,
            timestamp: now_iso8601(),
        },
    )
    .map_err(map_store_error)?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

// ===== request-scoped mutations =====

/// Abandon every open leg, then close the request.
///
/// A separate subcommand from [`abandon`] rather than the same verb
/// with an optional leg, so an unset shell variable fails argument
/// parsing instead of escalating one leg's abandonment into the whole
/// request's (DESIGN-request-lifecycle.md Decision 5).
///
/// Not fenced: this is a request-level operation with no single epoch
/// to compare against, so the presented identity is recorded for audit
/// instead.
fn abandon_request(
    id: &str,
    rationale: &str,
    issued_by: Option<&str>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let root = koto_root()?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    if view.request_state == RequestState::Closed {
        return Err(map_store_error(RequestStoreError::RequestClosed {
            request_id: view.header.request_id.clone(),
        }));
    }

    // Each leg's abandonment is its own lock-guarded append. A leg a
    // concurrent caller abandoned first is a no-op success in the
    // store, so the walk does not have to care who got there first.
    let open_legs: Vec<String> = view
        .legs
        .values()
        .filter(|leg| leg.disposition == LegDisposition::Open)
        .map(|leg| leg.name.clone())
        .collect();
    for leg_name in open_legs {
        request_store::abandon_leg(
            &root,
            &id,
            &AbandonLeg {
                leg_name,
                rationale: rationale.to_string(),
                issued_by: issued_by.clone(),
                timestamp: now_iso8601(),
            },
        )
        .map_err(map_store_error)?;
    }

    // The disposition is forced rather than derived: giving up on the
    // whole request is something only the caller can assert, and the
    // derived value cannot tell it apart from closing short-handed.
    let outcome = request_store::close_request(
        &root,
        &id,
        &CloseRequest {
            disposition: Some(CloseDisposition::RequestAbandoned),
            issued_by,
            timestamp: now_iso8601(),
        },
    )
    .map_err(map_store_error)?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

fn close(id: &str, issued_by: Option<&str>) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let issued_by = optional_identifier("--issued-by", issued_by)?;
    let root = koto_root()?;

    // The disposition is derived inside the store's lock, from state
    // that cannot change under it.
    let outcome = request_store::close_request(
        &root,
        &id,
        &CloseRequest {
            disposition: None,
            issued_by,
            timestamp: now_iso8601(),
        },
    )
    .map_err(map_store_error)?;

    let view = request_store::read_view(&root, &id).map_err(map_store_error)?;
    render(&view, Some(outcome.written))
}

// ===== wait =====

/// The four readiness predicates. Exactly one is required, enforced by
/// clap's argument group so the rejection is a usage error rather than
/// a runtime one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Predicate {
    /// That leg resolved.
    Leg(String),
    /// Every leg resolved.
    AllLegs,
    /// The request closed.
    Closed,
    /// At least N legs resolved. Satisfied by a partial fan-out.
    ResolvedCount(usize),
}

impl Predicate {
    fn from_flags(
        leg: Option<String>,
        all_legs: bool,
        closed: bool,
        resolved_count: Option<usize>,
    ) -> Result<Self, RequestError> {
        match (leg, all_legs, closed, resolved_count) {
            (Some(name), false, false, None) => Ok(Predicate::Leg(name)),
            (None, true, false, None) => Ok(Predicate::AllLegs),
            (None, false, true, None) => Ok(Predicate::Closed),
            (None, false, false, Some(n)) => Ok(Predicate::ResolvedCount(n)),
            // Unreachable through clap, which enforces exactly-one via
            // the argument group. Kept as a typed rejection rather
            // than a panic so a future caller constructing this by
            // hand gets an error instead of an abort.
            _ => Err(RequestError::new(
                RequestErrorCode::InvalidSubmission,
                "wait takes exactly one of --leg, --all-legs, --closed, --resolved-count",
            )),
        }
    }

    /// Whether the predicate holds on this view.
    fn satisfied(&self, view: &RequestView) -> Result<bool, RequestError> {
        let counts = view.leg_counts();
        Ok(match self {
            Predicate::Leg(name) => read_leg(view, name)?.disposition == LegDisposition::Resolved,
            Predicate::AllLegs => counts.resolved == counts.total,
            Predicate::Closed => view.request_state == RequestState::Closed,
            Predicate::ResolvedCount(n) => counts.resolved >= *n,
        })
    }

    /// Why the predicate can never hold from here, if it cannot.
    ///
    /// Evaluated after [`Predicate::satisfied`], so a predicate that
    /// is both satisfied and terminal reads as satisfied.
    fn impossible(&self, view: &RequestView) -> Option<String> {
        let counts = view.leg_counts();
        let closed = view.request_state == RequestState::Closed;
        match self {
            Predicate::Leg(name) => {
                let leg = view.legs.get(name.as_str())?;
                match leg.disposition {
                    LegDisposition::Abandoned => {
                        Some(format!("leg '{name}' was abandoned and can never resolve"))
                    }
                    LegDisposition::Open if closed => Some(format!(
                        "the request closed with leg '{name}' still unresolved"
                    )),
                    _ => None,
                }
            }
            Predicate::AllLegs => {
                if counts.abandoned > 0 {
                    Some(format!(
                        "{} of {} legs were abandoned, so every leg can never resolve",
                        counts.abandoned, counts.total
                    ))
                } else if closed && counts.open > 0 {
                    Some(format!(
                        "the request closed with {} legs still open",
                        counts.open
                    ))
                } else {
                    None
                }
            }
            // A request can always still be closed.
            Predicate::Closed => None,
            Predicate::ResolvedCount(n) => {
                // Only open legs can still resolve; abandoned ones
                // never will. This is also what rejects the
                // structurally impossible ask — more resolved legs
                // than the request has — before any polling.
                let reachable = counts.resolved + if closed { 0 } else { counts.open };
                if reachable < *n {
                    Some(format!(
                        "at most {reachable} of this request's {} legs can resolve, short of the \
                         {n} asked for",
                        counts.total
                    ))
                } else {
                    None
                }
            }
        }
    }
}

/// Poll the read path until the predicate holds, the deadline passes,
/// or the predicate becomes unreachable.
///
/// Writes nothing: this is [`request_store::read_view`] on an
/// interval, the same call `get` makes.
fn wait(
    id: &str,
    predicate: Predicate,
    timeout_secs: u64,
    interval_secs: Option<u64>,
) -> Result<String, RequestError> {
    let id = request_id(id)?;
    let root = koto_root()?;

    // Absolute and computed once, so a slow read cannot extend the
    // budget a caller asked for.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let interval = wait_interval(interval_secs);
    let interrupted = install_interrupt_flag();

    let mut first_poll = true;
    loop {
        let view = request_store::read_view(&root, &id).map_err(map_store_error)?;

        if predicate.satisfied(&view)? {
            return render(&view, None);
        }
        if let Some(reason) = predicate.impossible(&view) {
            // Structurally impossible on the first read is a caller
            // error the consumer could have avoided; impossible later
            // is news. Both are caller-class, with distinct codes so a
            // shell loop can tell "you asked for something that could
            // never hold" from "it stopped being reachable".
            let code = if first_poll {
                RequestErrorCode::PredicateImpossible
            } else {
                RequestErrorCode::PredicateBecameImpossible
            };
            return Err(RequestError::new(code, reason));
        }
        first_poll = false;

        let now = Instant::now();
        if now >= deadline {
            return Err(RequestError::new(
                RequestErrorCode::WaitTimeout,
                format!("the predicate was still unsatisfied after {timeout_secs}s"),
            ));
        }
        let next = (now + interval).min(deadline);
        sleep_until(next, &interrupted)?;
    }
}

/// Resolve `--interval-secs` against its default and its floor.
///
/// The floor is the whole point: an interval of zero would spin,
/// burning a core to observe a filesystem that changes on a human
/// timescale.
fn wait_interval(interval_secs: Option<u64>) -> Duration {
    let secs = interval_secs.unwrap_or(DEFAULT_WAIT_INTERVAL_SECS);
    Duration::from_secs(secs.max(MIN_WAIT_INTERVAL_SECS))
}

/// Sleep to `target` in slices, so a signal is noticed within one
/// slice rather than after a whole poll interval.
fn sleep_until(
    target: Instant,
    interrupted: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), RequestError> {
    use std::sync::atomic::Ordering;
    loop {
        if interrupted.load(Ordering::Relaxed) {
            return Err(RequestError::new(
                RequestErrorCode::WaitInterrupted,
                "the wait was interrupted by a signal",
            ));
        }
        let now = Instant::now();
        if now >= target {
            return Ok(());
        }
        std::thread::sleep(WAIT_SLICE.min(target - now));
    }
}

/// Register SIGINT and SIGTERM into a flag the sleep loop polls.
///
/// A failed registration is not fatal: the wait still honors its
/// deadline, it just stops noticing a signal early.
#[cfg(unix)]
fn install_interrupt_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let flag = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, Arc::clone(&flag)) {
            eprintln!("warning: could not install the wait's signal handler: {e}");
        }
    }
    flag
}

#[cfg(not(unix))]
fn install_interrupt_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- the contract pin --

    #[test]
    fn an_absent_pin_is_accepted() {
        assert!(validate_contract(None).is_ok());
    }

    #[test]
    fn the_current_contract_is_accepted() {
        assert!(validate_contract(Some("1.0")).is_ok());
    }

    #[test]
    fn a_different_major_is_a_caller_error() {
        let err = validate_contract(Some("2.0")).expect_err("a major bump must be refused");
        assert_eq!(err.code, RequestErrorCode::ContractMismatch);
        assert_eq!(err.code.exit_code(), 2);
    }

    #[test]
    fn a_newer_minor_is_refused_and_an_older_one_is_served() {
        assert!(
            validate_contract(Some("1.9")).is_err(),
            "a caller expecting fields this build does not emit must be refused"
        );
        // Nothing to assert for an older minor at 1.0 beyond the
        // current one; the rule is exercised by the newer-minor case
        // and by `the_current_contract_is_accepted`.
    }

    #[test]
    fn a_malformed_pin_is_a_contract_mismatch_not_a_panic() {
        for raw in ["1", "1.0.0", "x.y", "", "-1.0"] {
            let err = validate_contract(Some(raw)).expect_err("must be refused");
            assert_eq!(
                err.code,
                RequestErrorCode::ContractMismatch,
                "'{raw}' must reject as a contract mismatch"
            );
        }
    }

    #[test]
    fn the_contract_serializes_as_two_integers() {
        let json = serde_json::to_value(CliContract::current()).expect("serialize");
        assert_eq!(json["major"], 1);
        assert_eq!(json["minor"], 0);
        assert!(
            json["major"].is_number() && json["minor"].is_number(),
            "a string would let a consumer compare 1.10 against 1.9 lexicographically and be wrong"
        );
    }

    // -- the exit classes --

    #[test]
    fn every_code_lands_in_one_of_four_classes_and_avoids_sysexits() {
        let codes = [
            RequestErrorCode::RequestNotFound,
            RequestErrorCode::LegNotFound,
            RequestErrorCode::InvalidIdentifier,
            RequestErrorCode::InvalidSubmission,
            RequestErrorCode::ContractMismatch,
            RequestErrorCode::RequestClosed,
            RequestErrorCode::LegAlreadyResolved,
            RequestErrorCode::LegAbandoned,
            RequestErrorCode::LegBoundToDifferentChild,
            RequestErrorCode::ChildNotFound,
            RequestErrorCode::ChildNotFenceable,
            RequestErrorCode::ChildBoundToDifferentLeg,
            RequestErrorCode::ExplicitResolveOnBoundLeg,
            RequestErrorCode::RequestIdCollision,
            RequestErrorCode::IdempotencyConflict,
            RequestErrorCode::BoundExceeded,
            RequestErrorCode::EpochFenceViolation,
            RequestErrorCode::PredicateImpossible,
            RequestErrorCode::PredicateBecameImpossible,
            RequestErrorCode::WaitTimeout,
            RequestErrorCode::WaitInterrupted,
            RequestErrorCode::LockContention,
            RequestErrorCode::PersistenceError,
        ];
        for code in codes {
            let exit = code.exit_code();
            assert!(
                (1..=3).contains(&exit),
                "{code:?} exits {exit}, outside the 1..3 class space"
            );
            assert!(
                ![64, 65, 66, 75].contains(&exit),
                "{code:?} collides with a sysexits value used elsewhere in the crate"
            );
        }
    }

    #[test]
    fn the_transient_class_holds_exactly_the_retryable_codes() {
        assert_eq!(RequestErrorCode::WaitTimeout.exit_code(), 1);
        assert_eq!(RequestErrorCode::WaitInterrupted.exit_code(), 1);
        assert_eq!(RequestErrorCode::LockContention.exit_code(), 1);
    }

    #[test]
    fn a_persistence_failure_is_infrastructure_class() {
        assert_eq!(RequestErrorCode::PersistenceError.exit_code(), 3);
    }

    #[test]
    fn an_unsatisfiable_predicate_is_a_caller_error_not_a_transient_one() {
        // The whole point of Decision 5's correction: a shell loop
        // told to retry on the transient class must never be told to
        // retry a condition that can never become true.
        assert_eq!(RequestErrorCode::PredicateImpossible.exit_code(), 2);
        assert_eq!(RequestErrorCode::PredicateBecameImpossible.exit_code(), 2);
    }

    // -- store error mapping --

    #[test]
    fn store_rejections_land_in_the_caller_error_class() {
        let cases: Vec<(RequestStoreError, RequestErrorCode)> = vec![
            (
                RequestStoreError::NotFound {
                    request_id: "req-x".into(),
                },
                RequestErrorCode::RequestNotFound,
            ),
            (
                RequestStoreError::LegNotFound {
                    request_id: "req-x".into(),
                    leg_name: "a".into(),
                },
                RequestErrorCode::LegNotFound,
            ),
            (
                RequestStoreError::LegAlreadyResolved {
                    request_id: "req-x".into(),
                    leg_name: "a".into(),
                },
                RequestErrorCode::LegAlreadyResolved,
            ),
            (
                RequestStoreError::LegAbandoned {
                    request_id: "req-x".into(),
                    leg_name: "a".into(),
                },
                RequestErrorCode::LegAbandoned,
            ),
            (
                RequestStoreError::RequestClosed {
                    request_id: "req-x".into(),
                },
                RequestErrorCode::RequestClosed,
            ),
            (
                RequestStoreError::DuplicateLegName {
                    leg_name: "a".into(),
                },
                RequestErrorCode::InvalidSubmission,
            ),
        ];
        for (store_error, expected) in cases {
            let mapped = map_store_error(store_error);
            assert_eq!(mapped.code, expected);
            assert_eq!(mapped.code.exit_code(), 2);
        }
    }

    #[test]
    fn lock_contention_is_transient_and_io_is_infrastructure() {
        let contention = map_store_error(RequestStoreError::LockContention {
            request_id: "req-x".into(),
            waited_ms: 5000,
        });
        assert_eq!(contention.code.exit_code(), 1);

        let io = map_store_error(RequestStoreError::Io {
            context: "writing".into(),
            source: std::io::Error::other("disk"),
        });
        assert_eq!(io.code.exit_code(), 3);
    }

    #[test]
    fn a_bound_rejection_carries_the_dimension_that_rejected_it() {
        let mapped = map_store_error(RequestStoreError::BoundExceeded {
            dimension: "progress_appends_per_leg",
            limit: 256,
            observed: 257,
        });
        assert_eq!(mapped.code, RequestErrorCode::BoundExceeded);
        assert_eq!(mapped.details.len(), 1);
        assert_eq!(mapped.details[0].field, "progress_appends_per_leg");
    }

    // -- the error envelope shape --

    #[test]
    fn the_error_envelope_is_nested_not_flat() {
        let err = RequestError::new(
            RequestErrorCode::RequestNotFound,
            "request not found: req-x",
        );
        let json = serde_json::json!({ "error": err });
        assert!(
            json["error"].is_object(),
            "the flat legacy shape puts a string here; this group uses the nested one"
        );
        assert_eq!(json["error"]["code"], "request_not_found");
        assert_eq!(json["error"]["message"], "request not found: req-x");
        assert!(json["error"]["details"]
            .as_array()
            .expect("details")
            .is_empty());
    }

    // -- JSON flag guards --

    #[test]
    fn an_overlong_json_flag_is_rejected_before_parsing() {
        let raw = format!("\"{}\"", "a".repeat(MAX_JSON_FLAG_BYTES));
        let err = parse_json_flag("--with-data", &raw).expect_err("must be refused");
        assert_eq!(err.code, RequestErrorCode::BoundExceeded);
    }

    #[test]
    fn a_deeply_nested_json_flag_is_rejected() {
        let raw = format!(
            "{}{}",
            "[".repeat(MAX_JSON_FLAG_DEPTH + 1),
            "]".repeat(MAX_JSON_FLAG_DEPTH + 1)
        );
        let err = parse_json_flag("--with-data", &raw).expect_err("must be refused");
        // serde_json enforces its own recursion limit at the same
        // depth, so in practice the parser rejects first and the
        // failure reads as a malformed payload. Our walk stays as
        // defense in depth against a future parser whose limit moves;
        // what the caller is promised is the class, not which of the
        // two guards fired.
        assert!(
            matches!(
                err.code,
                RequestErrorCode::BoundExceeded | RequestErrorCode::InvalidSubmission
            ),
            "got {:?}",
            err.code
        );
        assert_eq!(err.code.exit_code(), 2);
    }

    #[test]
    fn a_non_object_progress_payload_is_rejected() {
        let err = parse_object_flag("--with-data", "[1,2,3]").expect_err("must be refused");
        assert_eq!(err.code, RequestErrorCode::InvalidSubmission);
    }

    // -- the fence --

    fn leg(bound_epoch: Option<u32>) -> LegView {
        let declaration = LegDeclaration {
            role: "reviewer".into(),
            template: "review".into(),
            inputs: serde_json::Value::Null,
        };
        let mut view = LegView {
            name: "reviewer-a".into(),
            declaration,
            disposition: LegDisposition::Open,
            bound_child: None,
            bound_epoch,
            result: None,
            result_source: None,
            abandoned_rationale: None,
            progress: Vec::new(),
        };
        if bound_epoch.is_some() {
            view.bound_child = Some("child-1".into());
        }
        view
    }

    #[test]
    fn an_unbound_leg_is_unfenceable_by_construction() {
        assert!(fence(&leg(None), None).is_ok());
        assert!(
            fence(&leg(None), Some(7)).is_ok(),
            "a wrapper that always passes the flag must keep working before the bind"
        );
    }

    #[test]
    fn a_matching_epoch_passes_and_a_stale_one_does_not() {
        assert!(fence(&leg(Some(3)), Some(3)).is_ok());
        let err = fence(&leg(Some(3)), Some(2)).expect_err("a stale epoch must be refused");
        assert_eq!(err.code, RequestErrorCode::EpochFenceViolation);
        assert_eq!(err.code.exit_code(), 2);
    }

    #[test]
    fn a_future_epoch_is_refused_too() {
        // Strict equality, matching the child-log fence: a spawner
        // that bakes the wrong value is caught in both directions.
        assert!(fence(&leg(Some(3)), Some(4)).is_err());
    }

    #[test]
    fn a_bound_leg_requires_the_flag() {
        let err = fence(&leg(Some(0)), None).expect_err("a bound leg must require the flag");
        assert_eq!(err.code, RequestErrorCode::EpochFenceViolation);
    }

    // -- predicate selection --

    #[test]
    fn exactly_one_predicate_is_accepted() {
        assert_eq!(
            Predicate::from_flags(Some("a".into()), false, false, None).expect("leg"),
            Predicate::Leg("a".into())
        );
        assert_eq!(
            Predicate::from_flags(None, true, false, None).expect("all"),
            Predicate::AllLegs
        );
        assert_eq!(
            Predicate::from_flags(None, false, true, None).expect("closed"),
            Predicate::Closed
        );
        assert_eq!(
            Predicate::from_flags(None, false, false, Some(2)).expect("count"),
            Predicate::ResolvedCount(2)
        );
        assert!(Predicate::from_flags(None, false, false, None).is_err());
        assert!(Predicate::from_flags(None, true, true, None).is_err());
    }

    #[test]
    fn the_interval_floor_forbids_a_spin() {
        assert_eq!(
            wait_interval(Some(0)),
            Duration::from_secs(MIN_WAIT_INTERVAL_SECS),
            "an interval of zero must be clamped, not honored: it would spin"
        );
        assert_eq!(
            wait_interval(None),
            Duration::from_secs(DEFAULT_WAIT_INTERVAL_SECS)
        );
    }
}
