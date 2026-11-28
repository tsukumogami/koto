//! The cloud backend's half of `koto init --supersede`
//! (`src/cli/init_supersede.rs`): the predecessor's state pulled from its
//! own workspace prefix, the fence pushed back conditional on that copy,
//! and the carried keys read from the same prefix.
//!
//! The close `koto init --replaces` and `koto session replace` make
//! (`src/cli/init_replace.rs`) goes the same way: every line it owes the
//! bucket's copy (the stop's events, the cancel and the replaced fence) is
//! worked out against the pulled bytes by the writer the local backend
//! uses, and goes up in the one conditional PUT, with the same rules.
//!
//! The fence is the one write this makes under another workspace's prefix,
//! and it is a PUT with `If-Match` on the ETag the pull returned, as every
//! state push has been since koto#480. Anything but a confirmed PUT (a 412,
//! a bucket that doesn't implement conditional writes, a copy with no ETag,
//! a failed request) is [`CloudFence::Unavailable`] with nothing written:
//! there is no unconditional fallback here, unlike an ordinary push.
//!
//! Every read here, the pull and each carried key, is one GET asked again
//! while it fails with a transient error, as a listing is; one that still
//! fails is a [`BucketReadFailed`] that keeps the bucket's error for the
//! caller to class.
//!
//! Everything read from the bucket is data. The prefix is checked to be a
//! workspace prefix and every name is validated before a key is built from
//! it; the predecessor's log is parsed whole, a carried key is checked as
//! `koto context add` checks one, and a carried value is checked against
//! the hash the predecessor's manifest gives it before it is returned.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::{
    asked_retrying, each_in_parallel, is_not_found, is_precondition_failed, is_transient,
    is_workspace_prefix, parse_source_log, response_etag, without_url_userinfo, CloudBackend,
    S3Error,
};
use crate::cache::sha256_hex;
use crate::engine::persistence::{
    append_supersede_fence, close_payloads, refuse_if_rebound, replace_log_bytes, superseded_by,
    with_log_replace_lock, Close, CloseOutcome, CommitInfo, SessionSupersededError, SupersedeFence,
};
use crate::engine::types::{Event, EventPayload, StateFileHeader};
use crate::session::context::Manifest;
use crate::session::links::is_reserved_key;
use crate::session::rebind_fence::SessionRebound;
use crate::session::remote_feed::{failure_class, FailureClass};
use crate::session::state_log::{self, LogOrder};
use crate::session::validate::{validate_context_key, validate_session_id};
use crate::session::{state_file_name, SessionBackend};

/// How long the fence waits for a local copy's append lock when it adds the
/// fence there too. The bucket's fence already holds; this only makes the
/// local copy refuse at once rather than at its next push.
const LOCAL_FENCE_WAIT: Duration = Duration::from_secs(2);

/// A read of `key` from the bucket that failed for good, after
/// [`CloudBackend::get_with_etag`]'s retries, with the last ask's error kept
/// whole so a caller can class it, in the [`super::ListingFailed`] style.
/// Its message is the one the read always gave.
#[derive(Debug)]
pub(crate) struct BucketReadFailed {
    /// The object's key.
    pub key: String,
    /// The last ask's error. A response the bucket answered with a status
    /// other than 200 and no error is held as
    /// [`S3Error::HttpFailWithBody`] with that status and its body.
    pub source: S3Error,
    /// How long the last ask took, so a transport failure that ran the
    /// full request timeout can be told from one that failed at once.
    pub took: Duration,
}

impl BucketReadFailed {
    /// The read's failure class, as [`failure_class`] gives it, except that
    /// a transport failure (no HTTP answer) that took the cloud backend's
    /// full [`super::REQUEST_TIMEOUT`] is `timeout`, whatever error the
    /// connection's end gave: 401 and 403 are `denied`, 429 and 503
    /// `throttled`, and any other failure `unreachable`.
    pub(crate) fn class(&self) -> FailureClass {
        match failure_class(&self.source) {
            FailureClass::Unreachable
                if self.took >= super::REQUEST_TIMEOUT
                    && matches!(self.source, S3Error::Atto(_) | S3Error::Io(_)) =>
            {
                FailureClass::Timeout
            }
            class => class,
        }
    }
}

impl std::fmt::Display for BucketReadFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key = &self.key;
        match &self.source {
            // rust-s3 fails every status outside 2xx itself, so a 2xx here
            // is a response that came back without an error but wasn't a 200.
            S3Error::HttpFailWithBody(status, _) if (200..300).contains(status) => {
                write!(f, "the bucket couldn't be read at {key}: status {status}")
            }
            e => write!(
                f,
                "the bucket couldn't be read at {key}: {}",
                without_url_userinfo(&e.to_string())
            ),
        }
    }
}

impl std::error::Error for BucketReadFailed {}

/// Why the predecessor's side of a carry can't be trusted: what the bucket
/// holds for it is missing or doesn't check out. Each renders as fixed
/// wording, so no text read from the bucket (a parser's message, a key
/// name, a hash) reaches the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CarrySourceError {
    /// The predecessor's context manifest isn't one.
    ManifestUnparsable,
    /// The bucket holds no context manifest for a predecessor whose log
    /// records a key the carry would take.
    ManifestMissing,
    /// The manifest names a carried key `koto context add` would refuse.
    BadKey,
    /// The manifest gives a carried key a hash that isn't 64 lowercase hex
    /// digits.
    BadHash,
    /// The bucket holds no object for a key the manifest names.
    ObjectMissing,
    /// A carried value read twice didn't match its manifest hash either
    /// time.
    HashMismatch,
}

impl std::fmt::Display for CarrySourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ManifestUnparsable => {
                "the predecessor's context manifest in the bucket doesn't parse"
            }
            Self::ManifestMissing => {
                "the bucket holds no context manifest for the predecessor, though its log records \
                 keys to carry"
            }
            Self::BadKey => "the predecessor's context manifest names a key koto refuses",
            Self::BadHash => "the predecessor's context manifest gives a key a malformed hash",
            Self::ObjectMissing => {
                "the bucket holds no object for a key the predecessor's context manifest names"
            }
            Self::HashMismatch => {
                "a carried value in the bucket doesn't match the hash the predecessor's context \
                 manifest gives it"
            }
        })
    }
}

impl std::error::Error for CarrySourceError {}

/// One key a carry copies from the predecessor, with the sha256 its manifest
/// gives the key's value. Only [`CloudBackend::carry_plan`] builds one, after
/// the key passed [`validate_context_key`] and the hash its shape check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CarryEntry {
    key: String,
    hash: String,
}

impl CarryEntry {
    /// The key's name, validated.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

/// Whether `hash` is a sha256 in the form the context store writes: 64
/// lowercase hex digits.
fn is_sha256_hex(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a workspace prefix or a predecessor id no key may be built from.
fn check_carry_source(prefix: &str, pred: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        is_workspace_prefix(prefix),
        "'{prefix}' isn't a workspace prefix"
    );
    validate_session_id(pred)?;
    Ok(())
}

/// A predecessor's state file as the bucket holds it, pulled for the fence.
pub(crate) struct PulledState {
    /// The workspace prefix it was read from.
    pub prefix: String,
    key: String,
    bytes: Vec<u8>,
    etag: String,
    /// Its header, parsed.
    pub header: StateFileHeader,
    /// Its events, parsed.
    pub events: Vec<Event>,
    last_seq: u64,
}

impl PulledState {
    /// The fence this copy already holds, with the names in it validated
    /// (the successor's name and session id come from the bucket).
    pub fn fence(&self) -> anyhow::Result<Option<SupersedeFence>> {
        let Some(fence) = superseded_by(&self.events) else {
            return Ok(None);
        };
        check_fence_names(&fence)?;
        Ok(Some(fence))
    }
}

/// A fence read from a log or the bucket names a session koto will look
/// up, print and walk to: its name must be one koto accepts, and its
/// session id the hex-and-hyphen shape koto mints
/// ([`crate::engine::types::generate_session_id`]).
pub(crate) fn check_fence_names(fence: &SupersedeFence) -> anyhow::Result<()> {
    crate::discover::validate_workflow_name(&fence.by)
        .map_err(|e| anyhow::anyhow!("its fence names an invalid successor: {e}"))?;
    let id = &fence.by_session_id;
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "its fence names an invalid successor session id"
    );
    Ok(())
}

/// What [`CloudBackend::push_fence`] did.
pub(crate) enum CloudFence {
    /// The fenced copy is in the bucket.
    Fenced,
    /// The push was refused and the bucket's copy now holds a fence, this
    /// one (an earlier run's PUT whose answer was lost) or a rival's.
    Taken(SupersedeFence),
    /// Nothing was written; the reason says why.
    Unavailable(String),
}

/// What [`CloudBackend::push_close`] did.
pub(crate) enum CloudClose {
    /// The bucket's copy holds every line of the close. `wrote` is false
    /// when it, and this host's copy, held them all already.
    Closed { wrote: bool },
    /// The bucket's copy holds a fence other than this close's own replaced
    /// fence. Nothing was written.
    FencedByOther(SupersedeFence),
    /// The bucket's copy records a rebind that fences this process off, as
    /// [`crate::engine::persistence::append_close`] refuses one in a local
    /// log. Nothing was written.
    Rebound(SessionRebound),
    /// Nothing was written; the reason says why.
    Unavailable(String),
}

/// One line a conditional push adds to a pulled copy: the event, and the
/// line as it went up.
type AddedLine = (Event, String);

impl CloudBackend {
    /// Pull `pred`'s state file from workspace prefix `prefix`: `Ok(None)`
    /// when the bucket holds none there, `Err` when it can't be read or
    /// the log doesn't parse.
    pub(crate) fn pull_for_fence(
        &self,
        prefix: &str,
        pred: &str,
    ) -> anyhow::Result<Option<PulledState>> {
        anyhow::ensure!(
            is_workspace_prefix(prefix),
            "'{prefix}' isn't a workspace prefix"
        );
        validate_session_id(pred)?;
        let key = format!("{prefix}/{pred}/{}", state_file_name(pred));
        let Some((bytes, etag)) = self.get_with_etag(&key)? else {
            return Ok(None);
        };
        let log = parse_source_log(&bytes)
            .map_err(|reason| anyhow::anyhow!("the bucket's log of '{pred}' {reason}"))?;
        let events = log
            .event_lines
            .iter()
            .map(|line| serde_json::from_str::<Event>(line))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("the bucket's log of '{pred}' doesn't parse: {e}"))?;
        Ok(Some(PulledState {
            prefix: prefix.to_string(),
            key,
            bytes,
            etag,
            header: log.header,
            events,
            last_seq: log.last_seq,
        }))
    }

    /// GET `key` with its ETag; `None` when the object isn't there.
    ///
    /// The GET is asked again as a listing is ([`asked_retrying`]) while it
    /// fails with a [transient](is_transient) error, so a throttle or a
    /// dropped connection costs a wait rather than the read. A response
    /// other than a 200 counts as a failure of its status, so a 5xx the
    /// bucket answered without an error is asked again too. A 404 or a 403
    /// is asked once. A read that still fails is a [`BucketReadFailed`]
    /// holding the last ask's error and how long that ask took.
    fn get_with_etag(&self, key: &str) -> anyhow::Result<Option<(Vec<u8>, String)>> {
        let fetched = asked_retrying(
            || {
                let started = Instant::now();
                match self.bucket.get_object(key) {
                    Ok(response) if response.status_code() == 200 => Ok(response),
                    Ok(response) => Err((
                        S3Error::HttpFailWithBody(
                            response.status_code(),
                            String::from_utf8_lossy(response.bytes()).into_owned(),
                        ),
                        started.elapsed(),
                    )),
                    Err(e) => Err((e, started.elapsed())),
                }
            },
            is_transient,
        );
        match fetched {
            Ok(response) => Ok(Some((
                response.bytes().to_vec(),
                response_etag(&response).unwrap_or_default(),
            ))),
            Err((e, _)) if is_not_found(&e) => Ok(None),
            Err((source, took)) => Err(BucketReadFailed {
                key: key.to_string(),
                source,
                took,
            }
            .into()),
        }
    }

    /// Append `fence` to the pulled copy and PUT it back with `If-Match` on
    /// the copy's ETag. On a 412 the copy is read again: a fence there now
    /// is [`CloudFence::Taken`]; anything else, and every other failure, is
    /// [`CloudFence::Unavailable`]. Nothing is ever PUT without the
    /// condition.
    pub(crate) fn push_fence(&self, pulled: &PulledState, fence: &SupersedeFence) -> CloudFence {
        self.push_lines(
            pulled,
            &[fence.to_payload()],
            &fence.at,
            |fenced, _, etag| self.adopt_fenced_copy(pulled, fence, fenced, etag),
        )
    }

    /// Append one line per payload to the pulled copy, with consecutive
    /// seqs from the copy's last, each stamped `at`, and PUT it back in one
    /// request with `If-Match` on the copy's ETag, as
    /// [`CloudBackend::push_fence`] describes. Once the bucket confirms the
    /// PUT, `adopt` runs with the bytes put, the lines added and the new
    /// ETag. Nothing is ever PUT without the condition.
    fn push_lines(
        &self,
        pulled: &PulledState,
        payloads: &[EventPayload],
        at: &str,
        adopt: impl FnOnce(&[u8], &[AddedLine], Option<String>),
    ) -> CloudFence {
        if let Err(e) = self.refuse_if_read_only(&format!("S3 PUT of {}", pulled.key)) {
            return CloudFence::Unavailable(format!("{e:#}"));
        }
        if pulled.etag.is_empty() {
            return CloudFence::Unavailable(
                "the bucket gave no ETag for the predecessor's state, so the fence can't be \
                 written conditionally"
                    .to_string(),
            );
        }
        let mut fenced = pulled.bytes.clone();
        if !fenced.ends_with(b"\n") {
            fenced.push(b'\n');
        }
        let mut added: Vec<AddedLine> = Vec::with_capacity(payloads.len());
        for (i, payload) in payloads.iter().enumerate() {
            let event = Event {
                seq: pulled.last_seq + 1 + i as u64,
                timestamp: at.to_string(),
                event_type: payload.type_name().to_string(),
                payload: payload.clone(),
                idempotency_hash: None,
            };
            let line = match serde_json::to_string(&event) {
                Ok(line) => line,
                Err(e) => {
                    return CloudFence::Unavailable(format!("the fence doesn't serialize: {e}"))
                }
            };
            fenced.extend_from_slice(line.as_bytes());
            fenced.push(b'\n');
            added.push((event, line));
        }

        match self.put_state_if(&pulled.key, &fenced, Some(&pulled.etag)) {
            Ok(response) => {
                adopt(&fenced, &added, response_etag(&response));
                CloudFence::Fenced
            }
            Err(e) if is_precondition_failed(&e) => {
                let pred = pred_of(&pulled.key);
                match self.pull_for_fence(&pulled.prefix, pred) {
                    Ok(Some(now)) => match now.fence() {
                        Ok(Some(existing)) => CloudFence::Taken(existing),
                        Ok(None) => CloudFence::Unavailable(
                            "the predecessor's state changed in the bucket under the fence's \
                             conditional push"
                                .to_string(),
                        ),
                        Err(e) => CloudFence::Unavailable(format!("{e:#}")),
                    },
                    Ok(None) => CloudFence::Unavailable(
                        "the predecessor's state left the bucket under the fence's conditional \
                         push"
                            .to_string(),
                    ),
                    Err(e) => CloudFence::Unavailable(format!("{e:#}")),
                }
            }
            Err(S3Error::HttpFailWithBody(501, _)) => CloudFence::Unavailable(
                "the bucket doesn't support conditional writes, and the fence is never written \
                 without one"
                    .to_string(),
            ),
            Err(e) => CloudFence::Unavailable(format!(
                "the fence's conditional push failed: {}",
                without_url_userinfo(&e.to_string())
            )),
        }
    }

    /// After the fence is in the bucket, make this host's copy of the
    /// predecessor, when it has one homed at the same prefix, refuse writes
    /// at once: a copy the fenced one extends is replaced by it and
    /// stamped, as a pull would; a copy holding events the bucket lacks
    /// gets the fence appended locally, so its next push conflicts and is
    /// refused `session_superseded`. Best-effort: the bucket's fence holds
    /// either way.
    fn adopt_fenced_copy(
        &self,
        pulled: &PulledState,
        fence: &SupersedeFence,
        fenced: &[u8],
        etag: Option<String>,
    ) {
        let pred = pred_of(&pulled.key);
        let Some(local) = self.read_local_state_bytes(pred) else {
            return;
        };
        if !self
            .home_prefix(pred)
            .is_ok_and(|h| h.prefix == pulled.prefix)
        {
            return;
        }
        let path = self.local.session_dir(pred).join(state_file_name(pred));
        self.forget_probe(pred);
        match state_log::compare(&local, &pulled.bytes) {
            LogOrder::Same | LogOrder::RemoteExtends | LogOrder::HeaderOnly => {
                if self.write_local_state_bytes(pred, fenced).is_ok() {
                    self.record_synced(pred, etag.as_deref(), fenced, false);
                }
            }
            LogOrder::LocalExtends | LogOrder::Diverged => {
                let _ = append_supersede_fence(&path, fence, &fence.at, LOCAL_FENCE_WAIT);
            }
            LogOrder::Unrelated => {}
        }
    }

    /// Close the replaced run whose bucket copy is `pulled`: the lines
    /// [`close_payloads`] owes that copy, worked out against its own bytes
    /// (the side events, the cancel unless it already stands cancelled, the
    /// replaced fence unless it holds it), go up in one PUT with `If-Match`
    /// on its ETag, then this host's copy takes the same close. A copy
    /// holding another fence is [`CloudClose::FencedByOther`]; a PUT the
    /// bucket refuses, or anything else that isn't a confirmed conditional
    /// PUT, is [`CloudClose::Unavailable`], with nothing written in either
    /// copy. There is no unconditional fallback.
    pub(crate) fn push_close(&self, pulled: &PulledState, close: &Close, at: &str) -> CloudClose {
        let fence = SupersedeFence {
            replaced: true,
            ..close.fence.clone()
        };
        let ours = |f: &SupersedeFence| {
            f.replaced && f.by == fence.by && f.by_session_id == fence.by_session_id
        };
        match pulled.fence() {
            Ok(Some(existing)) if !ours(&existing) => return CloudClose::FencedByOther(existing),
            Ok(_) => {}
            Err(e) => return CloudClose::Unavailable(format!("{e:#}")),
        }
        let content = String::from_utf8_lossy(&pulled.bytes);
        // `append_close`'s rebind rule, asked of the bucket's header: a
        // rebind there since this process first read this host's copy (the
        // epoch `note_epoch_read` recorded under its local path) refuses the
        // close before anything is built, so a run another host rebound in
        // between is never closed. A race after this pull loses the PUT.
        let run = pred_of(&pulled.key);
        let path = self.local.session_dir(run).join(state_file_name(run));
        if let Err(e) = refuse_if_rebound(&path, &content, &fence.to_payload()) {
            return match e.downcast::<SessionRebound>() {
                Ok(rebound) => CloudClose::Rebound(rebound),
                Err(e) => CloudClose::Unavailable(format!("{e:#}")),
            };
        }
        let payloads = match close_payloads(&content, close, &fence) {
            Ok(payloads) => payloads,
            Err(e) => {
                return CloudClose::Unavailable(format!(
                    "the close can't be worked out from the bucket's copy: {e:#}"
                ))
            }
        };
        if payloads.is_empty() {
            // The bucket's copy is closed already; this host's may not be.
            let etag = Some(pulled.etag.clone());
            let wrote = self.adopt_closed_copy(pulled, close, at, &pulled.bytes, &[], etag);
            return CloudClose::Closed { wrote };
        }
        let pushed = self.push_lines(pulled, &payloads, at, |closed, added, etag| {
            self.adopt_closed_copy(pulled, close, at, closed, added, etag);
        });
        match pushed {
            CloudFence::Fenced => CloudClose::Closed { wrote: true },
            // This close's own fence, put there since the pull: the lines
            // this push owed were worked out against a copy that is gone.
            CloudFence::Taken(existing) if ours(&existing) => CloudClose::Unavailable(
                "the run's state changed in the bucket under the close's conditional push"
                    .to_string(),
            ),
            CloudFence::Taken(existing) => CloudClose::FencedByOther(existing),
            CloudFence::Unavailable(reason) => CloudClose::Unavailable(reason),
        }
    }

    /// After a close is in the bucket, close this host's copy of the run
    /// too, when it has one homed at the same prefix. A copy the closed one
    /// extends is replaced by it and stamped, as a pull would, under the
    /// log's append lock, and the after-commit steps run for each line the
    /// push added. A copy holding events the bucket's lacks gets the close
    /// appended by [`crate::session::local::LocalBackend::append_close`],
    /// worked out against its own bytes, so it too stands cancelled and
    /// fenced. Best-effort: the bucket's close holds either way. Returns
    /// whether this host's copy was written.
    fn adopt_closed_copy(
        &self,
        pulled: &PulledState,
        close: &Close,
        at: &str,
        closed: &[u8],
        added: &[AddedLine],
        etag: Option<String>,
    ) -> bool {
        let run = pred_of(&pulled.key);
        if self.read_local_state_bytes(run).is_none()
            || !self
                .home_prefix(run)
                .is_ok_and(|h| h.prefix == pulled.prefix)
        {
            return false;
        }
        let path = self.local.state_path(run);
        self.forget_probe(run);
        // `Some(wrote)` when the copy was settled under the lock, `None`
        // when it holds events the bucket's lacks.
        let replaced = with_log_replace_lock(&path, LOCAL_FENCE_WAIT, || {
            let local = std::fs::read(&path)?;
            Ok(match state_log::compare(&local, &pulled.bytes) {
                LogOrder::Same | LogOrder::RemoteExtends | LogOrder::HeaderOnly => {
                    let differs = local.as_slice() != closed;
                    if differs {
                        replace_log_bytes(&path, closed)?;
                    }
                    Some(differs)
                }
                LogOrder::LocalExtends | LogOrder::Diverged => None,
                LogOrder::Unrelated => Some(false),
            })
        });
        match replaced {
            Ok(Some(true)) => {
                self.record_synced(run, etag.as_deref(), closed, false);
                let mut prior_log = String::from_utf8_lossy(&pulled.bytes).into_owned();
                if !prior_log.ends_with('\n') {
                    prior_log.push('\n');
                }
                for (event, line) in added {
                    let next = format!("{prior_log}{line}\n");
                    let commit = CommitInfo {
                        seq: event.seq,
                        prior_log,
                    };
                    self.local.after_commit(run, &event.payload, &commit);
                    prior_log = next;
                }
                true
            }
            Ok(Some(false)) | Err(_) => false,
            Ok(None) => matches!(
                self.local.append_close(run, close, at, LOCAL_FENCE_WAIT),
                Ok(CloseOutcome::Closed { commits, torn_backup })
                    if !commits.is_empty() || torn_backup.is_some()
            ),
        }
    }

    /// The keys a carry copies from `pred` under workspace prefix `prefix`,
    /// sorted, each with its manifest hash: one GET of `pred`'s context
    /// manifest, filtered as a sync filters it (host-local keys and pin
    /// records out), then to the keys under each of `prefixes`, reserved
    /// keys left out.
    ///
    /// A manifest the bucket doesn't hold is an empty plan, unless
    /// `log_names_carried` says `pred`'s log records a key the carry would
    /// take; then it is [`CarrySourceError::ManifestMissing`]. A manifest
    /// that doesn't parse, names a key `koto context add` would refuse, or
    /// gives a key a malformed hash is a [`CarrySourceError`], and a read
    /// that fails is the [`BucketReadFailed`] from
    /// [`CloudBackend::get_with_etag`].
    pub(crate) fn carry_plan(
        &self,
        prefix: &str,
        pred: &str,
        prefixes: &[String],
        log_names_carried: bool,
    ) -> anyhow::Result<Vec<CarryEntry>> {
        check_carry_source(prefix, pred)?;
        let Some((bytes, _)) = self.get_with_etag(&format!("{prefix}/{pred}/ctx/manifest.json"))?
        else {
            if log_names_carried {
                return Err(CarrySourceError::ManifestMissing.into());
            }
            return Ok(Vec::new());
        };
        let mut manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| CarrySourceError::ManifestUnparsable)?;
        manifest.drop_host_local();
        manifest.drop_pin_records();
        let mut plan = Vec::new();
        // A manifest's keys are a sorted map, so the plan comes out sorted.
        for (key, meta) in manifest.keys {
            if !prefixes.iter().any(|p| key.starts_with(p.as_str())) || is_reserved_key(&key) {
                continue;
            }
            validate_context_key(&key).map_err(|_| CarrySourceError::BadKey)?;
            if !is_sha256_hex(&meta.hash) {
                return Err(CarrySourceError::BadHash.into());
            }
            plan.push(CarryEntry {
                key,
                hash: meta.hash,
            });
        }
        Ok(plan)
    }

    /// The keys of `entries` this host's local store holds in `succ` with
    /// the entry's hash, read from its manifest once. Neither the bucket
    /// nor `succ`'s manifest there is read: a key counts as held only when
    /// this host wrote it or already holds it with that hash. A local
    /// manifest that can't be read holds nothing.
    pub(crate) fn held_locally<'e>(
        &self,
        succ: &str,
        entries: &'e [CarryEntry],
    ) -> BTreeSet<&'e str> {
        let Ok(local) = self.local.read_manifest(succ) else {
            return BTreeSet::new();
        };
        entries
            .iter()
            .filter(|e| local.keys.get(&e.key).is_some_and(|m| m.hash == e.hash))
            .map(|e| e.key.as_str())
            .collect()
    }

    /// Read one chunk of plan entries (at most [`super::FLUSH_PARALLELISM`])
    /// from `pred` under workspace prefix `prefix`, each GET on its own
    /// scoped thread, and return each entry's value in entry order. The
    /// threads only read.
    ///
    /// Each value is checked against its entry's hash before it is
    /// returned; one that differs is read once more, in case the first read
    /// caught the object mid-replace, and then is
    /// [`CarrySourceError::HashMismatch`]. An object the bucket doesn't hold
    /// is [`CarrySourceError::ObjectMissing`], read once; a read that fails
    /// is the [`BucketReadFailed`] from [`CloudBackend::get_with_etag`].
    pub(crate) fn fetch_carried(
        &self,
        prefix: &str,
        pred: &str,
        entries: &[CarryEntry],
    ) -> Vec<anyhow::Result<Vec<u8>>> {
        if let Err(e) = check_carry_source(prefix, pred) {
            let reason = format!("{e:#}");
            return entries
                .iter()
                .map(|_| Err(anyhow::anyhow!("{reason}")))
                .collect();
        }
        each_in_parallel(entries, |entry| {
            let key = format!("{prefix}/{pred}/ctx/{}", entry.key);
            for _ in 0..2 {
                let (value, _) = self
                    .get_with_etag(&key)?
                    .ok_or(CarrySourceError::ObjectMissing)?;
                if sha256_hex(&value) == entry.hash {
                    return Ok(value);
                }
            }
            Err(CarrySourceError::HashMismatch.into())
        })
    }

    /// Hold `session`'s writes in this process for one push, as a write
    /// window does, until the guard drops; [`CloudBackend::flush_window`]
    /// then pushes them together.
    pub(crate) fn defer_writes_to(&self, session: &str) -> DeferGuard<'_> {
        *self
            .probes
            .deferred_to
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(session.to_string());
        DeferGuard { backend: self }
    }

    /// Whether [`CloudBackend::defer_writes_to`] holds `session`'s writes.
    pub(super) fn defers_by_guard(&self, session: &str) -> bool {
        self.probes
            .deferred_to
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(session)
    }

    /// Push `session`'s writes a carry interrupted before its push, so a
    /// re-run starts from what the bucket holds.
    pub(crate) fn flush_interrupted_batch(&self, session: &str) -> anyhow::Result<()> {
        if self.journal(session).is_open() {
            self.flush_batch(session)?;
        }
        Ok(())
    }

    /// Refuse a write to `session` when this host's copy of it holds a
    /// supersede fence. Every write command checks the fence before it
    /// writes, but under the cloud backend that check reads the local copy
    /// before the command pulls; this runs after the pull, before the
    /// store or the bucket is written.
    pub(super) fn refuse_if_superseded(&self, session: &str) -> anyhow::Result<()> {
        let path = self
            .local
            .session_dir(session)
            .join(state_file_name(session));
        match crate::engine::persistence::read_fence(&path)? {
            Some(fence) => Err(SessionSupersededError {
                session: session.to_string(),
                fence,
            }
            .into()),
            None => Ok(()),
        }
    }

    /// The highest decision id in the bucket's copy of `pred` under
    /// workspace prefix `prefix`, from listings of its decision areas
    /// (keys only, no GET), or `None` when it holds none. The fence takes
    /// it before its conditional push, since no lock spans this host and
    /// the predecessor's; a decision written after the listing reads as
    /// written after the fence, which errs toward the fence. `Err` when a
    /// listing fails or an area holds more than a session may.
    pub(crate) fn decisions_through_at(
        &self,
        prefix: &str,
        pred: &str,
    ) -> anyhow::Result<Option<String>> {
        check_carry_source(prefix, pred)?;
        let ctx = format!("{prefix}/{pred}/ctx/");
        let mut keys = Vec::new();
        for area in crate::decisions::AREAS {
            let pages = self
                .bucket
                .list(format!("{ctx}{area}/"), None)
                .map_err(|e| anyhow::anyhow!("{}", without_url_userinfo(&e.to_string())))?;
            let before = keys.len();
            for obj in pages.iter().flat_map(|p| p.contents.iter()) {
                if let Some(key) = obj.key.strip_prefix(&ctx) {
                    keys.push(key.to_string());
                }
            }
            anyhow::ensure!(
                keys.len() - before <= crate::decisions::MAX_AREA_ENTRIES,
                "the bucket's {area}/ holds too many objects"
            );
        }
        Ok(crate::decisions::marker::decisions_through_keys(
            keys.iter().map(String::as_str),
        ))
    }

    /// The bucket key of `pred`'s supersede marker under workspace prefix
    /// `prefix`: beside its state object, under its `ctx/`.
    fn supersede_marker_key(&self, prefix: &str, pred: &str) -> anyhow::Result<String> {
        check_carry_source(prefix, pred)?;
        Ok(format!(
            "{prefix}/{pred}/ctx/{}",
            crate::decisions::marker::SUPERSEDED_BY_KEY
        ))
    }

    /// The bytes of `pred`'s supersede marker in the bucket under `prefix`,
    /// or `None` when there is none.
    pub(crate) fn supersede_marker_at(
        &self,
        prefix: &str,
        pred: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let key = self.supersede_marker_key(prefix, pred)?;
        Ok(self.get_with_etag(&key)?.map(|(bytes, _)| bytes))
    }

    /// PUT `pred`'s supersede marker under `prefix` create-only, with
    /// `If-None-Match: *` and never without it: `Ok(true)` when this call
    /// wrote it, `Ok(false)` when the bucket already held one (a 412), which
    /// is left as it is. The fence's own push was conditional, so the bucket
    /// is known to honour a condition before this runs.
    pub(crate) fn put_supersede_marker(
        &self,
        prefix: &str,
        pred: &str,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        let key = self.supersede_marker_key(prefix, pred)?;
        self.refuse_if_read_only(&format!("S3 PUT of {key}"))?;
        match self.put_state_if(&key, bytes, None) {
            Ok(r) if (200..300).contains(&r.status_code()) => Ok(true),
            Ok(r) => anyhow::bail!("PUT {key} answered {}", r.status_code()),
            Err(e) if is_precondition_failed(&e) => Ok(false),
            Err(S3Error::HttpFailWithBody(501, _)) => anyhow::bail!(
                "the bucket doesn't support conditional writes, and the supersede marker is \
                 never written without one"
            ),
            Err(e) => anyhow::bail!("PUT {key} failed: {}", without_url_userinfo(&e.to_string())),
        }
    }

    /// Publish `marker` in this host's copy of `pred` too, create-only, when
    /// it has one homed at `prefix`, so a local read finds it before the
    /// next pull. Best-effort, as [`Self::adopt_fenced_copy`] is: the
    /// bucket's marker holds either way, and a pull brings it down without
    /// replacing what is here.
    pub(crate) fn copy_supersede_marker_locally(
        &self,
        prefix: &str,
        pred: &str,
        marker: &crate::decisions::marker::SupersedeMarker,
    ) {
        if self.read_local_state_bytes(pred).is_none()
            || !self.home_prefix(pred).is_ok_and(|h| h.prefix == prefix)
        {
            return;
        }
        let _ = crate::decisions::marker::write_local(
            &self.local.session_dir(pred),
            &self.local.ctx_path(pred),
            marker,
        );
    }
}

/// Drops [`CloudBackend::defer_writes_to`]'s hold.
pub(crate) struct DeferGuard<'a> {
    backend: &'a CloudBackend,
}

impl Drop for DeferGuard<'_> {
    fn drop(&mut self) {
        *self
            .backend
            .probes
            .deferred_to
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The session name in a state key `<prefix>/<name>/<file>`.
fn pred_of(key: &str) -> &str {
    key.split('/').nth(1).unwrap_or_default()
}

/// A refused push of `id`'s state whose bucket copy holds a supersede
/// fence the pushed data lacks: the session was superseded on another host
/// (or by a command that left this copy behind), so the write is refused as
/// `session_superseded` rather than as a conflict to resolve.
pub(super) fn superseded_refusal(id: &str, data: &[u8], remote: &[u8]) -> Option<anyhow::Error> {
    let fence_in = |bytes: &[u8]| {
        crate::engine::persistence::superseded_by_in_log(&String::from_utf8_lossy(bytes))
    };
    if fence_in(data).is_some() {
        return None;
    }
    let fence = fence_in(remote)?;
    Some(
        SessionSupersededError {
            session: id.to_string(),
            fence,
        }
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::super::test_support::{cloud_backend_at, PREFIX};
    use super::*;

    const KEY: &str = "7e57000000000000/p/ctx/record/a";

    /// An endpoint on a `127.0.0.1` port that answers its `n`th request
    /// with `statuses[n]` and an empty body, and every request after the
    /// script with a 200 carrying `b"alpha"` and an ETag. It returns the URL
    /// and a count of the requests it has answered.
    ///
    /// rust-s3 asks a failed request once more on its own, a second later,
    /// so one ask that fails is two requests here.
    fn scripted(statuses: &[&'static str]) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let answered = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&answered);
        let statuses = statuses.to_vec();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let n = counted.fetch_add(1, Ordering::SeqCst);
                let (status, extra, body): (&str, &str, &[u8]) = match statuses.get(n) {
                    Some(status) => (status, "", b""),
                    None => ("200 OK", "ETag: \"e1\"\r\n", b"alpha"),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        (format!("http://{addr}"), answered)
    }

    /// The [`BucketReadFailed`] a failed read returned.
    fn read_failure(e: &anyhow::Error) -> &BucketReadFailed {
        e.downcast_ref::<BucketReadFailed>()
            .unwrap_or_else(|| panic!("not a BucketReadFailed: {e:#}"))
    }

    #[test]
    fn a_read_that_fails_transiently_is_asked_again_and_returns_the_value() {
        let tmp = tempfile::TempDir::new().unwrap();
        // One ask is a request and rust-s3's own retry of it: both fail,
        // so only koto's second ask can reach the 200.
        let (url, answered) = scripted(&["503 Slow Down", "503 Slow Down"]);
        let backend = cloud_backend_at(tmp.path(), url);
        let got = backend.get_with_etag(KEY).unwrap();
        assert_eq!(got, Some((b"alpha".to_vec(), "e1".to_string())));
        assert_eq!(answered.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_403_is_asked_once_and_fails_holding_the_403() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&["403 Forbidden"; 6]);
        let backend = cloud_backend_at(tmp.path(), url);
        let e = backend.get_with_etag(KEY).unwrap_err();
        let failed = read_failure(&e);
        assert_eq!(failed.key, KEY);
        assert!(
            matches!(failed.source, S3Error::HttpFailWithBody(403, _)),
            "{:?}",
            failed.source
        );
        assert!(
            e.to_string()
                .starts_with(&format!("the bucket couldn't be read at {KEY}: ")),
            "{e}"
        );
        assert_eq!(answered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_404_is_asked_once_and_reads_as_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&["404 Not Found"; 6]);
        let backend = cloud_backend_at(tmp.path(), url);
        assert_eq!(backend.get_with_etag(KEY).unwrap(), None);
        assert_eq!(answered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_response_other_than_a_200_fails_with_its_status() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A 204 is a success to rust-s3, so it comes back without an error
        // and without rust-s3's retry; it isn't a value, and isn't transient.
        let (url, answered) = scripted(&["204 No Content"; 6]);
        let backend = cloud_backend_at(tmp.path(), url);
        let e = backend.get_with_etag(KEY).unwrap_err();
        assert!(
            matches!(read_failure(&e).source, S3Error::HttpFailWithBody(204, _)),
            "{e:#}"
        );
        assert_eq!(
            e.to_string(),
            format!("the bucket couldn't be read at {KEY}: status 204")
        );
        assert_eq!(answered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_manifest_hash_is_64_lowercase_hex_digits() {
        let hash = crate::cache::sha256_hex(b"alpha");
        assert!(is_sha256_hex(&hash));
        assert!(!is_sha256_hex(&hash.to_uppercase()));
        assert!(!is_sha256_hex(&hash[1..]));
        assert!(!is_sha256_hex(&format!("{hash}0")));
        assert!(!is_sha256_hex(&format!("{}g", &hash[1..])));
        assert!(!is_sha256_hex(""));
    }

    #[test]
    fn a_plan_is_refused_before_any_key_is_built_from_a_bad_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let prefixes = vec!["record/".to_string()];
        assert!(backend
            .carry_plan("../elsewhere", "p", &prefixes, true)
            .is_err());
        assert!(backend.carry_plan(PREFIX, "../p", &prefixes, true).is_err());
        assert_eq!(answered.load(Ordering::SeqCst), 0);
    }

    /// The [`CarrySourceError`] a carry step returned.
    fn source_error(e: &anyhow::Error) -> CarrySourceError {
        *e.downcast_ref::<CarrySourceError>()
            .unwrap_or_else(|| panic!("not a CarrySourceError: {e:#}"))
    }

    #[test]
    fn a_missing_manifest_is_an_empty_plan_unless_the_log_names_a_carried_key() {
        let prefixes = vec!["record/".to_string()];
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, _) = scripted(&["404 Not Found"; 2]);
        let backend = cloud_backend_at(tmp.path(), url);
        assert!(backend
            .carry_plan(PREFIX, "p", &prefixes, false)
            .unwrap()
            .is_empty());

        let tmp = tempfile::TempDir::new().unwrap();
        let (url, _) = scripted(&["404 Not Found"; 2]);
        let backend = cloud_backend_at(tmp.path(), url);
        let e = backend
            .carry_plan(PREFIX, "p", &prefixes, true)
            .unwrap_err();
        assert_eq!(source_error(&e), CarrySourceError::ManifestMissing);
    }

    #[test]
    fn a_manifest_that_does_not_parse_says_so_without_the_parser_message() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Every request is answered `alpha`, which isn't a manifest.
        let (url, _) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let e = backend
            .carry_plan(PREFIX, "p", &["record/".to_string()], false)
            .unwrap_err();
        assert_eq!(source_error(&e), CarrySourceError::ManifestUnparsable);
        assert_eq!(
            format!("{e:#}"),
            CarrySourceError::ManifestUnparsable.to_string()
        );
    }

    #[test]
    fn a_fetched_value_is_returned_only_when_it_matches_its_hash() {
        let entry = |hash: String| CarryEntry {
            key: "record/a".to_string(),
            hash,
        };
        // Every request is answered `alpha`.
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let got = backend.fetch_carried(PREFIX, "p", &[entry(crate::cache::sha256_hex(b"alpha"))]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_ref().unwrap(), b"alpha");
        assert_eq!(answered.load(Ordering::SeqCst), 1);

        // A value that differs from its hash on both reads.
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let got = backend.fetch_carried(PREFIX, "p", &[entry(crate::cache::sha256_hex(b"beta"))]);
        let e = got.into_iter().next().unwrap().unwrap_err();
        assert_eq!(source_error(&e), CarrySourceError::HashMismatch);
        assert_eq!(answered.load(Ordering::SeqCst), 2);

        // An object the bucket doesn't hold is read once.
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&["404 Not Found"; 2]);
        let backend = cloud_backend_at(tmp.path(), url);
        let got = backend.fetch_carried(PREFIX, "p", &[entry(crate::cache::sha256_hex(b"alpha"))]);
        let e = got.into_iter().next().unwrap().unwrap_err();
        assert_eq!(source_error(&e), CarrySourceError::ObjectMissing);
        assert_eq!(answered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_read_says_what_the_pull_always_said() {
        let failed = BucketReadFailed {
            key: format!("{PREFIX}/p/koto-p.state.jsonl"),
            source: S3Error::HttpFailWithBody(500, "oops".to_string()),
            took: Duration::ZERO,
        };
        assert_eq!(
            failed.to_string(),
            format!(
                "the bucket couldn't be read at {PREFIX}/p/koto-p.state.jsonl: \
                 Got HTTP 500 with content 'oops'"
            )
        );
    }

    fn failed_after(source: S3Error, took: Duration) -> BucketReadFailed {
        BucketReadFailed {
            key: KEY.to_string(),
            source,
            took,
        }
    }

    fn io(kind: std::io::ErrorKind) -> S3Error {
        S3Error::Io(std::io::Error::from(kind))
    }

    #[test]
    fn a_failed_read_is_classed_by_its_status() {
        for (status, class) in [
            (401, FailureClass::Denied),
            (403, FailureClass::Denied),
            (429, FailureClass::Throttled),
            (503, FailureClass::Throttled),
            (500, FailureClass::Unreachable),
            (502, FailureClass::Unreachable),
        ] {
            let failed = failed_after(
                S3Error::HttpFailWithBody(status, String::new()),
                super::super::REQUEST_TIMEOUT * 2,
            );
            assert_eq!(failed.class(), class, "{status}");
        }
    }

    /// A transport failure is `timeout` when it says it timed out, or when
    /// it took the cloud backend's 60 s request timeout, not the remote
    /// feed's 10 s one; anything quicker is `unreachable`.
    #[test]
    fn a_failed_read_is_a_timeout_only_after_the_cloud_request_timeout() {
        let timeout = super::super::REQUEST_TIMEOUT;
        assert_eq!(timeout, Duration::from_secs(60));
        let eof = || io(std::io::ErrorKind::UnexpectedEof);
        assert_eq!(
            failed_after(io(std::io::ErrorKind::TimedOut), Duration::ZERO).class(),
            FailureClass::Timeout
        );
        assert_eq!(failed_after(eof(), timeout).class(), FailureClass::Timeout);
        assert_eq!(
            failed_after(eof(), Duration::from_secs(10)).class(),
            FailureClass::Unreachable
        );
        assert_eq!(
            failed_after(eof(), timeout - Duration::from_millis(1)).class(),
            FailureClass::Unreachable
        );
        // An answer that arrived late is still classed by its status.
        assert_eq!(
            failed_after(S3Error::HttpFailWithBody(500, String::new()), timeout).class(),
            FailureClass::Unreachable
        );
    }

    fn rebound_pulled(run: &str, anchor: &str, retired: &str) -> PulledState {
        let header = serde_json::json!({
            "schema_version": 1, "workflow": run, "template_hash": "abc",
            "created_at": "2026-01-01T00:00:00Z", "execution_dir": anchor,
            "origin": {"anchor": anchor, "store": {"kind": "local", "base": "/s"},
                       "anchor_epoch": 1, "whole_epoch": 1, "retired_anchors": [retired]},
        });
        PulledState {
            prefix: PREFIX.to_string(),
            key: format!("{PREFIX}/{run}/{}", state_file_name(run)),
            bytes: format!("{header}\n").into_bytes(),
            etag: "e1".to_string(),
            header: serde_json::from_value(header).unwrap(),
            events: Vec::new(),
            last_seq: 0,
        }
    }

    fn replaced_close() -> Close {
        Close {
            side_events: Vec::new(),
            cancel: true,
            fence: SupersedeFence {
                by: "s".to_string(),
                by_session_id: "id-s".to_string(),
                actor: Default::default(),
                at: "2026-01-01T00:00:00Z".to_string(),
                replaced: true,
            },
        }
    }

    /// The close's own rebind rule, asked of the bucket's copy: a whole move
    /// there past the epoch this process first read of its own copy refuses
    /// the close before any request is made.
    #[test]
    fn push_close_refuses_a_bucket_copy_rebound_after_the_first_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let local = backend.local.session_dir("p").join(state_file_name("p"));
        crate::session::fence_state::note_read(&local, 0);
        let pulled = rebound_pulled("p", "/ws/new", "/ws/old");
        let outcome =
            crate::session::fence_state::with_cwd(std::path::Path::new("/ws/new"), || {
                backend.push_close(&pulled, &replaced_close(), "2026-01-01T00:00:00Z")
            });
        match outcome {
            CloudClose::Rebound(r) => assert!(!r.from_retired),
            CloudClose::Closed { .. } => panic!("the rebound copy was closed"),
            _ => panic!("not Rebound"),
        }
        assert_eq!(
            answered.load(Ordering::SeqCst),
            0,
            "the refused close made a request"
        );
    }

    /// The same rule's other half: run from the directory the move retired,
    /// with no earlier read noted.
    #[test]
    fn push_close_refuses_a_bucket_copy_rebound_after_its_anchor_was_retired() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (url, answered) = scripted(&[]);
        let backend = cloud_backend_at(tmp.path(), url);
        let pulled = rebound_pulled("q", "/ws/new", "/ws/old");
        let outcome =
            crate::session::fence_state::with_cwd(std::path::Path::new("/ws/old"), || {
                backend.push_close(&pulled, &replaced_close(), "2026-01-01T00:00:00Z")
            });
        match outcome {
            CloudClose::Rebound(r) => assert!(r.from_retired),
            CloudClose::Closed { .. } => panic!("the rebound copy was closed"),
            _ => panic!("not Rebound"),
        }
        assert_eq!(
            answered.load(Ordering::SeqCst),
            0,
            "the refused close made a request"
        );
    }
}
