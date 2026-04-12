# Architecture Review: DESIGN-batch-child-spawning

Reviewer role: experienced Rust engineer familiar with the koto codebase
(engine advance loop, session backends, gate evaluator, CLI handlers).

Scope: the Solution Architecture and Implementation Approach sections of
`docs/designs/DESIGN-batch-child-spawning.md`, cross-referenced against
the six decision reports, the cross-validation note, and the actual code
in `src/engine/advance.rs`, `src/cli/mod.rs`, `src/engine/types.rs`, and
`src/gate.rs`.

## Clarity

Overall the design is implementable. Any competent Rust contributor who
has touched `handle_next` before can start coding Phase 1 or Phase 2
without more design work. The Components diagram, the Key Interfaces
block, and the Data Flow section between them cover the happy paths.
Decision reports fill in crash-recovery edge cases that would be tedious
to restate in the design itself. That combination is enough.

That said, several places leave the implementer with a decision to
make. The below are not blockers but should be resolved in the PR
descriptions or as inline comments to avoid drift:

1. **Scheduler-tick ordering vs advance-loop gate evaluation.**
   `handle_next` runs `advance_until_stop` first, which evaluates the
   `children-complete` gate if the parent has already reached
   `awaiting_children`. Only after the loop settles does
   `run_batch_scheduler` run. The implication is not spelled out: on
   the **first** submission of a batch, the advance loop reaches
   `awaiting_children`, the gate evaluates zero-children (which the
   current code at `src/cli/mod.rs:2507-2519` flags as
   `GateOutcome::Failed` with `"no matching children found"`), and the
   loop returns `GateBlocked` before the scheduler gets a chance to
   spawn anything. The batch scheduler then spawns and writes events,
   but the response returned to the caller is already serialized (or
   about to be) with `GateBlocked`. The caller must call `koto next`
   a second time to drive progress. That's a viable contract but it's
   not stated. If the intent is different — e.g., re-run `advance_until_stop`
   after the scheduler spawns children — the design needs to say so,
   because that's a non-trivial loop shape that the advance module
   currently does not support.

   Related: the zero-children "no matching children found" branch in
   `evaluate_children_complete` has to be taught about batch state so
   that "zero children but the scheduler will materialize some" does
   not return an error. Either the gate evaluator needs the batch
   context too, or the scheduler tick needs to run before the first
   `advance_until_stop` call. The design picks the latter in the
   diagram ("`handle_next → advance_until_stop → run_batch_scheduler`")
   but the consequence above indicates that's only sufficient for
   later ticks, not the first.

2. **`parent_events` threaded into the gate evaluator.** The design
   says `evaluate_children_complete` must also receive `parent_events`
   so it can read the batch definition. The current closure capture at
   `src/cli/mod.rs:1714-1720` has access to `name` and `backend` but
   not to `events`. Adding `events` to the capture is easy, but the
   design should explicitly note that `evaluate_children_complete`'s
   signature grows a parameter and that the closure at line 1719 is
   updated accordingly. Current wording leaves the implementer to
   infer it.

3. **Where `retry_failed` evidence enters the advance loop.** Section
   5.4 of Decision 5 says the parent transitions from a post-analysis
   state back to `awaiting_children` via "a template-defined route"
   driven by `retry_failed: "*"` evidence. The data flow section in
   the design says `handle_next` calls the scheduler which "detects
   the unconsumed `retry_failed` evidence and runs `handle_retry_failed`".
   Those two statements describe different mechanisms. One has the
   advance loop perform the transition via an evidence-gated
   transition; the other has the scheduler observe evidence and act.
   Both can coexist but the PR author needs to implement *exactly*
   one hand-off, and the current wording lets either reading through.

4. **`classify_task` state vocabulary.** The data flow text lists
   seven classifications (`Terminal`, `Failed`, `Skipped`, `Running`,
   `NotYetSpawned/Ready`, `NotYetSpawned/BlockedByDep`,
   `NotYetSpawned/ShouldBeSkipped`). The Components diagram references
   `classify_task` without saying whether the return type is an enum,
   a struct with a tag, or something else. A concrete `enum TaskStatus`
   should land in the interface block.

5. **`BatchError` surface.** The module interface shows three entry
   points returning `Result<_, BatchError>`. The error variants are
   not listed. Are cycles, dangling references, duplicate names,
   unresolvable template paths, backend I/O errors, and malformed
   evidence all one enum? How do they bubble through `handle_next`
   into `NextResponse`? The scheduler has to map batch-layer errors
   onto the existing `NextError` vocabulary (or add a new
   `BatchError` variant to it), and the design does not commit to
   one.

## Missing Pieces

### Underspecified helpers already named

- **`build_dag`.** The design mentions it but never specifies the
  return type or how it handles the runtime checks R1–R7. Is it
  `Result<Dag, R1Error | R2Error | ...>` or does it return a
  collection of diagnostics plus a best-effort graph? The
  distinction matters because R1–R7 mix hard errors (cycle, duplicate
  name) with fatal-to-one-task errors (bad template path) that
  probably should not fail the whole submission.

- **`derive_batch_view`.** Stated as "side-effect-free and callable
  from read-only paths", but the data-flow for `koto status` says it
  calls `backend.list()` plus per-child `read_events`. Those are I/O,
  just read-only I/O. Worth restating: `derive_batch_view` is pure
  with respect to state mutations but not with respect to I/O. The
  distinction matters because it determines whether the function
  is Send+Sync, whether it can be called from test code with a fake
  backend, and whether it's safe to call without taking the state
  file lock.

- **`handle_retry_failed`'s DAG source.** It receives
  `template: &CompiledTemplate` and `events: &[Event]` and
  `retry_set: &[String]`. The DAG is reconstructed from `events`.
  But: what happens if the parent is in a state that no longer
  references the original `materialize_children` hook (e.g., the
  parent has progressed to `analyze_results` which has no hook)?
  The retry handler needs to find the batch definition in the
  evidence log regardless of the current state. That lookup rule —
  "walk the evidence log backward to find the most recent
  `EvidenceSubmitted` with the batch `from_field` populated" — should
  be specified.

- **`classify_task` interface with `failure_policy`.** Classification
  depends on the failure policy (`skip_dependents` vs `continue`).
  The function signature should take the policy, or be a method on a
  `Scheduler` struct that owns the policy. This is a minor point but
  not stated.

### Helpers implied but not named

- **`init_child_from_parent(parent_header, task, template_source_dir, submitter_cwd)`**
  — the helper the scheduler uses to spawn a child by delegating to
  `init_state_file`. The design says "a helper refactored from
  `handle_init`" but never names it. This helper is subtle: it has
  to replicate the variable-resolution step from `handle_init`
  (`resolve_variables(vars, &compiled.variables)` at
  `src/cli/mod.rs:1096-1107`) using the child template's declared
  variables, not the parent's. That's a real refactor, not a trivial
  extraction. Name it and list its call sites (regular init, scheduler
  spawn, skipped-marker synthesis).

- **`repair_half_initialized_children(parent)`**. Decision 5's
  resume-story walkthrough mentions this as a pre-pass in
  `handle_next`, but the design's Implementation Approach section
  does not list it as a Phase 3 deliverable. It is required for the
  crash-mid-init recovery story to actually work. Add it to Phase 3
  explicitly, or note that Phase 1's atomic init bundle makes it
  unnecessary (arguable — the atomic init fix prevents *new*
  half-initialized children, but legacy state files created before
  Phase 1 shipped could still exist on disk). Given Phase 1 lands
  first, it's most likely unnecessary, but the design should say so
  and delete the Decision 5 helper reference, or keep the helper and
  document why.

- **`internal_rewind_to_initial(name)`**. Named in both the design
  and Decision 5, but the design doesn't say whether it lives in
  `src/engine/batch.rs`, in a new engine helper module, or in
  `src/cli/mod.rs` next to `handle_rewind`. Today `handle_rewind`
  reads events, computes the penultimate state, and appends one
  `Rewound` event. "Rewind to initial state" is different: it
  creates a single `Rewound` event with `to = initial_state`
  regardless of event history, effectively clearing the epoch.
  That's new machinery, not just an extraction — the existing
  `handle_rewind` only goes back one step. Worth calling out
  explicitly so the implementer doesn't assume a simple extract.

- **Cache invalidation story for `CompiledTemplate` cached JSON.**
  When Phase 2 adds `deny_unknown_fields` to `SourceState` and
  `TemplateState`, existing compiled-template cache files on disk
  (at whatever path `load_compiled_template` points to) may
  deserialize differently. If an old cached JSON contained a field
  that the new binary does not recognize, deserialization fails.
  That means every user upgrading to the batch-enabled release
  needs a cache-invalidation step. The design's pre-merge audit
  covers source templates, not the cache. Either the cache key
  includes a version bump, or the release notes tell users to
  clear `~/.koto/cache/templates/` (or wherever). Missing.

- **`validate_with_data_payload` and `@file` prefix.** Listed in
  Phase 2 but not shown in the Key Interfaces block. Resolution
  rules (relative to cwd? absolute? symlink-followed? 1 MB cap
  applies to resolved content, but what about the file itself —
  is there a read-size cap before parsing?). Needs at least a
  sentence on resolution and security (the security section
  doesn't mention it).

## Phasing

The three-phase split is reasonable. Phase 1 (atomic init bundle) is
genuinely independent and ships a pre-existing correctness bug fix
regardless of batch landing. Phase 2 (schema-layer) blocks Phase 3
because the scheduler reads the new template fields. Phase 3 is the
actual feature.

Things to reconsider:

- **Phase 2 is two phases in disguise.** It bundles six decisions'
  worth of changes: three `TemplateState` fields, `deny_unknown_fields`,
  a new accepts field type, a new `StateFileHeader` field, a new
  `EventPayload::EvidenceSubmitted` field, eight new compiler error
  rules, two new warnings, and `--with-data @file` support. The
  Mitigations section admits "split Phase 2 if review velocity stalls"
  — the design should just commit to the split upfront. A sensible
  break:
  - **Phase 2a (safety and compat).** `deny_unknown_fields` on
    `SourceState`/`TemplateState`, the pre-merge audit, cache
    invalidation, `template_source_dir` header field, `submitter_cwd`
    event field, `--with-data @file` resolution. All defensive
    plumbing with no new template surface. Ships on its own, risk is
    bounded to backward compat.
  - **Phase 2b (template surface).** `materialize_children` spec,
    `failure`, `skipped_marker`, the `json` accepts type, the E1–E8
    compiler rules, the W1–W2 warnings. Lands the authoring contract
    without any runtime.
  Splitting does not extend the critical path — 2a and 2b can ship
  concurrently once 2a's fields land — but it gives reviewers a
  smaller surface per PR and separates "backward-compat risk" from
  "schema risk".

- **Phase 1 could ship earlier than the batch feature entirely.**
  Phase 1 has no dependency on the rest of the work. It should be
  filed as its own issue right now, not wait for the batch
  implementation to start. The design says as much in Consequences
  but not in Implementation Approach; make it explicit.

- **Phase 3 is large but correctly one chunk.** Scheduler, retry,
  observability, extended gate output, skill updates all share
  enough code that splitting them forces ugly intermediate states.
  Keep it as one PR.

## Simpler Alternatives

**(a) Does the batch feature need a new engine module?**

Half-yes. The scheduler logic (`build_dag`, `classify_task`, cycle
detection, the transitive-closure math) is enough code that it
should live in its own module for testability. Those functions are
pure and benefit from isolated unit tests that don't have to
stand up a full `handle_next` harness.

But the module name is wrong. `src/engine/batch.rs` sits next to
`advance.rs`, `types.rs`, and `persistence.rs` — engine primitives
that are called by many CLI handlers. The batch scheduler is CLI-
layer concern: it calls `backend.list()`, it reads the session
backend, it knows about the `handle_next` response shape. Engine
modules today do not. `src/cli/batch.rs` or `src/cli/scheduler.rs`
is the right neighborhood. The design's own decision to "keep the
advance loop pure (I/O-free, closure-driven)" is at odds with
putting batch code in the engine.

This is a naming nit, not a structural objection. But if the design
is landing as-written, the author should understand they're
importing a session backend into `src/engine/`, which no current
engine module does. That is a precedent worth considering.

**(b) Is `init_state_file` really needed as a new trait method?**

Yes. This one is well-justified. The alternative is a local helper
in `src/cli/mod.rs` that calls `create` + `append_header` +
`append_event`, which is exactly what exists today, and which has
the same crash window. The whole point of the method is that the
*backend* is responsible for atomicity, so `LocalBackend` and
`CloudBackend` can implement it differently — local via
`tempfile::persist`, cloud via local + one `sync_push_state`. A
local helper cannot do this without the backend growing an escape
hatch. Keep the trait method.

Minor tweak: the method could take a single `Vec<Event>` instead of
splitting `header` from `initial_events`, folding the header
serialization into the bundle. Cleaner interface, same semantics.

**(c) Could `retry_failed` be simpler?**

Probably not meaningfully. The rewind-based approach is load-bearing
on keeping append-only semantics, cloud-sync compat, and history
preservation. A simpler approach (delete child state files and
re-init) was correctly rejected. The current design's one real
complexity cost is the "consume the retry evidence by writing a
clearing null event" pattern — this is subtle enough that future
maintainers may mis-handle it. Two possible simplifications:

1. **Encode retry as a new event type** (`RetryRequested { children
   }`) instead of a clearing evidence pattern. Cost: new event type,
   violates the "no new event types" constraint in Decision 5.
   Benefit: no `null`-evidence hack. Probably not worth it given
   the constraint.

2. **Store retry state in the parent's context store instead of
   evidence.** The context store already exists and is session-
   scoped. Context writes are append-only events too. This gives
   the same durability without overloading the evidence channel.
   Worth a paragraph in the design explaining why evidence won.

Neither is a blocker. The current design works. But the
"consumption by clearing" idiom is the kind of thing that will
confuse someone reading the code in six months, and it deserves a
prominent comment block in `handle_retry_failed`.

## Regret Candidates

Hindsight-bias sniff test on each non-obvious choice:

### Per-hook `failure_policy` (not per-task)

Low regret risk *for v1*, but moderate risk of expansion pressure.
The design's reasoning is correct: letting agents override the
policy per-submission would invalidate parent-template guarantees.
But per-task `trigger_rule` is deferred, not rejected, and when it
does land, it will compete with `failure_policy` for semantics.
If `trigger_rule: all_done` means "run even if deps failed", and
`failure_policy: skip_dependents` means "skip me if my dep failed",
the two interact. The design would benefit from a one-liner saying
"when per-task `trigger_rule` ships, per-hook `failure_policy`
becomes the default and `trigger_rule` overrides it per-task."
Pin the interaction now so it doesn't surprise us later.

### Deterministic `<parent>.<task>` naming

Low regret. The design admits that parents cannot be renamed because
children couple to parent names. That's a fine constraint for the
primary use case and the decision report makes the trade-off
explicit.

One hidden footgun: **if a user renames a parent template** (not
the workflow, the template file), the child names are stable because
they derive from workflow name not template name. So the regret
vector is "user renames workflow", which already breaks unrelated
things (state files, the hierarchical index). Acceptable.

### "Single batch per template" restriction (E8)

Moderate regret risk. The Consequences section acknowledges this is
"a real constraint". The design correctly notes that Reading B
(nested `koto init --parent`) is the escape hatch. But the escape
hatch requires falling back to the very spawn-loop prose that this
feature is supposed to eliminate. For users who want two parallel
fanouts in the same template, they will either:

- Give up and run everything sequentially, losing parallelism.
- Write two templates and chain them, adding indirection.
- Fall back to Reading B and write spawn-loop prose, negating the
  benefit.

This is not a blocker for v1 — the GH-issue use case is single-fanout
— but expect it to become the top feature request within two minor
releases. Worth noting in the design that the E8 check is explicitly
temporary, not a designed invariant, and worth an issue tracking
multi-batch support filed the same day this ships.

### Soft resource bounds without hard enforcement

Moderate regret risk. The Security Considerations section recommends
"no more than 1000 tasks per batch" and "no more than 10 `waits_on`
entries per task" as soft limits. "Soft" means the design does not
enforce them. In practice, someone will submit a 5000-task batch,
hit the quadratic-ish per-tick cost, and file a bug. At that point
the options are (a) add the hard limit retroactively with a
migration headache, or (b) optimize the scheduler to handle the
large case. Neither is free.

Recommendation: enforce the limits at evidence submission time (not
at compile time — the task list is agent-supplied, not template-
supplied) with clear error messages. "Your task list has 5000
entries; the current limit is 1000." Easier to loosen a limit later
than to tighten one. Also: bench the scheduler with 1000 tasks
before calling the limit soft; the design promises quadratic-ish
cost per tick but has not measured it.

### `retry_failed` evidence as a "null clearing" idiom

Moderate regret risk. Same concerns as under Simpler Alternatives
— the idiom works, but it is subtle. If a future feature adds
multiple concurrent retry submissions, or if `merge_epoch_evidence`
semantics change (it currently does last-write-wins, which is what
the design relies on), the retry loop breaks silently. Put a
safety comment in `merge_epoch_evidence` referencing
`handle_retry_failed` so the dependency is visible from both sides.

### `template_source_dir` in `StateFileHeader`

Low regret with one caveat. The field is optional and backward-
compatible. The caveat: when cloud sync copies a state file to
another machine, `template_source_dir` is an absolute path on the
originating machine. The design says "both bases point at repo
content, not koto cache" and assumes "koto already assumes repo
checkouts have stable paths across machines" — but that assumption
is not verified anywhere in the design. If the user's repo lives at
`/home/alice/projects/foo` on one machine and `/Users/alice/foo` on
another, relative path resolution via `template_source_dir` fails,
and the fallback `submitter_cwd` has the same problem. The real
answer is probably "resolve as relative to whichever ancestor
directory matches both paths" — but that's a feature. For v1, it is
enough to document that cross-machine template resolution requires
identical repo paths, and to make the error message helpful when it
fails.

## Invariant Violations

### Append-only state files

No violations found in the design for *existing* event writes. The
atomic init bundle writes a new file via `tempfile::persist` and
then uses `append_event` for every subsequent event. The `.tmp`
file lives outside the final path so `list()` ignores it. Header
mutations are avoided.

One soft concern: `init_state_file` writes multiple events in a
single I/O operation. The design promises byte-identical on-disk
layout compared to the three-call sequence, but JSONL format is
preserved only if the temp file is written with one event per line
and a trailing newline after each. The implementation needs to
match `append_event`'s existing format exactly (including any
trailing-newline or BOM behavior) or `read_events` will see the
bundle as a single malformed line. Not a design flaw; flagging so
the implementer writes a round-trip test.

### Stateless CLI / no daemon

Honored. `run_batch_scheduler` is a pure function of the on-disk
state plus the template, with no persistent cursors. The retry
loop consumes its own trigger. Every invariant the design depends
on is re-derived on each tick.

### Atomic event sequencing

Honored with one caveat. The scheduler spawns children sequentially:
for each ready task, call `init_state_file`, move on. If a crash
happens between spawning child N and child N+1, child N exists and
child N+1 does not. On resume, the scheduler classifies child N as
`Running` and child N+1 as `Ready` and spawns it. Idempotent.

Caveat: the scheduler also appends events to the parent's log to
record scheduler outcomes — does it? The design's data-flow section
says "handle_next maps SchedulerOutcome into the response JSON" but
does not say whether that outcome is persisted as an event on the
parent. If it is persisted, we need to define the event type. If it
is not, the scheduler is write-silent at the parent level, which is
consistent with the "derive from disk" strategy but means the
parent's event log has no record of scheduler ticks. That's fine,
just underspecified. Prefer silent.

### Cloud sync compatibility

Honored. `init_state_file` on `CloudBackend` is one `sync_push_state`
per spawn, which is actually a **reduction** in sync traffic (down
from three S3 PUTs to one per child init). Append-only events use
the existing append path unchanged. `template_source_dir` and
`submitter_cwd` are serialized as optional fields that old binaries
ignore cleanly via the custom `Event` deserializer at
`src/engine/types.rs:149-293` (which routes by `event_type` string
and uses helper structs without `deny_unknown_fields`).

One new cost class: `derive_batch_view` calls `backend.list()` +
per-child `read_events()` on every `koto status` invocation. On
cloud sync with a hot poll cadence and a 50+ task batch, that's
dozens of reads per poll. The design acknowledges this and proposes
a benchmark. Good — but it should also commit to an in-process
caching strategy if the benchmark shows the cost is unacceptable.
Don't leave the optimization as "future work"; call it out as a
Phase 3 test-driven decision.

### `deny_unknown_fields` on compiled cache

**Potential invariant violation, not currently covered.** Decision
3 adds `#[serde(deny_unknown_fields)]` to `SourceState` and
`TemplateState`. The design's pre-merge audit looks at *source*
template fixtures. But the cached compiled template JSON at whatever
path `load_compiled_template` returns — an abstraction the design
doesn't examine — is deserialized via the same type. If the
compiled cache on a user's machine contains a field the new binary
does not recognize, the load fails. There is no cache version bump
in the design.

Two fixes, pick one:

1. Bump the `template_hash` computation or cache key in Phase 2 so
   old caches are silently invalidated. Needs one line of code and
   one sentence of release note.
2. Scope `deny_unknown_fields` to `SourceState` only (compile-time
   parsing) and *not* `TemplateState` (which round-trips through
   cached JSON). The design's stated goal is catching source-level
   authoring mistakes, not defending the cache; scoping narrowly
   achieves the goal without the cache risk.

Option 2 is probably safer. The design as-written does not specify
which variant of `TemplateState` is being locked down, and the
implementer should not have to decide.

## Verdict

**Needs minor edits.** Specifically:

1. **Clarify the scheduler-tick vs advance-loop ordering for the
   first submission.** State explicitly that the first `koto next`
   tick spawns children and returns `GateBlocked`, and that the
   caller must re-invoke `koto next` to observe progress — or
   change the implementation to re-advance after spawning. Either
   choice is fine; pick one on paper.

2. **Split Phase 2 into 2a (compat plumbing) and 2b (template
   surface).** The design already admits this in Mitigations; just
   commit to it.

3. **Add `repair_half_initialized_children` and
   `internal_rewind_to_initial` to Phase 3 deliverables explicitly**,
   and state whether each is a new function or an extraction from
   existing code (`handle_rewind` does *not* today support
   rewind-to-initial; this is new machinery).

4. **Address compiled-template cache compatibility with
   `deny_unknown_fields`.** Either invalidate the cache on upgrade
   or scope the attribute to `SourceState` only. Not optional — as
   written, this can break first-tick loads on upgrade.

5. **Specify `BatchError` variants and how they map to `NextError`.**
   Short section; one paragraph in Key Interfaces.

6. **Enforce the 1000-task / 10-waits-per-task limits as hard errors
   at submission time**, not as soft recommendations in the security
   section. Easier to loosen later than to tighten.

7. **Name the child-spawning helper explicitly** (something like
   `init_child_from_parent`) and note that it has to re-run
   `resolve_variables` against the *child* template's variable
   declarations, not the parent's. This is the subtle part the
   design currently glosses.

8. **Move `src/engine/batch.rs` to `src/cli/batch.rs` or
   `src/cli/scheduler.rs`**. The new module depends on
   `SessionBackend` and knows about CLI response shapes; it does
   not belong next to the I/O-free advance loop. Nit but one worth
   fixing before the module name is cemented.

None of these blocks the feature. The six decisions compose
cleanly, the crash-recovery reasoning is sound, and the backward-
compat plan for events/headers is correct. The edits above are
clarifications, not rework.
