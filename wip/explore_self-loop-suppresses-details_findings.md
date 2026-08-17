# Exploration Findings: self-loop-suppresses-details

Round 1, five leads, all completed. Full research under
`wip/research/explore_self-loop-suppresses-details_r1_lead-*.md`.

## Measured baseline (built binary, not the diff)

Debug build of `main` @ `b7b0799`, `tests/instructions_delivery_test.rs`'s own
`DELIVERY_TEMPLATE` reproduced as a real template, one session directory per case:

| Case | Result on `main` | Wanted |
|---|---|---|
| A arrival at `implement` from `gather` | PRESENT(23) | PRESENT |
| B self-transition `implement -> implement` | PRESENT(23) | **OMITTED** |
| B2 second consecutive self-transition | PRESENT(23) | **OMITTED** |
| C loop-back `implement -> gather` | PRESENT(20) | PRESENT |
| C2 self-loop then loop-back to `gather` | PRESENT(20) | PRESENT |
| C3 re-entry at `implement` after leaving | PRESENT(23) | PRESENT |
| D arrival after `koto rewind` | PRESENT(20) | PRESENT |
| E `koto next --to implement` from `gather` | PRESENT(23) | PRESENT |
| F `koto next --to implement` while at `implement` | PRESENT(23) | **OMITTED** |
| G1 first tick, gate blocked | PRESENT(21) | PRESENT |
| G2 second tick, gate blocked | OMITTED | OMITTED |
| H `--full` on a delivered occupancy | PRESENT(21) | PRESENT |
| I `koto status` after suppression | PRESENT(21) | PRESENT |
| J arrival via `relay` (unconditional hop) | PRESENT(23) | PRESENT |

Three cases move: B, B2, F. Everything else is already right and is the
regression surface.

Repro scripts kept at `$CLAUDE_JOB_DIR/tmp/cases.sh` (outside the repo, so no
`wip/` reference lands in a committed file).

## What the research established

### The mechanism is already expressible; nothing needs to be recorded

`instructions_delivered_this_occupancy` (`src/engine/persistence.rs:1099`)
decides entirely through the private `occupancy_slice` (`:1028`), which matches
state-entry events on `to` and discards `from` with `..`. All three entry
variants already carry `from`: `Transitioned.from: Option<String>`,
`DirectedTransition.from: String`, `Rewound.from: String`
(`src/engine/types.rs:469`, `:506`, `:513`).

Verified independently: every production writer of `Transitioned` in the advance
loop sets `from: Some(state)` (`src/engine/advance.rs:504`, `:546`). The
`from: None` sites are initial entry (`src/session/local.rs:1426`,
`src/cli/init_child.rs:503`/`:672`, `src/workflows_surface/materialize.rs:367`,
`src/workflows_surface/project.rs:429`) and test fixtures. `None` therefore means
"initial entry", which is never a self-transition, so it needs no special case
and an old log without the field reads as "deliver" -- the safe direction the
design already accepts elsewhere.

So: no new event variant, no new field, no migration, `CURRENT_SCHEMA_VERSION`
stays at 1.

### The one structural hazard: `occupancy_slice` is shared

`occupancy_slice` also backs `latest_epoch_gate_failed` (`:1058`), which is the
dashboard's blocked classification (`src/cli/dashboard_data.rs:458`) and the
`/workflows` projection's (`src/workflows_surface/project.rs:183`). Its doc
comment sells the sharing as a correctness property: "Shared rather than copied
so the predicates built on it ... cannot come to disagree about where an
occupancy starts."

`latest_epoch_gate_failed` takes the *latest* `GateEvaluated` in its slice.
Widening the slice backwards across a self-entry makes a session that has just
self-looped, and not yet re-evaluated its gates, inherit the pre-loop verdict.
That is a real, user-visible change to a badge, with no unit test guarding it.
All four leads that looked at this reached the same conclusion independently:
**fork the slice, do not edit it.**

### A reachable case the brief's semantics list does not cover

`handle_rewind` (`src/cli/mod.rs:2031-2047`) picks its destination from the
*second-to-last* state-changing event's `to`. After a self-loop the last two
entry events both name `P`, so a rewind appends `Rewound { from: P, to: P }`.
A blanket `from != to` rule would hand an operator who explicitly asked to redo
the phase a response with no instructions.

### The upstream documents argued this out and ruled the other way

`docs/designs/current/DESIGN-inline-phase-details.md:247-256` carries a section
headed "A contradiction in the PRD was corrected", which resolved a conflict
between the PRD's Definitions and an acceptance criterion *in favour of*
re-delivery, and rewrote the criterion. So the reversal is not correcting an
oversight; it is overturning a written, argued decision, and the record has to
say so.

Counterweight found by the artifacts lead: the older Done PRD
`docs/prds/PRD-koto-next-output-contract.md` R9 already said "Subsequent visits
(retries, self-loops, polling): `directive` is present, `details` is absent".
That is the literal source of koto#90's AC 3. The ruling restores an older
written contract rather than inventing a new one.

Documented lifecycle gives no in-place-edit licence for a Done PRD or a Current
DESIGN, but repo *practice* does: `70ba97c` rewrote a Current design's prose
against the real code, kept the status, and recorded the change in the PR.
Supersession would demand a whole successor DESIGN for a change that moves one
clause, and the documented `shirabe transition ... Superseded` path writes a
`## Status` body line that fails FC03 today (reproduced by the lead, exit 2) on
any design carrying `schema:`.

### Blast radius, verified against files

Code:
- `src/engine/persistence.rs` -- the slice and the predicate.
- `src/cli/mod.rs:3377-3400` -- a 24-line comment arguing the directed path's
  predicate call "provably evaluates to `false` on every call". After the change
  it evaluates to `true` on `--to P` while at `P`. Must be rewritten, not trimmed.

Tests that flip:
- `src/engine/persistence.rs` `instructions_delivered_resets_on_a_self_transition`
  and the second half of
  `instructions_delivered_resets_on_arrival_by_directed_transition`.
- `tests/instructions_delivery_test.rs`
  `self_transition_arrival_carries_details_again` and the two-consecutive-
  directed-transitions test.
- `tests/status_phase_retrieval_test.rs:497-503` -- an unpredicted one: a
  `--to implement` while already at `implement` sits in the *setup* of a test
  about `koto status` not writing to the log.

Tests that must NOT change (the guard rails proving the change did not overshoot):
`loop_back_arrival_at_previously_occupied_phase_carries_details_again`,
`rewind_arrival_carries_details`, `gate_blocked_first_tick_carries_and_repeat_omits`,
both `--full` tests, and `tests/next_response_baseline.rs` against an unmodified
`tests/fixtures/next-response-baseline/instruction-free.json`.

On the fixture: it is provably unaffected -- none of its templates declare a
`<!-- details -->` marker, so no code path can move a byte. The one way to break
it is to edit `SEQUENCES[5].description` in the `.rs` file, whose text
("ending one occupancy and beginning another") is embedded in the JSON verbatim.
Both must move in lockstep or neither.

Evals: 11 and 12 keep their scenarios and verdicts. Eval 11's `expected_output`
explains a correct answer using the boundary definition that is changing, so it
needs a wording edit, not a verdict change. A new eval asserts the self-loop.

Docs: eight passages across `response-shapes.md`, `command-reference.md`,
`koto-author/SKILL.md`, `template-format.md`, `.cursor/rules/koto.mdc`,
`docs/guides/cli-usage.md`, plus `CHANGELOG.md`. The stock phrase "leave and
re-enter the state" recurs in five of them and becomes exactly backwards.
`README.md` says nothing about `details`.

Pre-existing drift found in passing, same feature, wrong on `main` today:
`docs/reference/session-feed.md:687` and `src/engine/types.rs:770` both say the
`instructions_delivered` event is "not emitted yet", while `src/cli/mod.rs:3459`
and `:4604` append it.

## Decisions taken during convergence

See `wip/explore_self-loop-suppresses-details_decisions.md`.

## Open questions carried into `/scope`

None blocking. The four the research raised were answered in convergence and are
recorded as decisions; each one is a design question the DESIGN hop must argue
rather than assert.

## Decision: Crystallize
