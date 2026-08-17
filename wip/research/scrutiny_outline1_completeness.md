# Verdict: PASS

Commit `9532a95`, `src/engine/persistence.rs` only (confirmed by `git show --stat`:
one file, +179/-58). All nine acceptance criteria of PLAN outline 1 are met. Five
findings, all MINOR; none blocks.

## Criterion walk

**AC1 — private `Boundary` enum, single private scan takes it, no call site
outside the two wrappers names a boundary value. MET.**
`enum Boundary { AnyEntry, ArrivalFromElsewhere }` at persistence.rs:1002-1008, no
`pub`, `#[derive(Clone, Copy, PartialEq, Eq)]` as the DESIGN sketch has it.
`fn entry_slice<'a>(events, current_state, boundary)` at :1039 is the one scan and
is private. `grep -n "Boundary::" src/engine/persistence.rs` returns exactly four
hits: :1044 and :1047 inside `entry_slice`'s own match, and :1077 / :1090 in the
two wrappers. Nothing outside names a value.

**AC2 — `epoch_slice` replaces `occupancy_slice`, behaviorally identical. MET.**
Proved arm by arm in the Case walk's identity section below. The name
`occupancy_slice` no longer exists anywhere in the crate (`grep -rn occupancy_slice
--include=*.rs` hits only the two `src/cli/` call sites still holding the old
*function* name, which is outline 2's).

**AC3 — `delivery_window` skips a `Transitioned`/`DirectedTransition` whose source
equals its target; the `Rewound` arm does not bind the source. MET.** :1042-1049:

```rust
EventPayload::Transitioned { from, to, .. } => {
    to == current_state
        && (boundary == Boundary::AnyEntry || from.as_deref() != Some(current_state))
}
EventPayload::DirectedTransition { from, to, .. } => {
    to == current_state && (boundary == Boundary::AnyEntry || from != current_state)
}
EventPayload::Rewound { to, .. } => to == current_state,
```

`Rewound` binds `to` and `..` and nothing else — matching the DESIGN sketch at
DESIGN:264 verbatim, including the reason (a field never read cannot be depended
on by accident). Type check: `Transitioned.from` is `Option<String>` and
`DirectedTransition.from` is `String` (types.rs:470, :507), so `.as_deref()` on one
and a bare `!=` on the other is right, not an inconsistency.

**AC4 — `latest_epoch_gate_failed` reads `epoch_slice`; delivery check reads
`delivery_window` and is renamed. MET.** :1112 `epoch_slice(events, current_state)`;
:1159-1160 `pub fn instructions_delivered_this_window(..)` over
`delivery_window(events, current_state)`. Body of the `.any(..)` predicate byte-for-byte
the old one.

**AC5 — exactly two assertions invert, both tests renamed. MET.**
Inverted #1: `instructions_delivered_resets_on_a_self_transition` →
`instructions_delivered_survives_a_self_transition` (:2655), first assertion
`assert!(!…)` → `assert!(…)`. Inverted #2:
`instructions_delivered_resets_on_arrival_by_directed_transition` →
`instructions_delivered_survives_a_directed_self_transition` (:2755), the assertion
after `directed(4, "implement", "implement")`, `assert!(!…)` → `assert!(…)`. The
other two assertions named by the criterion (second of the first test, first of the
second) still assert `true`. No third inversion exists in the diff. See Finding 1
for a setup rewrite under the first test's second assertion.

**AC6 — no other case in the module changes the boolean it asserts. MET.** Every
other touched line in the test module is a bare identifier swap
`instructions_delivered_this_occupancy` → `instructions_delivered_this_window`;
polarity preserved in all of: `false_when_nothing_was_delivered`,
`true_within_the_current_occupancy`, `false_when_the_record_predates_the_entry_event`,
`resets_on_arrival_by_rewind`, `ignores_an_intermediate_phases_record` (both),
`reads_the_whole_log_when_no_entry_event_names_the_state` (both). Twelve pre-change
assert lines, all touched by the rename, none flipped — exactly what the criterion
predicted a diff-shaped check would show.

**AC7 — three new unit cases. MET.**
`instructions_delivered_false_on_a_self_transition_with_no_record` (:2673),
`instructions_delivered_resets_on_a_same_phase_rewind` (:2687),
`the_epoch_and_the_delivery_window_disagree_across_a_self_transition` (:2703), the
last asserting `!latest_epoch_gate_failed` and `instructions_delivered_this_window`
over one log. See Finding 2 on the third one's entry event.

**AC8 — four derive walks unchanged, two consumer files no diff. MET.**
`derive_evidence` (:722), `derive_overrides` (:796), `derive_last_gate_evaluated`
(:844) and `derive_visit_counts` (:981) all sit above the first hunk (`@@ -994`)
and are untouched. `git show --stat` lists one file, so
`src/cli/dashboard_data.rs` and `src/workflows_surface/project.rs` have no diff.
No derive walk was folded into `epoch_slice`.

**AC9 — doc comments state which boundary and why they differ; the
cannot-disagree sentence is gone. MET.** `entry_slice` (:1010-1038),
`epoch_slice` (:1064-1075), `delivery_window` (:1080-1088),
`latest_epoch_gate_failed` (:1093-1110) and `instructions_delivered_this_window`
(:1125-1158) each name their boundary. The old
"Shared rather than copied so the predicates built on it … cannot come to
disagree about where an occupancy starts" is deleted and replaced at :1074-1075 by
"Sharing one scan is what keeps them from disagreeing about anything else," which
is the surviving true claim. The one remaining "cannot disagree" (:1132) is about
the two `koto next` *call sites* sharing the combinator, a different and still
correct statement.

## `epoch_slice` ≡ `occupancy_slice`, arm by arm

Old (`git show 9532a95^:src/engine/persistence.rs`) built `to: Option<&str>` from
three variants and `None` otherwise, then tested `to == Some(current_state)`.
New, with `boundary == Boundary::AnyEntry` substituted as `true`:

| Payload | Old evaluates to | New under `AnyEntry` | Same? |
|---|---|---|---|
| `Transitioned{to}` | `Some(to) == Some(cs)` | `to == cs && (true \|\| _)` — the `from` test is short-circuited and never evaluated | yes |
| `DirectedTransition{to}` | `Some(to) == Some(cs)` | `to == cs && (true \|\| _)` | yes |
| `Rewound{to}` | `Some(to) == Some(cs)` | `to == cs` | yes |
| anything else | `None == Some(cs)` → false | `_ => false` | yes |

Iteration is the same `events.iter().enumerate().rev().find_map(..)` — same
direction, same "last match wins". The tail is unmodified source:

```rust
match start {
    Some(idx) => &events[idx + 1..],
    None => events,
}
```

Both arms are literally the old lines (the diff does not touch them), so the
`Some` slice and the `None` whole-log fallback are identical. The short-circuit
matters and holds: under `AnyEntry` the `from` field is never read on either
transition arm, so no `from`-shaped datum can influence the epoch. Identity is
total, not case-by-case.

## Case walk — `delivery_window` against the PRD

`E` = the qualifying entry the backwards scan stops at; window = everything after
it. "Delivers" means no `InstructionsDelivered{state: P}` inside the window.

| Arrival case | Event(s) | Code stops at | Returns | PRD requires | Match |
|---|---|---|---|---|---|
| Initial entry | `Transitioned{from: None, to: P}` | that event (`None.as_deref() != Some(P)` is true) | window empty → delivers | R2 delivers | yes |
| Conditional arrival from another phase | `Transitioned{from: Q, to: P}` | that event | delivers | R3 | yes |
| Unconditional arrival | same variant, `condition_type` differs and is never read | that event | delivers | R3 | yes |
| Directed arrival at a different phase | `DirectedTransition{from: Q, to: P}` | that event (`Q != P`) | delivers | R3 | yes |
| Loop-back from a later phase | `Transitioned{from: R, to: P}` | that event | delivers | R3 | yes |
| Leave P, pass through Q, return to P in one tick | `…, {P→Q}, {Q→P}` | the `{Q→P}` — `.rev()` takes the *last* qualifying entry, and its source is `Q` | delivers | R3's explicit same-tick clause | yes |
| Self-transition | `{Q→P}, delivered(P), {P→P}` | the `{Q→P}` — `{P→P}` fails `from != to` | record in window → omits | R4 omits | yes |
| Repeated self-transition | `{Q→P}, delivered(P), {P→P}, {P→P}, …` | still the `{Q→P}`; every self-entry is skipped, no matter how many | omits | R4 "however many consecutive" | yes |
| Directed transition into the occupied phase | `{Q→P}, delivered(P), directed{P→P}` | the `{Q→P}` | omits | R5 (= R4) | yes |
| Rewind from elsewhere | `Rewound{from: R, to: P}` | that event; `from` unbound | window after it empty → delivers | R6 delivers | yes |
| Rewind recording the same phase twice | `Rewound{from: P, to: P}` | that event; the arm tests `to` only | delivers | R6 "whether or not … source equals … target" | yes |

Two cases outside the enumerated list that the widened window newly reaches, both
checked and both right:

- **Initial phase self-loops.** Initialization appends no entry event (the old doc
  said so and the fallback exists for it). Log `[delivered(P), {P→P}]`: the
  self-entry is skipped, no qualifying entry exists, the fallback returns the whole
  log, the record is found, and the response omits — R4 satisfied *through* the
  fallback arm rather than around it. Under the old boundary this case re-delivered.
- **Self-entry with no record at all.** `[{init→gather}, {gather→review}, {review→review}]`
  asked about `review`: window is `[{review→review}]`, no record, delivers. This is
  R1 and the DESIGN's crash-recovery argument (DESIGN:297-305) — the rule stays
  keyed on the record, not on the entry event's shape.

Non-entry events (`GateEvaluated`, `GateOverrideRecorded`, `EvidenceSubmitted`,
`DecisionRecorded`, scheduler/batch/wake) all fall to `_ => false` and neither open
nor close a window: R13, R7 unchanged.

## Discrimination of the three new tests

**`instructions_delivered_false_on_a_self_transition_with_no_record` (:2673).**
*Catches:* the tempting wrong fix — suppress on the *shape* of the entry event
("last entry naming P is a self-entry → already delivered"). That implementation
returns `true` here with no record in the log and the assertion fails. This is the
test the DESIGN asks for by name.
*Does not catch:* anything about the old versus the new boundary. The pre-change
`occupancy_slice` also returns `false` on this log, so the test passes on both
implementations; it is a guard against a specific wrong fix, not a regression test
for the change. It is also blind to the `Rewound` arm and to the whole-log fallback
(no record exists, so the fallback answers `false` too).

**`instructions_delivered_resets_on_a_same_phase_rewind` (:2687).**
*Catches:* the alternative the DESIGN explicitly rejects at :283-287 — apply
`from != to` uniformly to all three variants. Under that implementation
`rewound(4, "review", "review")` stops being an opener, the scan walks back past
`transitioned(3, review→review)` to `transitioned(1, None, "review")`, finds
`delivered(2)` in the window, and returns `true` where the test demands `false`.
This is the sharpest test of the three, and it is the one that pins R6.
*Does not catch:* a rewind from a *different* phase being mishandled (no case
here has `from != to` on a `Rewound`; the pre-existing
`instructions_delivered_resets_on_arrival_by_rewind` covers that), and nothing
about `DirectedTransition`.

**`the_epoch_and_the_delivery_window_disagree_across_a_self_transition` (:2703).**
*Catches:* both directions of collapsing the split. Widen the shared scan in place
(make the epoch use `ArrivalFromElsewhere` too, or invert the short-circuit to
`boundary == Boundary::ArrivalFromElsewhere ||`) and the epoch reaches back past the
self-transition to the failed `GateEvaluated` at seq 3, so
`assert!(!latest_epoch_gate_failed(..))` fails. Leave the delivery boundary
un-split and `assert!(instructions_delivered_this_window(..))` fails. It is the
R15 test and it earns its comment.
*Does not catch:* an epoch that breaks in the *always-false* direction (see
Finding 4), and nothing about the `DirectedTransition` arm of `epoch_slice` — the
log is `Transitioned`-only, so a mistake that made a directed self-entry stop
closing the epoch would go unseen here.

## Findings

**1. MINOR — the second half of `instructions_delivered_survives_a_self_transition`
was rewritten, not left alone.** AC5 says the first test's second assertion is
"already true under the new rule and stay[s] as [it is]". The diff replaced its
setup: `events.push(delivered(4, "review"))` became
`events.push(transitioned(4, Some("review"), "review"))`. The asserted boolean is
still `true`, so "exactly two assertions invert" holds on the letter of the check
that matters. The rewrite is a net gain — under the new rule the old line asserted
nothing `delivered(2)` had not already established, whereas the new one pins R4's
"however many consecutive self-transitions precede it". *Fix:* none needed; recorded
so the AC5 audit is not read as clean when one line moved.

**2. MINOR — the R15 disagreement test opens its window with an initialization
event, not the "cross-phase entry" AC7 specifies.** persistence.rs:2707 is
`transitioned(1, None, "review")`, whose `from` is `None`. Both boundaries open on
it, so the disagreement the test demonstrates is real and the assertions are sound;
only the enumerated shape differs. *Fix, if the letter is wanted:* prepend
`transitioned(1, None, "gather")` and make the entry `transitioned(2, Some("gather"),
"review")`, shifting the later seqs. One line, no assertion change.

**3. MINOR — the fallback's explanation was dropped exactly when it became more
load-bearing.** The old doc read "With no entry event naming the phase, every event
is in scope. The only phase that can be in that position is the initial one, which
the workflow occupies before it has transitioned anywhere." The new `entry_slice`
doc keeps only the first sentence (:1030). Under `ArrivalFromElsewhere` the fallback
is what makes the initial phase's self-loop suppress — log `[delivered(P), {P→P}]`
has no opener at all — so a reader who takes the fallback for a dead edge case will
misjudge the one arm doing that work. *Fix:* restore a sentence naming the initial
phase and noting that a self-loop there is answered by the whole-log arm.

**4. MINOR — nothing in this module asserts `latest_epoch_gate_failed` returns
`true`.** Its only unit assertion in the crate is the new negative one at
persistence.rs:2727. AC2's identity claim is carried entirely by the code argument
above plus `tests/native_workflows_shape.rs:124` (`assert_eq!(v["status"],
"blocked")`) at the integration level. An `epoch_slice` broken in the always-empty
direction would pass every unit test in this file. The identity is genuinely total,
so this is coverage hygiene, not a defect. *Fix:* one positive case — cross-phase
entry, failed `GateEvaluated`, assert `latest_epoch_gate_failed` is `true` — costs
eight lines and closes the direction.

**5. MINOR — stale vocabulary in an untouched test name.**
`instructions_delivered_true_within_the_current_occupancy` (:2613) still says
"occupancy" for what the module now calls a delivery window. No AC requires the
rename and the name asserts nothing false, so the plan's whole-plan grep (which
looks for lines claiming a self-transition re-delivers) will not catch it.
*Fix:* rename to `…_within_the_current_window`.

## Summary

All nine acceptance criteria are met against the actual diff, not the commit
message: the enum and single scan are private with no boundary named outside the two
wrappers, `epoch_slice` is provably identical to `occupancy_slice` on all four match
arms and both fallback arms because `AnyEntry` short-circuits the `from` test away,
`delivery_window` gives the PRD-required answer on all eleven arrival cases plus the
initial-phase self-loop that only the whole-log arm reaches, exactly two assertions
inverted, and the four derive walks and two consumer files are untouched.

The three new tests are real discriminators between them — the same-phase rewind
test kills the uniform `from != to` alternative the DESIGN rejects, the disagreement
test kills collapsing the split in either direction — though the no-record test
passes on the pre-change implementation too and is a guard against a wrong fix
rather than a regression test.

Five MINOR findings, no BLOCKING: a setup line rewritten where AC5 said one would
stay, an initialization event standing in for AC7's cross-phase entry, a dropped
doc sentence about the fallback that is now the arm carrying the initial-phase
self-loop, no positive assertion anywhere that the gate epoch still fires, and one
test name still saying "occupancy". Verdict PASS.
