# Cross-Validation — Round 1 (Decisions 9-14)

## Summary

Six new decisions composed cleanly with each other and with the eight
existing decisions. **No hard conflicts.** Seven coordination notes
recorded below as high-priority assumptions for the design-doc revision.

Cross-validation status: **passed** (round 1).

## Assumption inventory

### CD9 (Retry path end-to-end)

- a1: Agents consume a new top-level `reserved_actions` response field.
- a2: Template authors accept compile warning W4 against batch templates
  routing only on `all_complete: true`.
- a3: When-clause engine supports `evidence.<field>: present` matcher,
  OR it is added as part of CD9's implementation PR.
- a4: `reserved_actions` sibling response field does not conflict with D7
  (directive+details) or D8 (default_template+item_schema).
- a5: Delete-and-respawn of a real-template running child whose upstream
  flips to failure is the correct outcome.
- a6: Per-tick runtime reclassification is O(N) in batch size and
  acceptable for typical batches (tens of tasks).
- a7: Parent ticks are serialized (coordinator-driven, per CD12).

### CD10 (Mutation semantics + dynamic-addition primitives)

- a1: CD11's pre-append validation commitment holds; R8 runs in the same
  phase.
- a2: `WorkflowInitialized` event extends to carry a `spawn_entry`
  snapshot. **This is a D2 amendment.**
- a3: Agents accept `cancel_tasks` deferral to v1.1 and documented
  non-feature in v1.
- a4: Agents read `scheduler.feedback` map; the additive field does not
  conflict with CD9 or CD11 extensions.
- a5: CD12 upholds the serialized-parent-tick invariant.
- a6: Canonical-form comparison (sorted `waits_on`, null ≡ omitted for
  `template`, per-key `vars` diff) is the contract agents design against.

### CD11 (Error envelope, validation timing, validation edges)

- a1: Existing `NextError` struct at `src/cli/next_types.rs:283-289` is a
  stable v0.7.0 public contract.
- a2: Agents pattern-match on snake_case string literals at multiple
  nesting levels.
- a3: CD9 populates `InvalidRetryReason` variants.
- a4: CD10 introduces `SpawnedTaskMutated` rejection (reserved slot
  pre-populated).
- a5: CD12 renames `scheduler.spawned` → `spawned_this_tick`.

### CD12 (Concurrency model hardening)

- a1: Agents key on `materialized_children` for idempotent dispatch.
- a2: Linux kernel ≥ 3.15 for `renameat2` (release notes pin).
- a3: `flock` available on all supported Unix targets (already used by
  `LocalBackend` for `ContextStore` writes).
- a4: `CloudBackend::check_sync` can compute three-way `sync_status`
  cheaply.
- a5: `handle_retry_failed` tolerates early-exit insertion before child
  writes (CD9 permits step reorder).
- a6: Walkthrough + koto-user + koto-author skill updates are in-scope
  for CD12's PR.
- a7: 60-second tempfile-sweep threshold bounds leak duration without
  disturbing in-flight ticks.

### CD13 (Post-completion observability)

- a1: CD9 exposes a single well-known predicate for "this child was
  synthesized as skipped" regardless of on-disk mechanism.
- a2: Cloud sync tolerates a new `BatchFinalized` event type under
  existing append-only/idempotent rules.
- a3: `batch_final_view` payload size stays bounded by D1's existing
  task-count and depth caps.
- a4: Compile warnings W1-W4 have an existing surfacing/suppression
  convention that W5 can reuse.
- a5: Adding optional fields (`batch_final_view`, `synthetic`,
  `skipped_because_chain`, `reason_source`) is backward-compatible.

### CD14 (Path resolution contradictions)

- a1: **CD11 will expose a warnings vector on SchedulerOutcome.**
- a2: Agents understand per-task errors don't abort siblings.
- a3: `Path::exists()` per tick is a cheap probe.
- a4: Same-layout cross-machine is the common case.

## Conflict analysis

### Checked and resolved (no conflict)

1. **CD9's runtime reclassification vs CD10's R8.** CD10 explicitly
   scopes R8 to submitter evidence, not scheduler-driven reclassification.
   CD10's decision text: "R8 runs only at submission-validation time;
   CD9's reclassification operates below validation." Non-conflicting.

2. **CD11's `scheduler.spawned` rename assumption vs CD12's Q1 choice.**
   CD11.a5 assumes CD12 renames to `spawned_this_tick`. CD12 confirmed
   in Q1. Non-conflicting.

3. **CD12's retry-ordering reorder vs CD9's retry mechanism.** CD12 Q6
   reorders steps inside `handle_retry_failed` (push parent first, then
   child Rewound writes). CD9 explicitly permits step reorder. CD9's
   committed invariants (CLI interception before advance loop, template-
   declared transition on next tick, clearing event) are preserved.
   Non-conflicting.

4. **CD9's CD12 serialization assumption vs CD12's flock choice.** CD9.a7
   assumes parent ticks are serialized. CD12 Q3 delivers this via
   non-blocking flock on `<session>/<parent>.lock` during handle_next
   for batch parents. Confirmed.

5. **CD10's `scheduler.feedback` vs CD11's `errored` field on
   SchedulerOutcome.** CD10 adds a per-entry feedback map; CD11 adds a
   spawn-errored list. Both are additive and live on different top-level
   keys of the scheduler object. Non-conflicting.

6. **CD13's `synthetic: true` marker vs CD9's runtime reclassification.**
   CD9 replaced D5.2's dedicated synthetic template with runtime
   reclassification. The child's state file uses the real template but
   lands directly at a state where `skipped_marker: true`. CD13's
   `synthetic: true` marker in `koto status` output is computed from
   `skipped_marker: true` on the child's current state — the same
   predicate works under both mechanisms. Non-conflicting, but CD13.a1's
   phrasing "single well-known predicate" should be made concrete in the
   design-doc revision: `skipped_marker: true` on the child's current
   TemplateState.

7. **CD14's warnings vector assumption (CD14.a1) vs CD11's envelope.**
   CD11 didn't explicitly add a `warnings` vector to `SchedulerOutcome`,
   but CD11's envelope is explicitly extensible and CD14's additions
   (`SchedulerWarning::MissingTemplateSourceDir`,
   `SchedulerWarning::StaleTemplateSourceDir`) are orthogonal to errors.
   **Action:** the design-doc revision promotes this from a CD14
   assumption to a shared extension of CD11's `SchedulerOutcome`,
   explicitly listed in the Key Interfaces section.

### Coordination notes requiring design-doc amendments

These are not conflicts — they are places where one decision's scope
crosses into another's, and the boundary must be noted explicitly so
the revised doc is internally consistent.

1. **D2 gains `spawn_entry` snapshot on `WorkflowInitialized`.** Per
   CD10.a2. This is additive and `#[serde(default,
   skip_serializing_if = "Option::is_none")]`-compatible. D2's original
   "atomic init bundle" now bundles one more field. Revise D2's Key
   Interfaces accordingly.

2. **D1 gains compile rules from CD9 and CD13.**
   - W4 (CD9): warn when a template with `materialize_children` routes
     only on `all_complete: true` without also handling `any_failed >
     0` / `any_skipped > 0`.
   - W5 (CD13): warn when a `failure: true` state has no
     `default_action` writing `failure_reason` to context.
   - F5 (CD9): when a child template is referenced as `default_template`
     (or per-task override) in a batch-eligible parent, the child
     template must declare at least one state with `skipped_marker:
     true` that the scheduler can transition into on reclassification.

3. **D1 gains runtime rules from CD10, CD11, CD14.**
   - R0 (CD11): non-empty task list.
   - R8 (CD10): spawn-time immutability.
   - R9 (CD11): task-name regex `^[A-Za-z0-9_-]+$`, 1-64 chars, reserved
     names `{retry_failed, cancel_tasks}`.
   - R6 depth definition (CD14): node count along longest root-to-leaf
     path; limit 50.
   - R1 amendment (CD14): bad per-task child template does NOT halt
     the submission; it is reported as `BatchTaskView.outcome:
     spawn_failed` with an error payload.

4. **D4 gains absent-template-source-dir handling (CD14).** When
   `header.template_source_dir` is absent (pre-D4 state files), the
   scheduler skips step (b) of the resolution order and goes directly
   to `submitter_cwd`, emitting
   `SchedulerWarning::MissingTemplateSourceDir`. When it is present but
   doesn't exist on the current machine,
   `SchedulerWarning::StaleTemplateSourceDir` fires before falling
   through to `submitter_cwd`.

5. **D5 is substantially revised by CD9.** D5.2 (synthetic-per-skipped-
   child template) is superseded by runtime reclassification. D5.3's
   extended gate output gains `all_success`, `any_failed`, `any_skipped`,
   `needs_attention` alongside `all_complete`. D5.4's direct-transition
   claim (step 1) is deleted; retry now routes via advance-loop through
   a template-declared transition on `when: evidence.retry_failed:
   present`. D5.5 walkthrough is rewritten to match.

6. **D6 is extended by CD13.**
   - Terminal (`done`) responses gain `batch_final_view` when the
     workflow had a batch during its lifetime.
   - `BatchFinalized` event is appended to the parent log when the
     children-complete gate first reports all-terminal.
   - `koto status` output's batch section emits `synthetic: true` per
     child when `skipped_marker: true` on current state.
   - `koto status` output carries `skipped_because` (singular,
     direct blocker) AND `skipped_because_chain` (array, attribution
     path to root cause).

7. **CD11's `SchedulerOutcome` extended with:**
   - `errored: Vec<TaskSpawnError>` (CD11)
   - `warnings: Vec<SchedulerWarning>` (CD14)
   - `feedback: BTreeMap<String, EntryOutcome>` (CD10)
   These all live as sibling fields on the scheduler outcome object.

8. **CD9's `evidence.<field>: present` when-clause matcher.** CD9.a3
   assumes either the matcher exists or CD11 adds it. CD11 did not
   address the when-clause engine. **Resolution:** treat the matcher as
   an implementation scope of CD9's PR, flag in the
   Implementation-Approach section as a prerequisite of Phase 3.

## High-priority assumptions surfaced for the revised design

These go into the design doc's Assumptions / Consequences section:

1. **When-clause engine extension.** Adding `evidence.<field>: present`
   to the matcher vocabulary is in-scope for CD9's implementation PR.
   Must be delivered before any retry-routing template can work.
2. **Kernel version requirement.** `renameat2` requires Linux 3.15+.
   Release notes pin this; fallback to `link()` + `unlink()` on other
   Unixes.
3. **Advisory lockfile compatibility.** `flock` on
   `<session>/<parent>.lock` is process-lifetime only; `LocalBackend`
   already uses the primitive for `ContextStore` writes, so the
   stateless-CLI principle is preserved.
4. **`spawn_entry` snapshot on `WorkflowInitialized`.** Additive, marked
   `serde(default, skip_serializing_if = "Option::is_none")`; pre-CD10
   children deserialize fine.
5. **F5 compile rule is authoring burden.** Child templates used in
   batches must declare at least one state with `skipped_marker: true`.
   W4/W5 warnings surface common omissions. W-level (warn), not
   E-level (error), because batch-eligibility is not statically
   knowable at child-compile time.
6. **Per-task spawn failures are recoverable.** A submission with 10
   valid tasks and one bad template resolves: the 10 spawn, the 1
   appears with `outcome: spawn_failed` in the batch view. Subsequent
   resubmissions can retry the 1 (via corrected `template` or default).

## Round

Round 1. Cross-validation passed without restarting any decision.
