//! Who may reverse a decision, and with what standing.
//!
//! Only the original decision's chain decides. A reversal is accepted when
//! its decider is:
//!
//! - the same decider as the original: the same session (whatever agent
//!   within it), the same person handle, or the same check run by the same
//!   session;
//! - for an original decided by a session or a check, a session that
//!   appears in the original's chain, or the person at that chain's root;
//! - for an original decided by a person, the person at the root of the
//!   original's chain. A session can't reverse a person's decision.
//!
//! Matching is on recorded values. The standing is `verified` when the
//! match rests on values both sides observed, and `asserted` when it rests
//! on a handed value; a match through an `incomplete` chain is never
//! `verified`. When nothing matches and the original's chain is
//! `incomplete`, the reversal stands as `asserted`; when nothing matches
//! and the chain is complete, it is refused.
//!
//! [`authorize`] is the one function every caller uses: `koto decisions
//! reverse`, the clearing rule for a reversed gate pass in `koto next`, and
//! the gate-passing commands' refusals.

use super::record::{Chain, DecidedBy, DeciderKind, Slot, Source, Sourced};

/// How well a reversal's decider is placed to reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    /// The match rests on a handed value, or on an incomplete chain.
    Asserted,
    /// The match rests on values both sides observed, through a complete
    /// chain.
    Verified,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Asserted => "asserted",
            Standing::Verified => "verified",
        }
    }

    pub fn parse(s: &str) -> Option<Standing> {
        match s {
            "asserted" => Some(Standing::Asserted),
            "verified" => Some(Standing::Verified),
            _ => None,
        }
    }

    /// Whether this standing is enough to clear something that stands
    /// with `other`: a `verified` reversal is cleared only by a `verified`
    /// match, an `asserted` one by either.
    pub fn meets(self, other: Standing) -> bool {
        self >= other
    }
}

/// Why a reversal was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A session or check tried to reverse a person's decision.
    SessionOverPerson,
    /// Nothing matched and the original's chain is complete.
    NoStanding,
}

impl Refusal {
    /// The machine-readable reason a `refused` decision records.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::SessionOverPerson => "session_over_person",
            Refusal::NoStanding => "no_standing",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Refusal::SessionOverPerson => {
                "a session can't reverse a person's decision; only the person at the root of its chain can"
            }
            Refusal::NoStanding => {
                "the reverser is neither the original decider nor in the original's chain, and that chain is complete"
            }
        }
    }
}

const SESSION_PREFIX: &str = "session:";
const PERSON_PREFIX: &str = "person:";

fn standing_of(a: Source, b: Source, through_complete_chain: bool) -> Standing {
    if a == Source::Observed && b == Source::Observed && through_complete_chain {
        Standing::Verified
    } else {
        Standing::Asserted
    }
}

/// The best match between two sets of values: every `a` value compared
/// with every `b` value.
fn best_match<'a>(
    a: impl Iterator<Item = &'a Sourced> + Clone,
    b: impl Iterator<Item = (&'a str, Source)>,
    complete: bool,
) -> Option<Standing> {
    let mut best = None;
    for (bv, bs) in b {
        for av in a.clone() {
            if av.value == bv {
                let s = standing_of(av.source, bs, complete);
                best = best.max(Some(s));
            }
        }
    }
    best
}

fn prefixed<'a>(slot: &'a Slot, prefix: &'a str) -> impl Iterator<Item = (&'a str, Source)> + 'a {
    slot.iter()
        .filter_map(move |v| v.value.strip_prefix(prefix).map(|rest| (rest, v.source)))
}

fn plain(slot: &Slot) -> impl Iterator<Item = (&str, Source)> {
    slot.iter().map(|v| (v.value.as_str(), v.source))
}

/// Whether `by` is the same decider as `orig`, and with what standing.
fn same_decider(orig: &DecidedBy, by: &DecidedBy) -> Option<Standing> {
    match (orig.kind, by.kind) {
        (DeciderKind::Session, DeciderKind::Session) => {
            best_match(by.session.iter(), plain(&orig.session), true)
        }
        // A handle is always handed, so this is never verified.
        (DeciderKind::Person, DeciderKind::Person) => {
            best_match(by.handle.iter(), plain(&orig.handle), true)
        }
        (DeciderKind::Check, DeciderKind::Check) if orig.name.is_some() && orig.name == by.name => {
            best_match(by.ran_in.iter(), plain(&orig.ran_in), true)
        }
        _ => None,
    }
}

/// The person at the root of a chain: the last link, when it names one.
fn root_person(chain: &Chain) -> Option<&Slot> {
    chain
        .links
        .last()
        .filter(|slot| slot.iter().any(|v| v.value.starts_with(PERSON_PREFIX)))
}

/// Decide whether `by` may reverse a decision made by `orig_by` with chain
/// `orig_chain`, and with what standing.
pub fn authorize(
    orig_by: &DecidedBy,
    orig_chain: &Chain,
    by: &DecidedBy,
) -> Result<Standing, Refusal> {
    let complete = !orig_chain.incomplete;
    let mut best = same_decider(orig_by, by);

    if by.kind == DeciderKind::Person {
        if let Some(root) = root_person(orig_chain) {
            best = best.max(best_match(
                by.handle.iter(),
                prefixed(root, PERSON_PREFIX),
                complete,
            ));
        }
    }
    if orig_by.kind != DeciderKind::Person && by.kind == DeciderKind::Session {
        for link in &orig_chain.links {
            best = best.max(best_match(
                by.session.iter(),
                prefixed(link, SESSION_PREFIX),
                complete,
            ));
        }
    }

    if let Some(s) = best {
        return Ok(s);
    }
    if orig_by.kind == DeciderKind::Person && by.kind != DeciderKind::Person {
        return Err(Refusal::SessionOverPerson);
    }
    if orig_chain.incomplete {
        return Ok(Standing::Asserted);
    }
    Err(Refusal::NoStanding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(v: &str) -> Sourced {
        Sourced::observed(v)
    }
    fn h(v: &str) -> Sourced {
        Sourced::handed(v)
    }
    fn session(values: Vec<Sourced>) -> DecidedBy {
        DecidedBy::session(values, vec![o("main")], false)
    }
    fn person(handle: &str) -> DecidedBy {
        DecidedBy::person(
            vec![h(handle)],
            "terminal".into(),
            None,
            vec![o("entering")],
        )
    }
    fn chain(links: Vec<Vec<Sourced>>, incomplete: bool) -> Chain {
        Chain { links, incomplete }
    }
    fn complete_from(coord: Sourced) -> Chain {
        chain(vec![vec![coord], vec![o("none")]], false)
    }

    #[test]
    fn the_same_session_observed_on_both_sides_is_verified() {
        let orig = session(vec![o("w")]);
        let c = chain(vec![], true);
        assert_eq!(
            authorize(&orig, &c, &session(vec![o("w")])),
            Ok(Standing::Verified)
        );
        // Another agent in the same session is the same decider.
        let mut other_agent = session(vec![o("w")]);
        other_agent.agent = vec![o("agent-2")];
        assert_eq!(authorize(&orig, &c, &other_agent), Ok(Standing::Verified));
    }

    #[test]
    fn a_handed_session_match_is_asserted() {
        let orig = session(vec![o("w")]);
        let c = complete_from(o("session:coord"));
        assert_eq!(
            authorize(&orig, &c, &session(vec![h("w")])),
            Ok(Standing::Asserted)
        );
    }

    #[test]
    fn a_session_in_the_chain_may_reverse() {
        let orig = session(vec![o("w")]);
        let c = complete_from(o("session:coord"));
        assert_eq!(
            authorize(&orig, &c, &session(vec![o("coord")])),
            Ok(Standing::Verified)
        );
        let handed = complete_from(h("session:coord"));
        assert_eq!(
            authorize(&orig, &handed, &session(vec![o("coord")])),
            Ok(Standing::Asserted)
        );
    }

    #[test]
    fn a_match_through_an_incomplete_chain_is_never_verified() {
        let orig = session(vec![o("w")]);
        let c = chain(vec![vec![o("session:coord")]], true);
        assert_eq!(
            authorize(&orig, &c, &session(vec![o("coord")])),
            Ok(Standing::Asserted)
        );
    }

    #[test]
    fn nothing_matching_a_complete_chain_is_refused_and_an_incomplete_one_asserted() {
        let orig = session(vec![o("coord")]);
        let complete = chain(vec![vec![o("none")]], false);
        assert_eq!(
            authorize(&orig, &complete, &session(vec![o("worker")])),
            Err(Refusal::NoStanding)
        );
        let incomplete = chain(vec![], true);
        assert_eq!(
            authorize(&orig, &incomplete, &session(vec![o("worker")])),
            Ok(Standing::Asserted)
        );
    }

    #[test]
    fn a_session_never_reverses_a_persons_decision() {
        let orig = person("ana");
        let c = chain(vec![vec![o("session:coord")], vec![h("person:ana")]], false);
        assert_eq!(
            authorize(&orig, &c, &session(vec![o("coord")])),
            Err(Refusal::SessionOverPerson)
        );
        // Even through an incomplete chain.
        let inc = chain(vec![], true);
        assert_eq!(
            authorize(&orig, &inc, &session(vec![o("coord")])),
            Err(Refusal::SessionOverPerson)
        );
        assert_eq!(authorize(&orig, &c, &person("ana")), Ok(Standing::Asserted));
    }

    #[test]
    fn the_person_at_the_root_may_reverse_a_sessions_decision() {
        let orig = session(vec![o("w")]);
        let c = chain(vec![vec![o("session:coord")], vec![h("person:ana")]], false);
        assert_eq!(authorize(&orig, &c, &person("ana")), Ok(Standing::Asserted));
        assert_eq!(
            authorize(&orig, &c, &person("bo")),
            Err(Refusal::NoStanding)
        );
    }

    #[test]
    fn a_check_is_the_same_decider_only_with_its_name_and_session() {
        let orig = DecidedBy::check("grader".into(), vec![o("s")]);
        let c = chain(vec![vec![o("none")]], false);
        assert_eq!(
            authorize(&orig, &c, &DecidedBy::check("grader".into(), vec![o("s")])),
            Ok(Standing::Verified)
        );
        assert_eq!(
            authorize(&orig, &c, &DecidedBy::check("other".into(), vec![o("s")])),
            Err(Refusal::NoStanding)
        );
    }

    #[test]
    fn standing_meets_its_own_level_or_lower() {
        assert!(Standing::Verified.meets(Standing::Verified));
        assert!(Standing::Verified.meets(Standing::Asserted));
        assert!(Standing::Asserted.meets(Standing::Asserted));
        assert!(!Standing::Asserted.meets(Standing::Verified));
    }
}
