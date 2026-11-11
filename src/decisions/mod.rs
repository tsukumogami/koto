//! The decision record (DESIGN-decision-record.md).
//!
//! A decision is one create-only key in the deciding session's own context:
//! `koto/decisions/<id>` for every decision not aimed at another session's
//! decision, and `koto/aimed/<target-session-id>/<id>` for one that is (a
//! reversal, written by `koto decisions reverse`). `koto/packs/` is reserved and held
//! empty. The id is a dense, zero-padded [`ID_WIDTH`]-digit sequence, unique
//! across both areas of one session, so its byte order is the order the
//! session recorded its decisions in.
//!
//! Decision files sit outside the context manifest, as the pin does, so the
//! directory listing is the one source of truth, and no context-store write
//! or removal reaches them: [`crate::session::links::check_decision_key`]
//! refuses every writer under the three areas. Only [`store`] writes them.
//!
//! - [`body`]: the decision body, its kinds, outcomes and subjects, and its
//!   size caps.
//! - [`decider`]: the decider, agent, run mode and authority, from koto's
//!   own variables and keys.
//! - [`chain`]: the chain snapshot over dispatched-by and held-by links.
//! - [`store`]: the allocator, the create-only writer and the listing of a
//!   session's areas with their integrity checks.
//! - [`gate_pass`]: the decisions koto's own commands record with the
//!   events they append, and the gate-pass decider rules.
//! - [`read`]: the pure read path behind `list` and `show`.
//! - [`standing`]: who may reverse what, where a reversal is recorded,
//!   racing reversals and each decision's status.

pub mod body;
pub mod chain;
pub mod decider;
pub mod gate_pass;
pub mod read;
pub mod standing;
pub mod store;

/// The area holding every decision not aimed at another session's decision.
pub const DECISIONS_AREA: &str = "koto/decisions";

/// The area holding decisions aimed at another session's decision, one
/// directory per target session id.
pub const AIMED_AREA: &str = "koto/aimed";

/// Reserved for a later bulk format; guarded and held empty.
pub const PACKS_AREA: &str = "koto/packs";

/// The three areas, as context-key prefixes without the trailing slash.
pub const AREAS: [&str; 3] = [DECISIONS_AREA, AIMED_AREA, PACKS_AREA];

/// How many digits a decision id has.
pub const ID_WIDTH: usize = 12;

/// The format version every body and every new output carries.
pub const FORMAT_VERSION: u32 = 1;

/// The most entries one area (or one aimed target directory) may hold. A
/// reader treats a larger area as unreadable and a writer refuses to grow
/// one past it, so a runaway session can't make itself unreadable to the
/// sessions below it.
pub const MAX_AREA_ENTRIES: usize = 100_000;

/// Whether `key` lies in one of the three decision areas, or names an area
/// root itself. The first component is compared case-insensitively, as
/// [`crate::session::links::is_reserved_key`] does, so `KOTO/decisions/...`
/// can't reach the same directory on a case-insensitive filesystem; the
/// area component is compared the same way.
pub fn is_decision_area_key(key: &str) -> bool {
    let mut parts = key.split('/');
    let first = parts.next().unwrap_or("");
    let second = parts.next().unwrap_or("");
    first.eq_ignore_ascii_case("koto")
        && ["decisions", "aimed", "packs"]
            .iter()
            .any(|area| second.eq_ignore_ascii_case(area))
}

/// Whether `id` is a decision id: exactly [`ID_WIDTH`] ASCII digits.
pub fn is_decision_id(id: &str) -> bool {
    id.len() == ID_WIDTH && id.bytes().all(|b| b.is_ascii_digit())
}

/// The id `n` formats to.
pub fn format_id(n: u64) -> String {
    format!("{n:0width$}", width = ID_WIDTH)
}

/// The typed error codes the decision commands print.
pub mod codes {
    /// No decider could be established: no `--by`, no id-shaped harness
    /// session identifier and no readable anchor.
    pub const DECIDER_MISSING: &str = "decider_missing";
    /// The key a decision would be written under already exists.
    pub const DECISION_EXISTS: &str = "decision_exists";
    /// The kind, outcome, subject or another field isn't allowed.
    pub const INVALID_DECISION: &str = "invalid_decision";
    /// The decision isn't there.
    pub const DECISION_NOT_FOUND: &str = "decision_not_found";
    /// The bucket can't hold write-once decisions.
    pub const BUCKET_UNSUPPORTED: &str = "decision_bucket_unsupported";
    /// A reversal was refused; the output names the reason from
    /// [`super::standing::reasons`].
    pub const REVERSAL_REFUSED: &str = "reversal_refused";
}

/// A decision command's refusal: a code from [`codes`], the field it names
/// when there is one, and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionError {
    pub code: &'static str,
    pub field: Option<String>,
    pub message: String,
}

impl DecisionError {
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self {
            code: codes::INVALID_DECISION,
            field: Some(field.to_string()),
            message: message.into(),
        }
    }

    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            field: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DecisionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_areas_are_recognised_whatever_their_case() {
        assert!(is_decision_area_key("koto/decisions/000000000001"));
        assert!(is_decision_area_key("KOTO/Decisions/000000000001"));
        assert!(is_decision_area_key("koto/aimed/abc/000000000001"));
        assert!(is_decision_area_key("koto/packs"));
        assert!(is_decision_area_key("koto/decisions"));
        assert!(!is_decision_area_key("koto/links/held-by/a"));
        assert!(!is_decision_area_key("decisions/000000000001"));
        assert!(!is_decision_area_key("koto/decisionsx/1"));
    }

    #[test]
    fn ids_are_twelve_digits() {
        assert_eq!(format_id(42), "000000000042");
        assert!(is_decision_id("000000000042"));
        assert!(!is_decision_id("42"));
        assert!(!is_decision_id("00000000004a"));
        assert!(!is_decision_id("0000000000420"));
    }
}
