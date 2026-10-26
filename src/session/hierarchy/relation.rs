//! The continues relation as retention reads it
//! (DESIGN-koto-session-continues.md, Decision 2): a directed, per-member
//! overlay beside the pin.
//!
//! A successor holds `koto/links/continues/<PRED>`. The graph scan keeps
//! that link out of every component and resolves it into
//! [`super::Graph`]'s directed list instead, so no hierarchy's members,
//! keepers or release instant change with it (R7). [`super::Judge::relation`]
//! walks the reverse of that list breadth-first from a session, reaching
//! every later member (its successors, theirs, and so on), and judges each
//! one's standing from that member alone: a pin, readable or not, then
//! alive, then within `retention.keep` of its own stop. Every later member
//! with standing is a keeper. Nothing earlier is ever reached, so nothing
//! earlier keeps anything later.
//!
//! What can't be judged keeps, and is reported in [`RelationVerdict`]'s
//! `unreadable` with the session holding the relation and a [`Reason`]: a
//! body that doesn't parse or is too large, a recorded session id that
//! names no session while another session holds the name, a cycle, and a
//! session whose reserved area can't be listed, which may name anything.

use crate::session::pin::PinFact;

/// Why a relation keeps a session without being judged, as
/// `relation_unreadable` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// The relation's body doesn't parse, is missing, or names another
    /// session than its key.
    Unparseable,
    /// The relation's body is larger than 4 KiB.
    TooLarge,
    /// The relation's recorded session id names no session in the store,
    /// and a session with another id holds the name it records.
    Mismatch,
    /// Following the relations from the session comes back to a session
    /// already reached.
    Cycle,
    /// The holder's reserved area can't be listed, so it may hold a
    /// relation naming any session.
    Unlistable,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Unparseable => "unparseable",
            Reason::TooLarge => "too_large",
            Reason::Mismatch => "mismatch",
            Reason::Cycle => "cycle",
            Reason::Unlistable => "unlistable",
        }
    }
}

/// A relation that keeps a session because it can't be judged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unreadable {
    /// The session holding the relation.
    pub holder: String,
    pub reason: Reason,
}

/// What a keeper's own state says about keeping, in order of precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    /// It holds a pin, readable or not.
    Pin,
    /// It runs, or its log can't be read.
    Alive,
    /// `retention.keep` hasn't passed since its own stop.
    Retention,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Pin => "pin",
            Standing::Alive => "alive",
            Standing::Retention => "retention",
        }
    }
}

/// One later member whose standing keeps the judged session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationKeeper {
    pub keeper: String,
    pub standing: Standing,
    /// For [`Standing::Retention`]: when the keeper's retention passes.
    pub retention_passes_at: Option<String>,
    /// For [`Standing::Pin`]: the keeper's pin, and where it was read.
    pub pin: PinFact,
    pub pin_source: &'static str,
    /// The members between the judged session and the keeper, nearest
    /// first: empty when the keeper is the judged session's successor.
    pub path: Vec<String>,
}

/// Why the bucket couldn't be checked for what continues a session, as
/// `bucket_unchecked`'s `error` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UncheckedError {
    /// A listing or a read failed: the bucket couldn't be reached, or
    /// refused (a permission error included).
    Unreachable,
    /// A listing came back as something koto can't parse.
    Unparseable,
    /// The walk ran out of the requests it may spend, or reached its
    /// ceiling on members or requests, before it got there.
    Budget,
    /// The session's `continued-by/` area holds more back-references than
    /// a walk follows.
    TooManyPointers,
}

impl UncheckedError {
    pub fn as_str(self) -> &'static str {
        match self {
            UncheckedError::Unreachable => "unreachable",
            UncheckedError::Unparseable => "unparseable",
            UncheckedError::Budget => "budget",
            UncheckedError::TooManyPointers => "too_many_pointers",
        }
    }
}

/// A session the bucket walk couldn't check, which keeps the judged
/// session (R18): `name` is the session whose objects couldn't be listed
/// or read, or couldn't be afforded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unchecked {
    pub name: String,
    pub error: UncheckedError,
}

/// What the relation says about one session: its keepers, ordered by
/// name, what couldn't be judged, and, under the cloud backend, what in
/// the bucket couldn't be checked. Empty for a session no relation names,
/// which the relation doesn't keep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelationVerdict {
    pub keepers: Vec<RelationKeeper>,
    pub unreadable: Vec<Unreadable>,
    pub unchecked: Vec<Unchecked>,
}

impl RelationVerdict {
    /// The verdict of a session no relation keeps.
    pub const EMPTY: RelationVerdict = RelationVerdict {
        keepers: Vec::new(),
        unreadable: Vec::new(),
        unchecked: Vec::new(),
    };

    /// Whether the relation keeps the session: a keeper has standing, or
    /// something on the way couldn't be judged or checked.
    pub fn keeps(&self) -> bool {
        !self.keepers.is_empty() || !self.unreadable.is_empty() || !self.unchecked.is_empty()
    }

    /// Add `other`'s keepers, unreadable relations and unchecked sessions
    /// to these: the merge a removal site makes of the bucket's answer into
    /// the local one. Kept ordered by name, one keeper per name (the first
    /// seen wins).
    pub fn merge(&mut self, other: RelationVerdict) {
        for k in other.keepers {
            if !self.keepers.iter().any(|mine| mine.keeper == k.keeper) {
                self.keepers.push(k);
            }
        }
        self.unreadable.extend(other.unreadable);
        self.unchecked.extend(other.unchecked);
        self.normalize();
    }

    /// Sort keepers by name, unreadable entries by holder and unchecked
    /// ones by name, dropping repeats.
    pub(crate) fn normalize(&mut self) {
        self.keepers.sort_by(|a, b| a.keeper.cmp(&b.keeper));
        self.unreadable.sort();
        self.unreadable.dedup();
        self.unchecked.sort();
        self.unchecked.dedup();
    }
}
