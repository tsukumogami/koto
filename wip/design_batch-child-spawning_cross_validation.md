# Cross-Validation: batch-child-spawning

## Summary

All six decisions composed cleanly. No hard conflicts found. Three
interaction notes recorded below as high-priority assumptions to surface
in the Implementation Approach section.

Cross-validation status: **passed**.

## Assumption Inventory

### Decision 1 (schema + hook + validation)

- `json` accepts field type will be implemented in the same PR
- Child naming is `<parent>.<task>` (dots legal in workflow names)
- Scheduler runs at CLI layer after the advance loop
- `format_version` stays at 1 (Decision 3 may override)
- v1 allows at most one `materialize_children` block per template
- `trigger_rule` field is reserved; only `all_success` accepted in v1

### Decision 2 (atomic init bundle)

- POSIX `rename(2)` is atomic on all supported filesystems
- `tempfile::NamedTempFile::persist` is the idiomatic in-tree pattern
  (already used by `write_manifest` in `src/session/local.rs:189-209`)
- The new `init_state_file` method is added to the `SessionBackend` trait
- Both `LocalBackend` and `CloudBackend` implement it

### Decision 3 (narrow deny_unknown_fields)

- Pre-merge audit confirms no existing templates rely on unknown fields
  as free-form annotations
- **Depends on Decision 1** choosing the state-level block shape (NOT
  gate type): if D1 had gone with the gate-type option, this decision
  would collapse entirely. D1 did choose `materialize_children` as a
  state-level field, so D3 stands.

### Decision 4 (template path resolution)

- New optional `template_source_dir` on `StateFileHeader` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`
- New optional `submitter_cwd` on `EventPayload::EvidenceSubmitted`
  (FIELD addition, not new event type — still compatible with D5's
  "no new event types" constraint)
- Cloud sync carries repo content at stable paths across machines
- Parent's original template path is captured at init time

### Decision 5 (failure + skip representation + retry)

- New `failure: bool` field on `TemplateState`
- New `skipped_marker: bool` field on `TemplateState`
- Context store (for `skipped_because`) is available during init
- Existing rewind machinery (epoch boundary via `Rewound` event) can
  re-run partially-completed children without breaking append-only
- `retry_failed: null` clears the retry evidence after the loop consumes
  it
- Non-batch parents report `blocked: 0` in the extended gate output
- Relies on **Decision 2's atomic init bundle** for skipped-child
  state file creation
- No `StateFileHeader` changes required by D5 itself (D4 adds one
  unrelated field)

### Decision 6 (observability)

- Decision 5 produces real child state files for skipped children
  (confirmed by D5's synthetic state file choice)
- `classify_task` and `build_dag` (new in `src/engine/batch.rs`) are
  side-effect-free and callable from read-only status paths
- Cloud backend tolerates `backend.list()` plus N child reads per
  `koto status` call at poll cadence

## Conflict Analysis

### Checked and resolved (no conflict)

1. **D4 adds a field to `EvidenceSubmitted`; D5 requires "no new event
   types."** Adding a field to an existing event (with `#[serde(default,
   skip_serializing_if = "Option::is_none")]`) is not the same as adding
   a new event type. The serialized form is backward-compatible — old
   binaries ignore the new field. D5's constraint is satisfied.

2. **D4 adds `template_source_dir` to `StateFileHeader`; D5 claims "no
   `StateFileHeader` changes."** D5's claim is scoped to its own design
   (D5 doesn't need any header fields), not a constraint on other
   decisions. D4 adds one header field and D5 adds none; both can
   coexist.

3. **D5's retry_failed rewinds children; D4's path resolution relies on
   `template_source_dir` captured at init time.** Rewinding appends a
   new epoch marker; it does not mutate the header or re-init. The
   `template_source_dir` field, once written at init time, is stable
   across rewinds. No conflict.

4. **D1 allows one `materialize_children` block per template; D5's
   retry transitions the parent back to `awaiting_children`.** The
   single-block constraint is about the parent template having one
   batch lifecycle, not about which state holds the hook. D5's retry
   operates on the existing batch's children via rewind — it does not
   re-materialize new tasks. The single-block invariant is preserved.

5. **D2's `init_state_file` method vs D5's "atomic header+first-event
   append" requirement.** These are the same concept at different
   levels. D2's backend-level `init_state_file` is the implementation;
   D5's reference to "atomic init path" is the consumer perspective.
   The retry handler in D5 calls through to the new `init_state_file`
   method, same as the scheduler's spawn path.

6. **D6's batch view consumes D5's extended gate output.** D5 defines:
   `success`, `failed`, `skipped`, `pending`, `blocked` aggregates, per-
   child `outcome` enum, `failure_mode`/`skipped_because`/`blocked_by`
   fields. D6 exposes these plus an additional `tasks[]` array that
   enumerates declared tasks from the batch definition (including
   un-spawned ones). The two views are compatible because D6 adds
   strictly over what D5 publishes.

### Cross-decision integration notes (for Implementation Approach)

- **Schema-layer changes all land together.** `TemplateState` grows
  four fields (`materialize_children`, `failure`, `skipped_marker`,
  plus implicit parsing for the json accepts type). The narrow
  `deny_unknown_fields` in D3 must include all of them. One PR.

- **`init_state_file` has three call sites.** The regular `koto init`
  path, the batch scheduler's spawn path, and D5's skipped-marker
  synthesis path. All three call through the same helper. Extracted
  once in the same PR as D2.

- **`EvidenceSubmitted` gains one optional field.** `submitter_cwd`
  from D4. The `EventPayload::Deserialize` path handles it via serde
  defaults; no version bump.

- **`StateFileHeader` gains one optional field.** `template_source_dir`
  from D4. Same serde pattern.

- **Retry loop reuses existing rewind machinery.** D5's retry_failed
  handler calls through to whatever `handle_rewind` exposes for
  per-child rewind. If `handle_rewind` today is CLI-only, an internal
  helper (`rewind_to_initial(name)`) is extracted and the CLI command
  becomes a thin wrapper.

## High-Priority Assumptions Surfaced

These are recorded in the final design doc's Assumptions section (to be
written in Phase 6 / frontmatter):

1. **Pre-merge audit of existing templates.** Before merging the
   `deny_unknown_fields` change from D3, grep the repo for any template
   fixture that relies on unknown fields as annotations. Remove or
   migrate first.

2. **`retry_failed` uses the existing rewind machinery.** If rewind
   semantics change in a future koto version, the retry loop must be
   re-validated. Note in the design.

3. **Scheduler cost at poll cadence.** D6 relies on
   `backend.list()` + per-child reads inside `koto status`. For large
   batches (50+ children) on cloud sync, this may approach per-call
   rate limits. Benchmark during implementation.

## Round

Round 0 (no restarts required).
