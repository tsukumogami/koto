---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-koto-ci-wait-stale-keys.md
milestone: "koto owns the CI wait and stale-key clearing"
issue_count: 6
---

# PLAN: koto owns the CI wait and stale-key clearing

## Status

Active

## Scope Summary

Implement `clear_on_entry` on states and `poll:` on command gates as the
design describes, with the session-feed contract, the authoring docs and
skills, a v0.14.1 compatibility job and the scripted checks, in one pull
request. Clearing lands first.

## Decomposition Strategy

**Horizontal decomposition, clearing before polling.** The two features share
no code path beyond the event log, and each splits cleanly into a template
layer (field, compile validation) and an engine layer that depends on it. A
walking skeleton buys nothing here: the integration seam, the advance loop, is
existing and well tested, and each engine change is exercised end to end by
its own integration tests. Clearing is ordered first because it is what makes
the largest block of shirabe prose deletable. Documentation and the
compatibility job come last because they describe and exercise the finished
behaviour.

The work lands as one pull request: nothing here needs to reach the default
branch before the rest, and a half-landed feature (a field the engine ignores)
isn't useful on its own.

## Issue Outlines

### Issue 1: feat(template): declare context keys cleared on entry

**Complexity**: testable

**Goal**: Add `clear_on_entry` to template states, with compile-time
validation, without changing any template that doesn't use it.

**Acceptance Criteria**:
- [ ] `TemplateState.clear_on_entry` compiles from source YAML and is omitted
  from compiled JSON when empty; every fixture template's compiled JSON and
  template hash are unchanged (`tests/compat_baseline_test.rs` passes as is).
- [ ] The compiler rejects an invalid key, a `{{VAR}}` reference, a duplicate,
  and the field on a terminal state, each with an error naming the state and
  the key (PRD R1).
- [ ] The compiler rejects a listed key that any transition writes through
  `context_assignments`, naming the key, the state and the transition (R2).
- [ ] Unit tests cover each rejection and one accepted template.

**Dependencies**: None

### Issue 2: feat(engine): clear declared keys on every entry into a state

**Complexity**: critical

**Goal**: koto decides from the log alone whether the current entry has been
cleared, removes the keys not written since it through the store's ordinary
removal, and appends one `context_cleared` event.

**Acceptance Criteria**:
- [ ] `EventPayload::ContextCleared { state, keys, entry_seq }` with type name
  `context_cleared`, carrying key names only; `entry_seq` is the entry event's
  persisted sequence number.
- [ ] `pending_clearing` skips `koto init`'s entry, returns nothing once a
  `context_cleared` for the entry exists, and spares keys with a later
  `context_added` whose writer isn't `sync` (R3, R4, R6); unit-tested for
  transition, self-transition, `skip_if` self-transition, directed transition
  and rewind entries.
- [ ] The advance loop clears at the top of each iteration, before the
  integration, `default_action` and gates; the tick's append closure removes
  the keys before appending, and a removal error fails the tick with no event
  (R7). Integration test: on a review -> fix -> review loop the review gate
  blocks on the first tick after re-entry and a `default_action` that checks
  the key sees it absent.
- [ ] `koto next --to` and `koto rewind` clear before returning; `koto context
  exists` right after either reports the key absent. `handle_rewind` takes the
  store and template it needs.
- [ ] A batch retry's child rewind is cleared on the child's next tick.
- [ ] A gate override and a re-tick in the same epoch clear nothing.
- [ ] Clearing appends no `context_read` event (R5), and several ticks after
  one entry leave exactly one `context_cleared` (R6).
- [ ] `outstanding_assignments` and the terminal-result failure-reason read
  treat cleared keys as removed (R8).
- [ ] On a store whose removal fails (including a cloud version conflict),
  no event is appended and the next tick clears and records it (R8a).

**Dependencies**: Issue 1

### Issue 3: feat(template): declare a polling command gate

**Complexity**: testable

**Goal**: Add `poll:` to command gates with its validation.

**Acceptance Criteria**:
- [ ] `Gate.poll` with `interval_secs`, `timeout_secs`, `hold_secs` (default
  0) and `pending_exit_code` (default 75), omitted from compiled JSON when
  absent; fixture hashes unchanged.
- [ ] The compiler rejects `poll:` on a non-command gate, zero
  `interval_secs` or `timeout_secs`, `hold_secs` above `timeout_secs`, a
  pending exit code of 0 or above 255, and a polling gate in a state whose
  `default_action` declares `polling:` (R9).
- [ ] The field is in the gate's exhaustive field lists as not substitutable.

**Dependencies**: None

### Issue 4: feat(engine): re-evaluate a polling gate until it settles

**Complexity**: critical

**Goal**: The advance loop holds and re-runs a pending polling gate within
`hold_secs`, classifies the result, logs it with a `poll` record, and reports
pending as a temporal wait.

**Acceptance Criteria**:
- [ ] `GateOutcome::Pending`, logged as `outcome: "pending"`; exit 0 done,
  the pending code pending, anything else failed, a per-run timeout
  `timed_out` and a spawn failure `error`, both with `poll.status: "failed"`
  (R10).
- [ ] Every tick runs the command at least once; the hold re-runs every
  `interval_secs` and starts no run past `hold_secs` or the deadline; a
  signal ends the hold (R11). Integration test with a script that counts its
  runs: pending twice then done passes in one `koto next`, and the log holds
  exactly one `gate_evaluated` for the gate from that tick, with
  `poll.evaluations: 3`.
- [ ] The window starts at the epoch's first run and is logged as
  `poll.since`; the deadline is `since + timeout_secs`; done or failed after
  the deadline is taken as is; pending at or past it is `timed_out` with
  `poll.status: "timed_out"` and a koto finding (R12, R12a, R15). Every
  evaluation in one epoch carries the same `since`, and a new entry into the
  state (a self-transition included) opens a window with a new `since`.
- [ ] A pending evaluation has no `failure`, no fallback finding, no
  `rule_counts`, no `attempt` and no `visit_attempt`; the resolving evaluation
  takes the next attempt and its `poll.evaluations` sums the runs since the
  window opened (R13, R16, R18).
- [ ] The pending blocking condition has status `pending`, category
  `temporal`, `agent_actionable: false` and a `poll` object with
  `retry_after_secs`, `elapsed_secs` and `timeout_secs`; a failed one is the
  usual corrective command-gate condition plus `poll.status: "failed"`
  (R13, R14).
- [ ] Overriding a polling gate whose last result was pending, failed or
  timed out passes the state without running the command or appending
  `gate_evaluated`; the epoch's `since` is unchanged by the override;
  `overridable: false` refuses (R15a).

**Dependencies**: Issue 3

### Issue 5: docs: document clearing and polling gates

**Complexity**: simple

**Goal**: Describe both features wherever authors and agents read about
templates and responses.

**Acceptance Criteria**:
- [ ] `docs/reference/session-feed.md` documents `context_cleared`, the
  `gate_evaluated.poll` object and the `pending` outcome value, and
  `koto template validate-feed` accepts a log that carries them (R19).
- [ ] The template-format reference, the gate-authoring guide, and the
  `koto-author` and `koto-user` skills describe `clear_on_entry` and `poll:`,
  including where to declare keys and what a pending temporal block asks of
  an agent (R22).
- [ ] `cargo test --test doc_names` passes.

**Dependencies**: Issue 2, Issue 4

### Issue 6: ci: prove v0.14.1 compatibility and forge neutrality

**Complexity**: testable

**Goal**: Script every scriptable criterion and run the scripts in CI.

**Acceptance Criteria**:
- [ ] A compatibility script drives a session through a clearing and a
  polling gate with the new koto, then runs v0.14.1's `koto status`,
  `koto next` and `koto context get` on it: all exit 0, the state matches, the
  cleared key stays absent. Its `--self-test` mutations make it fail (R21).
- [ ] A CI job installs v0.14.1 through the pinned installer and runs the
  script and its self-test.
- [ ] The existing v0.14.1 job still passes, showing templates without a
  polling gate write no `pending` outcome (R20).
- [ ] A forge-neutrality script greps the polling and clearing code for
  forge names and credential reads and runs in CI (R17).

**Dependencies**: Issue 2, Issue 4

## Implementation Sequence

**Critical path:** Issue 1 -> Issue 2 -> Issue 5 (or Issue 6), with Issue 3 ->
Issue 4 alongside.

**Recommended order:** 1, 2, 3, 4, 6, 5. Clearing first, because it carries
the priority; the compatibility job before the docs, so the docs describe
behaviour the job already exercises.

**Parallelization:** Issues 1 and 3 have no dependencies; Issues 2 and 4 touch
different parts of the advance loop and can proceed in parallel; Issues 5 and
6 can proceed in parallel once both engines land.
