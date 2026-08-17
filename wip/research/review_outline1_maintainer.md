# Verdict: PASS

Reviewed `git diff 9532a95^..a7fc426 -- src/engine/persistence.rs` at HEAD (`a7fc426`),
branch `docs/self-loop-suppresses-details`. Read-only; no build, no `koto`, no writes
outside this file.

Nothing here blocks. The central "why" is present and unusually well argued: the
`epoch_slice` doc names the failure mode of unifying the two boundaries, names its
mechanism, names its direction, and names the one test that catches it. The
`instructions_delivered_this_window` paragraph on why the rule stays keyed on the
recorded delivery rather than on the shape of the entry event is the best writing in
the diff, and it has a matching test. A maintainer who reads only the code — never the
DESIGN — will understand why the two boundaries differ and what breaks if they are
merged. That was the bar, and it clears it.

The findings below are about the seams: one name that asserts a rule the code
deliberately breaks, one field whose accuracy became load-bearing without being
declared, and one sibling predicate whose own unread field is now conspicuous next to
its neighbor's carefully justified one.

## Findings

### 1. MODERATE — `ArrivalFromElsewhere` names a rule the code breaks for one of its three arms, and its doc line contradicts itself

`persistence.rs:1006-1007`:

```rust
/// Only an entry from a different phase, plus every rewind.
ArrivalFromElsewhere,
```

"Only an entry from a different phase" and "plus every rewind" are the same sentence
saying opposite things, because a `Rewound { from: "review", to: "review" }` is not
from a different phase and still opens the window (`persistence.rs:1054`). The variant
is named for the rule that governs two of the three arms; the third is an exception
the name gives no hint of. Read at the call site — `entry_slice(events, current_state,
Boundary::ArrivalFromElsewhere)` in `delivery_window` — a newcomer concludes a
same-phase rewind is excluded. That is the exact opposite of the truth, and the branch
has a dedicated test for it (`instructions_delivered_resets_on_a_same_phase_rewind`).

The same slippage appears in prose. `persistence.rs:1004` — "Every entry naming the
phase, a self-entry included" — and `persistence.rs:1077-1078` — "so they now disagree
across a self-entry, on purpose." Take "self-entry" at its word and apply it to a
same-phase rewind: the two boundaries do *not* disagree there, they both close. The
term is loose in precisely the place where the distinction is load-bearing.

**Why it matters**: this is the one name in the diff a maintainer reads wrong on first
encounter, and reading it wrong points them at the rewind arm as the inconsistency to
tidy up.

**Fix**: name the variant for what the flag actually does rather than for the intent it
mostly serves — `Boundary::SkipsSelfTransition` (accurate for all three arms, since only
`Transitioned`/`DirectedTransition` self-edges are skipped) or, matching the wrappers,
`Boundary::Epoch` / `Boundary::Delivery`. Whichever is chosen, replace the
self-contradicting doc line with one that states the rewind exception as an exception,
and tighten "self-entry" to "self-transition" at 1004 and 1077 so it excludes rewinds.

### 2. MODERATE — `from` became behaviorally load-bearing in this diff; the field inventory does not say so

`persistence.rs:1037-1039` still reads:

> Only inspects `.payload` and position within `events`; `.seq`, `.timestamp`,
> `.event_type`, and `.idempotency_hash` never factor in.

True, and inherited verbatim from the old doc — but before this commit the scan read
only `to`. It now reads `from` on two of three variants, so the delivery answer depends
on every producer of `Transitioned` and `DirectedTransition` recording an accurate
source phase. That contract is stated nowhere in this module. Two specifics a
maintainer would want and does not get: `Transitioned.from` is `Option<String>` while
`DirectedTransition.from` is `String` (which is the only reason the two arms are
written differently — `from.as_deref() != Some(current_state)` versus `from !=
current_state`, `persistence.rs:1049,1052`), and a `Transitioned` whose `from` is
missing from the JSONL deserializes to `None` (serde's implicit `Option` default,
`types.rs:1405`) and therefore reads as an arrival from elsewhere. That fallback errs
toward re-delivering, which is the safe direction — but it is a silent policy decision
nobody wrote down.

**Why it matters**: the doc block deliberately enumerates what is *not* read so callers
know what they may fake. A caller synthesizing an `Event` — which the directed path in
`src/cli/mod.rs` does, on this doc's explicit invitation — now must get `from` right,
and the invitation does not say so.

**Fix**: extend the inventory sentence to name `from` as read and state the consequence
in one line: `Transitioned.from` absent or `None` reads as arrival from a different
phase and re-opens the window, which is the deliberate safe direction.

### 3. MODERATE — `latest_epoch_gate_failed` ignores `GateEvaluated.state`; its sibling spends a paragraph justifying the opposite choice

`persistence.rs:1131` matches `EventPayload::GateEvaluated { outcome, .. }` — any gate
evaluation inside the epoch decides, whatever phase it names. Twenty lines below,
`instructions_delivered_this_window` takes the other road and explains itself at
length:

> The record is matched on the phase it names as well as on its position in the slice.
> Position alone would be enough for the ordinary case ... The name check answers the
> cases where that does not hold: a record for another phase landing inside this
> window, and the fallback where no entry event names the phase and the slice is the
> whole log.

If a record for another phase can land inside the delivery window, the analogous
`GateEvaluated` can land inside the epoch, and the gate predicate would take it. The
asymmetry is pre-existing and out of this outline's scope to change — but the diff
rewrote this function's doc comment and had the opening. Worse, the diff adds a test
helper whose signature makes the field look load-bearing:

```rust
fn gate_evaluated(seq: u64, state: &str, gate: &str, outcome: &str) -> Event
```

`the_epoch_and_the_delivery_window_disagree_across_a_self_transition` passes
`"review"` there. A reader assumes that argument is what scopes the gate to the phase.
It is inert.

**Why it matters**: the next developer touching either predicate has to discover by
experiment which of two neighbouring functions scopes by name and which does not, with
the documentation pointing the wrong way.

**Fix**: one sentence on `latest_epoch_gate_failed` saying the phase is scoped by the
epoch slice alone and `GateEvaluated.state` is deliberately not consulted — with the
reason, if there is one, or an explicit "unlike the delivery check" if it is simply
older. A `// state deliberately unread` at the new test helper's call site would close
the smaller half.

### 4. MINOR — "window" is both the generic term for any scan slice and the name of one specific boundary

`entry_slice`'s doc uses "window" generically throughout — "Which state-entry events
close a scan window" (`:997`), "every one of them naming the phase closes the
window", "A rewind opens both windows whatever phases it records" — while
`delivery_window`, `instructions_delivered_this_window`, and the epoch doc's "the wider
window does" (`:1084`) use it for one specific boundary. The epoch/delivery pair reads
cleanly; "window" alone does not, and `instructions_delivered_this_window` gives the
public name no way to say *which*.

**Fix**: prose only — reserve "window" for the delivery boundary and use "slice" or
"scan" for the generic in `entry_slice`'s and `Boundary`'s docs. The public function
name is fixed by the plan and is fine once the prose stops competing with it.

### 5. MINOR — the rewind arm's rationale sits 20 lines from the arm

`persistence.rs:1054` is `EventPayload::Rewound { to, .. } => to == current_state,`,
with no marker distinguishing "no boundary check needed here" from "boundary check not
yet added here." The justification is real and bolded — "**The rewind arm deliberately
does not bind `from`.**" at `:1020` — but it is at the top of a 34-line doc block,
which is not where the "make these three arms consistent" edit happens. The stated
reason is also the mechanical one (not reading the field keeps the answer independent
of rewind's destination choice); the semantic one — a rewind means redo, and a redoing
agent needs the procedure again — lives in `delivery_window`'s doc, a function away.

Mitigated: `instructions_delivered_resets_on_a_same_phase_rewind` fails if someone
"fixes" it, and its name says what broke. That net is why this is MINOR rather than
MODERATE.

**Fix**: `// no boundary check: a rewind opens both windows, see above` on the arm.

### 6. MINOR — `epoch_slice`'s doc calls itself a predicate

`persistence.rs:1082-1083`: "**Unifying them breaks the dashboard, silently.** This
predicate takes the *latest* gate evaluation inside its slice". `epoch_slice` returns a
slice; the predicate is `latest_epoch_gate_failed`. The claim is correct about the
right function and lands on the wrong one, in the sentence carrying the whole argument.

**Fix**: "The predicate built on it takes the *latest* gate evaluation inside the
slice."

### 7. MINOR — a precondition cross-reference points at a doc that states no precondition

`persistence.rs:1152`: "The same precondition applies as for the epoch: `current_state`
is the phase the workflow currently occupies." A reader who follows "the epoch" to
`epoch_slice` finds no precondition — it is stated on the private `entry_slice`
(`:1029`). The sentence restates it inline so nobody is starved, but the pointer is a
dead end, and `latest_epoch_gate_failed` (the public function that actually carries the
same precondition) never states it at all.

**Fix**: state the precondition on `epoch_slice` and `delivery_window`, or point both
public functions at `entry_slice` directly.

### 8. MINOR — the append-order precondition is never stated, and position-dependence is framed only as a feature

`entry_slice` takes the *last* qualifying index by scanning `.rev()`, so correctness
depends on `events` arriving in append order. The doc addresses the neighbouring fact —
`.seq` is not consulted, therefore callers may pass placeholder metadata — as a
convenience, and never states its cost: a caller that filters, sorts, or concatenates
event lists out of order gets a wrong answer with no diagnostic. The directed path's
in-memory concatenation happens to preserve order; nothing says it must.

**Fix**: one clause on the existing sentence — position is authoritative *because*
`.seq` is not read, so `events` must be in append order.

### 9. MINOR — the scenario the doc singles out as newly load-bearing has no test

`persistence.rs:1033-1036`: "Under [`Boundary::ArrivalFromElsewhere`] the whole-log arm
does more work than that: a self-transition on the initial phase leaves a log with no
opener at all, and this arm is what makes that lap suppress." No test covers that
combination. `instructions_delivered_when_no_entry_event_names_the_state` covers the
no-opener fallback without a self-transition;
`instructions_delivered_survives_a_self_transition` covers the self-transition but its
log opens with `transitioned(1, None, "review")`, which *is* an opener under this
boundary, so the fallback arm never runs. The fallback arm itself is covered (change it
to `&[]` and the first test fails), so the risk is narrow — but the doc asserts a
specific behavior the suite does not hold.

**Fix**: push `transitioned(2, Some("gather"), "gather")` onto the existing
no-entry-event log and assert delivery still reports true.

### 10. MINOR — `derive_visit_counts` sits directly above and now embodies a third, undeclared notion of "entry"

`persistence.rs:981` walks the same three variants with its own inline match and counts
every entry including self-transitions. The plan deliberately leaves it alone. But a
maintainer reading top-to-bottom meets three walks over the same event kinds in eighty
lines and gets told why two of them differ and nothing about the third — the obvious
next question ("should visit counts follow the delivery boundary too?") has a decided
answer nobody wrote down.

**Fix**: one clause in `Boundary`'s doc noting that the independent walks above
deliberately keep the any-entry notion and are not folded in. Putting it there rather
than on `derive_visit_counts` keeps that function's zero-diff requirement intact.

## On the tests specifically

The names do say what they assert, and the two renames earn their keep:
`instructions_delivered_survives_a_self_transition` and
`instructions_delivered_survives_a_directed_self_transition` state the new rule in the
name rather than describing a setup.
`the_epoch_and_the_delivery_window_disagree_across_a_self_transition` is the right name
for a sentinel and is correctly cross-referenced from `epoch_slice`'s doc — a maintainer
who breaks the split gets a failure whose name is the design decision they violated.

Failure messages are adequate but not better: every assertion is a bare `assert!`, so
the panic carries the stringified expression and a line number. In the sentinel test,
`assert!(instructions_delivered_this_window(&events, "review"))` appears twice with
identical text and only the line number separating "before the lap" from "after the
lap." A `assert!(cond, "after the lap: ...")` on the four sentinel assertions would make
the failure name the phase of the scenario, not just the predicate. Optional.

## Summary

PASS. The two boundaries are explained, the disagreement is justified where a maintainer
will look, and the sentinel test is named in the doc that would be edited to break it.
Three things to tighten: `ArrivalFromElsewhere` names a rule its own rewind arm breaks
and its doc line contradicts itself in one sentence; `from` became load-bearing without
joining the doc's field inventory, including the `Option`/`String` asymmetry and the
missing-field-reads-as-elsewhere fallback; and `latest_epoch_gate_failed` silently
ignores `GateEvaluated.state` right beside a sibling that justifies the opposite choice
at length, with a new test helper taking a `state` argument the code never reads.
