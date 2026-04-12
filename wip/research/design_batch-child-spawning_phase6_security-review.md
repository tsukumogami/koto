# Security Review (Phase 6, second pass): batch-child-spawning

## Scope

Second-pass verification of the Security Considerations section added
after the Phase 5 recommendation (Option 2 - document considerations).
Focus is on attack vectors Phase 5 may have missed, whether documented
mitigations are actually enforced in the implementation plan, whether
"not applicable" classifications still hold, and residual risk for
koto's threat model. Content governance is re-verified as a
sanity check.

## 1. Attack vectors Phase 5 may have missed

### 1a. `--with-data` reserved-key collision with batch task lists

**Finding: not a collision risk today, but a latent one.**

`src/cli/mod.rs::evidence_has_reserved_gates_key` rejects only
top-level `"gates"` keys on `--with-data` submissions (lines 537-545).
The check does not recurse into objects or arrays.

A batch submission takes the form `{"tasks": [{...}, {...}]}`. A
task entry carries `name`, `template`, `vars`, `waits_on`,
`trigger_rule`. None of these collide with `gates` at the top level,
so the existing reserved-key check is not defeated. Good.

However, two latent risks are worth flagging:

1. **`retry_failed` is not reserved.** Decision 5.4 introduces a
   top-level evidence field `{"retry_failed": {...}}` that the parent
   submits to trigger rewind. Unlike `gates`, there is no compile-time
   or runtime check that `retry_failed` is only used as intended — it
   is just evidence. A template author could declare `retry_failed`
   in accepts for an unrelated purpose, and the scheduler's
   `handle_retry_failed` pre-pass at Phase 3 would trigger on a
   submission that wasn't meant as a retry action. The design should
   add `retry_failed` to the reserved-key check (same path as `gates`)
   OR validate that a template declaring `materialize_children` also
   uses `retry_failed` exclusively for the retry action OR document
   that `retry_failed` is a reserved evidence field name in Decision
   1 alongside `gates`. The cleanest path is "reserve
   `retry_failed` symmetrically with `gates`" — implementation is a
   one-line addition to `evidence_has_reserved_gates_key`.

2. **`tasks` is not reserved either.** The `from_field` in
   `materialize_children` declares which accepts field carries the
   task list, but that field is chosen by the template author. If an
   author names it `tasks` and the compiler E8 check ensures a single
   hook per template, there is no collision within a template. Across
   submissions with no hook, an agent submitting evidence with a
   `tasks` field is ignored — no harm done. This is acceptable.

**Mitigation requested.** Add `retry_failed` to
`evidence_has_reserved_gates_key` (rename it or generalize it) so
that an agent submitting `{"retry_failed": ...}` without the parent
template explicitly opting into that evidence action gets rejected.
This is a small, high-value guard; without it, the semantics of
`retry_failed` depend on template authors not accidentally declaring
it as a regular accepts field.

### 1b. `retry_failed` double-rewind / retry-of-skipped semantics

**Finding: real implementation gap.**

The design at Decision 5.4 says `rewind_to_initial(name)` reuses
`handle_rewind`'s machinery. Today `handle_rewind` in
`src/cli/mod.rs:1198-1204` refuses to rewind when there is only one
state-changing event, erroring with `"already at initial state,
cannot rewind"`.

Consider a skipped child synthesized by the scheduler: its only
state-changing event is the initial `Transitioned: null →
skipped_marker_state`. When `retry_failed` with `include_skipped:
true` runs `rewind_to_initial` on that child, the existing rewind
guard would abort the retry. The design does not say how
`rewind_to_initial` differs from `handle_rewind` in this case.
Three options, all acceptable, but one must be chosen:

- **(a)** `rewind_to_initial` is permissive: it no-ops when the
  child is already at its initial state, allowing the normal
  scheduler tick to re-classify the child. This is consistent with
  the idempotency theme.
- **(b)** `rewind_to_initial` appends a `Rewound` event even when
  already at initial, to mark that a new epoch has opened. This
  ensures downstream `merge_epoch_evidence` starts fresh.
- **(c)** Skipped children are deleted and re-synthesized instead of
  rewound. Cleaner semantically but breaks append-only.

Option (b) is probably right — it matches the "retry opens a new
epoch" mental model — but the design must pick and document one. A
second `retry_failed` invoked before the child has had a chance to
resume would then either no-op or append a second `Rewound`, both
recoverable.

There is no state corruption risk from double-rewind on a
non-skipped child: each `Rewound` event opens a new epoch, and
`merge_epoch_evidence` starts over. The risk is narrowly the
skipped-child case where there is nothing to rewind *from*.

**Recommendation.** Decision 5 should be extended with one paragraph
describing `rewind_to_initial`'s behavior on a child whose only
state-changing event is the initial `Transitioned`, and must cover
the skipped-child case explicitly.

### 1c. Cloud sync race: two machines submit different task lists simultaneously

**Finding: real risk, but the existing conflict-detection machinery
catches it. Documentation gap.**

I traced the cloud sync path in `src/session/cloud.rs` and
`src/session/version.rs`. The sync protocol is a three-way version
check:

- `sync_push_state` writes a full state file PUT to S3 (not an
  incremental append). See `src/session/cloud.rs:81-97`.
- `check_sync` in `src/session/version.rs:88-113` compares local
  version, remote version, and `last_sync_base`. If both sides
  advanced past the sync base (Conflict variant), the push is
  refused and the user must run `koto session resolve --keep local`
  or `--keep remote`.

This means the append-only log does NOT diverge on disk — S3
overwrites atomically, and the version check prevents one side
from silently clobbering the other. BUT:

- **Only whole-state resolution is offered.** There is no
  merge-both-sides path. If machine A submits `tasks=[1,2,3]` and
  machine B submits `tasks=[4,5,6]` in parallel, then on sync, the
  losing side's `EvidenceSubmitted` event is discarded. Because the
  batch scheduler derives the task set entirely from evidence, the
  losing side's task list vanishes, and any children it already
  spawned locally are orphaned (their state files still exist on
  the losing machine but are not referenced by the parent's batch
  definition on the winning side).
- **The orphaned children get cleaned up... how?** They have
  `parent_workflow` set but no corresponding task entry. On the
  next `koto workflows --orphaned` call they should show up; on the
  next `koto next parent` call on the winning side, the scheduler
  classifies tasks in the batch definition — it does not
  spontaneously spawn what was on the losing side.

The severity is bounded: koto is single-user, cloud sync is used
for cross-machine convenience, and the documented conflict
resolution is explicit user action (`koto session resolve`). The
attacker model is not adversarial. But the documentation gap is
real: the design's Dynamic Additions story (line 58-61) says
"`merge_epoch_evidence` unions new tasks with existing" — which is
true WITHIN a single event log, but across a sync conflict, the
losing side's events are lost wholesale.

**Recommendation.** Add one bullet under Security Considerations →
Resource bounds (or a new subsection "Cloud sync and concurrent
submission"):

> Two machines submitting batch evidence to the same parent
> concurrently trigger koto's standard sync-conflict detection
> (`koto session resolve`). The losing side's task submissions and
> any children it spawned locally become orphaned until the user
> reconciles. Do not rely on cloud sync to merge two independently-
> composed task lists — compose the full list on one machine and
> submit it once.

### 1d. Symlinks inside the repo (legit, not pre-seeded)

**Finding: no new risk; existing canonicalization is sufficient.**

Decision 4 says `template_source_dir` is populated "by canonicalizing
the `--template` argument's parent directory" at `handle_init` time.
`std::fs::canonicalize` resolves symlinks transitively, so a
developer who symlinks `templates/parent.md → ../shared/parent.md`
gets the real path persisted. Relative resolution in the scheduler
joins against the canonicalized path, which is stable across
cwd changes.

The one subtlety: if a developer later removes or moves the
symlink target, the persisted `template_source_dir` still points
at the old canonical path. This is a correctness issue, not a
security issue — the scheduler will fail to resolve templates and
error out.

Cloud-sync across machines with different symlink layouts is a
separate concern (already flagged in Phase 5 under "New fields
carry local absolute paths") — the canonicalized path is machine-
local and may not exist on the syncing peer. That is documented.

No new recommendation.

### 1e. Cycle detection timing

**Finding: pre-spawn for initial submission; the design does not
address the dynamic-addition edge case.**

The Data Flow section at step 4 (line 998-999) is explicit: the
scheduler "Builds the DAG and runs runtime validation (R1-R7).
Cycles, dangling refs, and duplicate names fail the whole
submission." This ordering means cycle detection runs BEFORE any
spawn happens on the initial submission. Good.

The dynamic-additions case is murkier. When a running child
submits an `EvidenceSubmitted` event with additional tasks (via
`merge_epoch_evidence`), the scheduler's next tick re-derives the
full merged task set and re-runs R1-R7. But some tasks from the
pre-merge set have already been spawned. If the merge creates a
cycle that involves an already-spawned task, the design says
"fail the whole submission" — but what does that mean
operationally?

- **Option A:** the scheduler logs a failure and refuses to
  classify further. Already-spawned children keep running to
  completion but no new tasks spawn. The batch is stuck.
- **Option B:** the scheduler emits a scheduler error, the
  `await` state waits indefinitely on the `children-complete`
  gate, and the operator must intervene.
- **Option C:** the scheduler rejects the cyclic *new* tasks
  only, preserving the old DAG. This would require DAG-level
  validation that can isolate the problematic edges.

The design does not pick. Without a choice, Option A/B are the
de-facto behavior (failure bubbles up, batch is jammed).

The security impact is minimal — it's a self-DoS where a
misbehaving child injects a cyclic task. On koto's threat model
(trusted collaborator agents), this is an honest-mistake
failure mode, not an attack. But it is worth documenting.

**Recommendation.** Add one sentence to Decision 5 or the Data
Flow section: "If a dynamic addition introduces a cycle with
already-spawned tasks, the scheduler fails classification on that
tick and emits a `BatchError`; existing children continue to
completion, but no further tasks spawn until the cyclic evidence
is cleared via `retry_failed` or manual intervention."

## 2. Are mitigations sufficient for the identified risks?

### 2a. Soft resource bounds (1000 tasks, 10 edges/task)

**Finding: documented but not specced. Enforcement location is
undefined.**

Security Considerations → Resource bounds (lines 1253-1263) says:
"The scheduler additionally enforces soft limits on task count
(recommended: no more than 1000 tasks per batch) and edge count
(recommended: no more than 10 `waits_on` entries per task)."

The word "enforces" is aspirational: the Implementation Approach
(Phase 3 deliverables, lines 1088-1141) does not include a task
count check, edge count check, or an explicit `R8` validation
rule. The E1-E8 and R1-R7 validation tables stop at the existing
checks. There is no line item like "R8: task count ≤ 1000" or
"R9: `waits_on` edges per task ≤ 10".

This is an implementation gap. One of the following must be true
for the security documentation to be accurate:

- Add R8 and R9 rules to Decision 1's validation table and
  Phase 3 deliverables, with explicit error codes and messages.
- Or demote the language from "enforces" to "recommends,
  unchecked" and accept that 100k-task submissions will degrade
  performance but not be rejected.

The cleanest fix is the first option: add two runtime checks to
`build_dag`/`run_batch_scheduler`, with Phase 3 tests. The soft
limits become hard limits with clear error messages. This
matches the 1 MB `--with-data` cap, which IS enforced
(`src/cli/mod.rs:1296-1308`).

**Recommendation.** Either specify R8/R9 in the validation tables
with enforcement location, or rewrite the Security Considerations
bullet to say "recommended but not enforced; agents SHOULD keep
batches under these bounds to avoid self-DoS."

### 2b. Per-child `reason` sourced from context key, not stderr

**Finding: documented, enforced by design (not by code — because
there is no code path that could accidentally scrape stderr).**

Decision 6 and Security Considerations both say `reason` comes from
a `failure_reason` context key. The implementation plan in Phase 3
describes `derive_batch_view` as reusing `classify_task` and
`build_dag`. Neither of these needs stderr. The only risk is a
future implementer adding stderr scraping as a "convenience
fallback". As long as code review enforces the documented contract,
this is fine.

**Recommendation.** Add a code comment on `derive_batch_view` in
Phase 3 saying `// reason must come from the failure_reason context
key only; NEVER scrape stderr or raw tool output` so future
contributors don't regress this.

### 2c. Session directory assumptions (O_EXCL, no symlink-follow on rename)

**Finding: correct, matches existing `write_manifest` pattern. No
action needed.**

`tempfile::NamedTempFile::persist` uses `rename(2)`, which operates
on the link, not the symlink target. A pre-existing symlink at the
final path would be overwritten as a symlink, not followed. This
matches `src/session/local.rs:189-209`. The Security Considerations
paragraph on symlinks is accurate.

### 2d. `template_source_dir` canonicalization

**Finding: implied but not specified.**

The design at line 394-396 says `template_source_dir` is "populated
at `handle_init` time by canonicalizing the `--template` argument's
parent directory." Good. This needs to land as `std::fs::canonicalize(path).parent()`
or equivalent, and must handle the case where canonicalize errors
(e.g., template file deleted between parse and canonicalize). Today
`handle_init` does not call canonicalize at all — see
`src/cli/mod.rs:1112-1125`, which stores the template hash but not
the source path. Phase 2 deliverables must include this code path.

**Recommendation.** Phase 2 deliverables should add an explicit
bullet: "`handle_init` canonicalizes `--template` before storing
`template_source_dir`; errors on canonicalize surface as
`InvalidTemplatePath` with the raw and canonical attempts."

## 3. Supply Chain: does the claim still hold?

**Finding: yes, fully holds.**

I verified `Cargo.toml`:

- `tempfile = "3"` — pre-existing, used by `write_manifest`.
- `serde = "1"` — pre-existing.
- `serde_json = "1"` — pre-existing.
- No new crates, no feature flag changes, no version bumps.

The init bundle refactor introduces no new dependencies. The
`--with-data @file.json` prefix uses `std::fs::read_to_string`,
not a new file-loading crate. The JSON shape extensions use the
existing `serde_json::Value`. The DAG builder, cycle detector, and
task classifier are all pure Rust with no crate dependency.

Phase 5's "Supply Chain: No new trust risk" classification is
accurate and still applicable.

## 4. Residual risk for koto's threat model

Koto's threat model: local-user tool, trusted-collaborator agents,
developer-authored templates, single-user cloud sync. Within that
model, after the second-pass check, the residual risk is:

- **Low:** `retry_failed` not being a reserved evidence key
  (addressable via one-line check, see 1a).
- **Low:** Undefined `rewind_to_initial` behavior on skipped
  children (implementation gap, not a threat).
- **Low:** Dynamic-addition cycle edge case is undocumented but not
  exploitable (self-DoS only).
- **Low:** Soft resource bounds are unenforced; a misbehaving agent
  can submit a 100k-task batch. Self-DoS only; 1 MB `--with-data`
  cap still protects against pathological payloads.
- **Low:** Cloud-sync conflict on concurrent batch submissions
  loses one side's task list. Existing conflict-detection surfaces
  the problem to the user, but the docs don't mention the batch
  angle.

None of these cross a privilege boundary. None enable a classic
confidentiality/integrity/availability compromise beyond what an
invoking user could already inflict on themselves.

## 5. Content governance re-check (public repo)

Scanned `docs/designs/DESIGN-batch-child-spawning.md` for internal
references, competitor names, and private issue numbers.

- `tsukumogami/shirabe#67` — `tsukumogami` is the public GitHub org
  (verified via `git remote -v`: `git@github.com:tsukumogami/koto.git`),
  `shirabe` is a public repo under it, issue #67 is public.
- `issue #129` — public koto issue.
- No competitor names appear.
- Writing-style banned words: none found (`tier|robust|leverage|
  comprehensive|holistic|facilitate` all absent).
- `wip/` cross-references: the design body references
  `wip/design_batch-child-spawning_decision_2_report.md` (line 314),
  `wip/design_batch-child-spawning_decision_5_report.md` (line
  575), and `wip/explore_batch-child-spawning_*` / `wip/research/`
  (lines 73-74). Per `CLAUDE.md`, `wip/` must be empty before
  merge, so these references will dangle once the PR squash-
  merges. This is a hygiene issue, not a security issue, but the
  doc author should either inline the relevant material or drop
  the cross-references before the PR lands.

Content governance is clean. The only item is the wip/
cross-references, which is already flagged in Phase 5.

## Verdict

**Concur with Phase 5, with minor additions.**

Phase 5 correctly identified that the design is sound for koto's
threat model and recommended Option 2 (document considerations).
The Security Considerations section written into the design is
accurate as far as it goes. Second-pass review surfaces four
small additions, none blocking, all addressable with
documentation or one-line code changes:

1. **Reserve `retry_failed` symmetrically with `gates`** in the
   `--with-data` validator (one line in
   `evidence_has_reserved_gates_key`, rename or generalize the
   function). Without this, a template that inadvertently
   declares a `retry_failed` accepts field can trigger scheduler
   rewind behavior on unrelated submissions.

2. **Specify `rewind_to_initial` behavior on skipped children**
   in Decision 5.4. The current design implies reuse of
   `handle_rewind`, which refuses to rewind workflows with only
   one state-changing event — exactly the case for a synthesized
   skipped child. Pick the no-op path or explicit-new-Rewound
   path and document it.

3. **Either enforce soft resource bounds (R8/R9) or downgrade the
   Security Considerations language from "enforces" to
   "recommends, unchecked."** The current wording promises
   enforcement that the Implementation Approach does not deliver.

4. **Add one paragraph to Security Considerations on
   cloud-sync-conflict behavior for concurrent batch
   submissions**, clarifying that the losing side's task list is
   discarded and orphaned children must be reconciled via
   `koto session resolve`. The underlying mechanism is already
   implemented and correct; the gap is documentation.

Additionally, the dynamic-additions cycle edge case (1e)
warrants one sentence in Decision 5 or the Data Flow section.
Already-spawned children should continue running; only new
tasks in the cyclic evidence should be rejected, and the
failure mode should be explicit.

The implementation can ship with items 3 and 4 alone if review
velocity requires tight scope — items 1 and 2 are true gaps that
should land before Phase 3, but they are small. The design's
overall security posture is ready for implementation.
