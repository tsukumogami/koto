---
schema: plan/v1
status: Active
execution_mode: multi-pr
split_rationale: |
  Hard Constraints and Incremental Value. Two pieces have consumers waiting
  on a release: koto session identity (needed by shirabe#1415) and the
  run_journal.enabled key (needed by koto#545), so each lands as its own
  small pull request early. The module moves and the exit helper touch
  dozens of call sites and are kept apart from behaviour changes so each is
  reviewed as a pure refactor. The sender, the carry and the flush sites
  then land in the order each one's tests need.
tracking_level: none
upstream: docs/designs/DESIGN-event-sink.md
milestone: "Session records through an external event sink"
issue_count: 7
---

# PLAN: Session records through an external event sink

## Status

Active

## Scope Summary

Release A of koto#544. koto queues a stamped copy of each session record and
hands it to an external event sink at fixed flush points, while the journal
keeps being written byte for byte as today. With no sink installed at the
fixed path, koto queues and calls nothing. The work also delivers
`koto session identity <name> --json` and the `run_journal.enabled` key, and
moves `src/run_journal/` to `src/telemetry/`. Release B (koto#545), which
removes the journal, is outside this plan.

No issues are filed from this plan. The implementation dispatch files one per
outline when it starts the work.

## Decomposition Strategy

**Horizontal, one pull request per layer, in the order the tests need.** The
design's Implementation Approach already names the seven pull requests:

- Outline 1 lands the golden journal test before any code moves, so the
  moves are proven to change nothing.
- Outline 2 is the exit helper on its own. It touches every exit site, and
  nothing else should ride along.
- Outlines 3 and 4 are the two pieces another repository waits on. Each is
  small and lands early.
- Outline 5 is the sender. It carries the counting half of the carry
  sidecar, so its breaker and deadline tests assert on real drop counts.
- Outline 6 completes the carry. Outline 7 wires the in-code flush sites,
  which do nothing until the queue exists.

## Issue Outlines

### Issue 1: refactor(telemetry): move the run journal module and pin its output

**Goal**: Move `src/run_journal/` to `src/telemetry/`, `run_id.rs` to `src/session/lineage.rs`, and `is_id`, `is_name`, `name_or_digest` and `digest` to `src/names.rs`, with no change in behaviour.

**Acceptance Criteria**:
- [ ] A golden test runs one scripted session (init, a gate, a decider check, a review record, a terminal) and compares its journal byte for byte with a checked-in fixture, `at` values masked. It lands first and passes before and after the moves.
- [ ] `src/telemetry/`, `src/session/lineage.rs` and `src/names.rs` exist, and `src/run_journal/` doesn't.
- [ ] `tests/run_journal_doc_test.rs` reads the new source directory and passes.
- [ ] File names on disk are unchanged: `_run_journal.jsonl`, the `run-journal.json` sidecar, the `[run_journal]` config table.

**Dependencies**: None

**Complexity**: testable

**Type**: code

### Issue 2: refactor(cli): route every exit through one exit helper with a hook list

**Goal**: Add `src/exit.rs` with `register`, `exit(code)` and `crash(code)`, route every exit through it, and replace `EXIT_FLUSH` with registered hooks.

**Acceptance Criteria**:
- [ ] `clippy.toml` bans `std::process::exit`, and CI's clippy run fails on a direct call outside `src/exit.rs`.
- [ ] `main` ends in `exit(code)` for both its `Ok` and `Err` arms.
- [ ] `koto next`'s cloud flush and the `--workspace` read path's scratch removal run as registered hooks, on a normal return and on an error exit (tests).
- [ ] Hooks run in reverse registration order, and a panicking hook doesn't stop the rest (unit tests).
- [ ] `crash_at` exits 86 through `crash`, which runs no hooks.

**Dependencies**: None

**Complexity**: testable

**Type**: code

### Issue 3: feat(session): print a session's identity with koto session identity

**Goal**: Cache the run template in the `run-journal.json` sidecar and add `koto session identity <name> [--json]`.

**Acceptance Criteria**:
- [ ] On a root created with `koto init`, `--json` prints `koto.session.id`, `koto.run.id`, `koto.run.template.name` and `koto.run.template.hash`, matching the session's header (`root_identity_matches_header`).
- [ ] A child prints its root's template name and hash (`child_carries_run_template`).
- [ ] A missing or invalid session exits 2 with a JSON error (`missing_session_exits_2`).
- [ ] The command writes nothing to the store.
- [ ] A sidecar without the new fields keeps its bytes.
- [ ] The `koto-user` skill mentions the command.

**Dependencies**: Issue 1

**Complexity**: testable

**Type**: code

### Issue 4: feat(config): add run_journal.enabled

**Goal**: A user-config key, default `true`, that stops journal writes when set to `false`.

**Acceptance Criteria**:
- [ ] With `run_journal.enabled = false` in user config, a session's commands write nothing to `_run_journal.jsonl` (test).
- [ ] The key is read from the user layer only, like the other `[run_journal]` keys (test).

**Dependencies**: Issue 1

**Complexity**: testable

**Type**: code

### Issue 5: feat(telemetry): queue session records and send them to the event sink

**Goal**: `emit`, the queue, discovery, the sink call with its deadline and group kill, the breaker, the stamps, the `context_reads` exclusion, the exit flush, the counting half of the carry, and the stub-sink harness.

**Acceptance Criteria**:
- [ ] No sink, or a sink failing the symlink, mode or parent check, makes no call, and the owner check is unit-tested through `trusted(meta, uid)` (`tests/event_sink_discovery_test.rs`).
- [ ] `FLEET_EVENT_SINK=off` disables sending; another value changes nothing.
- [ ] 450 queued records produce three calls (200, 200, 50). With the deadline in the second call, the third chunk is counted as dropped, and the flush ends within 3.0 s (`chunks_200_200_50`, `deadline_drops_unreached_chunk`).
- [ ] A chunk with under 2.0 s left is counted without a kill, and the breaker stays open (`late_chunk_dropped_breaker_stays_open`).
- [ ] Exit 3 means one call, with later records counted in `telemetry.json` (`exit_3_trips_breaker_and_counts`).
- [ ] Exit 2 means one call, with nothing counted.
- [ ] A hung sink is killed with its group.
- [ ] The sink's environment holds only the allowlist (`sink_env_is_allowlist_only`).
- [ ] Sent records match the journal's kinds and fields plus the queued-copy stamps (`sent_records_match_journal_plus_stamps`).
- [ ] With `run_journal.enabled=false`, records still go to the sink (`journal_disabled_still_sends`).
- [ ] A `context_reads` record never reaches the queue (`context_reads_never_enqueued`).
- [ ] A command exiting with records queued sends them (`exit_helper_flushes_queued_records`).
- [ ] `docs/reference/run-journal.md` names the three queue-only fields.

**Dependencies**: Issue 2, Issue 3, Issue 4

**Complexity**: testable

**Type**: code

### Issue 6: feat(telemetry): carry dropped counts to the next successful send

**Goal**: The take before each flush, `--carry` on the first chunk, the add-back after exit 3 or a kill, and the orphan count for removed sessions.

**Acceptance Criteria**:
- [ ] A session's `dropped` count goes out as `--carry N` on the next successful call and is zeroed (`carry_sent_and_zeroed`).
- [ ] Two concurrent processes on one session never carry the same count (`concurrent_processes_never_double_carry`).
- [ ] After exit 3, the taken counts plus the failed chunk's records are written back (`exit_3_adds_back_taken_carry`).
- [ ] A removed session's remaining count moves to the store's orphan count and is carried by a later flush (test).

**Dependencies**: Issue 5

**Complexity**: testable

**Type**: code

### Issue 7: feat(telemetry): flush at the action, decider, wait and terminal sites

**Goal**: The eight in-code `flush()` calls in the design's flush-point table, each with its ordering test.

**Acceptance Criteria**:
- [ ] A gate's spawn follows the flush, and its 30 s timer starts after the flush returns (`action_flush_precedes_gate_spawn`).
- [ ] Records reach the stub before a decider provider call (`decider_flush_precedes_provider_call`).
- [ ] Records reach the stub before a polling gate's sleep, before `koto request`'s wait slices, and before the check evaluator's lock-poll sleep (one test each).
- [ ] The terminal tick flushes after the `terminal` record and before the session directory is removed (`terminal_flush_precedes_removal`).

**Dependencies**: Issue 5

**Complexity**: testable

**Type**: code

## Implementation Sequence

**Critical path:** Issue 1, then Issue 5, then Issue 6 or Issue 7, with
Issues 2, 3 and 4 merged before Issue 5.

**Open with Issue 1.** Every later pull request edits the moved paths.

**Then Issues 2, 3 and 4, in any order.** They're independent of each
other. Issue 3 should merge as early as possible, since shirabe#1415 waits
on a release with it.

**Then Issue 5, then Issues 6 and 7 in either order.** Release A is cut after
all seven merge, before 2026-11-09. The rollout installs it on hosts after
2026-11-09T02:00Z.
