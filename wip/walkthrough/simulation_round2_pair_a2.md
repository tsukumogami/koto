# Simulation Round 2, Pair A2: Retry Edge Atomic-Rejection Validation

**Shape:** A -> B, A -> C, B -> D, C -> D (diamond). Same topology as
round-1 pair 1b.

**Round-2 purpose:** verify that CD9's atomic-rejection commitment and
CD11's typed error envelope actually close the 8 retry edges round 1
surfaced. Round 1 showed underspecified or permissive behavior on every
edge. CD9 + CD11 now claim: each edge rejects pre-append with a typed
`InvalidRetryReason` discriminator, no state mutation survives the
rejected call, and the envelope is machine-parseable.

The AGENT role has no design-doc access; KOTO role synthesizes JSON
against the revised design. Findings call out whether each edge
verifies the round-1 claim or exposes residual ambiguity.

**Reached state before Section 1 begins** (elided drive):
- `coord.issue-A`: `done`, outcome `success`.
- `coord.issue-B`: `done_blocked`, outcome `failure`,
  `failure_reason: "dependency compile error"`.
- `coord.issue-C`: `done`, outcome `success`.
- `coord.issue-D`: `skipped_due_to_dep_failure`, outcome `skipped`,
  `skipped_because: "coord.issue-B"`,
  `skipped_because_chain: ["coord.issue-B"]`, `skipped_marker: true`.
- Parent `coord`: at `analyze_failures`. Gate output carries
  `any_failed: true, any_skipped: true, needs_attention: true`.
  Response carries the `reserved_actions` block (CD9 Part 3) with
  `applies_to: ["coord.issue-B", "coord.issue-D"]`.

---

## Section 1: Transcript

### Edge 1 -- Double `retry_failed` without an intervening tick

**AGENT** (first submission, intentional -- B is the root failure):

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}}'
```

**KOTO:** submission accepted. Canonical CD9 sequence runs: append
`EvidenceSubmitted { retry_failed: {...} }` to parent, append
`EvidenceSubmitted { retry_failed: null }` clearing event, write
`Rewound` to `coord.issue-B`, delete-and-respawn `coord.issue-D`. The
advance loop fires the template transition on
`evidence.retry_failed: present` back to `plan_and_await`. Scheduler
tick reports both children as `already` (fresh epochs exist on disk).

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Drive the children. coord.issue-B and coord.issue-D are ready.",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 2,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.issue-A", "outcome": "success", "state": "done"},
      {"name": "coord.issue-B", "outcome": "pending", "state": "working"},
      {"name": "coord.issue-C", "outcome": "success", "state": "done"},
      {"name": "coord.issue-D", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C", "coord.issue-D"],
    "blocked": [], "skipped": [], "errored": [],
    "warnings": [], "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

**AGENT** (hits Ctrl-Up and resubmits the exact same payload *before*
ticking `coord.issue-B`):

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}}'
```

**KOTO reasoning:**
- CD9 Part 4: "Double retry (second submission before the first's tick)
  rejects with `InvalidRetryReason::RetryAlreadyInProgress`."
- But the first submission already ticked. The parent is now at
  `plan_and_await`; B is `working` (outcome `pending`); there is no
  in-progress retry closure.
- So this call is not actually a "double retry" under the CD9
  definition. It is "retry on a running child," which falls to Edge 2
  handling (`ChildNotEligible`).

For a *true* double-submit (i.e., two `koto next coord --with-data ...`
calls serialized before the first returns), CD12 Q3's advisory flock
serializes them. The loser sees `concurrent_tick` (mapped to
`integration_unavailable`, retryable). `RetryAlreadyInProgress` is
therefore reachable only in a narrow window: between the first
submission's parent log writes and the advance loop's clearing-event
merge within the *same* tick -- which, under CD9's canonical sequence,
cannot happen because both writes occur atomically before
`advance_until_stop` runs.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: coord.issue-B is currently running (outcome 'pending') and cannot be retried.",
    "details": [{"field": "retry_failed.children", "reason": "child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name": "coord.issue-B", "current_outcome": "pending"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

`[GAP -- see Finding 1]` The round-1 scenario that motivated
`RetryAlreadyInProgress` was "user fat-fingers Ctrl-Up." Under CD9's
atomic single-tick retry (append submit + append clear in the same
`handle_retry_failed` call), there is no durable "retry in progress"
window for a second external submission to observe. `ChildNotEligible`
subsumes the second-call case. `RetryAlreadyInProgress` appears
unreachable from outside a single `handle_next` call.

**Gloss:** The second call is rejected pre-append. Parent log has
exactly the two events from the first call. B is at `working` epoch 2
unchanged. Envelope matches CD11: `action: "error"`,
`error.batch.kind: "invalid_retry_request"`,
`error.batch.reason: "child_not_eligible"`, typed
`children: [{name, current_outcome}]`.

---

### Edge 2 -- Retry on a running (non-terminal) child

Reset: probe state as described in the preamble. Agent submits a valid
retry for B, drives B partway (B now at `working` with in-flight work).
Mid-flight the agent submits a retry for B again.

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}}'
```

**KOTO:**
- R10 loads B's state file. `classify_task` reports B at state
  `working`, outcome `pending`.
- CD9 Part 4: "Retries on running or successful children reject with
  `InvalidRetryReason::ChildNotEligible` listing each child's current
  outcome."
- Pre-append rejection. Zero writes to parent log, zero writes to B's
  log.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: coord.issue-B is currently running (outcome 'pending') and cannot be retried.",
    "details": [{"field": "retry_failed.children", "reason": "child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name": "coord.issue-B", "current_outcome": "pending"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** Exact same shape as Edge 1's rejection -- the discriminator
is `child_not_eligible`, not a separate "running" sub-kind. The typed
`current_outcome` field carries the detail. CD11's commitment to
machine-parseable envelope holds: agent pattern-matches
`error.batch.reason == "child_not_eligible"` and reads
`children[*].current_outcome` to branch on pending vs. success.

Verifies round-1 pair-1b finding 4 ("retry on a running child is not
guarded") -- yes. The permissive "silently restart" behavior of round 0
is gone.

---

### Edge 3 -- Retry with a successful child in the set

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-A"]}}'
```

**KOTO:**
- R10 loads A's state. `classify_task` reports A at `done`,
  outcome `success`.
- CD9 Part 4 atomic rejection: "Any non-retryable child in the set
  rejects the whole submission." A is success -> not eligible.

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: coord.issue-A is already successful and cannot be retried.",
    "details": [{"field": "retry_failed.children", "reason": "child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name": "coord.issue-A", "current_outcome": "success"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** The prompt's proposed label `NonRetryableChildren` does not
appear in the design. CD9's enum uses `ChildNotEligible { children:
Vec<ChildEligibility> }` (see `InvalidRetryReason` at design line
2944). Both "success" and "pending" cases share the same discriminator;
the typed `current_outcome` is the discriminator for recovery logic.

Verifies round-1 finding 5 ("successful child in retry set") -- yes,
atomic rejection with a named outcome. Round-0 silent-filter behavior
is gone.

---

### Edge 4 -- Closure direction: retry only D (skip marker), not B

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-D"]}}'
```

**KOTO:** R10 validates.
- `coord.issue-D` exists, outcome is `skipped`, eligible on paper.
- CD9 Part 5: closure is runtime reclassification. Closure *extends
  downward* from each named child to its skipped dependents; it does
  not extend upward to a skipped node's failed ancestors.
- So the retry is accepted on R10 grounds: D is eligible, closure is
  `{D}` (no downstream skips), payload is well-formed.

Canonical sequence runs:
1. Append `EvidenceSubmitted { retry_failed: {children:["coord.issue-D"]} }`.
2. Append `EvidenceSubmitted { retry_failed: null }` clearing event.
3. D is a skip marker -> delete-and-respawn D with the real template
   (`impl-issue.md`), fresh at `working` epoch 1 post-respawn.
4. Advance loop sees `evidence.retry_failed: present`, transitions
   `analyze_failures -> plan_and_await`.
5. Scheduler tick on `plan_and_await` runs `classify_task` on D. D's
   `waits_on` references B and C. B is still `failure` (unretried).
   CD9 Part 5 runtime reclassification: "Every tick, the scheduler
   re-evaluates every skip marker against current dependency outcomes.
   If no `waits_on` dep is still in `failure`, the marker is stale."
   D *is* a fresh real-template child this tick, not a skip marker
   (it was just respawned). But B is still in failure -- so the *other*
   reclassification rule fires: "Real-template running children whose
   upstream flips to failure are also delete-and-respawned, this time
   as skip markers."
6. D is deleted and respawned as a skip marker pointing at B.
7. Gate output: B failed, D skipped again.

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. ...",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "values": ["give_up", "acknowledge"], "required": false}
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 3, "pending": 0,
      "success": 2, "failed": 1, "skipped": 1, "blocked": 0, "spawn_failed": 0,
      "all_complete": true, "all_success": false,
      "any_failed": true, "any_skipped": true, "needs_attention": true,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked", "complete": true, "outcome": "failure", "reason": "dependency compile error", "reason_source": "failure_reason"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]}
      ]
    }
  }],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": false, "default": ["coord.issue-B", "coord.issue-D"]},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.issue-B", "coord.issue-D"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.issue-B\"]}}'"
    }
  ],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [...same as above...],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C"],
    "blocked": [],
    "skipped": ["coord.issue-D"],
    "errored": [],
    "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

**Gloss:** The submission is NOT rejected. It succeeds in the narrow
sense (R10 passes; events append; D is rewound), but CD9 Part 5's
runtime reclassification fires on the same tick, re-skipping D because
B is still failed. The agent observes "I spent a retry, D is still
skipped." The audit trail on D is: delete (skip marker) -> respawn
(real template) -> delete (real template) -> respawn (skip marker),
all in one parent tick.

This matches round-1 pair-1b finding 6 ("closure direction is
ambiguous"): the CD9 resolution is *downward only*, and the design
tolerates the thrash (agent wastes a retry) rather than rejecting the
retry. The alternative -- "auto-expand upward to include failed
ancestors" -- was explicitly rejected in CD9's Alternatives block:
"changes the semantic of a named retry. The user names what they mean."

`[GAP: CD9 does not reject a retry whose closure cannot make forward
progress. The thrash is visible to the agent only through the gate
output -- D is still skipped with `skipped_because: "coord.issue-B"`.
The `reserved_actions` block DOES correctly point at B as retryable,
so a well-written agent sees the hint. But a naive agent loops.
CD9 could add a new `InvalidRetryReason::ClosureCannotProgress {
names, blockers }` to reject upfront, and the current design chose not
to. This is a deliberate choice, but surface it in the round-2
findings.]`

---

### Edge 5 -- Mixed payload

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}, "tasks": [{"name": "issue-E", "waits_on": []}]}'
```

**KOTO:**
- R10 checks payload shape: top-level keys include `retry_failed` *and*
  `tasks`. CD9 Part 4 + CD11 Q11: "Mixed payloads (`retry_failed` +
  other evidence keys) reject with
  `InvalidRetryReason::MixedWithOtherEvidence`; `extra_fields` names
  the offending keys."

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: retry_failed cannot be combined with other evidence fields. Split into separate koto next calls.",
    "details": [{"field": "retry_failed", "reason": "mixed_with_other_evidence"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "mixed_with_other_evidence",
      "extra_fields": ["tasks"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** Pre-append rejection. `error.batch.extra_fields` is typed
`Vec<String>` and lists every non-retry_failed top-level key. The agent
can pattern-match on `error.batch.reason == "mixed_with_other_evidence"`
and read `extra_fields` to know exactly which sibling fields to split
out.

Verifies round-1 pair-1b finding 8 ("mixed payload unspecified") --
yes. Round-0's option-iii "both take effect" is off the table.

---

### Edge 6 -- Premature retry (no batch yet)

Scenario: fresh workflow. Parent `coord` at `plan_and_await`, no tasks
submitted, no children materialized.

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}}'
```

**KOTO:**
- R10: "each name exists in the declared task set; each named child
  exists on disk with outcome `failure` or `skipped`." The declared
  task set is empty (no `EvidenceSubmitted { tasks: [...] }` event on
  parent log yet); no children exist on disk.
- CD11 Q11 reserved `InvalidRetryReason::NoBatchMaterialized` for this
  case (the round-2 prompt labels it `NoBatchYet`; the design's chosen
  name is `NoBatchMaterialized` -- see design line 2945).

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: no batch has been materialized on this parent yet.",
    "details": [{"field": "retry_failed", "reason": "no_batch_materialized"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "no_batch_materialized"
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** Pre-append rejection. No children to enumerate, no
`ChildEligibility` payload, just the discriminator. Parent log is
unchanged (still only `WorkflowInitialized`).

Verifies CD11 Q11's `NoBatchMaterialized` reservation -- yes.

`[GAP -- see Finding 2]` The prompt used the name `NoBatchYet`. The
design file uses `NoBatchMaterialized`. Name drift between discussion
and spec; not semantically broken but worth flagging so the skills/
docs use the authoritative name.

---

### Edge 7 -- Empty retry set

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": []}}'
```

**KOTO:**
- R10: non-empty `children` array required.
- `InvalidRetryReason::EmptyChildList` (design line 2946).

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: children list is empty.",
    "details": [{"field": "retry_failed.children", "reason": "empty_child_list"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "empty_child_list"
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** Pre-append. No `children` field in the error body -- the
discriminator is the entire signal. Clean.

---

### Edge 8 -- Retry set includes non-existent child

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.ghost"]}}'
```

**KOTO:**
- R10: "each name exists in the declared task set; each named child
  exists on disk..."
- `coord.ghost` is neither in the declared task set (A, B, C, D) nor on
  disk.

The design's `InvalidRetryReason` enum does NOT have an
`UnknownChild` or `ChildDoesNotExist` variant. Options consistent with
the enum:

(a) Reuse `ChildNotEligible { children: [{name: "coord.ghost",
    current_outcome: "unknown"}] }`. Overloads the eligibility enum
    with a non-existence case. Consistent with "each child has an
    outcome" interpretation if "unknown" is the sentinel.

(b) Route through `InvalidBatchDefinition` (a different top-level
    `BatchError` variant) via a new `InvalidBatchReason` sub-kind like
    `UnknownChildReferenced`. But `InvalidBatchDefinition` is
    task-definition territory, not retry.

(c) Add a new `InvalidRetryReason::UnknownChild { children: Vec<String> }`
    variant.

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: coord.ghost is not a declared child of this parent.",
    "details": [{"field": "retry_failed.children", "reason": "child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name": "coord.ghost", "current_outcome": "unknown"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss:** This is KOTO's best-effort rendering under option (a).
Pre-append, typed, machine-parseable -- but conflates "ghost doesn't
exist" with "ghost is pending." The agent's recovery logic
(`if current_outcome == "pending": wait; else if == "unknown": check
spelling`) works, but the shared discriminator `child_not_eligible`
blurs two very different failure modes.

`[GAP -- see Finding 3]` The `InvalidRetryReason` enum in the design
has no variant for "child does not exist." `ChildNotEligible` plus a
sentinel `current_outcome: "unknown"` is a workaround, not a typed
contract. CD11's commitment is "typed enum discriminators throughout;
JSON wire shape uses snake_case serde renaming." Sentinels inside enum
payloads are the exact shape CD11 replaced elsewhere.

---

### Pre-append invariants check

For each rejection in Edges 1, 2, 3, 5, 6, 7, and 8, the parent log is
untouched. `koto query coord --events` after each rejected call
returns the same event list that existed before the call. Specifically:

- No `EvidenceSubmitted { retry_failed: ... }` event.
- No `EvidenceSubmitted { retry_failed: null }` clearing event.
- No `Rewound` event on any child.
- No `SchedulerRan` event (scheduler doesn't execute on rejection).
- No `BatchFinalized` event.

Edge 4 is the exception: submission IS accepted, events DO append,
the thrash happens downstream during the scheduler tick. Rejection
semantics do not apply because R10 passed.

This confirms CD11's "pre-append validation" commitment (design
lines 1977-1990): "Rejected submissions are ephemeral -- their
existence is observable only in the response of the tick that produced
them. Accepted-state audit lives in the log."

---

### Machine-parseability check (CD11)

Across all rejections in the transcript, the error envelope carries:

| Field | Type | Always present? |
|---|---|---|
| `action` | `"error"` literal | yes |
| `advanced` | `false` literal | yes |
| `error.code` | `"invalid_submission"` literal (for retry) | yes |
| `error.message` | human-readable | yes |
| `error.details[]` | `[{field, reason}]` | yes |
| `error.batch.kind` | `"invalid_retry_request"` literal | yes |
| `error.batch.reason` | `InvalidRetryReason` snake_case discriminator | yes |
| `error.batch.<payload>` | variant-specific typed fields | yes when variant has payload |
| `blocking_conditions` | `[]` | yes |
| `scheduler` | `null` | yes |

An agent pattern-match is:

```python
if resp["action"] == "error" and resp["error"]["batch"]["kind"] == "invalid_retry_request":
    match resp["error"]["batch"]["reason"]:
        case "no_batch_materialized": ...
        case "empty_child_list": ...
        case "child_not_eligible":
            for ch in resp["error"]["batch"]["children"]:
                # ch = {name, current_outcome}
                ...
        case "retry_already_in_progress": ...
        case "mixed_with_other_evidence":
            # resp["error"]["batch"]["extra_fields"] = [...]
            ...
```

No string-scraping of `error.message` is required. `details[*].reason`
duplicates `error.batch.reason` in a less-typed way, but is consistent
-- the two layers agree. CD11's commitment holds for the retry path.

---

### Does the response echo the submitted payload?

No. None of the rejection responses echo the agent-submitted
`retry_failed` payload. The `details[*].field` identifies the dotted
path ("retry_failed", "retry_failed.children") but the submitted values
are not returned. Agents with reproducibility concerns must re-read
their own stdin or shell history.

`[GAP -- see Finding 4]` Not a design violation (the design never
promised echo), but an ergonomic affordance worth considering for CLI
error output quality. CD11's envelope is extensible; adding an
optional `error.batch.submitted` field would be backward-compatible.

---

## Section 2: Findings

### Finding 1 — `RetryAlreadyInProgress` may be unreachable

- **Observation:** CD9 Part 4 reserves
  `InvalidRetryReason::RetryAlreadyInProgress` for "second submission
  before the first's tick." Under CD9's canonical sequence
  (append-submit + append-clear atomically inside one
  `handle_retry_failed` call, under CD12 Q3's per-parent flock), two
  external `koto next --with-data '{retry_failed: ...}'` calls
  serialize: either the first completes (parent now shows B at
  `working`, second call hits `ChildNotEligible`) or the second blocks
  on the flock and returns `concurrent_tick`. There is no observable
  window in which the parent log carries the submit event but not the
  clear event *and* an external caller can submit.
- **Verifies round-1 claim?** Partial. Round-1 pair-1b finding 2 asked
  for double-retry semantics and proposed "idempotent no-op." CD9
  replaced that with `RetryAlreadyInProgress` + `ChildNotEligible`.
  The `ChildNotEligible` path is reachable and useful; the
  `RetryAlreadyInProgress` path may be dead code under CD9 + CD12.
- **Severity:** nice-to-have
- **Proposed resolution:** Either (a) keep
  `RetryAlreadyInProgress` and document it as reachable only from
  in-process test harnesses that bypass the flock (cheap), or
  (b) remove the variant from the enum and document the flock + outcome
  check as the sufficient mechanism. Option (a) preserves the enum
  surface for future use.

### Finding 2 — Variant name drift: `NoBatchMaterialized` vs `NoBatchYet`

- **Observation:** CD9's round-2 prompt uses the name `NoBatchYet`. The
  design file uses `NoBatchMaterialized` (line 2945). Either name works
  semantically, but the skills, docs, and agent pattern-match guidance
  should all settle on one.
- **Verifies round-1 claim?** N/A (new in round-2 discussion).
- **Severity:** nice-to-have
- **Proposed resolution:** Use `NoBatchMaterialized` as the
  authoritative wire value (matches the struct). Update any prompt-
  level or exploration-doc references to match.

### Finding 3 — Non-existent child has no typed discriminator

- **Observation:** CD9's `InvalidRetryReason` enum does not distinguish
  "child does not exist on disk and was never declared" from
  "child exists but is in a non-retryable outcome." The most
  natural-looking response reuses `ChildNotEligible` with
  `current_outcome: "unknown"`, which blurs two distinct failure modes
  under one discriminator. This contradicts CD11's stated commitment:
  "typed enum discriminators throughout... replace every free-string
  or `&'static str` field."
- **Verifies round-1 claim?** Yes -- round-1 pair-1b finding 5 flagged
  "retry set including a successful child: behavior unspecified." CD9
  resolves eligibility, but the enum doesn't cover the reference-
  validity dimension.
- **Severity:** should-fix
- **Proposed resolution:** Add a new variant to `InvalidRetryReason`:

  ```rust
  UnknownChildren { children: Vec<String> },
  ```

  Fires when any name in the retry set is not in the declared task set
  and not on disk. Consistent with CD9's atomic-rejection rule: one
  unknown name rejects the whole submission. Pairs with the existing
  atomicity property.

### Finding 4 — Response does not echo the submitted payload

- **Observation:** Rejection responses identify the offending field by
  dotted path but do not echo what the agent submitted. For a CLI user
  debugging from a pasted shell snippet this is fine; for a pipeline
  agent reconstructing the call from a response, it is not. CD11's
  envelope is extensible and adding this would be additive.
- **Verifies round-1 claim?** No (new observation).
- **Severity:** nice-to-have
- **Proposed resolution:** Defer. If telemetry shows agents routinely
  re-constructing their own payload from shell history for retry,
  consider adding an optional `error.batch.submitted: serde_json::Value`
  field. Gated on real usage data.

### Finding 5 — Downward-only closure accepts "cannot progress" retries

- **Observation:** CD9 Part 5 commits to downward-only closure: a retry
  naming only a skipped child (D) whose failed ancestor (B) is NOT in
  the set gets accepted, rewinds D, and then the same tick re-skips D
  via runtime reclassification. The audit trail on D becomes
  delete->respawn->delete->respawn in one tick. The agent observes "I
  spent a retry and D is still skipped." The `reserved_actions`
  hint points at B as retryable, so a well-written agent recovers -- but
  a naive agent can loop.
- **Verifies round-1 claim?** Yes -- round-1 pair-1b finding 6
  ("closure direction is ambiguous") flagged this. CD9 resolved the
  ambiguity in favor of downward-only and explicitly accepted the
  thrash as a deliberate trade-off. This is a *known* design choice,
  not a bug. Including here as round-2 corroboration rather than a new
  ask.
- **Severity:** nice-to-have
- **Proposed resolution:** Either (a) add a new
  `InvalidRetryReason::ClosureCannotProgress { names: Vec<String>,
  blocked_by: Vec<String> }` variant that rejects retries whose closure
  contains no failure-eligible root, or (b) leave the thrash and rely
  on `reserved_actions.applies_to` to guide agents. CD9's alternatives
  block rejected auto-upward expansion; rejection-on-detection is
  a different third option that preserves "user names what they mean"
  semantics while eliminating the thrash.

### Finding 6 — Pre-append invariant verified for 7 of 8 edges

- **Observation:** Edges 1, 2, 3, 5, 6, 7, 8 all reject pre-append. No
  state file writes, no parent log events, no scheduler run. This
  matches CD11's commitment exactly. Edge 4 is not a rejection -- it is
  an accepted submission whose runtime reclassification produces a
  no-op outcome. The pre-append invariant does not apply.
- **Verifies round-1 claim?** Yes. Round-1 pair-1b repeatedly flagged
  "behavior unspecified; will this leave the log in a partial state?"
  CD11 closed that category cleanly.
- **Severity:** informational (positive confirmation)
- **Proposed resolution:** None. Land as-is.

### Finding 7 — Machine-parseable envelope shape holds

- **Observation:** Every rejection response carries
  `action: "error"`, `advanced: false`, `error.code`,
  `error.batch.kind: "invalid_retry_request"`, and
  `error.batch.reason` as a snake_case discriminator. Variant-specific
  payloads (`children`, `extra_fields`) are typed. An agent match
  statement on `error.batch.reason` suffices for recovery branching
  without string-scraping `error.message`.
- **Verifies round-1 claim?** Yes. Round-1 Cluster B demanded typed
  envelopes; CD11 delivered; round-2 confirms the promised shape is
  consistent across all 5 `InvalidRetryReason` variants.
- **Severity:** informational (positive confirmation)
- **Proposed resolution:** None.

### Finding 8 — Prompt's proposed names diverge from authoritative enum

- **Observation:** The round-2 task description proposed
  `InvalidRetryRequest::NonRetryableChildren`, `NoBatchYet`, and
  `InvalidRetryRequest` as the top-level kind. The design uses
  `InvalidRetryRequest` as the `BatchError` variant, `invalid_retry_request`
  as the JSON `error.batch.kind`, and `ChildNotEligible` (not
  `NonRetryableChildren`) and `NoBatchMaterialized` (not `NoBatchYet`)
  as `InvalidRetryReason` variants. None of the divergences change
  semantics, but skill documentation and agent-facing patterns should
  anchor on the design-file names.
- **Verifies round-1 claim?** N/A (meta-observation).
- **Severity:** nice-to-have
- **Proposed resolution:** Treat the design file as authoritative;
  update the koto-user skill and any round-1/round-2 exploration notes
  that propose alternate names to use the chosen variants.

---

## Summary of CD9 + CD11 verification

| Edge | Round-1 claim | CD9/CD11 resolution | Round-2 verdict |
|---|---|---|---|
| 1: Double retry no tick | Unspecified | `RetryAlreadyInProgress` | Likely unreachable (Finding 1); `ChildNotEligible` subsumes |
| 2: Retry running child | Permissive | `ChildNotEligible` | Confirmed; typed |
| 3: Retry successful child | Unspecified | `ChildNotEligible` atomic | Confirmed; typed |
| 4: Closure direction | Ambiguous | Downward only | Confirmed; accepts thrash (Finding 5) |
| 5: Mixed payload | Unspecified | `MixedWithOtherEvidence` | Confirmed; `extra_fields` typed |
| 6: Premature retry | Unspecified | `NoBatchMaterialized` | Confirmed; name drift (Finding 2) |
| 7: Empty retry set | Unspecified | `EmptyChildList` | Confirmed; clean |
| 8: Unknown child | Unspecified | No typed variant; reuses `ChildNotEligible` + sentinel | Gap (Finding 3) |

CD9's atomic-rejection commitment is verified for edges where the
enum covers the case. CD11's pre-append, machine-parseable envelope
commitment is verified across all rejections. Two residual gaps:
`UnknownChildren` (Finding 3) needs a typed variant; the
cannot-progress thrash (Finding 5) is a deliberate trade-off worth
documenting in the koto-user skill.
