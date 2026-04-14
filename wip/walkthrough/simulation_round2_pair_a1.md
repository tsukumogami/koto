# Round 2, Pair A1 — Retry flow end-to-end (Shape A, diamond)

Scenario: 4-issue diamond `A → B, A → C, B → D, C → D` with
`failure_policy: skip_dependents`. Templates are the revised reference
`coord.md` (two-transition: `plan_and_await` → `summarize` on
`all_success: true`, → `analyze_failures` on `needs_attention: true`;
latter accepts `retry_failed` via `reserved_actions` and transitions
back to `plan_and_await` on `evidence.retry_failed: present`) and
`impl-issue.md` (working / done / done_blocked failure-true /
skipped_due_to_dep_failure with `skipped_marker: true`).

Drives the end-to-end retry flow and specifically probes the CD9,
CD10, CD11, CD13 claims: reachability, retry-mechanism two-step,
reserved_actions discoverability, runtime reclassification, atomic
rejection of double retry, `batch_final_view` on terminal, and
`synthetic: true` on skip markers.

---

## Section 1: Transcript

All `koto status` omissions assume `LocalBackend` (no `sync_status` /
`machine_id` fields).

### Turn 1 — AGENT: init + first tick

```bash
koto init coord --template coord.md --var plan_path=PLAN-diamond.md
koto next coord
```

KOTO response to `init`:
```json
{"action":"initialized","workflow":"coord","state":"plan_and_await","template":"coord.md"}
```

KOTO response to `koto next coord` (no evidence yet):
```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name": {"type": "string", "required": true, "description": "Child workflow short name"},
          "template": {"type": "string", "required": false, "default": "impl-issue.md"},
          "vars": {"type": "object", "required": false},
          "waits_on": {"type": "array", "required": false, "default": []},
          "trigger_rule": {"type": "string", "required": false, "default": "all_success"}
        }
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 0, "completed": 0, "pending": 0, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": []
    }
  }],
  "scheduler": null
}
```

Gloss for AGENT: "The parent is waiting on a `tasks` payload. No
batch exists yet; `scheduler` is null. `item_schema` tells me
`template` defaults to `impl-issue.md`."

### Turn 2 — AGENT: submit the 4-task diamond

AGENT writes `tasks.json`:
```json
{"tasks": [
  {"name":"issue-A","vars":{"ISSUE_NUMBER":"101"}},
  {"name":"issue-B","vars":{"ISSUE_NUMBER":"102"},"waits_on":["issue-A"]},
  {"name":"issue-C","vars":{"ISSUE_NUMBER":"103"},"waits_on":["issue-A"]},
  {"name":"issue-D","vars":{"ISSUE_NUMBER":"104"},"waits_on":["issue-B","issue-C"]}
]}
```

```bash
koto next coord --with-data @tasks.json
```

KOTO:
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 0, "pending": 1, "success": 0,
      "failed": 0, "skipped": 0, "blocked": 3, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-B","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.issue-A"]},
        {"name":"coord.issue-C","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.issue-A"]},
        {"name":"coord.issue-D","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.issue-B","coord.issue-C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-A"],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"pending","state":"working"}
    ],
    "already": [], "blocked": ["coord.issue-B","coord.issue-C","coord.issue-D"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "issue-A": {"outcome":"accepted"},
        "issue-B": {"outcome":"blocked","waits_on":["issue-A"]},
        "issue-C": {"outcome":"blocked","waits_on":["issue-A"]},
        "issue-D": {"outcome":"blocked","waits_on":["issue-B","issue-C"]}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: "A is spawned and pending. B, C, D are blocked. The
ledger (`materialized_children`) currently holds only A — use this
set for idempotent dispatch. `needs_attention: false`, so no
`reserved_actions`."

### Turn 3 — AGENT: drive A to completion

```bash
koto next coord.issue-A
koto next coord.issue-A --with-data '{"status":"complete"}'
```

Responses as in the canonical walkthrough; second call returns:
```json
{"action":"done","state":"done","directive":"Issue #101 implemented successfully.","is_terminal":true}
```

### Turn 4 — AGENT: re-tick parent; expect B and C to spawn

```bash
koto next coord
```

KOTO:
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 1, "pending": 2, "success": 1,
      "failed": 0, "skipped": 0, "blocked": 1, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-C","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-D","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.issue-B","coord.issue-C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-B","coord.issue-C"],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done"},
      {"name":"coord.issue-B","outcome":"pending","state":"working"},
      {"name":"coord.issue-C","outcome":"pending","state":"working"}
    ],
    "already": ["coord.issue-A"], "blocked": ["coord.issue-D"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "issue-A": {"outcome":"already"},
        "issue-B": {"outcome":"accepted"},
        "issue-C": {"outcome":"accepted"},
        "issue-D": {"outcome":"blocked","waits_on":["issue-B","issue-C"]}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: "B and C are now live; D still blocked on both."

### Turn 5 — AGENT: drive B → blocked; drive C → complete

AGENT runs worker on B:
```bash
koto next coord.issue-B --with-data '{"status":"blocked"}'
```
```json
{"action":"done","state":"done_blocked","directive":"Issue #102 is blocked and cannot proceed.","is_terminal":true}
```

AGENT runs worker on C:
```bash
koto next coord.issue-C --with-data '{"status":"complete"}'
```
```json
{"action":"done","state":"done","directive":"Issue #103 implemented successfully.","is_terminal":true}
```

### Turn 6 — AGENT: re-tick parent; D must be synthesized as skip

```bash
koto next coord
```

KOTO: gate reclassifies D as skipped (B failed, `waits_on`
includes B, `skip_dependents` policy). Scheduler delete-and-respawns
nothing here (D had no state file yet), so it directly
materializes D in `skipped_due_to_dep_failure` via the real
`impl-issue.md` template. `all_complete: true`, `any_failed: true`,
`any_skipped: true`, `needs_attention: true`. Advance loop appends
a `BatchFinalized` event on this first `all_complete: true` pass
(CD13) and fires the `needs_attention: true` transition to
`analyze_failures`. Scheduler then runs on `analyze_failures`, no
hook → `NoBatch` (scheduler value still `null`).

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. Inspect the batch view ... Two recovery paths: - Retry the failures. Submit the retry_failed reserved action ... - Give up or acknowledge.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type":"enum","values":["give_up","acknowledge"],"required":false}
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 4, "pending": 0, "success": 2,
      "failed": 1, "skipped": 1, "blocked": 0, "spawn_failed": 0,
      "all_complete": true, "all_success": false,
      "any_failed": true, "any_skipped": true, "needs_attention": true,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"done_blocked","complete":true,"outcome":"failure","failure_mode":true,"reason":"Issue 102 hit an unresolvable blocker during implementation.","reason_source":"failure_reason"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":"skipped_due_to_dep_failure","complete":true,"outcome":"skipped","skipped_because":"coord.issue-B","skipped_because_chain":["coord.issue-B"]}
      ]
    }
  }],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type":"array<string>","required":true},
        "include_skipped": {"type":"boolean","required":false,"default":true}
      },
      "applies_to": ["coord.issue-B","coord.issue-D"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.issue-B\"]}}'"
    }
  ],
  "scheduler": null
}
```

Gloss: "Batch is terminal-with-failures. `reserved_actions[0]`
gives me a ready-to-run invocation. I did not read any skill doc.
`applies_to` lists the retryable set; the invocation already
defaults to naming just the failed parent (B) — the default
`include_skipped: true` will pull D in via downward closure."

### Turn 7 — AGENT: probe the synthetic D before retry

AGENT is skeptical; it wants to inspect D.

```bash
koto status coord.issue-D
```

KOTO:
```json
{
  "workflow": "coord.issue-D",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-B",
  "skipped_because_chain": ["coord.issue-B"],
  "reason_source": "state_name"
}
```

```bash
koto next coord.issue-D
```

KOTO:
```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "Issue #104 was skipped because a dependency failed. No action required - the scheduler materialized this child directly into its terminal skip state.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-B",
  "skipped_because_chain": ["coord.issue-B"]
}
```

Gloss: "D carries `synthetic: true`, driven by its current state's
`skipped_marker: true` predicate. This is what I need to
distinguish skip markers from real terminal work."

### Turn 8 — AGENT: submit retry_failed

AGENT copies the invocation string but edits to include D
explicitly to test behavior — the payload is now redundant (D is
pulled in via closure anyway) but CD9 Part 4 doesn't explicitly
address the case where `children` names a skipped dependent
directly.

```bash
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B", "coord.issue-D"], "include_skipped": true}}'
```

KOTO: `handle_retry_failed` intercepts pre-advance-loop. Validates
R10:
- non-empty: ok
- each child exists: ok
- each outcome is `failure` or `skipped`: B is `failure`, D is
  `skipped` — ok
- no mixed payload: ok

Canonical sequence: append `EvidenceSubmitted{retry_failed:...}`,
append clearing `EvidenceSubmitted{retry_failed:null}`, write
`Rewound` to B (outcome `failure` → rewind target `working`),
delete-and-respawn D (skip marker → respawn real-template entry).

Then advance loop runs: current state `analyze_failures`,
transition `evidence.retry_failed: present` matches the
un-cleared submission (the guard checks the first event of the
pair, before clearing), transitions to `plan_and_await`.
Scheduler runs on `plan_and_await`: B is Running (state file
exists, working state, not terminal), C is `success`, A is
`success`, D is Running (just respawned). Nothing to spawn.
SchedulerRan is **not** appended (no spawn/skip/error this tick)
per the event policy.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 2, "success": 2,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":"working","complete":false,"outcome":"pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done"},
      {"name":"coord.issue-B","outcome":"pending","state":"working"},
      {"name":"coord.issue-C","outcome":"success","state":"done"},
      {"name":"coord.issue-D","outcome":"pending","state":"working"}
    ],
    "already": ["coord.issue-A","coord.issue-B","coord.issue-C","coord.issue-D"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

**[GAP 1: ambiguous]** The revised design is silent on whether, in
this state, D should be classified `pending` or `blocked_by:
["coord.issue-B"]`. B is now Running (non-terminal), but D's
`waits_on` includes B. Under the original gate rules, D is not yet
terminal and its blocker isn't terminal either → logically it should
be `outcome: "blocked", blocked_by: ["coord.issue-B"]`. But
`materialized_children` shows D with `state: "working"` (the
scheduler delete-and-respawned D with a state file), so the gate
says D is `pending`, not `blocked`. This is a real ambiguity between
"D exists on disk" (so Running) and "D shouldn't yet be actionable"
(because B hasn't re-succeeded). The walkthrough shows in-flight
re-work as `pending`, but nothing stops the agent from dispatching a
worker on D immediately — which would run D against a B that hasn't
completed yet.

Gloss: "Retry accepted. Parent routed back to `plan_and_await`. B
and D are both `pending` on `working`. Feedback entries is empty
because no `tasks` payload was submitted — retry doesn't touch the
task set." AGENT notes the D-pending-but-B-not-done issue for later.

### Turn 9 — AGENT: probe "does next tick show anything useful"

AGENT issues `koto status coord`:
```json
{
  "workflow": "coord",
  "state": "plan_and_await",
  "is_terminal": false,
  "batch": {
    "phase": "final",
    "summary": {"total":4,"success":2,"failed":1,"skipped":1,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks": [
      {"name":"issue-A","child":"coord.issue-A","outcome":"success"},
      {"name":"issue-B","child":"coord.issue-B","outcome":"failure"},
      {"name":"issue-C","child":"coord.issue-C","outcome":"success"},
      {"name":"issue-D","child":"coord.issue-D","outcome":"skipped"}
    ],
    "ready":[], "blocked":[], "skipped":[], "failed":[]
  }
}
```

**[GAP 2: stale]** `koto status coord` after retry still shows the
`BatchFinalized` snapshot from the *previous* failed pass. That
snapshot says B is `failure` and D is `skipped`, but the live state
(after retry) has B and D `pending` again. `batch.phase: "final"`
labels it as final, but it's already stale — the agent just
submitted a retry that invalidated every `failure`/`skipped` outcome
in that snapshot. The design says a new `BatchFinalized` is appended
on the *next* `all_complete: true` pass; there's no mechanism to
mark the old one as superseded while we're mid-retry.

### Turn 10 — AGENT: probe atomic rejection (CD9 #4)

Before driving the child workers, AGENT tries a second retry
("what if I issued it twice by mistake?"):

```bash
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"]}}'
```

KOTO: R10 validates. `coord.issue-B` exists on disk with current
state `working`, outcome `pending`. This outcome is neither
`failure` nor `skipped` → reject with `InvalidRetryReason::ChildNotEligible`.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "retry_failed rejected: one or more children are not eligible for retry",
    "details": [{"field":"retry_failed.children","reason":"child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name":"coord.issue-B","current_outcome":"pending"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: "Second retry on an already-re-running child rejects cleanly
with a typed reason. Pre-append — no state change."

**[Partial verification of CD9 #4]** The design names
`InvalidRetryReason::RetryAlreadyInProgress` for the "second
submission before the first's tick" case. But the *first* retry has
already committed (clearing event written, advance loop already ran,
children already rewound). So the `RetryAlreadyInProgress` variant
doesn't apply here — `ChildNotEligible` does. The
"RetryAlreadyInProgress" variant is only reachable if the first and
second retry share a single tick boundary, which under the advisory
flock (CD12 Q3) can only happen if both calls reach
`handle_retry_failed` in the same call. The advisory flock makes
this impossible in practice. So `RetryAlreadyInProgress` may be
dead code — either document when it fires, or delete it.

### Turn 11 — AGENT: drive B re-run, then D

```bash
koto next coord.issue-B
```

But B was rewound to `working` — its state file is the original
`impl-issue.md` `working` state. `koto next` on B returns:
```json
{
  "action":"evidence_required",
  "state":"working",
  "directive":"Implement issue #102. ...",
  "expects":{"event_type":"evidence_submitted","fields":{"status":{"type":"enum","values":["complete","blocked"],"required":true}}},
  "blocking_conditions":[],
  "scheduler":null
}
```

AGENT: "But wait — D is also `working`, and *my worker pool could
dispatch it right now*. That's wrong; D shouldn't run until B
succeeds."

**[Flagging as GAP 1 continuation]** The response has no signal
telling the agent "don't dispatch D yet." `materialized_children`
lists D as `pending, working`. Under Decision 9's runtime-
reclassification model, D will only become a skip marker *if and
when* B re-fails. There is no "D is waiting on an in-flight retry"
state. Worker dispatch based on `materialized_children` would
dispatch D prematurely.

AGENT proceeds anyway (the design implies D just runs and the
coordinator must manually sequence). Submits complete on B:
```bash
koto next coord.issue-B --with-data '{"status":"complete"}'
```
```json
{"action":"done","state":"done","directive":"Issue #102 implemented successfully.","is_terminal":true}
```

Then drives D (assuming coordinator held off until B finished):
```bash
koto next coord.issue-D
koto next coord.issue-D --with-data '{"status":"complete"}'
```
Second returns done/done.

### Turn 12 — AGENT: final tick, expect `batch_final_view`

```bash
koto next coord
```

KOTO: advance at `plan_and_await`. Gate: all 4 children terminal
with outcome `success`. `all_success: true`. Transition to
`summarize`. New `BatchFinalized` appended (supersedes prior).
Scheduler on `summarize` → `NoBatch`.

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {"total":4,"success":4,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks": [
      {"name":"issue-A","child":"coord.issue-A","outcome":"success"},
      {"name":"issue-B","child":"coord.issue-B","outcome":"success"},
      {"name":"issue-C","child":"coord.issue-C","outcome":"success"},
      {"name":"issue-D","child":"coord.issue-D","outcome":"success"}
    ],
    "ready":[], "blocked":[], "skipped":[], "failed":[]
  }
}
```

### Turn 13 — AGENT: post-terminal `koto status coord`

```bash
koto status coord
```

KOTO:
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": {
    "phase": "final",
    "summary": {"total":4,"success":4,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks": [
      {"name":"issue-A","child":"coord.issue-A","outcome":"success"},
      {"name":"issue-B","child":"coord.issue-B","outcome":"success"},
      {"name":"issue-C","child":"coord.issue-C","outcome":"success"},
      {"name":"issue-D","child":"coord.issue-D","outcome":"success"}
    ],
    "ready":[], "blocked":[], "skipped":[], "failed":[]
  }
}
```

Gloss: "`batch_final_view` is available on the terminal `done`
response AND `koto status` still carries the batch section after
the parent advanced out of `plan_and_await`. CD13's preservation
works."

### Turn 14 — AGENT: probe the clearing event in `koto query`

```bash
koto query coord --events
```

(abbreviated) The log contains consecutively:
```
EvidenceSubmitted { fields: { retry_failed: {...} } }
EvidenceSubmitted { fields: { retry_failed: null } }
Rewound { target_state: "working", ... }  # for coord.issue-B via child log reference
```

**[GAP 3: confusing for readers]** The two consecutive
`EvidenceSubmitted` events — one with a retry payload, one clearing
it — would look like the same event repeated to a future agent
reading the log. CD9 describes this as the "null-clearing idiom"
but the design doesn't describe how agents (or operators reading
`koto query`) should interpret them. A comment field or a dedicated
`EvidenceSubmitted.purpose: "submit" | "clear"` would make this
self-documenting.

---

## Section 2: Findings

### F1 — D classification during in-flight retry is ambiguous

- **Observation**: After `retry_failed` submits and delete-and-
  respawns D, the gate classifies D as `outcome: pending, state:
  working`. But D's `waits_on` includes B, which is also running
  (rewound). The response gives no "D is waiting on in-flight
  retry" signal. An agent keying worker dispatch on
  `materialized_children` would dispatch D immediately, running D
  before its upstream B finishes.
- **Verifies round-1 claim?**: No — this is a **new issue** introduced
  by CD9 Part 5's runtime-reclassification model. Under the old
  synthetic-template model, D stayed synthetic until B terminated,
  so the problem didn't exist.
- **Severity**: blocker. This defeats the whole point of
  `waits_on` dependencies during a retry.
- **Proposed resolution**: when the scheduler delete-and-respawns a
  skip marker whose `waits_on` contains a Running child (i.e., a
  rewound sibling), put the respawn into a deferred state. Options:
  (a) don't respawn D at all while B is Running — leave D as a skip
  marker until B reaches a new terminal; (b) respawn D but compute
  gate `outcome: "blocked", blocked_by: ["coord.issue-B"]` when D's
  `waits_on` contains a non-terminal sibling. Option (a) is
  cleaner — matches the "skip marker until all `waits_on` are
  terminal" invariant that Decision 5's original model maintained.

### F2 — Stale `BatchFinalized` during retry window

- **Observation**: `koto status coord` mid-retry returns the
  *previous* finalized batch view (showing B as `failure`, D as
  `skipped`) even though the live state has both back to `pending`.
  `batch.phase: "final"` labels this "final" but it's visibly
  stale.
- **Verifies round-1 claim?**: Partial. CD13 claims "persist
  last-known batch view on the parent through terminal states"
  and the view does persist — but CD13 didn't consider the case
  where a new retry *invalidates* the finalized view before a new
  one replaces it.
- **Severity**: should-fix. Not catastrophic (agent can see from
  the gate output that things changed), but `phase: "final"` is
  misleading.
- **Proposed resolution**: when `handle_retry_failed` writes the
  retry payload, also append a `BatchInvalidated` event (or set
  `batch.phase: "superseded"` in `koto status` when the parent log
  contains a `BatchFinalized` followed by an `EvidenceSubmitted{
  retry_failed }` with no later `BatchFinalized`). Simpler option:
  drop `batch.phase: "final"` and emit `batch.phase: "current"`
  whenever any post-final retry is pending.

### F3 — `InvalidRetryReason::RetryAlreadyInProgress` may be dead code

- **Observation**: Under the advisory flock (CD12 Q3), two
  concurrent `retry_failed` calls cannot both enter
  `handle_retry_failed`. The second hits the lock and returns
  `concurrent_tick`. After the first commits (clearing event +
  advance loop), any subsequent retry rejects on `ChildNotEligible`
  (children are now Running, not Failed/Skipped).
- **Verifies round-1 claim?**: No — this surfaces a **new gap**
  introduced by CD12 hardening making CD9's
  `RetryAlreadyInProgress` unreachable.
- **Severity**: nice-to-have. Dead code isn't harmful, but the
  design should either document the window in which the variant
  fires or delete it.
- **Proposed resolution**: either (a) delete
  `InvalidRetryReason::RetryAlreadyInProgress` from Decision 9 Part
  4 and rely on `ChildNotEligible` + advisory-flock `concurrent_tick`;
  or (b) document the precise scenario where it fires (e.g.,
  single-threaded path where the submit event is persisted but the
  clearing event fails mid-write).

### F4 — Clearing-event pair is opaque in `koto query`

- **Observation**: The log contains two consecutive
  `EvidenceSubmitted` events, one with a `retry_failed` payload and
  one with `retry_failed: null`. To a future reader (agent or
  human), they look like duplicate entries. CD9 calls this the
  "null-clearing idiom" but the design doesn't describe how readers
  should interpret it.
- **Verifies round-1 claim?**: Partial — CD9 committed the
  null-clearing mechanism but left its observability story
  undocumented.
- **Severity**: should-fix.
- **Proposed resolution**: add an optional `purpose: "submit" |
  "clear"` field to `EvidenceSubmitted`, OR introduce a
  `ReservedActionCleared` event type distinct from
  `EvidenceSubmitted`. The latter is cleaner but requires schema
  plumbing; the former is minimal.

### F5 — Verified: `reserved_actions` is discoverable without skill

- **Observation**: Turn 6's response carries `reserved_actions[0]`
  with `name`, `description`, `payload_schema`, `applies_to`, and a
  ready `invocation` string. AGENT copied the invocation and
  modified it without reading any skill documentation.
- **Verifies round-1 claim?**: **Yes, fully.** CD9 Part 3's
  discoverability claim holds.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change. Consider adding a note that
  the `applies_to` list reflects "children that would accept this
  retry" — which is useful when the agent wants to know the set
  without having to derive it from the children array.

### F6 — Verified: `batch_final_view` on terminal `done`

- **Observation**: Turn 12's terminal `done` response carries
  `batch_final_view` with the full post-retry snapshot. Turn 13's
  `koto status coord` also carries the `batch` section even though
  the current state is `summarize` (no `materialize_children`
  hook).
- **Verifies round-1 claim?**: **Yes.** CD13's `batch_final_view`
  and post-terminal status preservation both fire.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F7 — Verified: `synthetic: true` marker

- **Observation**: Before retry, `koto status coord.issue-D` and
  `koto next coord.issue-D` both carry `synthetic: true` and a
  terminal response with no directive. After retry respawn, D's
  state file is the real `working` state, `synthetic` is absent
  (the predicate is false — current state is not a skip marker).
- **Verifies round-1 claim?**: **Yes.** CD13's predicate-based
  synthetic marker is present when it should be and absent when it
  should be.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F8 — Retry `invocation` string is optimistic about quoting

- **Observation**: The `invocation` field is
  `"koto next coord --with-data '{\"retry_failed\": ...}'"`. When
  shelled out literally, the single-quote + escaped-double-quote
  form works on POSIX but breaks on Windows PowerShell (single
  quotes don't inhibit variable expansion the same way) and mixes
  awkwardly with agents that feed the string through another
  shell-escaping layer.
- **Verifies round-1 claim?**: No — this is a **new issue** with
  CD9 Part 3's invocation string.
- **Severity**: nice-to-have.
- **Proposed resolution**: either (a) use `@-` to read from stdin
  and document agents should pipe JSON; or (b) add a parallel
  `invocation_with_data_file` field that writes JSON to a temp
  file and references it via `@path`.

### F9 — Ambiguous: what happens when the agent names a skipped
dependent in `children`?

- **Observation**: Turn 8 named both B and D explicitly in
  `retry_failed.children`. R10 validates "each named child has
  outcome `failure` or `skipped`" — D is skipped, so both pass.
  But the design's downward-closure description implies the
  scheduler computes dependents of the retry set; naming D
  explicitly *plus* include_skipped=true is redundant. Design is
  silent on whether redundant specification is normalized, logged,
  or triggers a warning.
- **Verifies round-1 claim?**: No — new gap.
- **Severity**: nice-to-have.
- **Proposed resolution**: add explicit normalization: `children`
  is deduplicated with the computed downward closure. No warning;
  agents should be able to over-specify safely.

---

## Summary note

Six of nine findings are new (F1, F2, F3, F4, F8, F9); three verify
round-1 claims positively (F5, F6, F7). The blocker (F1) is a
semantic regression introduced by runtime reclassification itself:
by deleting skip markers on retry and respawning D as a real-
template child, the design loses the "wait for upstream" signal
that the synthetic model provided. This needs a design-level fix
before implementation, not just a documentation patch.
