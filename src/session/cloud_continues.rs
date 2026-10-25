//! The cloud backend's half of the continues relation: the back-reference
//! a successor writes beside its predecessor, the facts a removal reads
//! beside the session it would delete, and the reads the bucket walk makes
//! ([`crate::session::continues_bucket`]).
//!
//! Every write here is a standalone object outside the state file and the
//! manifest: `put_backref` and the DELETEs that take a pointer back. None
//! goes through `put_state`, so the conditional state writes and the
//! strict-prefix pull rule are untouched.

use std::collections::{BTreeSet, HashMap};

use super::{is_not_found, listing_error, without_url_userinfo, CloudBackend, MIGRATED_MARKER};
use crate::session::continues_bucket::{
    self, in_backref_area, is_backref_key, is_workspace_prefix, owed_pointer, pointer_body,
    pointer_key, wanted_pointer, BucketReads, BucketRemovalFacts, Clock, CtxListing, Listed,
    ListedObject, Memo, OwedPointer, Pointer, Start, MAX_POINTERS, MAX_REQUESTS,
};
use crate::session::hierarchy::{RelationVerdict, UncheckedError};
use crate::session::pin::BucketError;
use crate::session::state_file_name;
use crate::session::SessionBackend;

/// What the walk keeps between calls in one process: the judgment pass's
/// memo, and the `ctx/koto/` listing the pin guard last made of each
/// session, which the next removal check of that session takes instead of
/// listing again.
#[derive(Default)]
pub(super) struct WalkState {
    memo: Memo,
    stash: HashMap<String, Vec<ListedObject>>,
    /// The whole listing of a session's prefix a removal check made when
    /// no pin guard listing was there to share, kept for the remote-copy
    /// check of the same removal (`koto session prune`'s rule g), which
    /// reads the same objects.
    prefix_stash: HashMap<String, Vec<ListedObject>>,
}

fn unchecked_of(e: BucketError) -> UncheckedError {
    match e {
        BucketError::Unreachable => UncheckedError::Unreachable,
        BucketError::Unparseable => UncheckedError::Unparseable,
    }
}

/// Whether `hash` has the shape of a template hash (64 lowercase hex
/// digits), so it can be joined into a cache path.
fn is_template_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BucketReads for CloudBackend {
    fn list(&self, prefix: &str) -> Result<Listed, UncheckedError> {
        let mut listed = Listed::default();
        let mut token = None;
        loop {
            listed.requests += 1;
            let (page, _) = self
                .bucket
                .list_page(prefix.to_string(), None, token, None, None)
                .map_err(|e| unchecked_of(listing_error(&e)))?;
            listed
                .objects
                .extend(page.contents.into_iter().map(|o| ListedObject {
                    key: o.key,
                    size: o.size,
                    etag: o.e_tag,
                }));
            token = page.next_continuation_token;
            // A listing that pages past the request ceiling can't be
            // afforded anyway; what was read is enough to say so.
            if token.is_none() || listed.requests > MAX_REQUESTS {
                break;
            }
        }
        Ok(listed)
    }

    fn get(&self, key: &str, max: usize) -> Result<Option<Vec<u8>>, UncheckedError> {
        let response = if max <= crate::session::links::MAX_LINK_BODY_BYTES {
            self.bucket.get_object(key)
        } else {
            self.bucket.get_object_range(key, 0, Some(max as u64))
        };
        match response {
            Ok(r) => match r.status_code() {
                200 | 206 => Ok(Some(r.bytes().to_vec())),
                404 => Ok(None),
                // A range over an empty object.
                416 => Ok(Some(Vec::new())),
                _ => Err(UncheckedError::Unreachable),
            },
            Err(e) if is_not_found(&e) => Ok(None),
            Err(s3::error::S3Error::HttpFailWithBody(416, _)) => Ok(Some(Vec::new())),
            Err(_) => Err(UncheckedError::Unreachable),
        }
    }

    fn cached_template(&self, hash: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        if !is_template_hash(hash) {
            return None;
        }
        let path = crate::cache::cache_dir().join(format!("{hash}.json"));
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(continues_bucket::MAX_TEMPLATE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    }

    fn marker_target(
        &self,
        prefix: &str,
        name: &str,
    ) -> (Result<Option<Pointer>, UncheckedError>, usize) {
        use super::TargetStatus;
        let mut requests = 1;
        let marker = match self.get(
            &format!("{prefix}/{name}/{MIGRATED_MARKER}"),
            crate::session::links::MAX_LINK_BODY_BYTES,
        ) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return (Ok(None), requests),
            Err(e) => return (Err(e), requests),
        };
        let field = |f: &str| super::marker_target_field(&marker, f);
        let (t_prefix, t_session, t_id) = (field("prefix"), field("session"), field("session_id"));
        // A marker that names no target this koto can read points at no
        // later session: the session's own back-references still count.
        if !is_workspace_prefix(&t_prefix)
            || crate::session::validate::validate_session_id(&t_session).is_err()
        {
            return (Ok(None), requests);
        }
        // The source copy is listed and read, and the target too.
        requests += 4;
        let source = self.migrated_source_copy(prefix, name);
        let status = self.marker_target_status(
            &t_prefix,
            &t_session,
            &t_id,
            source.as_ref().map_err(String::as_str),
        );
        let target = Pointer {
            prefix: t_prefix,
            name: t_session,
        };
        let answer = match status {
            TargetStatus::Present | TargetStatus::Migrated => Ok(Some(target)),
            TargetStatus::Absent | TargetStatus::Replaced(_) => Ok(None),
            TargetStatus::Unverified(_) | TargetStatus::Unknown(_) => {
                Err(UncheckedError::Unreachable)
            }
        };
        (answer, requests)
    }
}

impl CloudBackend {
    fn walk_state(&self) -> std::sync::MutexGuard<'_, WalkState> {
        self.probes.walk.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// This backend's workspace prefix in the bucket.
    pub(crate) fn workspace_prefix(&self) -> &str {
        &self.prefix
    }

    /// The header of session `name`'s state file under workspace prefix
    /// `prefix`, for `--continues` naming a predecessor this host holds no
    /// copy of: `Ok(None)` when the bucket holds no state file there,
    /// `Err` when it can't be listed or read, or the header doesn't parse.
    pub(crate) fn remote_header_at(
        &self,
        prefix: &str,
        name: &str,
    ) -> anyhow::Result<Option<crate::engine::types::StateFileHeader>> {
        anyhow::ensure!(
            is_workspace_prefix(prefix),
            "'{prefix}' isn't a workspace prefix"
        );
        crate::session::validate::validate_session_id(name)?;
        let key = format!("{prefix}/{name}/{}", state_file_name(name));
        let Some(bytes) = self.get_object_if_present(&key)? else {
            return Ok(None);
        };
        let line = bytes.split(|b| *b == b'\n').next().unwrap_or_default();
        let header = serde_json::from_slice(line).map_err(|e| {
            anyhow::anyhow!("the bucket's header line of '{name}' doesn't parse: {e}")
        })?;
        Ok(Some(header))
    }

    /// Keep the pin guard's fresh listing of `id`'s `ctx/koto/` for the
    /// removal check that follows it.
    pub(super) fn stash_ctx_listing(&self, id: &str, objects: Vec<ListedObject>) {
        self.walk_state().stash.insert(id.to_string(), objects);
    }

    /// [`SessionBackend::bucket_removal_facts`]: the `ctx/koto/` listing of
    /// `id` (the pin guard's, when it just made one, else a new one), and,
    /// with `top`, one delimiter listing of `<prefix>/<id>/` for a
    /// migration marker and the state object's ETag. When the session's
    /// header names another `origin.store.prefix`, that prefix's
    /// `ctx/koto/` listing of the session too.
    pub(crate) fn removal_facts(&self, id: &str, top: bool) -> BucketRemovalFacts {
        let session = self.session_prefix(id);
        let mut requests = 0;
        let stashed = self.walk_state().stash.remove(id);
        let ctx = match stashed {
            Some(objects) => Ok(CtxListing::parse(&session, &objects)),
            // No pin guard listing to share: one listing of the whole
            // prefix answers both questions (the `ctx/koto/` objects and a
            // marker beside it), and the remote-copy check that follows
            // reads it too.
            None if top => match BucketReads::list(self, &session) {
                Ok(listed) => {
                    requests += listed.requests;
                    let marker = format!("{session}{MIGRATED_MARKER}");
                    let state = format!("{session}{}", state_file_name(id));
                    let top_facts = (
                        Ok(listed.objects.iter().any(|o| o.key == marker)),
                        listed
                            .objects
                            .iter()
                            .find(|o| o.key == state)
                            .and_then(|o| o.etag.clone()),
                    );
                    let ctx = CtxListing::parse(&session, &listed.objects);
                    let mut state_guard = self.walk_state();
                    state_guard.memo.set_top(&self.prefix, id, top_facts);
                    state_guard
                        .prefix_stash
                        .insert(id.to_string(), listed.objects);
                    Ok(ctx)
                }
                Err(e) => {
                    requests += 1;
                    Err(e)
                }
            },
            None => self.ctx_listing(&session, &mut requests),
        };
        let known = self.walk_state().memo.top(&self.prefix, id);
        let (migrated, state_etag) = match (known, &ctx) {
            _ if !top => (Ok(false), None),
            (Some(known), _) => known,
            // The whole listing failed: the session is unchecked already,
            // and listing its prefix again would fail the same way.
            (None, Err(e)) => (Err(*e), None),
            (None, Ok(_)) => {
                requests += 1;
                let listed = match self.bucket.list_page(
                    session.clone(),
                    Some("/".to_string()),
                    None,
                    None,
                    None,
                ) {
                    Ok((page, _)) => {
                        let marker = format!("{session}{MIGRATED_MARKER}");
                        let state = format!("{session}{}", state_file_name(id));
                        (
                            Ok(page.contents.iter().any(|o| o.key == marker)),
                            page.contents
                                .iter()
                                .find(|o| o.key == state)
                                .and_then(|o| o.e_tag.clone()),
                        )
                    }
                    Err(e) => (Err(unchecked_of(listing_error(&e))), None),
                };
                self.walk_state()
                    .memo
                    .set_top(&self.prefix, id, listed.clone());
                listed
            }
        };
        let header = self.local.read_header(id).ok();
        let session_id = header
            .as_ref()
            .map(|h| h.session_id.clone())
            .filter(|id| !id.is_empty());
        let origin = header
            .and_then(|h| h.origin)
            .and_then(|o| o.store.prefix)
            .filter(|p| is_workspace_prefix(p) && *p != self.prefix)
            .map(|p| {
                let listing = self.ctx_listing(&format!("{p}/{id}/"), &mut requests);
                (p, listing)
            });
        BucketRemovalFacts {
            prefix: self.prefix.clone(),
            name: id.to_string(),
            session_id,
            ctx,
            migrated,
            state_etag,
            origin,
            requests,
        }
    }

    fn ctx_listing(
        &self,
        session_prefix: &str,
        requests: &mut usize,
    ) -> Result<CtxListing, UncheckedError> {
        match BucketReads::list(self, &format!("{session_prefix}ctx/koto/")) {
            Ok(listed) => {
                *requests += listed.requests;
                Ok(CtxListing::parse(session_prefix, &listed.objects))
            }
            Err(e) => {
                *requests += 1;
                Err(e)
            }
        }
    }

    /// The bucket's answer about what continues `pred`: a walk of the
    /// back-references beside it. With `final_guard`, the check right
    /// before a delete: only the pointers the pass's judgment of `pred`
    /// didn't see are walked, from the pin guard's fresh listing; with no
    /// judgment in this pass, every pointer is.
    pub(crate) fn relation_walk(
        &self,
        pred: &str,
        clock: &Clock,
        final_guard: bool,
    ) -> RelationVerdict {
        let judged: Option<BTreeSet<Pointer>> = final_guard
            .then(|| self.walk_state().memo.judged(&self.prefix, pred).cloned())
            .flatten();
        let facts = self.removal_facts(pred, judged.is_none());
        let mut state = self.walk_state();
        let walked = continues_bucket::walk(
            self,
            &mut state.memo,
            &Start {
                facts: &facts,
                skip: judged.as_ref(),
                extra: &[],
            },
            clock,
        );
        walked.verdict
    }

    /// The walk from `pred` under `pred_prefix` with `extra` passed in as
    /// later members (a migration marker's target): what `koto session
    /// prune --migrated-from` asks about a migrated source it would remove.
    pub(crate) fn relation_walk_with(
        &self,
        pred: &Pointer,
        pred_id: Option<&str>,
        extra: &[Pointer],
        clock: &Clock,
    ) -> RelationVerdict {
        let mut requests = 0;
        let session = format!("{}/{}/", pred.prefix, pred.name);
        let ctx = if is_workspace_prefix(&pred.prefix)
            && crate::session::validate::validate_session_id(&pred.name).is_ok()
        {
            self.ctx_listing(&session, &mut requests)
        } else {
            Err(UncheckedError::Unparseable)
        };
        let facts = BucketRemovalFacts {
            prefix: pred.prefix.clone(),
            name: pred.name.clone(),
            session_id: pred_id.map(str::to_string),
            ctx,
            migrated: Ok(false),
            state_etag: None,
            origin: None,
            requests,
        };
        let mut state = self.walk_state();
        continues_bucket::walk(
            self,
            &mut state.memo,
            &Start {
                facts: &facts,
                skip: None,
                extra,
            },
            clock,
        )
        .verdict
    }

    /// Start a judgment pass: forget the memo's reads and every stashed
    /// listing.
    pub(crate) fn begin_relation_pass(&self) {
        let mut state = self.walk_state();
        state.memo.begin_pass();
        state.stash.clear();
        state.prefix_stash.clear();
    }

    /// The whole listing of `id`'s prefix the removal check just made, as
    /// keys and sizes, for the remote-copy check of the same removal; taken,
    /// so a later check lists afresh.
    pub(super) fn take_prefix_listing(&self, id: &str) -> Option<Vec<(String, u64)>> {
        self.walk_state()
            .prefix_stash
            .remove(id)
            .map(|objects| objects.into_iter().map(|o| (o.key, o.size)).collect())
    }

    // -----------------------------------------------------------------
    // The successor's side
    // -----------------------------------------------------------------

    /// PUT the back-reference `owed` names beside its predecessor, and fail
    /// unless the bucket confirms it. The key is rebuilt from its checked
    /// parts, never taken as given.
    pub(crate) fn put_backref(&self, owed: &OwedPointer) -> anyhow::Result<()> {
        self.refuse_if_read_only("writing a back-reference")?;
        anyhow::ensure!(
            is_workspace_prefix(&owed.pred_prefix)
                && crate::session::validate::validate_session_id(&owed.pred).is_ok()
                && continues_bucket::is_session_id(&owed.holder_session_id),
            "the back-reference for '{}' can't be built: its predecessor or its session id \
             doesn't check out",
            owed.pred
        );
        let key = owed.key.clone();
        anyhow::ensure!(
            is_backref_key(&format!("{}/{}/", owed.pred_prefix, owed.pred), &key).is_some(),
            "'{key}' isn't a back-reference key"
        );
        self.bucket
            .put_object(&key, &pointer_body(&owed.holder_session_id))
            .map_err(|e| {
                anyhow::anyhow!("PUT {key} failed: {}", without_url_userinfo(&e.to_string()))
            })?;
        Ok(())
    }

    /// DELETE `key`, a back-reference this host built from a session's own
    /// relation and name, and fail unless the bucket confirms it.
    fn delete_pointer(&self, key: &str) -> anyhow::Result<()> {
        self.bucket.delete_object(key).map_err(|e| {
            anyhow::anyhow!(
                "DELETE {key} failed: {}",
                without_url_userinfo(&e.to_string())
            )
        })?;
        Ok(())
    }

    /// DELETE the back-reference `holder` keeps beside its predecessor: the
    /// key is built only from this workspace's prefix, `holder`'s own name
    /// and the predecessor its own local relation names, never from
    /// anything read from the bucket. Nothing is deleted when `holder`
    /// holds no relation here.
    pub(crate) fn delete_backref(&self, holder: &str) -> anyhow::Result<()> {
        let Some(wanted) = wanted_pointer(&self.local.session_dir(holder), &self.prefix, holder)
        else {
            return Ok(());
        };
        self.delete_pointer(&wanted.key)
    }

    /// Before a removal of `id` deletes its copies: take its own
    /// back-reference out, best-effort. One left behind names a holder with
    /// no relation, which every walk reads as stale.
    pub(super) fn drop_own_backref(&self, id: &str) {
        if self.probes.read_only {
            return;
        }
        if let Err(e) = self.delete_backref(id) {
            eprintln!(
                "warning: cloud sync: the back-reference of '{id}' beside its predecessor wasn't \
                 removed, and is left for walks to read as stale: {e:#}"
            );
        }
    }

    /// PUT the pointer session `id`'s relation calls for when it is owed
    /// (none confirmed, or one for another name), before the session's next
    /// cloud write or push, and record it as confirmed. A rebind fence that
    /// refuses the session's writes refuses this one too. A failed PUT
    /// warns: the relation stays out of the session's pushes until a later
    /// command writes the pointer, and the session's other writes go on.
    pub(crate) fn ensure_backref(&self, id: &str) {
        // From another directory's workspace the pointer would name the
        // wrong prefix: the command refuses there anyway.
        if self.probes.read_only || self.bound_elsewhere(id).is_some() {
            return;
        }
        let dir = self.local.session_dir(id);
        let Some(owed) = owed_pointer(&dir, &self.prefix, id) else {
            return;
        };
        let result = self
            .check_fence(id)
            .and_then(|()| self.put_backref(&owed))
            .and_then(|()| {
                continues_bucket::mark_confirmed(&dir, &owed.key).map_err(anyhow::Error::from)
            });
        if let Err(e) = result {
            eprintln!(
                "warning: cloud sync: the back-reference '{}' continues '{}' wasn't written, so \
                 its relation stays out of the bucket until a later command writes it: {e:#}",
                id, owed.pred
            );
        }
    }

    /// The relation keys a push of `id` leaves out: its relation, while its
    /// pointer is owed.
    pub(super) fn withheld_keys(&self, id: &str) -> Vec<String> {
        continues_bucket::unpushable_keys(&self.local.session_dir(id), &self.prefix, id)
    }

    /// The pointer `successor` (in this workspace) needs beside `pred`
    /// under `pred_prefix`, with `holder_session_id` its own id.
    pub(crate) fn pointer_for(
        &self,
        successor: &str,
        pred_prefix: &str,
        pred: &str,
        holder_session_id: &str,
    ) -> OwedPointer {
        OwedPointer {
            key: pointer_key(pred_prefix, pred, &self.prefix, successor),
            pred_prefix: pred_prefix.to_string(),
            pred: pred.to_string(),
            relation_key: format!("{}{pred}", crate::session::links::continues_prefix()),
            holder_session_id: holder_session_id.to_string(),
        }
    }

    /// Record, for `koto init --continues` and `koto session link
    /// --continues` under the cloud backend, that `successor` continues
    /// the predecessor `owed` names, pointer first: PUT the pointer
    /// (nothing is written when that fails), record it as confirmed, write
    /// the relation through the context store (`write`), then push the
    /// key, the log and the session's version strictly. When the relation
    /// can't be written, or the bucket doesn't confirm its push, what was
    /// written is taken back: the local key, the mark, the relation's
    /// object and the pointer. A writer the rebind fence refuses writes no
    /// pointer.
    pub(crate) fn record_continues_with_pointer(
        &self,
        successor: &str,
        owed: &OwedPointer,
        write: impl FnOnce() -> anyhow::Result<crate::session::links::LinkWrite>,
    ) -> anyhow::Result<crate::session::links::LinkWrite> {
        self.check_fence(successor)?;
        let dir = self.local.session_dir(successor);
        self.put_backref(owed)?;
        if let Err(e) = continues_bucket::mark_confirmed(&dir, &owed.key) {
            self.take_back_relation(successor, owed, false);
            return Err(e.into());
        }
        let outcome = match write() {
            // The session held this relation already: its pointer is now
            // confirmed, and the next push carries the relation.
            Ok(crate::session::links::LinkWrite::Unchanged) => {
                return Ok(crate::session::links::LinkWrite::Unchanged)
            }
            Ok(outcome) => outcome,
            Err(e) => {
                self.take_back_relation(successor, owed, false);
                return Err(e);
            }
        };
        if let Err(e) = self.push_key_strictly(successor, &owed.relation_key) {
            self.take_back_relation(successor, owed, true);
            return Err(e);
        }
        Ok(outcome)
    }

    /// Push context key `key` of `session`, its manifest, its log and its
    /// version, failing on the first that the bucket doesn't confirm.
    fn push_key_strictly(&self, session: &str, key: &str) -> anyhow::Result<()> {
        crate::session::sync::try_push_context_key(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.withheld_keys(session),
        )?;
        self.manifest_cache.invalidate();
        self.forget_remote_manifest(session);
        self.strict_push_state(session)?;
        self.push_version(session)?;
        Ok(())
    }

    /// Undo a relation whose write the bucket didn't confirm: the local
    /// key and the mark, the relation's object and the pointer, each
    /// best-effort. A pointer whose DELETE fails is left with no relation
    /// behind it, which walks read as stale.
    fn take_back_relation(&self, session: &str, owed: &OwedPointer, written: bool) {
        use crate::session::context::ContextStore;
        let dir = self.local.session_dir(session);
        continues_bucket::clear_mark(&dir);
        if written {
            if let Err(e) = self.local.remove(session, &owed.relation_key) {
                eprintln!(
                    "warning: the relation of '{session}' couldn't be taken back from this host: \
                     {e:#}"
                );
            }
            let object = format!("{}ctx/{}", self.session_prefix(session), owed.relation_key);
            let _ = self.bucket.delete_object(&object);
            // The manifest the failed push may have left names the key.
            if let Ok(Some(data)) = crate::session::sync::bucket_manifest_bytes(
                &dir.join("ctx").join("manifest.json"),
                &[],
            ) {
                let manifest = format!("{}ctx/manifest.json", self.session_prefix(session));
                let _ = self.bucket.put_object(manifest, &data);
            }
        }
        if let Err(e) = self.delete_pointer(&owed.key) {
            eprintln!(
                "warning: cloud sync: the back-reference {} wasn't taken back; with no relation \
                 behind it, walks read it as stale: {e:#}",
                owed.key
            );
        }
    }

    // -----------------------------------------------------------------
    // Renames
    // -----------------------------------------------------------------

    /// Copy the valid back-references under `from` to `to`, at most 64, from
    /// `objects` (a listing of `from`'s prefix). An invalid key, and any past
    /// the 64th, isn't carried: each is returned, for the caller to report.
    pub(super) fn copy_backrefs(
        &self,
        from: &str,
        to: &str,
        objects: &[(String, u64)],
    ) -> anyhow::Result<Vec<String>> {
        let old = self.session_prefix(from);
        let mut not_carried = Vec::new();
        let mut carried = 0usize;
        for (key, _) in objects.iter().filter(|(k, _)| in_backref_area(&old, k)) {
            match is_backref_key(&old, key) {
                Some(p) if carried < MAX_POINTERS => {
                    carried += 1;
                    let body = self.fetch_object(key)?.ok_or_else(|| {
                        anyhow::anyhow!("{key} was listed but could not be fetched")
                    })?;
                    let new_key = pointer_key(&self.prefix, to, &p.prefix, &p.name);
                    self.put_object(&new_key, &body)?;
                }
                _ => not_carried.push(key.clone()),
            }
        }
        Ok(not_carried)
    }

    /// PUT the pointer session `to` (just renamed from another name, its
    /// local copy already at `to`) owes under its new name, and record it.
    /// `Ok(None)` when it holds no relation.
    pub(super) fn put_renamed_backref(&self, to: &str) -> anyhow::Result<Option<OwedPointer>> {
        let dir = self.local.session_dir(to);
        let Some(owed) = wanted_pointer(&dir, &self.prefix, to) else {
            return Ok(None);
        };
        self.put_backref(&owed)?;
        continues_bucket::mark_confirmed(&dir, &owed.key)?;
        Ok(Some(owed))
    }

    /// After `old` moved to `new` for good: carry `old`'s remaining valid
    /// back-references to `new` and delete them under `old` (an invalid one
    /// is never deleted), and delete the pointer `new`'s relation named
    /// under its old name. Returns what wasn't carried or taken back.
    pub(crate) fn finish_backref_move(&self, old: &str, new: &str) -> Vec<String> {
        let mut notes = Vec::new();
        let area = format!(
            "{}ctx/{}",
            self.session_prefix(old),
            continues_bucket::CONTINUED_BY
        );
        match BucketReads::list(self, &area) {
            Ok(listed) => {
                let objects: Vec<(String, u64)> = listed
                    .objects
                    .into_iter()
                    .map(|o| (o.key, o.size))
                    .collect();
                let old_prefix = self.session_prefix(old);
                match self.copy_backrefs(old, new, &objects) {
                    Ok(not_carried) => {
                        for (key, _) in &objects {
                            if not_carried.contains(key)
                                || is_backref_key(&old_prefix, key).is_none()
                            {
                                continue;
                            }
                            if let Err(e) = self.delete_pointer(key) {
                                notes.push(format!("{key} wasn't removed: {e:#}"));
                            }
                        }
                        notes.extend(
                            not_carried
                                .into_iter()
                                .map(|k| format!("{k} wasn't carried to '{new}'")),
                        );
                    }
                    Err(e) => notes.push(format!(
                        "the back-references of '{old}' weren't carried to '{new}': {e:#}"
                    )),
                }
            }
            Err(e) => notes.push(format!(
                "the back-references of '{old}' couldn't be listed ({})",
                e.as_str()
            )),
        }
        if let Some(stale) = wanted_pointer(&self.local.session_dir(new), &self.prefix, old) {
            if let Err(e) = self.delete_pointer(&stale.key) {
                notes.push(format!("{} wasn't removed: {e:#}", stale.key));
            }
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{cloud_backend_at, serve, Endpoint, PREFIX};
    use super::*;

    const PRED_WS: &str = "0123456789abcdef";

    fn holder_id() -> String {
        "00000002-0000-4000-8000-000000000000".to_string()
    }

    /// Local session `name` under `base`, holding a relation naming `pred`
    /// under `PRED_WS`.
    fn holder(base: &std::path::Path, name: &str, pred: &str) {
        let dir = base.join(name).join("ctx/koto/links/continues");
        std::fs::create_dir_all(&dir).unwrap();
        let body = serde_json::json!({
            "v": 1, "session": pred, "session_id": "00000001-0000-4000-8000-000000000000",
            "workspace": "/w", "prefix": PRED_WS, "holder_session_id": holder_id(),
            "at": "2026-10-25T00:00:00.000Z",
        });
        std::fs::write(dir.join(pred), serde_json::to_vec(&body).unwrap()).unwrap();
    }

    fn deletes(endpoint: &Endpoint) -> Vec<String> {
        endpoint
            .requests()
            .into_iter()
            .filter(|r| r.method == "DELETE")
            .map(|r| r.path)
            .collect()
    }

    #[test]
    fn delete_backref_builds_its_key_from_the_holders_own_relation_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        // The bucket holds pointers naming the holder beside other
        // sessions: none of them is the holder's to delete.
        let planted = [
            format!("/{PRED_WS}/other/ctx/koto/continued-by/{PREFIX}/s"),
            format!("/{PRED_WS}/v1/ctx/koto/continued-by/fedcba9876543210/s"),
        ];
        let endpoint = serve(
            planted
                .iter()
                .map(|k| (k.clone(), b"{}".to_vec()))
                .collect(),
        );
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());

        // No relation here: nothing is deleted, whatever the bucket holds.
        std::fs::create_dir_all(tmp.path().join("s")).unwrap();
        backend.delete_backref("s").unwrap();
        assert!(deletes(&endpoint).is_empty(), "{:?}", deletes(&endpoint));

        // A relation naming v1: exactly the key its own parts build.
        holder(tmp.path(), "s", "v1");
        backend.delete_backref("s").unwrap();
        assert_eq!(
            deletes(&endpoint),
            vec![format!(
                "/test-bucket/{}",
                pointer_key(PRED_WS, "v1", PREFIX, "s")
            )]
        );
    }

    #[test]
    fn a_walk_reads_and_never_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let planted = format!("/{PREFIX}/v1/ctx/koto/continued-by/{PRED_WS}/s");
        let endpoint = serve(vec![(planted, b"{}".to_vec())]);
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let clock = Clock::configured();
        let _ = backend.relation_walk("v1", &clock, false);
        let _ = backend.relation_walk("v1", &clock, true);
        let requests = endpoint.requests();
        assert!(!requests.is_empty());
        assert!(requests.iter().all(|r| r.method == "GET"), "{requests:?}");
    }

    #[test]
    fn a_pointer_whose_key_does_not_check_out_is_never_put() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = serve(Vec::new());
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let mut owed = backend.pointer_for("s", PRED_WS, "v1", &holder_id());
        owed.key = format!("{PRED_WS}/v1/ctx/koto/continued-by/{PRED_WS}/s/../../x");
        assert!(backend.put_backref(&owed).is_err());
        let owed = backend.pointer_for("s", "not-a-prefix", "v1", &holder_id());
        assert!(backend.put_backref(&owed).is_err());
        let owed = backend.pointer_for("s", PRED_WS, "v1", "not-an-id");
        assert!(backend.put_backref(&owed).is_err());
        // This test backend's own prefix isn't a workspace prefix, so even a
        // well-formed pointer naming it is refused.
        let owed = backend.pointer_for("s", PRED_WS, "v1", &holder_id());
        assert!(backend.put_backref(&owed).is_err());
        assert!(endpoint.requests().iter().all(|r| r.method != "PUT"));
    }
}
