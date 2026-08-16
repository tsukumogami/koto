# Plan Decomposition: inline-phase-details

## Phase 1 — Analysis

- `input_type`: design
- Source: `docs/designs/DESIGN-inline-phase-details.md`, status Accepted at read time.
- Visibility: Public. Scope: Tactical. Execution mode flags: none; `--auto`
  semantics inherited from the parent chain.
- No `--upstream`; `docs/roadmaps/` does not exist in this repo.

The design settles four decisions and names four implementation phases. The
components it touches: `src/engine/types.rs` (event variant), `src/engine/persistence.rs`
(delivery predicate), `src/cli/next_types.rs` (response combinator and the pointer
splice), `src/cli/mod.rs` (both response-construction call sites, and `handle_status`),
plus the documentation and skill surfaces PRD R20 through R25 make mandatory.

## Phase 2 — Milestone

`inline-phase-details`. One logical unit: make instruction delivery reliable and
give an agent a way back to the instructions. No GitHub milestone is created,
because the resolved tracking level is `none`.

## Phase 3 — Decomposition

**Strategy: horizontal.** The design describes components with stable interfaces
and a genuine prerequisite ordering — the predicate must exist before either
call site can consume it, and the pointer must know what command to name before
it can be written. Nothing here carries integration risk that an end-to-end thin
slice would surface earlier: there is no new component, no new data flow across a
process boundary, and no new infrastructure. A walking skeleton would add a
throwaway vertical pass over a change that is already vertical by construction.
The design's own four-phase sequence is horizontal, and this decomposition follows
it rather than re-deriving a different one.

### Issues

**ISSUE:1 — Record the delivery.** Add `EventPayload::InstructionsDelivered`
with its `type_name` arm, deserialize arm, payload struct and doc comment, and
add `instructions_delivered_this_occupancy` beside `latest_epoch_gate_failed`.
Inert: nothing calls the predicate yet and nothing appends the event. Complexity:
**testable** — pure additions with a defined unit-test surface over synthetic
event lists, no behavior change to observe.

**ISSUE:2 — One delivery rule at both construction sites.** Add
`with_details_suppressed_unless_full` beside the two existing combinators, call it
at both `src/cli/mod.rs:3357` (directed) and `src/cli/mod.rs:4198` (natural), wire
both to the predicate, and append the delivery record after printing the response.
Complexity: **critical** — this is the behavior change, it alters the
directed-transition path that existing callers may depend on, and its two halves
must land together or the paths disagree.

**ISSUE:3 — Return the current phase's instructions from `koto status`.** Add
`directive`, `details`, and `expects` as conditionally-present keys, substituted
through the same pipeline `next` uses, and add the template-hash verification the
design's security ruling requires, reporting a mismatch rather than failing.
Complexity: **testable**.

**ISSUE:4 — Splice the recovery pointer into the directive.** Reuse
`with_directive_prefix`, applied after substitution, when the phase declares
instructions, ordered so the abandonment notice stays closest to the front.
Complexity: **testable**.

**ISSUE:5 — Update the skills, evals and documentation.** `koto-user`'s
response-shapes and command-reference, `koto-author`'s SKILL.md and
template-format reference, `docs/guides/cli-usage.md`, the Cursor rules file, the
evals that assert the old delivery behavior, and `CHANGELOG.md`. Complexity:
**simple**.

## Phase 3.5a — Value confirmation

The guard asks whether each unit delivers observable incremental value to a
reader who meets it alone.

- ISSUE:1 delivers none on its own and is not claimed to: it is inert by
  construction.
- ISSUE:2 delivers the fix, but an agent that hits the suppression it introduces
  has no way back to the instructions until ISSUE:3 lands.
- ISSUE:3 delivers a retrieval nothing yet advertises.
- ISSUE:4 advertises a retrieval that only exists after ISSUE:3.
- ISSUE:5 documents behavior that only exists after the rest.

**Result: no unit is a standalone increment.** That is the expected shape for
this feature rather than a mis-decomposition — the usable value is "instruction
delivery an agent can rely on", and no proper subset of these five delivers it.
The guard therefore confirms the Incremental Value branch does **not** fire, which
is an input to the mode decision below rather than a failure.

Decision block: `status: confirmed`. The evidence is unambiguous in both
directions — each unit's dependency on its successors is stated in the design, and
no unit was proposed as independently shippable.

## Phase 3.6 — Execution mode

`## Delivery Preference:` is absent from koto's CLAUDE.md, so the preference
resolves to `consolidated` on the `flag > CLAUDE.md-header > consolidated` stack.
Under `consolidated` the default is one PR, and an escape requires a named branch:

1. **Hard Constraint** — does not fire. Single repo, no landing order across
   repositories, no workflow that must reach the default branch before it can be
   invoked, no merge gate between steps.
2. **Incremental Value** — does not fire, per the guard above.
3. **Stated Preference** — does not fire; the repo has not declared `atomic`.

**Execution mode: `single-pr`.** No branch fired, so no `split_rationale` is
recorded — that field is owed only by a departure from the preference, and this
is not one.

Tracking level: `none`. No GitHub issues, no milestone. The PLAN therefore
auto-transitions Draft to Active when authoring finishes, per the lifecycle's
no-issues gate.

## Phase 5 — Dependencies

```
ISSUE:1 -> ISSUE:2 -> ISSUE:3 -> ISSUE:4 -> ISSUE:5
```

- ISSUE:2 needs ISSUE:1's predicate and event.
- ISSUE:3 does not strictly need ISSUE:2, but lands after it so its output shape
  matches what `next` produces once the rule is settled.
- ISSUE:4 needs ISSUE:3, because the pointer names the retrieval, and needs
  ISSUE:2 for the splice-ordering rule against the abandonment notice.
- ISSUE:5 documents all of the above.

Critical path is the whole chain. There is no parallelization opportunity worth
naming inside a single PR; the sequence is the sequence.
