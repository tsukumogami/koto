//! The cloud backend's half of `koto init --supersede`
//! (`src/cli/init_supersede.rs`): the predecessor's state pulled from its
//! own workspace prefix, the fence pushed back conditional on that copy,
//! and the carried keys read from the same prefix.
//!
//! The fence is the one write this makes under another workspace's prefix,
//! and it is a PUT with `If-Match` on the ETag the pull returned, as every
//! state push has been since koto#480. Anything but a confirmed PUT (a 412,
//! a bucket that doesn't implement conditional writes, a copy with no ETag,
//! a failed request) is [`CloudFence::Unavailable`] with nothing written:
//! there is no unconditional fallback here, unlike an ordinary push.
//!
//! Everything read from the bucket is data. The prefix is checked to be a
//! workspace prefix and every name is validated before a key is built from
//! it; the predecessor's log is parsed whole, and a carried key is checked
//! as `koto context add` checks one.

use std::collections::BTreeSet;

use super::{
    is_not_found, is_precondition_failed, is_workspace_prefix, parse_source_log, response_etag,
    without_url_userinfo, CloudBackend, S3Error,
};
use crate::engine::persistence::{
    append_supersede_fence, superseded_by, SessionSupersededError, SupersedeFence,
};
use crate::engine::types::Event;
use crate::session::context::Manifest;
use crate::session::links::is_reserved_key;
use crate::session::state_log::{self, LogOrder};
use crate::session::validate::{validate_context_key, validate_session_id};
use crate::session::{state_file_name, SessionBackend};

/// How long the fence waits for a local copy's append lock when it adds the
/// fence there too. The bucket's fence already holds; this only makes the
/// local copy refuse at once rather than at its next push.
const LOCAL_FENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// A predecessor's state file as the bucket holds it, pulled for the fence.
pub(crate) struct PulledState {
    /// The workspace prefix it was read from.
    pub prefix: String,
    key: String,
    bytes: Vec<u8>,
    etag: String,
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
            events,
            last_seq: log.last_seq,
        }))
    }

    /// GET `key` with its ETag; `None` when the object isn't there.
    fn get_with_etag(&self, key: &str) -> anyhow::Result<Option<(Vec<u8>, String)>> {
        match self.bucket.get_object(key) {
            Ok(response) if response.status_code() == 200 => Ok(Some((
                response.bytes().to_vec(),
                response_etag(&response).unwrap_or_default(),
            ))),
            Ok(response) if response.status_code() == 404 => Ok(None),
            Err(e) if is_not_found(&e) => Ok(None),
            failed => anyhow::bail!(
                "the bucket couldn't be read at {key}: {}",
                match failed {
                    Ok(response) => format!("status {}", response.status_code()),
                    Err(e) => without_url_userinfo(&e.to_string()),
                }
            ),
        }
    }

    /// Append `fence` to the pulled copy and PUT it back with `If-Match` on
    /// the copy's ETag. On a 412 the copy is read again: a fence there now
    /// is [`CloudFence::Taken`]; anything else, and every other failure, is
    /// [`CloudFence::Unavailable`]. Nothing is ever PUT without the
    /// condition.
    pub(crate) fn push_fence(&self, pulled: &PulledState, fence: &SupersedeFence) -> CloudFence {
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
        let payload = fence.to_payload();
        let event = Event {
            seq: pulled.last_seq + 1,
            timestamp: fence.at.clone(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        };
        let line = match serde_json::to_string(&event) {
            Ok(line) => line,
            Err(e) => return CloudFence::Unavailable(format!("the fence doesn't serialize: {e}")),
        };
        let mut fenced = pulled.bytes.clone();
        if !fenced.ends_with(b"\n") {
            fenced.push(b'\n');
        }
        fenced.extend_from_slice(line.as_bytes());
        fenced.push(b'\n');

        match self.put_state_if(&pulled.key, &fenced, Some(&pulled.etag)) {
            Ok(response) => {
                self.adopt_fenced_copy(pulled, fence, &fenced, response_etag(&response));
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
                    self.record_synced(pred, etag.as_deref(), fenced);
                }
            }
            LogOrder::LocalExtends | LogOrder::Diverged => {
                let _ = append_supersede_fence(&path, fence, &fence.at, LOCAL_FENCE_WAIT);
            }
            LogOrder::Unrelated => {}
        }
    }

    /// Every key under each of `prefixes` in `pred`'s context under
    /// workspace prefix `prefix`, with its value, read from the bucket: one
    /// GET of the manifest, then one per key. Reserved keys are left out;
    /// a key name that `koto context add` would refuse fails the read.
    pub(crate) fn carry_source(
        &self,
        prefix: &str,
        pred: &str,
        prefixes: &[String],
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        anyhow::ensure!(
            is_workspace_prefix(prefix),
            "'{prefix}' isn't a workspace prefix"
        );
        validate_session_id(pred)?;
        let ctx = format!("{prefix}/{pred}/ctx/");
        let manifest = match self.get_with_etag(&format!("{ctx}manifest.json"))? {
            Some((bytes, _)) => {
                let mut manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| {
                    anyhow::anyhow!("the bucket's context manifest of '{pred}' doesn't parse: {e}")
                })?;
                manifest.drop_host_local();
                manifest.drop_pin_records();
                manifest
            }
            None => Manifest::default(),
        };
        let keys: BTreeSet<&String> = manifest
            .keys
            .keys()
            .filter(|k| prefixes.iter().any(|p| k.starts_with(p.as_str())))
            .filter(|k| !is_reserved_key(k))
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            validate_context_key(key).map_err(|e| {
                anyhow::anyhow!("the bucket's context manifest of '{pred}' names a bad key: {e}")
            })?;
            let (value, _) = self.get_with_etag(&format!("{ctx}{key}"))?.ok_or_else(|| {
                anyhow::anyhow!("the bucket holds no object for '{pred}''s key '{key}'")
            })?;
            out.push((key.clone(), value));
        }
        Ok(out)
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
