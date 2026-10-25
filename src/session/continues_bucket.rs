//! Where the bucket's answer about the continues relation meets the local
//! one (DESIGN-koto-session-continues.md, "Where the bucket answer meets
//! `keeps`").
//!
//! [`crate::session::hierarchy::Judgment::keeps`] stays synchronous and
//! local. Every removal point that can delete a session asks
//! [`relation_at_removal`] after `keeps` returns false, and keeps the
//! session when the answer keeps it, merging the answer into the entry's
//! report. The points are: `koto session prune`'s judgment and its apply,
//! `koto workspace prune`'s judgment and its reclaim, the same-name
//! replace's disposition, the parent sweep, and the report of
//! `koto session cleanup` and `koto cancel --cleanup`.
//!
//! [`bucket_keepers`] is that answer under the cloud backend: the walk over
//! the back-references the bucket holds beside the session. This build
//! writes no relation into a bucket (the verb refuses under the cloud
//! backend), so no back-reference exists to walk yet and the answer is
//! empty everywhere; the next phase fills it in.

use crate::session::hierarchy::{Judgment, RelationVerdict};
use crate::session::SessionBackend;

/// What the bucket says keeps `pred` through the continues relation:
/// later members the bucket holds that this host never pulled. Always
/// empty under the local backend, which makes no request. Under the cloud
/// backend it is empty in this build too: no relation has been written to
/// a bucket, so none is there to find.
pub fn bucket_keepers(backend: &dyn SessionBackend, pred: &str) -> RelationVerdict {
    let _ = (backend, pred);
    RelationVerdict::default()
}

/// What keeps `member` through the relation at a removal point: the local
/// judgment's verdict, and, only when the local judgment doesn't keep the
/// member at all, the bucket's answer merged in. A member kept for its own
/// standing or by the local relation costs no bucket request.
pub fn relation_at_removal(
    backend: &dyn SessionBackend,
    judgment: &Judgment,
    member: &str,
) -> RelationVerdict {
    let mut verdict = judgment.relation_of(member).clone();
    if !judgment.keeps(member) {
        verdict.merge(bucket_keepers(backend, member));
    }
    verdict
}
