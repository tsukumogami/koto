# Decision 9: Retry Path End-to-End

**Prefix:** `design_batch-child-spawning_decision_9`
**Complexity:** critical
**Mode:** --auto (confirmed=false, assumptions recorded)
**Scope:** koto (public, tactical)
**Round:** 1 follow-up (revises D5.2, D5.3, D5.4)

<!-- decision:start id="batch-child-spawning-retry-path" status="assumed" -->

### Decision: Retry Path End-to-End

## Context

The walkthrough round-1 pair simulations surfaced four blocker-class gaps
in the retry story (Cluster A and Cluster B of `round1_synthesis.md`):

1. **Reachability.** The reference `coord.md` routes
   `plan_and_await -> summarize` on `gates.done.all_complete: true`. Under
   D5.3 semantics, `all_complete == (pending == 0 AND blocked == 0)`, which
   is TRUE even for a batch where every child failed or was skipped. So the
   canonical template takes a failed batch straight to terminal, and the
   agent never sees a window in which to submit `retry_failed`.
2. **Mechanism self-contradiction.** D5.4 step 1 says
   `handle_retry_failed` transitions the parent directly (from
   `analyze_results` back to `awaiting_children`). The Data Flow "Retry"
   walkthrough says the *advance loop* routes via a template-declared
   transition. Both cannot be true.
3. **Discovery.** `retry_failed` is reserved and prohibited from `accepts`,
   so `expects.fields` never surfaces it. Agents without the `koto-user`
   skill cannot discover the retry action from the response alone.
4. **Unsafe edges.** Double `retry_failed` without intervening tick, retry
   on a running child, retry with successful children in the set, closure
   direction, mixed-payload handling, and stale skip markers after partial
   retry are all unspecified.
5. **Synthetic template fragility.** D5.2's synthetic skipped-child state
   file is unreachable from a real-template running child (it has the
   wrong template_hash) and persists stale skip markers when a partial
   retry succeeds. Two round-1 blockers flow directly from this.

This decision supersedes D5.2's synthetic-template mechanism, replaces
D5.3's `all_complete`-only gate vocabulary, re-authors D5.4's retry
sequence into one internally-consistent story, and rewrites the reference
`coord.md` to be retry-reachable out of the box.

## Assumptions

- **A1.** Agents will reliably consume a new top-level response field
  (`reserved_actions`) when it is present on gate-blocked or terminal
  responses whose gate output reports `failed > 0` or `skipped > 0`.
- **A2.** Template authors will accept a compile-time warning (W4) that
  fires when a `materialize_children` state routes only on
  `all_complete: true` without guarding against `failed > 0`.
- **A3.** Deleting-and-respawning a child on runtime reclassification is
  safe even if the child is in a non-terminal state, because the
  classification step re-derives from disk on every tick and only triggers
  when dependency outcomes flip. When a flip happens, the existing child
  either (a) is a synthetic skip marker (cheap, throwaway) or (b) is a
  real-template child whose upstream just became failed (legitimately
  needs to become skipped). Case (b) is the "running child loses upstream"
  corner; its work was going to be invalidated anyway.
- **A4.** `reserved_actions` as a response-shape extension does not
  conflict with D7 (directive + details) or D8 (default_template +
  item_schema). It is a new sibling field on the response envelope.
- **A5.** A template-declared transition on `evidence.retry_failed:
  present` can be expressed in the current when-clause engine (equality
  against the literal `true`/`present` sentinel, or a new `present`
  matcher). If the matcher is missing, D11 adds it; this decision assumes
  it is available and flags the dependency.

## Chosen: Template-routed retry, extended gate vocabulary, discovery via `reserved_actions`, runtime reclassification

A five-part package answering the five sub-questions coherently. Each
part is load-bearing for the others.

### Part 1 — Reachability: extend gate vocabulary, rewrite reference template

**Gate output (supersedes D5.3 exposed surface):** alongside
`all_complete`, expose three derived boolean guards that templates can
route on without needing comparison operators:

| Field | Definition |
|-------|------------|
| `all_complete` | `pending == 0 AND blocked == 0` (unchanged from D5.3) |
| `all_success` | `all_complete AND failed == 0 AND skipped == 0` |
| `any_failed` | `failed > 0` |
| `any_skipped` | `skipped > 0` |
| `needs_attention` | `all_complete AND (failed > 0 OR skipped > 0)` |

`failed`, `skipped`, `success`, `blocked`, `pending` integer aggregates
remain as specified in D5.3. The new booleans are pre-computed so
equality-based when-clauses can route on them.

**Reference template (supersedes walkthrough.md's `coord.md`):**

```yaml
states:
  plan_and_await:
    transitions:
      - target: summarize
        when:
          gates.done.all_success: true
      - target: analyze_failures
        when:
          gates.done.needs_attention: true
    gates:
      done:
        type: children-complete
    accepts:
      tasks:
        type: tasks
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents
      default_template: impl-issue.md

  analyze_failures:
    transitions:
      - target: plan_and_await
        when:
          evidence.retry_failed: present
      - target: summarize
        when:
          evidence.acknowledge_failures: true
    accepts:
      acknowledge_failures:
        type: boolean
        required: false
    # retry_failed is NOT declared in accepts (reserved); it surfaces via
    # response.reserved_actions instead.

  summarize:
    terminal: true
```

The template routes the failed-batch case to `analyze_failures`, where
the agent can either submit `retry_failed` (reserved) to go back to
`plan_and_await`, or submit `acknowledge_failures: true` to terminate.

**Compile warning W4:** a state with `materialize_children` whose
transitions route on `all_complete: true` (or no `failure`/`skip`-aware
guard) without a second transition handling `failed > 0` or
`skipped > 0` emits: `"state {name}: materialize_children transitions do
not handle the failure/skip case; failed or skipped children will route
to the success branch silently. Add a transition on gates.<gate>.any_failed
or gates.<gate>.needs_attention."`

### Part 2 — Mechanism: template-declared transition, `handle_retry_failed` never transitions directly

**Delete D5.4 step 1's "transitions the parent directly" claim.** The
authoritative retry story is:

1. Agent submits `{"retry_failed": {...}}` while the parent is in the
   analysis state (`analyze_failures` in the reference template).
2. `handle_next` intercepts the submission at the CLI layer, BEFORE the
   advance loop (because `retry_failed` is reserved and would otherwise
   fail `deny_unknown_fields` in evidence validation).
3. `handle_retry_failed` does three things, in this order:
   - Validates the retry set (see Part 4 edges).
   - Writes `EvidenceSubmitted {retry_failed: <payload>}` to the parent
     log.
   - Computes the retry closure and writes `Rewound` events to each
     child in the closure whose current outcome is `failure` or
     `skipped`. Synthetic skip markers in the closure are
     **deleted-and-respawned** instead of rewound (see Part 5).
   - Writes `EvidenceSubmitted {retry_failed: null}` clearing event to
     the parent.
4. `handle_retry_failed` returns control to the advance loop. The
   advance loop reads the parent's current state (`analyze_failures`),
   finds the template transition `when: evidence.retry_failed: present`,
   and fires it to route the parent back to `plan_and_await`.
5. On the next tick (or within the same advance loop iteration, if the
   advance loop re-evaluates after a transition), the scheduler runs in
   `plan_and_await` and re-spawns/un-skips children as needed.

**Why the clearing event comes before the transition:** by clearing the
field first, the template transition's `when: evidence.retry_failed:
present` reads the original submission's value (not the null), because
evidence within an epoch is last-write-wins and the advance loop's
when-clause evaluator reads merged evidence at the moment the transition
is evaluated. If this interacts badly with when-clause semantics, swap
steps 3d and 4: transition first, then clear. Both orders are
recoverable, but transition-first is slightly cleaner because the
`retry_failed: present` guard still sees the payload.

**Canonical order (committed):**

```
a. validate retry set
b. append EvidenceSubmitted { retry_failed: <payload> } to parent
c. append Rewound to each failed/skipped child in closure (delete+respawn synthetic skips)
d. run advance loop; it reads retry_failed and fires template transition
e. append EvidenceSubmitted { retry_failed: null } to parent (clears for next epoch)
```

The advance loop does the transition work. `handle_retry_failed` only
stages child-side side effects and writes evidence; it never appends
`Transitioned` to the parent itself.

### Part 3 — Discovery: synthetic `reserved_actions` block on the response

**When present:** any response where the current state's gate output
reports `any_failed: true` or `any_skipped: true`, regardless of
`action` value (`gate_blocked`, `evidence_required`, `done`). The field
is synthesized by `handle_next` after gate evaluation.

**Shape:**

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "...",
  "expects": { "fields": { "acknowledge_failures": {...} } },
  "blocking_conditions": [...],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {
          "type": "array<string>",
          "required": true,
          "description": "Child workflow names to retry. Must be in outcome=failure or outcome=skipped."
        },
        "include_skipped": {
          "type": "boolean",
          "required": false,
          "default": true,
          "description": "When true, retry cascades to skipped dependents of the named children."
        }
      },
      "applies_to": ["coord.issue-B"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.issue-B\"]}}'"
    }
  ]
}
```

`applies_to` enumerates the currently-retryable children (those with
`outcome == failure` or `outcome == skipped`), so agents do not have to
scan `children[*].outcome` themselves. The `invocation` string is a
ready-to-run example.

**Why a top-level field, not stuffed into `expects`:** `expects.fields`
declares what the NEXT advance will accept as regular evidence. Reserved
actions are orthogonal — they bypass the advance loop. Conflating them
would confuse agents that already scan `expects.fields` to decide what
to submit.

**Terminal responses:** if the parent reaches a terminal state with
`any_failed` or `any_skipped` still true on its last batch view (see
D13 coordination for the batch-final-view preservation story), the
`done` response also carries `reserved_actions` — even though retry from
a terminal state requires a template that handles it. D13 will decide
whether terminal-state retry is admissible. For now: emit the block;
downstream decides whether it is actionable.

### Part 4 — Edges: explicit behavior for every corner

| Edge | Behavior |
|------|----------|
| **Double `retry_failed` without intervening tick** | Second submission is rejected with `BatchError::InvalidRetryRequest { reason: "retry_already_in_progress" }` because the first submission's `EvidenceSubmitted {retry_failed: <payload>}` is still the latest-epoch value in the parent's merged evidence. The clearing event from step (e) has not been written yet on a mid-tick resubmit; if the user successfully got two `koto next` calls in, the second one sees `retry_failed: null` and rejects with `no_retry_in_progress`. Either way, exactly one retry closure per submission. |
| **Retry on a running child** | `children: [X]` where X is in outcome `pending` (running): rejected with `InvalidRetryRequest { reason: "child_not_retryable", child: "X", outcome: "pending" }`. No rewinds are written. The submission is atomic: either all named children are retryable or the request is rejected. |
| **Retry with a successful child** | `children: [X]` where X is in outcome `success`: rejected with `InvalidRetryRequest { reason: "child_not_retryable", child: "X", outcome: "success" }`. Same atomicity as above. |
| **Mixed retry set (some retryable, some not)** | All-or-nothing: any non-retryable child in the set rejects the whole submission. This preserves the "atomic state transition" invariant. |
| **Closure direction** | **Downward only** (to dependents of the retry set) with `include_skipped: true` as the default. Upstream dependencies are NOT included automatically because a retry is user-directed: the user names the failure, and the closure extends to what got blocked by that failure. If the user wants to retry an ancestor, they name the ancestor. |
| **Stale skip markers (partial retry)** | Solved by Part 5 runtime reclassification: on every scheduler tick, synthetic skip markers are re-evaluated against current dependency outcomes. If a skipped child's blocker is no longer failed (because the blocker was retried and succeeded), the skip marker is deleted, and the child is either re-skipped (if a different dep failed) or spawned with the real template. |
| **Mixed payload (`retry_failed` + other evidence)** | Rejected with `InvalidRetryRequest { reason: "mixed_payload", extra_fields: [...] }`. `retry_failed` submissions must be payload-exclusive. Agents wanting to record a decision alongside retry submit them as two separate `koto next` calls. Rationale: interception happens before advance-loop evidence validation, so mixing could silently swallow a required field. |
| **Retry of a retry (epoch N+1 failure)** | Treated identically to the original retry. Each retry opens a new child epoch via `Rewound`. The parent accumulates one `EvidenceSubmitted {retry_failed: <payload>}` + one `EvidenceSubmitted {retry_failed: null}` pair per retry cycle in its event log. |
| **Crash between Rewound writes and clearing event** | On resume, `retry_failed` is still present in merged evidence. `handle_retry_failed` re-runs the classification and only rewinds children still in `failure`/`skipped` outcome — idempotent. Writes the clearing event. Converges. |
| **Concurrent `retry_failed` from two callers** | Coordinator-driven assumption (D12 hardens this). Here: the second caller sees `retry_failed` already in evidence (from caller 1), rejects with `retry_already_in_progress`. If the first caller's clearing event has already landed, caller 2 sees a clean slate and runs normally. No split-brain possible with serialized parent ticks. |

**Validation rule R10** (new): `retry_failed` payload must satisfy —

- `children`: non-empty array of strings.
- Each name must exist in the batch's declared task set.
- Each named child must be spawned (have a state file on disk) with
  outcome `failure` or `skipped`.
- `include_skipped`: optional boolean.
- No other top-level fields in the retry_failed object.

Violations produce `InvalidRetryRequest { reason: <specific>, details }`.

### Part 5 — Synthetic template fate: replace with runtime reclassification

**Delete D5.2's synthetic-per-skipped-child state file mechanism.**
Replace with:

- **Skip markers are ephemeral disk records.** When the scheduler decides
  a task must be skipped (dep failed, not yet spawned), it writes a
  minimal state file using the child's *real* template, immediately
  transitioning into a `skipped` terminal state (`skipped_marker: true`
  required on the child template per D5.2 — this part survives).
- **Every scheduler tick re-evaluates skip markers.** For each existing
  skip marker, the scheduler checks: does any `waits_on` dep still have
  outcome `failure`? If yes, leave as-is. If no (all deps are `success`
  or in progress), the skip marker is stale.
- **Stale skip markers are deleted-and-respawned.** The scheduler deletes
  the child's state file (and any context keys keyed to the child), then
  re-runs its spawn logic on the task. If deps are now all success, the
  task spawns fresh with a new `WorkflowInitialized` event. If a
  different dep is still failed (shouldn't happen given the prior check,
  but possible if deps flip between ticks), re-skip.

**Why this works under D2 (atomic init):** atomic init is the guarantee
that state file creation is one rename-or-nothing. Delete-and-respawn is
two operations (delete, then atomic-init), but the delete is
idempotent-on-failure: if the delete succeeds and the respawn fails, the
next tick observes "no state file, deps are success, needs spawn" and
retries. Append-only invariant: the deleted state file contained only a
synthetic skip transition which was never user-visible progress.

**Why the "running child loses upstream" blocker disappears:** in the
old synthetic-template model, if D was running (real template) and its
upstream B re-failed after a retry, D had no legal transition to the
synthetic `skipped_marker` state — D was on a different template. Under
runtime reclassification, D's state is re-evaluated each tick: if its
dep is failed, the scheduler deletes D's in-progress state file and
re-spawns it as a skip marker with D's real template (which must
declare `skipped_marker: true` per D5.2's retained portion). D's
in-progress work is invalidated, which is the correct outcome — its dep
failed, so its work cannot be trusted.

**Why the "stale skip after partial retry" blocker disappears:** if
B failed, D was skipped (marker), and the user retries just B: on the
next scheduler tick after B's retry succeeds, D's skip marker is
evaluated. D's only failed dep is B, and B is now `success`. The skip
marker is stale, gets deleted-and-respawned, and D spawns fresh as a
real running child. No orphan skip markers.

**Compile rule F5** (new): child templates participating in a batch
must declare a `skipped_marker: true` terminal state AND that state's
initial transition (from the template's `initial_state`) must be
declarable as a scheduler-synthesized auto-transition. In practice: the
template must accept `status: "skipped"` as evidence or include the
skip state in the initial_state's transitions. The compiler validates
that `initial_state -> skipped_marker_state` is reachable via a
scheduler-writable transition.

**Observability note:** the skip marker's state file is a real state
file on the child's real template. `koto status coord.issue-D` returns
`state: skipped`, `is_terminal: true`, and reads the `skipped_because`
context key. No "synthetic template" exists as a separate artifact.
Coordinates with D13: the `kind: "skip_marker"` discriminator lives on
the response, not on a hidden template.

## Rationale

**Tied to blockers (traceability to round-1 findings):**

| Round-1 blocker | Resolution |
|-----------------|------------|
| 1a (reference template routes failures to terminal) | Part 1: `analyze_failures` intermediate state + `any_failed`/`needs_attention` guards + W4 compile warning |
| 1c F1 (no compile warning) | Part 1: warning W4 |
| 1c F2/F3 (batch data lost on terminal) | Part 3: `reserved_actions` carried into terminal responses; D13 carries batch_final_view |
| 1a #2 / Cluster B (discovery) | Part 3: synthetic `reserved_actions` block, independent of `expects.fields` |
| 1b blocker 1 (mechanism contradiction) | Part 2: one authoritative story — template-declared transition, `handle_retry_failed` writes child side effects and evidence, never appends `Transitioned` to parent |
| 1b blocker 2 (running child loses upstream) | Part 5: runtime reclassification deletes+respawns any child whose dep flips to failure, even real-template running children |
| 1b should-fix (double retry) | Part 4: `retry_already_in_progress` error on second submission |
| 1b should-fix (retry on running) | Part 4: rejected with `child_not_retryable` |
| 1b should-fix (retry on successful) | Part 4: rejected with `child_not_retryable` |
| 1b should-fix (closure direction) | Part 4: downward-only, `include_skipped: true` default |
| 1b should-fix (mixed payload) | Part 4: rejected with `mixed_payload` |
| 1a #5 (stale skip markers) | Part 5: runtime reclassification deletes stale markers |
| 1c F12 (synthetic template undiscoverable) | Part 5: no synthetic template exists; skip markers use real templates |

**Why the package is internally consistent:**

1. **One discovery surface (Part 3) means agents find retry without the
   skill.** Without this, Part 2's template-declared transition requires
   the agent to know about `retry_failed` from out-of-band docs —
   brittle in the field.
2. **One mechanism (Part 2) means every retry follows the same path.**
   Without this, agents observing event logs would see two distinct
   transition patterns (direct vs advance-loop) and have no way to
   reason about ordering or crash recovery.
3. **Runtime reclassification (Part 5) unifies skip-marker lifecycle
   with real-child lifecycle.** Every tick, every child gets evaluated.
   No separate "synthetic" state model to reason about.
4. **Extended gate vocabulary (Part 1) composes with runtime
   reclassification.** The new `any_failed`/`needs_attention` guards
   can route on the reclassification's output without new evaluator
   logic — the gate still reads terminal outcomes, runtime
   reclassification just changes which children are in which terminal
   state.
5. **Explicit edges (Part 4) close every round-1 GAP marker.** The
   `InvalidRetryRequest` variant is a single error family with
   sub-kinds, matching D11's requirement for typed error details.

**Why reject the direct-transition model (D5.4 step 1):**

- It hides the retry in implementation code. Template authors cannot
  read the template and know how retry works.
- It conflates CLI-layer evidence handling with engine-layer state
  advancement, violating the separation that makes advance.rs testable
  in isolation.
- It forces `handle_retry_failed` to know about parent template
  structure to decide where to transition. Template-declared transitions
  let the template own its own routing.

**Why reject keeping the synthetic-per-skipped-child template:**

- Round-1 pair 1b blocker 2 is unrepairable under that model: a real-
  template running child has no legal transition to a different
  template's `skipped_marker` state.
- Round-1 pair 1a #5 (stale skip markers) has no clean fix without
  either runtime reclassification or an explicit "refresh" primitive
  that costs a new event type.
- Runtime reclassification is derivable-from-disk (honors storage
  strategy c); synthetic templates require a second discovery path
  (what template does coord.issue-D claim in its header vs what
  scheduler decided).

## Alternatives Considered

### Sub-question 1 (Reachability)

- **A1a. Template author adds manual failure branches in every
  template.** Status quo. Rejected: round-1 Cluster A shows authors
  consistently miss this. W4 warning is necessary.
- **A1b. Change `all_complete` semantics to `pending == 0 AND blocked
  == 0 AND failed == 0`.** Rejected: breaks back-compat for non-batch
  consumers and conflates "nothing to do" with "everything succeeded",
  losing a useful discriminator.
- **A1c. Chosen: extended gate vocabulary (`all_success`, `any_failed`,
  `needs_attention`) + W4 compile warning + reference template pattern
  with `analyze_failures` state.** Preserves `all_complete`, adds
  composable guards, catches author mistakes at compile time.

### Sub-question 2 (Mechanism)

- **A2a. Direct parent transition from `handle_retry_failed` (D5.4 step
  1 as-written).** Rejected: round-1 pair 1b blocker 1; hides retry in
  implementation; violates template-owns-routing invariant.
- **A2b. Advance-loop-only, no CLI interception.** Would require
  `retry_failed` to flow through `accepts` and break the "reserved
  evidence" rule that prevents collision with user field names. Would
  also leak retry-specific validation into the evidence validator.
  Rejected.
- **A2c. Chosen: CLI interception for child-side side effects + template
  transition for parent routing.** `handle_retry_failed` stages rewinds
  and writes evidence; the advance loop fires the template transition.
  Two layers, one story.

### Sub-question 3 (Discovery)

- **A3a. Nothing — agents learn via koto-user skill.** Rejected:
  violates the explicit constraint that retry be discoverable without
  the skill.
- **A3b. Add a synthetic `retry_failed` entry to `expects.fields` on
  failed-batch responses.** Rejected: `expects.fields` is validated by
  the evidence validator, which would then need to special-case the
  reserved field — reopens D3 (`deny_unknown_fields`).
- **A3c. Embed retry hint in `directive` text.** Rejected: directives
  are template-author-controlled; koto cannot inject text without
  surprising the author. Also not machine-readable.
- **A3d. Chosen: top-level `reserved_actions` array, synthesized by
  koto.** Orthogonal to `expects.fields`, orthogonal to `directive`,
  machine-readable, and includes the ready-to-run invocation string.

### Sub-question 4 (Edges)

- **A4a. Lenient: silently ignore non-retryable children in the retry
  set, retry what you can.** Rejected: round-1 pair 1b showed this
  leads to partial-success ambiguity and log noise (accepted retries
  that did nothing). Atomicity is clearer.
- **A4b. Auto-extend closure upward to the nearest failed ancestor.**
  Rejected: `retry_failed: [D]` silently retrying B changes the
  semantic of a named retry. Principle: the user names what they mean.
- **A4c. Chosen: strict validation, downward-only closure with
  `include_skipped: true` default, atomic all-or-nothing submissions.**

### Sub-question 5 (Synthetic template)

- **A5a. Keep D5.2 synthetic template with explicit rules closing edge
  cases.** Requires specifying (a) how a running real-template child
  transitions to a different template's skipped state (impossible
  without cross-template transition support — new engine feature) and
  (b) a stale-marker cleanup primitive. Rejected: cost exceeds
  reclassification.
- **A5b. Hybrid: synthetic template for initial skip, delete-and-respawn
  only on retry.** Rejected: two code paths for the same conceptual
  operation. Surface area doubles.
- **A5c. Chosen: runtime reclassification on every tick.** Skip markers
  use the child's real template. Staleness is self-healing: each tick
  re-derives from dependency outcomes. The running-child-loses-upstream
  case is handled uniformly: delete the in-progress file, respawn as a
  skip marker on the real template.

## Consequences

### What becomes easier

- **Reference template is retry-reachable by default.** Authors who
  copy `coord.md` get a working retry path without reading any skill.
- **One retry story.** Implementers, docs, and agents all see the same
  sequence: submit retry_failed, parent routes via template, child
  rewinds happen, parent cleared. No "which step actually moves the
  parent" ambiguity.
- **Discovery without skills.** Agents that have never seen koto-user
  can submit retry by reading `reserved_actions[0].invocation` verbatim.
- **No synthetic-template dual model.** `koto status`, `koto query`,
  `koto workflows --children` all work uniformly on skip markers.
  Skip markers look like skipped children on their real templates.
- **Stale markers are impossible by construction.** Each tick reconciles.

### What becomes harder

- **Child templates must declare scheduler-writable transitions to
  their skipped state.** Compile rule F5 enforces this. Authors of
  existing templates opting into batch participation need a one-line
  change. D5.2's `skipped_marker: true` field survives; this adds a
  reachability requirement.
- **Runtime reclassification adds per-tick work.** Each scheduler tick
  walks all existing children and re-checks dep outcomes for skip
  markers. For large batches (100+ tasks), this is O(N) per tick.
  Acceptable given typical batch sizes; document the scaling note.
- **Delete-and-respawn of real-template running children** invalidates
  their work. This is correct (dep failed, downstream can't trust the
  result), but may surprise agents. The `skipped_because` context key
  records the reason; add a note to koto-user that running children can
  flip to skipped if their upstream fails after the fact.
- **`reserved_actions` is a new response envelope field.** Consumers
  parsing the envelope must tolerate new top-level keys. This is
  expected behavior under JSON contracts, but doc it.
- **`handle_retry_failed` grows validation logic** (R10) and writes
  five things per call (evidence + rewinds + clearing event + advance
  loop-triggering). Crash recovery at each step is idempotent by
  design, but testing needs to cover the partial-completion cases.

### Implementation touch points

- `src/cli/next_types.rs`: add `reserved_actions: Option<Vec<ReservedAction>>`
  to the response envelope. Add `ReservedAction` struct.
- `src/cli/mod.rs handle_next`: after gate evaluation, synthesize
  `reserved_actions` if gate output reports `any_failed` or `any_skipped`.
- `src/cli/batch.rs` (or new `src/cli/retry.rs`): `handle_retry_failed`
  with R10 validation, child-closure computation, Rewound writes, evidence
  append + clearing event append. Never appends `Transitioned` to parent.
- `src/gate/children_complete.rs`: add `all_success`, `any_failed`,
  `any_skipped`, `needs_attention` to `output` JSON.
- `src/template/compile.rs`: add W4 warning (materialize_children state
  without failure/skip-aware transitions) and F5 rule (batch-eligible
  child template has reachable skipped_marker state).
- `src/engine/batch.rs` scheduler tick: runtime reclassification sweep
  before spawn pass. For each existing child with skipped_marker=true in
  current state: re-evaluate deps; if stale, delete state file + any
  context keys, return to task queue for respawn.
- `src/engine/batch.rs`: for real-template children whose dep outcome
  just flipped to `failure`, delete state file + respawn as skip
  marker (only if the child is not already terminal with a different
  outcome). Guard against thrashing: only reclassify on dep-outcome
  change, not on every tick unconditionally.
- `wip/walkthrough/walkthrough.md`: rewrite `coord.md` with
  `analyze_failures` state.
- `plugins/koto-skills/skills/koto-author/` + `koto-user/`: update both
  skills per CLAUDE.md section.

### Coordination with parallel decisions

- **D10 (mutation semantics):** D10 must decide whether
  `retry_failed` + other resubmission fields are permitted in the same
  payload. This decision says no (Part 4 mixed-payload edge); D10
  should align.
- **D11 (error envelope):** the `InvalidRetryRequest` variant and R10
  sub-kinds feed into D11's typed-error work. D11 decides the JSON
  shape; this decision names the sub-kinds to include.
- **D12 (concurrency):** serialized parent-tick assumption underpins
  the "double retry" edge. D12's advisory-lockfile mechanism (if
  adopted) is what makes this edge deterministic rather than racy.
- **D13 (post-completion observability):** this decision says
  `reserved_actions` is emitted on terminal responses too. D13 decides
  whether terminal-state retry is admissible and what `batch_final_view`
  looks like. Hooks left at `reserved_actions` synthesis (Part 3).

<!-- decision:end -->

---

## YAML Summary

```yaml
decision_result:
  status: COMPLETE
  chosen: >
    Retry is an end-to-end five-part mechanism: (1) the gate output
    exposes all_success, any_failed, any_skipped, needs_attention
    alongside all_complete, and the reference coord.md routes failed
    batches to an analyze_failures state; compile warning W4 catches
    templates that swallow failures. (2) handle_retry_failed intercepts
    at the CLI layer, writes child rewinds and evidence, and a
    template-declared transition (evidence.retry_failed: present) moves
    the parent; handle_retry_failed never appends Transitioned to the
    parent directly. (3) a top-level reserved_actions block on
    responses where any_failed or any_skipped is true surfaces the
    retry action with schema and a ready-to-run invocation string. (4)
    edges are handled strictly: atomic all-or-nothing submissions,
    rejection of retries on running/successful children, rejection of
    double retry and mixed payloads, downward-only closure with
    include_skipped: true default. (5) synthetic per-skipped-child
    templates are replaced by runtime reclassification — every
    scheduler tick re-evaluates skip markers against dependency
    outcomes, deleting-and-respawning stale markers and any
    real-template running child whose upstream flips to failure.
  confidence: high
  rationale: >
    Round-1 pair simulations surfaced every corner of the retry path
    and supplied concrete GAP markers. The chosen package resolves all
    four blockers and all six should-fix gaps in Clusters A and B
    without introducing new mechanisms beyond what D5.2, D5.3, and D5.4
    already carried. The main replacements — template-routed transition
    (supersedes D5.4 step 1), runtime reclassification (supersedes
    D5.2 synthetic templates), extended gate vocabulary (extends D5.3),
    and the new reserved_actions response field — are each minimal and
    compose orthogonally. The package is internally consistent: one
    discovery surface, one mechanism, one lifecycle for skip markers.
  assumptions:
    - Agents reliably consume a new top-level reserved_actions response field
    - Template authors tolerate compile warning W4 on materialize_children states
    - Delete-and-respawn is safe at runtime (work invalidation is correct outcome)
    - reserved_actions does not conflict with D7/D8
    - When-clause engine supports evidence.<field>: present matcher (or D11 adds it)
  rejected:
    - name: Direct parent transition in handle_retry_failed
      reason: Hides retry in implementation, violates template-owns-routing invariant
    - name: Synthetic `retry_failed` entry in expects.fields
      reason: Reopens D3 deny_unknown_fields, conflates reserved actions with evidence
    - name: Keep synthetic template per skipped child
      reason: Round-1 blocker 2 unrepairable without cross-template transitions; stale markers require new primitive
    - name: Auto-extend retry closure upward
      reason: Changes semantic of named retry; principle is user names what they mean
    - name: Lenient retry edge handling
      reason: Partial-success ambiguity, accepted-but-inert retries clutter audit log
    - name: Change all_complete semantics to exclude failures
      reason: Breaks back-compat, loses useful discriminator
  report_file: wip/design_batch-child-spawning_decision_9_report.md
```
