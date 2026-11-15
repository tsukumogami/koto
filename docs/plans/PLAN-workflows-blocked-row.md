---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-workflows-blocked-row.md
milestone: "a gate-blocked koto session's /workflows row reads blocked"
step_count: 5
---

# PLAN: a gate-blocked koto session's `/workflows` row reads blocked

## Status

Active

## Scope Summary

A gate-blocked koto session's `/workflows` entry gets a `workflowName` of
`blocked · <session label>`, so its list row differs from a running session's,
while `name` keeps the plain label and the top-level `status` stays `running`.
The retention sweep restores the plain label when it marks an entry `killed`,
the contract goes to version 7, the native-workflows guard fixture pins a
blocked and a running golden, the convention guard probes where Claude Code
takes the row label from, and the verification guide gains a two-session list
check. Everything lands in one pull request, which shows one run of
`scripts/verify-native-workflows.sh` on a gate that fails and then passes, and
refers to koto#590 without a closing keyword.

## Decomposition Strategy

Walking skeleton. Step 1 crosses the whole path a reviewer can run without
Claude Code, on one real koto session: its gate fails, `koto next` appends the
event, the commit funnel writes the entry with the marked row label, the gate
then passes and the next entry is plain. The blocked golden and the existing
clearing tests change in the same step, so the guard fixture holds the shape
from the first commit. Step 2 hardens the label (cap, escapes, no intent,
terminal, cancel) and adds the running golden; steps 3 to 5 add the sweep, the
convention probe and the reader-facing docs on top of step 1.

## Steps

### Step 1: a gate-blocked session's row label reads blocked and clears when the gate passes

**Goal**: A real koto session held on a failing gate writes an entry whose `workflowName` starts `blocked · ` followed by its intent, with `name` the plain intent and `status` `running`; after the gate passes, the next entry's `workflowName` equals `name`. This walking skeleton crosses the whole commit funnel on one real session: `koto next`, the event append, `try_materialize`, `with_blocked_gate`, and the written file.

**Acceptance Criteria**:

- [ ] While a real session is held on a failing gate, the entry it writes has `workflowName` equal to `blocked · ` followed by the session's intent, and `name` equal to the intent with no marker.
- [ ] That same blocked entry keeps `status` at `running` and `koto.status` at `blocked`, and its `summary` still ends `· blocked on gate <name>` with the failing gate's name.
- [ ] Every entry koto writes carries `contractVersion` 7, and the reconciled (swept) entry fixture reports 7 too.
- [ ] After the gate passes and the session is evaluated again, the next entry has `workflowName` equal to `name`, no `blocked · ` prefix, and no blocked suffix in `summary`.
- [ ] Each existing clearing case (a passing re-evaluation, an advance to the next state, a rewind, and a re-entry with no new evaluation) shows `workflowName` equal to `name` in the entry written afterwards.
- [ ] The regenerated blocked golden entry matches what the canonical blocked build writes, with the marked `workflowName`, the unmarked `name` and `contractVersion` 7, so the shape test fails if either field drifts.
- [ ] A running entry (no failing gate) is written exactly as version 6 wrote it apart from the version number: `workflowName` equal to `name`, no prefix.

**Check**: Build koto fresh (`CARGO_BUILD_JOBS=2 cargo build`), then run `scripts/verify-native-workflows.sh` against that build; the script uses its own scratch `KOTO_HOME`, and runs with the `KOTO_SESSION_*` and other session-binding variables unset. It starts a real session on a failing gate and must exit 0, with its new row-label assertions passing in both places: first that `workflowName` starts `blocked · ` and `name` is the plain intent while `status` is `running`, then, after the gate passes (render #2), that `workflowName` equals `name`. Next run `CARGO_BUILD_JOBS=2 cargo test --test native_workflows_blocked_test` and see it pass; once it finishes, run `CARGO_BUILD_JOBS=2 cargo test --test native_workflows_shape` and see it pass. Run them one at a time because the host is memory-bound, and run only these targeted tests, never the full suite.

**Dependencies**: None

**Files**: `src/workflows_surface/contract.rs`, `src/workflows_surface/materialize.rs`, `tests/native_workflows_blocked_test.rs`, `tests/native_workflows_shape.rs`, `tests/fixtures/native-workflows/enriched-shape.json`, `tests/fixtures/native-workflows/reconciled-shape.json`, `scripts/verify-native-workflows.sh`

### Step 2: long, unnamed and hostile labels keep the marker, and a running golden pins the plain row

**Goal**: A blocked session's `blocked · ` marker survives a 2,000-character intent with wide characters, a session with no intent, and control characters or ESC in the intent and the state name. A terminal state or a cancel after a failed gate writes a plain label, and a running golden fixture pins the unmarked row beside the blocked one, so removing the marker from either golden fails the shape test.

**Acceptance Criteria**:

- [ ] A blocked session whose intent is 2,000 characters of wide characters writes a `workflowName` that starts with the full `blocked · ` and whose total length, cut marker included, is at most 1,024 characters, koto's fact cap.
- [ ] A unit test that puts an escape sequence across the cut point of a long label gets a `workflowName` with that sequence dropped whole: no half-written escape appears at the end of the label.
- [ ] A blocked session with no intent writes a `workflowName` of `blocked · <template> · <state>`, and its `name` is `<template> · <state>` with no marker.
- [ ] A blocked session whose intent and state name hold a control character and an ESC writes both escaped in `workflowName` exactly as the summary escapes them, with no raw control byte in the file.
- [ ] A label that is empty after sanitizing writes a `workflowName` of `blocked`.
- [ ] After a failed gate, a session that reaches a terminal state writes a `workflowName` equal to its `name` and without the `blocked · ` prefix.
- [ ] After a failed gate, a cancelled session writes a `workflowName` equal to its `name` and without the `blocked · ` prefix.
- [ ] The running golden file `running-shape.json` has a `workflowName` equal to its `name`, no `blocked · ` prefix, and `contractVersion` 7, and a comparison test fails if the file stops matching what koto writes for a running entry.
- [ ] Both the blocked and the running goldens are in the conventions test's shape list, and deleting either entry from that list makes that test fail.
- [ ] Putting `blocked · ` into the running golden's `workflowName`, or taking it out of the blocked golden's, makes `native_workflows_shape` fail.

**Check**: Run these one at a time, with `CARGO_BUILD_JOBS=2` and no other cargo job running: `cargo test --test native_workflows_blocked_test`, then `cargo test --test native_workflows_shape`, then `cargo test --test native_workflows_conventions`, plus `cargo test --lib workflows_surface::contract` for the cut-point unit tests. Each exits 0 and the new tests (long intent, no intent, escapes, empty label, terminal after a failed gate, cancel after a failed gate, running golden comparison and pinning) appear as passed. Then edit `workflowName` in `tests/fixtures/native-workflows/enriched-shape.json` to drop the marker and rerun `cargo test --test native_workflows_shape`, which fails. Restore it, add `blocked · ` to `workflowName` in `running-shape.json`, rerun, and see it fail again. Restore that file too and rerun to see the suite pass.

**Dependencies**: Step 1

**Files**: `tests/native_workflows_blocked_test.rs`, `tests/native_workflows_shape.rs`, `tests/native_workflows_conventions.rs`, `tests/fixtures/native-workflows/running-shape.json`, `src/workflows_surface/contract.rs`

### Step 3: a stale blocked entry the sweep marks killed reads its plain label

The retention sweep's `killed` rewrite sets `workflowName` back to a sanitized, cut `name`, so a swept blocked entry carries no marker and a swept running entry keeps its label.

### Step 4: the convention guard fails by name when Claude Code stops labeling rows from workflowName

A `list-row` convention probes the row label's source on the pinned release, its `--alter` run fails naming it, and the pin moves to the release the guard passes on.

### Step 5: a reader reproduces the two-session list check and finds the row label documented

The verification guide's two-session steps quote the expected rows and summary line on a named Claude Code release, and the session-feed reference, koto-user skill and CHANGELOG describe the row label.

## Implementation Sequence

Step 1 first. Steps 2 to 5 each need only step 1 and can follow in any order; the owner details step 3 after step 2 reaches its terminal, and each later step the same way.
