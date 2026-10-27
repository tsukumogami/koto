//! The bucket's half of `koto session needed`: later members of a running
//! session's lineage that only the bucket knows.
//!
//! A run continued on another host has a successor this host never pulled,
//! and a session imported into another workspace leaves its copy there. The
//! bucket records both beside the session they follow: a successor writes a
//! back-reference, `<p>/<name>/ctx/koto/continued-by/<sp>/<s>`, and an
//! import leaves a migration marker naming its copy, `<p>/<name>/migrated.json`
//! (see [`crate::session::continues_bucket`]).
//!
//! [`walk`] starts from one running session and lists its prefix once. Each
//! back-reference is a hint: the holder counts only when its own continues
//! relation names the session by its session id. A marker counts only when
//! [`BucketReads::marker_target`] verifies its target. For each later member
//! it reads the anchor from the first line of its state file in the bucket;
//! one anchored outside the path releases it, and the walk goes on through
//! each member's own back-references and marker, breadth first, never
//! revisiting one.
//!
//! The walk lists and GETs and nothing else. It stops at the continues walk's
//! limits ([`MAX_POINTERS`] back-references in a listing, [`MAX_MEMBERS`]
//! members, [`MAX_REQUESTS`] requests). Anything it can't list, read, parse
//! or afford leaves the path [`Found::Unchecked`], which keeps it.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::session::cloud::MIGRATED_MARKER;
use crate::session::continues_bucket::{
    parse_relation, BucketReads, CtxListing, Pointer, MAX_MEMBERS, MAX_POINTERS, MAX_REQUESTS,
};
use crate::session::inventory::under_dir;
use crate::session::links::{continues_prefix, Prefix, MAX_LINK_BODY_BYTES};
use crate::session::state_file_name;

use super::MAX_HEADER_BYTES;

/// What the bucket says about a running session under the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Found {
    /// A later member is anchored outside the path.
    Released,
    /// Every later member was read, and none is anchored outside it.
    Kept,
    /// Something the walk needed couldn't be listed, read or afforded.
    Unchecked,
}

/// A later member the walk read: its anchor, and its session id, which its
/// own successors' relations must name.
struct Member {
    anchor: Option<PathBuf>,
    id: Option<String>,
}

/// Walk the bucket from session `name` under workspace prefix `prefix`,
/// whose header id is `id`, and say whether a later member is anchored
/// outside `target`.
pub(super) fn walk(
    reads: &dyn BucketReads,
    prefix: &str,
    name: &str,
    id: Option<&str>,
    target: &Path,
) -> Found {
    let mut requests = 0usize;
    let mut members = 0usize;
    let mut unchecked = false;
    let start = Pointer {
        prefix: prefix.to_string(),
        name: name.to_string(),
    };
    let mut visited: BTreeSet<Pointer> = BTreeSet::from([start.clone()]);
    let mut queue: VecDeque<(Pointer, Option<String>)> =
        VecDeque::from([(start, id.map(str::to_string))]);

    while let Some((pred, pred_id)) = queue.pop_front() {
        if requests >= MAX_REQUESTS {
            unchecked = true;
            break;
        }
        let session = format!("{}/{}/", pred.prefix, pred.name);
        let listed = match reads.list(&session) {
            Ok(listed) => {
                requests += listed.requests;
                listed
            }
            Err(_) => {
                requests += 1;
                unchecked = true;
                continue;
            }
        };

        // Each later member, and whether a verified marker named it.
        let mut next: Vec<(Pointer, bool)> = Vec::new();
        let ctx = CtxListing::parse(&session, &listed.objects);
        if ctx.pointer_count() > MAX_POINTERS || !ctx.invalid_pointers.is_empty() {
            unchecked = true;
        }
        if ctx.pointer_count() <= MAX_POINTERS {
            next.extend(ctx.pointers.into_iter().map(|p| (p, false)));
        }
        let marker = format!("{session}{MIGRATED_MARKER}");
        if listed.objects.iter().any(|o| o.key == marker) {
            let (target, cost) = reads.marker_target(&pred.prefix, &pred.name);
            requests += cost;
            match target {
                Ok(Some(t)) => next.push((t, true)),
                Ok(None) => {}
                Err(_) => unchecked = true,
            }
        }

        for (at, by_marker) in next {
            // A member reached again is a cycle: it adds nothing.
            if visited.contains(&at) {
                continue;
            }
            if members >= MAX_MEMBERS || requests >= MAX_REQUESTS {
                unchecked = true;
                break;
            }
            match member(
                reads,
                &at,
                &pred,
                pred_id.as_deref(),
                by_marker,
                &mut requests,
            ) {
                Ok(Some(m)) => {
                    visited.insert(at.clone());
                    members += 1;
                    // Only an anchor read and found outside releases.
                    if m.anchor
                        .as_deref()
                        .is_some_and(|dir| !under_dir(dir, target))
                    {
                        return Found::Released;
                    }
                    queue.push_back((at, m.id));
                }
                // A stale back-reference: its holder names someone else.
                Ok(None) => {}
                Err(()) => unchecked = true,
            }
        }
    }
    if unchecked {
        Found::Unchecked
    } else {
        Found::Kept
    }
}

/// Read the later member at `at`: `Ok(None)` when it holds no relation
/// naming `pred` (with `pred_id`, when that is known), `Err` when what it
/// needs can't be read. A member a verified marker named needs no relation.
fn member(
    reads: &dyn BucketReads,
    at: &Pointer,
    pred: &Pointer,
    pred_id: Option<&str>,
    by_marker: bool,
    requests: &mut usize,
) -> Result<Option<Member>, ()> {
    let session = format!("{}/{}/", at.prefix, at.name);
    if !by_marker {
        let key = format!("{session}ctx/{}{}", continues_prefix(), pred.name);
        *requests += 1;
        let Some(bytes) = reads.get(&key, MAX_LINK_BODY_BYTES).map_err(|_| ())? else {
            return Ok(None);
        };
        if bytes.len() > MAX_LINK_BODY_BYTES {
            return Err(());
        }
        let body = parse_relation(&bytes, &pred.name).map_err(|_| ())?;
        let same_id = match (pred_id, body.session_id.as_deref()) {
            (Some(ours), Some(theirs)) => ours == theirs,
            (Some(_), None) => false,
            (None, _) => true,
        };
        let same_prefix = match &body.prefix {
            Prefix::Value(p) => p == &pred.prefix,
            _ => true,
        };
        if !same_id || !same_prefix {
            return Ok(None);
        }
    }
    let key = format!("{session}{}", state_file_name(&at.name));
    *requests += 1;
    let max = MAX_HEADER_BYTES as usize;
    let Some(bytes) = reads.get(&key, max).map_err(|_| ())? else {
        return Err(());
    };
    let line = match bytes.iter().position(|b| *b == b'\n') {
        Some(end) => &bytes[..end],
        None if bytes.len() > max => return Err(()),
        None => &bytes[..],
    };
    let header: serde_json::Value = serde_json::from_slice(line).map_err(|_| ())?;
    let anchor = match header.get("execution_dir") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(dir)) => Some(PathBuf::from(dir)),
        Some(_) => return Err(()),
    };
    let id = header
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    Ok(Some(Member { anchor, id }))
}

#[cfg(test)]
mod tests;
