# Verdict: PASS

Commit `9532a95`, reviewed for intent only (advisory). No file outside this one
was modified, no `koto` subcommand was run, no build or test was run.

## Findings

### 1. MINOR — the first inverted test's second half changed scenario, not just name

PLAN Issue 1 says the two assertions that are *not* inverted "are already true
under the new rule and stay as they are." The first test's second half did not
stay as it was. Before:

```rust
// ...and a delivery after the self-transition does.
events.push(delivered(4, "review"));
assert!(instructions_delivered_this_occupancy(&events, "review"));
```

After:

```rust
// ...and it keeps answering however many laps follow.
events.push(transitioned(4, Some("review"), "review"));
assert!(instructions_delivered_this_window(&events, "review"));
```

**Why it does not block.** No asserted boolean flipped — that is the check the
criterion says actually matters, and it holds. And the dropped case had become a
tautology: under the new rule the assertion is already `true` before
`delivered(4)` is pushed, so pushing a second delivery record proves nothing.
The substitute — a second consecutive lap still suppresses — is the case PRD
line 368 and PLAN Issue 3 both name, and it discriminates. This is an
improvement the plan criterion should have anticipated, not drift from a
decision.

**Fix:** none in code. Say so in the PR body so a reviewer diffing against the
criterion does not read it as an unannounced edit.

### 2. MINOR — the R15 cross-check test opens with an initial entry, not a cross-phase one

PLAN Issue 1 asks for "a cross-phase entry, a delivery record, a failed gate
evaluation, then a self-transition." Shipped
(`the_epoch_and_the_delivery_window_disagree_across_a_self_transition`) opens
with `transitioned(1, None, "review")` — the initial entry.

**Why it does not block.** Both forms are openers under both boundaries, so the
test discriminates exactly as designed. Traced by hand: `epoch_slice` finds the
self-transition at idx 3, returns `[]`, no `GateEvaluated` in scope,
`latest_epoch_gate_failed` is `false`; `delivery_window` skips it and finds the
opener at idx 0, returns `events[1..]`, which holds
`InstructionsDelivered { state: "review" }`, so the delivery check is `true`.
It also does the job the design assigns it — it fails loudly against Considered
Option C: widen `epoch_slice` in place and the failed gate re-enters scope,
blowing the `assert!(!latest_epoch_gate_failed(...))`. Wording divergence only.

### 3. MINOR — a stale intra-doc link will not announce itself to outline 2

`src/cli/next_types.rs:378` reads
`[`crate::engine::persistence::instructions_delivered_this_occupancy`]`. Unlike
`src/cli/mod.rs:2914/3417/4298`, this is an intra-doc link, not a use — it is a
rustdoc warning, and there is no `deny(rustdoc::broken_intra_doc_links)` in
`src/lib.rs`, `src/main.rs` or `Cargo.toml`, so nothing fails on it. PLAN Issue
2 names the combinator's doc comment but not the link inside it.

**Why it matters here.** The commit message's "the crate does not build until
the two call sites follow the rename" is the load-bearing signal to outline 2,
and this fourth reference is precisely the one that reference does not cover.

**Fix (outline 2, not this commit):** `grep -rn instructions_delivered_this_occupancy`
after the call-site edits, not just `cargo build`.

### 4. MINOR — the design's legacy-log safety argument is moot rather than wrong

DESIGN Decision Outcome argues "A legacy log missing the field reads the same
way, which is the safe direction." `TransitionedPayload.from` (`src/engine/types.rs:1405`)
carries no `#[serde(default)]`, and serde does not implicitly default `Option`
fields, so a log line missing `from` fails to deserialize and
`read_log_inner` rejects it as corruption rather than reaching this scan. The
shipped code makes no such claim, so there is nothing to fix in the diff; the
design sentence is simply unreachable.

### 5. Observation — issue 90 AC3 is satisfied for the cases the PRD scoped, not for every reading of "subsequent visits"

Issue 90 AC3 reads "Subsequent visits (retries, self-loops) omit `details` from
the response." A retry modeled as `P -> holding -> P` still re-delivers.
`PRD-self-loop-suppresses-details:558` decides this explicitly — "A tick that
leaves a phase and returns to it delivers" — so it is an upstream ruling, not
drift introduced here. Naming it because AC3's parenthetical says "retries" and
a reader could expect that to cover both shapes.

## Remit answers

**1. Against issue 90 AC3.** Satisfied at this unit's level. Under
`delivery_window`, `Transitioned { from: P, to: P }` and
`DirectedTransition { from: P, to: P }` are no longer window openers, so a
delivery record from before the loop stays in scope and the check reports
already-delivered. Suppression itself is applied by
`with_details_suppressed_unless_full`, wired at the two `src/cli/mod.rs` sites —
outline 2's work. Non-advancing ticks (the other "retry" shape) already
suppressed and still do: they append no state-entry event, so the window does
not move.

**2. Scope of files.** `git show --stat` reports one file, 179 insertions, 58
deletions, all in `src/engine/persistence.rs`. `src/cli/dashboard_data.rs` and
`src/workflows_surface/project.rs` have no diff, as PLAN Issue 1 requires.

**3. The four walks the design leaves alone.** Untouched, verified by line
position rather than by the commit message: `derive_evidence` (`:722`),
`derive_overrides` (`:796`), `derive_last_gate_evaluated` (`:844`) and
`derive_visit_counts` (through `:995`) all sit above the first changed line,
`:997`. The first diff hunk's context line is `derive_visit_counts`'s closing
brace. All four keep the `None => return Vec::new()` / `?` behavior that differs
from the helper's whole-log fallback, which is exactly why the design refuses to
fold them in.

**4. Unintended consequences — every reader of the public surface.**

- `latest_epoch_gate_failed` — answer unchanged for every input.
  `epoch_slice` passes `AnyEntry`, and `boundary == Boundary::AnyEntry` is the
  left operand of the `&&`, so `from` is never even evaluated on that path. The
  three match arms reduce to the old `to == Some(current_state)` test over the
  same three variants, and both `match start` arms are byte-identical to the
  old ones. Its two consumers — `src/cli/dashboard_data.rs:458` and
  `src/workflows_surface/project.rs:183` — therefore see the same answer as
  before, which is what PRD R15 demands of a badge with no test coverage.
- `instructions_delivered_this_occupancy` -> `instructions_delivered_this_window`
  — four references outside the module, all in `src/cli`: the test-module `use`
  at `mod.rs:2914`, the directed path at `mod.rs:3417`, the natural path at
  `mod.rs:4298`, and the doc link at `next_types.rs:378` (finding 3). The first
  three are hard compile errors; that is the intentional outline boundary and
  the commit message states it.
- No `pub use` re-export of either function exists in `src/engine/mod.rs` or
  `src/lib.rs`, and no `tests/*.rs` file names either — so no consumer outside
  `src/cli` exists to be surprised.
- Both call sites pass the **whole** log, not a partial one: the natural path
  re-reads via `backend.read_events`, and the directed path chains the full
  `events` with one synthetic event. So the widened window never degenerates
  into the whole-log fallback by construction of a short list. The natural
  path's `.unwrap_or_default()` on a read failure still yields an empty slice,
  which reports not-delivered and re-delivers — the safe direction, unchanged.
- The directed path is the one whose *answer* moves: `koto next --to P` issued
  at `P` now finds the real arrival's record instead of an empty slice. That is
  the change, stated as such in DESIGN Decision Outcome ("The directed path's
  correctness proof breaks, and that is the change"), and its 24-line proof
  comment at `mod.rs:3377-3402` is now false in the tree. Outline 2 owns it.

**5. The case the design admits is unsafe.** It exists in the shipped code and
it is not reachable in normal operation. With `ArrivalFromElsewhere`, a log
whose only entries naming a phase are self-entries yields `start == None`, the
`None => events` arm returns the whole log, and `.any()` can find an
`InstructionsDelivered` from an earlier visit — suppressing where
`occupancy_slice` would have delivered. Reaching it needs a head-truncated log,
and `read_log_inner` (`src/engine/persistence.rs:651,663-670`) anchors
`expected_seq` at `1` and errors `sequence gap at line N` on the first event
whose `seq` does not match; only the *final* line is recoverable. So a
head-truncated session log is rejected before it ever reaches this scan. The
design's "not reachable through normal operation" understates its own safety
margin — the reader refuses the input outright.

**6. Commit message.** Factual on every claim I checked. It states the split, it
states `epoch_slice`'s behavioral identity (verified above), it states the
rewind arm's deliberate non-binding of `from` (verified at `:1049`), and it
volunteers the build break rather than hiding it. No AI attribution, no
co-author line, no emoji, no overclaim — it does not say tests pass, which they
cannot at this point in the chain. One imprecision: "the two call sites" is
three references in `mod.rs` plus the doc link in `next_types.rs`, though "call
site" fairly names the two response-construction sites the design's component
table lists.

## Design-sketch comparison

| Sketch (DESIGN:225-277) | Shipped (`persistence.rs`) | Verdict |
|---|---|---|
| `#[derive(Clone, Copy, PartialEq, Eq)] enum Boundary` | `:1002-1008`, same derives | same |
| `/// Every entry naming the phase. The gate and evidence epochs.` | `/// Every entry naming the phase, a self-entry included.` | diverges — improvement. The sketch's comment claims the evidence epoch reads this boundary; it does not. `derive_evidence` open-codes its own walk and, per DESIGN:330-336, deliberately keeps it. Shipping the sketch's wording would have planted a false cross-reference. |
| `/// Only an entry from somewhere else. The delivery window.` | `/// Only an entry from a different phase, plus every rewind.` | diverges — improvement. Adds the rewind exemption, which the sketch's comment omits and the sketch's code has. |
| `fn entry_slice<'a>(events, state: &str, boundary) -> &'a [Event]` | `:1039`, param named `current_state` | same (name only) |
| `fn epoch_slice` -> `entry_slice(.., AnyEntry)` | `:1076-1078` | same |
| `fn delivery_window` -> `entry_slice(.., ArrivalFromElsewhere)` | `:1089-1091` | same |
| `Transitioned { from, to, .. } => to == state && (boundary == AnyEntry \|\| from.as_deref() != Some(state))` | `:1042-1045`, identical modulo `state`/`current_state` | same |
| `DirectedTransition { from, to, .. } => to == state && (boundary == AnyEntry \|\| from != state)` | `:1046-1048`, identical | same |
| `Rewound { to, .. } => to == state` — `from` not bound | `:1049`, `from` not bound | same |
| `_ => false` | `:1050` | same |
| `match start { Some(idx) => &events[idx + 1..], None => events }` | `:1058-1061` | same |
| Sketch's inline comment on the `Rewound` arm citing koto#199 | moved up into the `entry_slice` doc block as a bolded paragraph; no `koto#199` number in the code | diverges — neutral. The reasoning survives verbatim in substance and is more visible in the doc block than as an inline comment. Dropping the issue number costs a reader one grep; the file cites no issue numbers elsewhere, so it is consistent. |
| DESIGN:246-248 "`latest_epoch_gate_failed` calls `epoch_slice` ... Neither names a boundary value" | `:1112`; only the two wrappers name a variant | same |
| DESIGN:279-281 "a unit test pins [the `None` arm]" | `instructions_delivered_falls_back_to_the_whole_log`, assertions unchanged | same |
| DESIGN:289-293 initial entry falls out of `from.as_deref() != Some(state)` | holds; no special case in the code | same |
| DESIGN:297-305 "delivery rule stays keyed on the record, not the entry event's shape" | `:1159-1166` unchanged predicate; `:1140-1145` states it; `instructions_delivered_false_on_a_self_transition_with_no_record` pins it | same |

No divergence from the sketch is a drift from a decision. The two substantive
ones both fix comments the sketch got wrong.

## Summary

The commit implements the design's chosen option exactly: one private
`Boundary`-parameterized scan, two named wrappers, no boundary value at any call
site, and a `Rewound` arm that does not bind `from`. `epoch_slice` is
behaviorally identical to `occupancy_slice` under inspection of every match arm
and both fallback arms, so the dashboard's untested blocked badge and its two
consumers cannot have moved; `derive_evidence`, `derive_overrides`,
`derive_last_gate_evaluated` and `derive_visit_counts` are untouched, all four
sitting above the first changed line.

Exactly two asserted booleans invert, both the ones the plan names, and the
three new unit cases the plan requires are present — including the one-log
two-answers test that fails loudly against widening the shared scan in place.
The unsafe whole-log-fallback case the design admits does exist in the shipped
code and is unreachable in normal operation, because `read_log_inner` anchors
the sequence check at 1 and rejects a head-truncated log before this scan sees
it.

Findings are all MINOR: one test's non-inverted half swapped scenario (an
improvement over a tautology, but a literal divergence from a plan criterion),
one test opening with an initial rather than cross-phase entry (equivalent), a
stale intra-doc link in `next_types.rs` that will not fail the build and so will
not announce itself to outline 2, and a design safety argument about legacy logs
that is moot rather than wrong. The commit message is factual, volunteers the
build break, and carries no AI attribution.
