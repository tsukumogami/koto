# Verdict: PASS

Re-review of `git diff 9532a95^..afd5983`, tree re-read from disk. The first-pass
verdict was BLOCKING; the blocking finding is fixed and the two moderates are
fixed. Three minors remain, none of which should hold the change.

## Re-review

### Finding 1 (BLOCKING, the fallback comment) — FIXED, and the replacement is correct.

`src/engine/persistence.rs:1047-1055` now reads:

> With no qualifying entry event, every event is in scope. Under either boundary
> that arm is unreachable for a log `koto init` produced: init always appends
> `Transitioned { from: None, to: <initial> }`, and a `None` source opens under
> both -- `AnyEntry` never looks at the source, and `ArrivalFromElsewhere` reads
> `None` as "not from this phase". The arm exists for synthetic and truncated
> logs, and under `ArrivalFromElsewhere` it is the one place this boundary's
> failure direction inverts: a log whose only entries naming the phase are
> self-entries falls through here and can be answered by a record from an earlier
> visit, suppressing where `AnyEntry` would deliver.

Checked clause by clause rather than taken on report:

- *"init always appends `Transitioned { from: None, to: <initial> }`"* —
  `init_child_core` (`src/cli/init_child.rs:502-506`) and
  `init_inline_into_session` (`:671-675`) both do, and `handle_init` hard-fails
  without one (`src/cli/mod.rs:1857-1862`: `"newly initialized workflow {:?} has
  no Transitioned event"`). The claim is scoped to `koto init` and is true there.
  It is also true of the one path that could have been a counterexample: `koto
  session create` (`src/cli/session.rs:354-365`) writes a placeholder log holding
  only `WorkflowInitialized` and no opener at all, but `derive_state_from_log`
  returns `None` for it so no caller can reach `entry_slice` with a phase, and the
  claim path that later populates it is `init_inline_into_session`, which appends
  the `from: None` opener.
- *"`AnyEntry` never looks at the source"* — `opens_on_self_entry()` returns
  `true`, short-circuiting the `||` before `from` is compared. True.
- *"`ArrivalFromElsewhere` reads `None` as 'not from this phase'"* —
  `from.as_deref() != Some(current_state)` is `None != Some(_)`, true. Correct.
- *"the one place this boundary's failure direction inverts"* — with only
  self-entries naming the phase, `ArrivalFromElsewhere` finds no opener and takes
  the whole log, where a record from an earlier visit suppresses;`AnyEntry` opens
  at the last self-entry and delivers. Correct, and it is now the same statement
  the design makes at `:109-115` rather than a different one.

The loose `AnyEntry` sentence inherited from `occupancy_slice` is gone. Nothing in
the paragraph now asserts a mechanism the code does not have.

### Finding 2 (MODERATE, the design's claim about the neighbouring walks) — FIXED.

`DESIGN:68-77` now lists all four (`derive_decisions` at `:759` added between the
two it previously straddled) and states the reachability argument correctly: "that
divergence is unreachable: all four derive their own phase from
`derive_state_from_log` (`:709`), which returns the target of the last entry
event, and that event necessarily satisfies the walk's own test. So they are
structurally foldable, not behaviorally distinct."

`DESIGN:333-343` no longer argues impossibility. It says they could be folded
behind their existing `None` guard without changing behavior, gives the two
reasons that actually hold — an unreviewable diff, and independence as an R15
hedge — and closes with "Consolidating them is follow-up work, not an
impossibility." That is the correction I asked for, and it leaves the next reader
free rather than stopped.

Counts corrected in Context (`:76`), Option A (`:138-140`), and Consequences
(`:522`). One count was missed; see Finding 7 below.

### Finding 3 (MODERATE, `==` instead of an exhaustive match) — FIXED.

`src/engine/persistence.rs:1012-1024` adds the method, and both arms
(`:1071`, `:1075`) call it. Behavior is unchanged: `AnyEntry` returns `true` and
short-circuits exactly where the equality test did, `ArrivalFromElsewhere` returns
`false` and falls through to the source comparison. The doc comment states the
reason in the terms that matter — "adding a variant fails to compile here -- at
the one place that names the decision -- instead of silently defaulting at the two
call sites inside the scan."

### Finding 4 (MINOR, five paraphrases of one rule) — PARTIALLY FIXED.

Four of the five are consolidated. `entry_slice` holds the mechanism;
`epoch_slice` (`:1099-1100`) and `delivery_window` (`:1119-1120`) point at it with
"see [`entry_slice`] for the mechanism" and keep only what is local;
`instructions_delivered_this_window` (`:1171-1173`) is cut to "The window is
[`delivery_window`]'s." plus the precondition. `epoch_slice` no longer names the
test by string. Good.

The fifth was not touched. `latest_epoch_gate_failed` (`:1129-1147`) still spells
the mechanism out twice — once as "the events after the last transition, directed
transition or rewind into `current_state`, a self-transition included" in the
sentence immediately after it says "The epoch is [`epoch_slice`]'s", and again in
a full closing paragraph:

> This boundary is *not* the delivery window. A self-transition closes the epoch,
> so a gate verdict from before a loop's last lap drops out of scope and the
> blocked badge clears; it does not close the delivery window, so an agent looping
> in one phase is not re-sent instructions it holds. The two are separate answers
> to separate questions and a change to one must not move the other.

It is the longest of the five and now overlaps its own private helper: `epoch_slice`
was rewritten in this commit to explain the blocked-badge consequence ("Widening it
to match [`delivery_window`] breaks the dashboard, silently"), so two adjacent
functions carry the same warning in different words. Cutting the closing paragraph
to a pointer at `epoch_slice`, and dropping the re-spelling in the first, finishes
the consolidation. Not worth blocking — it is the `pub` surface and some
redundancy there is defensible — but it is the one restatement most likely to
drift next, because it is the only one a reader can reach without seeing
`entry_slice`.

### Finding 5 (MINOR, engine doc comments encoding dashboard semantics) — NOT ADDRESSED, and that is fine.

`epoch_slice` still says "keep the blocked badge on" and `latest_epoch_gate_failed`
still names "the dashboard read seam (`read_session`)" and "the `/workflows`
projection writer". Code-level dependency direction remains clean —
`src/engine/persistence.rs` imports nothing from `src/cli` or
`src/workflows_surface`, and the arrows still run inward from
`src/cli/dashboard_data.rs:458` and `src/workflows_surface/project.rs:183`. I
flagged this as accept-deliberately and the fix commit did not claim to change it.
Standing disposition: accepted.

### Finding 6 (MINOR, crate does not build, dangling rustdoc link) — CARRIED, unchanged.

`src/cli/mod.rs:2914`, `:3417`, `:4298` still call
`instructions_delivered_this_occupancy`; `src/cli/next_types.rs:378` still carries
the intra-doc link to it; and `src/cli/mod.rs:3377-3402` still carries the
"provably evaluates to `false` on every call" proof that `delivery_window` has
falsified. Outline 2's scope, as designed. The only requirement is that outlines 1
and 2 land as one commit or be squashed, so no commit on `main` is
bisectable-broken.

## Findings

### 7. MINOR (new, introduced by the fix) — the design's frontmatter now contradicts its own body on the walk count.

`docs/designs/DESIGN-self-loop-suppresses-details.md:28-29`:

> Duplicating the scan would give the file **a fifth copy** of a walk it already
> carries **five** of

The second number was corrected four→five and the first was left alone, so the
sentence now says a sixth walk would be the fifth. Option A got this right in the
same commit — "it takes the file from five backwards walks over entry events to
six" (`:138-139`) — so the frontmatter is the only place that disagrees with the
body.

**Fix.** "a sixth copy of a walk it already carries five of."

### 8. MINOR (new) — the added field-inventory paragraph asserts a legacy-log shape the repo gives no evidence ever existed.

`src/engine/persistence.rs:1064-1070`, added from the maintainer review:

> an absent source reads as "not from this phase" and opens the window, which is
> both what initialization needs and the safe direction for **a log written before
> the field existed**.

The mechanism is right: serde resolves a missing `Option<T>` field to `None`
through `missing_field`, so a line without `from` does deserialize rather than
fail, and it does open the window. What is unsupported is that such a line was
ever written. `TransitionedPayload.from` (`src/engine/types.rs:1396`) carries no
`#[serde(default)]` and no additive-field comment, unlike the two fields in the
same struct and its sibling that are genuinely additive and say so
(`skip_if_matched`, `submitter_cwd`, each with a "pre-feature state files
round-trip unchanged" note). `from` reads as having been present since the payload
was defined.

Harmless, and the design makes the same claim at `:291-293`, so this is not a
regression introduced here. But it is the same category as Finding 1 — a comment
asserting a scenario the code does not evidence — and the cheap fix keeps the
paragraph's value without the unfounded half: say that an absent source reads as
"not from this phase" and opens the window, which is what initialization needs and
the safe direction if the field is ever absent. Drop "written before the field
existed".

### 9. MINOR (carried from the first pass, unchanged) — the `_ => false` catch-all still swallows a future entry variant.

`src/engine/persistence.rs:1077`. A fourth state-entry payload added to
`EventPayload` opens neither window, with no compiler signal. Pre-existing in
`occupancy_slice` and not made worse by the split in any mechanical sense, but the
split doubles the cost of missing it — the author of a new variant now has to
decide the question twice. Worth a line in the catch-all naming what belongs there
("non-entry events; a new state-entry variant must be added above, under both
boundaries") rather than any structural change.

## Summary

PASS. The blocking finding is properly fixed, not papered over: I re-derived the
init-opener invariant from `init_child_core`, `init_inline_into_session` and
`handle_init`'s hard failure, and checked the `koto session create` placeholder
path that could have been a counterexample, and the new paragraph is accurate on
every clause including the failure-direction inversion it now inherits from the
design. The design's justification for leaving the neighbouring walks duplicated
is corrected to say they are foldable and left as follow-up, with `derive_decisions`
added and the counts fixed; `Boundary::opens_on_self_entry` is an exhaustive match
called from both arms with behavior identical to the equality test it replaces.

Three minors remain and none block. The design frontmatter kept "a fifth copy"
while its body moved to five walks, so it now contradicts Option A one page later.
`latest_epoch_gate_failed` is the one doc comment left restating the mechanism in
full, and it now overlaps the blocked-badge warning this commit added to
`epoch_slice` — the consolidation is four-fifths done and the remaining fifth is
the `pub` surface a reader reaches without seeing `entry_slice`. And the new field
paragraph asserts a pre-`from` log shape that `TransitionedPayload` gives no
evidence of, in a struct whose genuinely additive fields all say so explicitly.

Structure, layering, contracts and dependency direction are all where they should
be, and were before the fix commit; nothing in this pass changes that assessment.
The outstanding carryover is outline 2's: the crate does not build until the three
call sites and the `next_types.rs` intra-doc link follow the rename, and the
directed path's falsified "provably false" proof is the most misleading comment on
the branch until it does. Squash or combine so no commit lands bisectable-broken.
