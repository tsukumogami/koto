# Simulation Round 3 Pair A3: Typed envelope cleanups end-to-end

Round 3 verifies that round-2's polish fixes to `BatchError`,
`InvalidBatchReason`, `InvalidRetryReason`, `InvalidNameDetail`, and
`SchedulerWarning` produce a single canonical, machine-parseable shape
for every rejection an agent will plausibly hit while iterating on a
bad task list. Round-2 Pair B1 identified five envelope rough edges
(B1-1 through B1-5); round 3 applied the fixes:

- B1-1: `InvalidNameDetail` inner serde tag renamed from `detail` to
  `kind`. Wire shape now reads `reason.kind.kind = "invalid_chars"`
  instead of `.detail.detail`.
- B1-2 / B1-3: `InvalidBatchReason::LimitExceeded{Tasks,WaitsOn,Depth}`
  variants dropped. `BatchError::LimitExceeded` is now the sole path,
  hoisted to `error.batch.kind = "limit_exceeded"` with `which:
  LimitKind`, `limit`, `actual`, and new `task: Option<String>`.
- B1-4: `TaskSpawnError` gained a typed `compile_error: Option<CompileError>`
  field; `BatchError::TemplateCompileFailed` shares the same struct.
- B1-5: `BatchError::ConcurrentTick { holder_pid: Option<u32> }` is now
  a typed variant. On the wire it takes the `error.batch.kind =
  "concurrent_tick"` slot — not a free string in `details[].reason`.
- New `InvalidRetryReason::UnknownChildren { children }` replaces the
  prior overload of `ChildNotEligible` with a sentinel outcome.
- New `SchedulerWarning::OmittedPriorTask { task }` surfaces silent
  omission without failing the submission.

Grounding: DESIGN lines 3103-3302 (revised Key Interfaces),
1983-1986 (redaction sentinel), 2642-2700 (path canonicalization and
CompileError split), round-2 Pair B1 findings.

AGENT iterates against the canonical `coord.md` parent from
`walkthrough.md`. Parent state is `plan_and_await`; the agent's goal
is to drive the tick loop through a realistic error sequence then
recover.

---

## Section 1: Transcript

### Step 1 — `LimitExceeded` (tasks)

AGENT submits 1001-entry task list.

```bash
koto next coord --with-data @huge_tasks.json
```

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Submission exceeds the tasks limit (1001 > 1000)",
    "details": [{"field": "tasks", "reason": "limit_exceeded"}],
    "batch": {
      "kind": "limit_exceeded",
      "which": "tasks",
      "limit": 1000,
      "actual": 1001
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Verify:

- `error.batch.kind == "limit_exceeded"` — top-level, not nested under
  `invalid_batch_definition`. Matches DESIGN line 3202
  (`BatchError::LimitExceeded` is a sibling of `InvalidBatchDefinition`
  on `BatchError`).
- `which: "tasks"` — snake_case serialization of `LimitKind::Tasks`
  (line 3257).
- No `task` field (this is a whole-submission limit, not per-task).
  `task: Option<String>` is omitted via `skip_serializing_if` rather
  than rendered as `null` — consistent with the rest of the enum.
- No phantom `invalid_batch_definition` sibling with a
  `limit_exceeded_tasks` reason. Round-2 B1-2 closed.

AGENT side-check: `koto query coord --events` shows zero
`EvidenceSubmitted` for this call. Pre-append holds.

---

### Step 2 — `LimitExceeded` (waits_on per-task)

AGENT submits: task `A` with 11 entries in `waits_on`.

```json
{"tasks": [
  {"name": "A", "waits_on": ["B","C","D","E","F","G","H","I","J","K","L"]},
  {"name": "B"}, {"name": "C"}, ... (B..L as plain entries)
]}
```

`error.batch`:

```json
{
  "kind": "limit_exceeded",
  "which": "waits_on",
  "task": "A",
  "limit": 10,
  "actual": 11
}
```

Verify:

- Same `kind: "limit_exceeded"` envelope as Step 1. Single path.
- `task: "A"` populated — the new `Option<String>` field from DESIGN
  line 3202 carries the per-task context. B1-3 closed.
- No competing `invalid_batch_definition` / `limit_exceeded_waits_on`
  shape on the wire. Dual representation eliminated.

---

### Step 3 — `LimitExceeded` (depth)

AGENT submits a 51-node linear chain: `T0 → T1 → ... → T50`, each
`waits_on` the previous.

`error.batch`:

```json
{
  "kind": "limit_exceeded",
  "which": "depth",
  "limit": 50,
  "actual": 51
}
```

Verify:

- `which: "depth"` — `LimitKind::Depth`.
- No `task` field (depth is a whole-graph property, not per-task).
- Envelope shape is byte-identical to Steps 1 and 2 modulo `which` and
  `task`. AGENT pattern-matching is a single branch.

---

### Step 4 — `ConcurrentTick`

AGENT accidentally starts two `koto next coord --with-data @tasks.json`
processes concurrently. Process A acquires the advisory flock; process
B loses the race.

Process B response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "integration_unavailable",
    "message": "Another koto tick is holding the parent lock (pid 12345)",
    "details": [{"field": "workflow", "reason": "concurrent_tick"}],
    "batch": {
      "kind": "concurrent_tick",
      "holder_pid": 12345
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Verify:

- `error.batch.kind == "concurrent_tick"` — typed sibling of
  `invalid_batch_definition` and `limit_exceeded`. B1-5 closed.
- `holder_pid: 12345` — typed `Option<u32>` from DESIGN line 3208.
  When the holding process is on a different machine or the OS
  doesn't expose the PID, `holder_pid` is omitted (not `null`).
- `error.code` remains `integration_unavailable` per CD11 mapping
  (line 2093). The typed `error.batch` block is additive — existing
  consumers that key on `error.code` still work.
- `details[0].reason == "concurrent_tick"` is retained for legacy
  consumers. Agents preferring typed discriminators use
  `error.batch.kind`; both point at the same condition.

---

### Step 5 — R9 reserved-name collision

AGENT submits `[{"name": "retry_failed"}]`.

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "reserved_name_collision",
  "task": "retry_failed",
  "reserved": "retry_failed"
}
```

Verify:

- `kind: "invalid_batch_definition"` — this is a name-rule violation,
  not a limit violation. Stays under the compound envelope.
- `reason: "reserved_name_collision"` — tag from
  `InvalidBatchReason::ReservedNameCollision` (line 3239). Does NOT
  route through `InvalidName`; reserved-name collisions are a sibling
  variant (round-2 B1 established this; still holds).
- No `InvalidNameDetail::ReservedName` variant exists or is expected
  here. The request prompt's reference to "Shape: `reason.details[].kind
  == reserved_name`" does not match the revised design; reserved-name
  collisions have their own top-level `reason` tag. Recorded as a
  prompt/design mismatch, the design shape is authoritative.

---

### Step 6 — R9 invalid chars (renamed inner tag)

AGENT submits `[{"name": "has spaces"}]`.

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "invalid_name",
  "task": "has spaces",
  "kind_detail": {
    "kind": "invalid_chars",
    "pattern": "^[A-Za-z0-9_-]+$"
  }
}
```

Wait — the outer field is declared in DESIGN line 3238 as
`InvalidName { task: String, kind: InvalidNameDetail }`, and
`InvalidNameDetail` itself uses `#[serde(tag = "kind")]` (line 3249).
Serde flattens the outer struct field named `kind` alongside the inner
enum tag also named `kind`. On the wire this collides: serde would
serialize the outer struct as

```json
{
  "kind": "invalid_batch_definition",
  "reason": "invalid_name",
  "task": "has spaces",
  "kind": { "kind": "invalid_chars", "pattern": "^[A-Za-z0-9_-]+$" }
}
```

The outer field name `kind` on `InvalidName.kind: InvalidNameDetail`
collides with the outer envelope's `kind: "invalid_batch_definition"`
key at the same JSON object level because `InvalidBatchReason`'s serde
tag flattens variant fields into the parent. Round-3 rename fixed the
inner tag (`detail` → `kind`) but reintroduced an outer/outer conflict.
**Finding A3-1 (medium):** the field-name collision.

Proposed resolution: rename the outer field on `InvalidName` from
`kind` to `name_rule` (or `detail_kind`), giving a wire shape like:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "invalid_name",
  "task": "has spaces",
  "name_rule": { "kind": "invalid_chars", "pattern": "^[A-Za-z0-9_-]+$" }
}
```

With that rename, AGENT dispatch is unambiguous:

```
match response.error.batch.kind:
  "invalid_batch_definition" =>
    match response.error.batch.reason:
      "invalid_name" => match response.error.batch.name_rule.kind:
                         "empty" | "invalid_chars" | "too_long" => ...
```

Without the rename, serde will either overwrite the outer `kind` key
(silent correctness bug) or serde's derive will reject the duplicate
at compile time. Either way the design needs to pin the outer field
name to something other than `kind`.

---

### Step 7 — `TemplateCompileFailed` (per-task, typed payload)

AGENT submits a valid 3-task graph. Task `X` references
`broken.md`; the file exists but has malformed YAML frontmatter.

KOTO response (relevant slice):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned_this_tick": ["coord.Y", "coord.Z"],
    "materialized_children": [...],
    "errored": [{
      "task": "X",
      "kind": "template_compile_failed",
      "paths_tried": null,
      "message": "Template compile failed: unexpected end of stream at line 7",
      "template_source": "override",
      "compile_error": {
        "kind": "yaml_parse",
        "message": "unexpected end of stream",
        "location": {"line": 7, "column": 1}
      }
    }],
    "warnings": []
  },
  "blocking_conditions": [...]
}
```

Verify:

- Per CD14 mapping (design line 3313), runtime template failures do
  NOT promote to top-level `error`. `action` remains `gate_blocked`.
- `scheduler.errored[].compile_error` is the typed `CompileError`
  struct (line 3214) with `kind`, `message`, and optional `location`.
  Not a free string. B1-4 closed.
- `compile_error.kind` is a short discriminator (`yaml_parse` here;
  `missing_field` or `state_reference` in other cases per DESIGN line
  3215). Agents can programmatically route on compile-error class.
- `template_source: "override"` — the agent's explicit `template:`
  field was used (as opposed to the hook's `default_template`). Typed
  `TemplateSource::Override` (line 3104).
- `paths_tried: null` — the file WAS found; there's no path-probing
  list to report. `Option<Vec<String>>` is serialized as `null` here
  rather than omitted; confirm against the struct's
  `skip_serializing_if`. DESIGN line 3090 does not put
  `skip_serializing_if` on `paths_tried`, so `null` is on-wire.
  Acceptable; worth pinning.

---

### Step 8 — `TemplateNotFound` (paths_tried canonicalized)

AGENT submits task `X` with `template: "../evil/../missing.md"`. All
three configured base dirs probe, none hit.

```json
"errored": [{
  "task": "X",
  "kind": "template_not_found",
  "paths_tried": [
    "/home/user/.claude/templates/missing.md",
    "/home/user/.koto/templates/missing.md",
    "/workspace/templates/missing.md"
  ],
  "message": "Template not found at any configured base",
  "template_source": "override",
  "compile_error": null
}]
```

Verify:

- `paths_tried` entries are fully canonicalized (absolute, no `..`
  segments, no `.`, no symlink components). DESIGN line 2694-2695
  ("`paths_tried` canonicalization. The absolute paths echoed in
  `TemplateNotFound.paths_tried` and `TaskSpawnError.paths_tried`
  are...") commits this. The agent never sees their own `..` echoed
  back. This matters because an agent that logs `paths_tried` into a
  diagnostic blob shouldn't embed a path that contains `..` segments
  derivable from the original submission.
- `compile_error: null` because the kind is `template_not_found`, not
  a compile failure. `Option<CompileError>` — the "shared shape"
  commitment from DESIGN line 3098 holds; both error kinds use one
  `TaskSpawnError` shape.

---

### Step 9 — `InvalidRetryReason::UnknownChildren`

Setup: batch `[A, B, C]` ran; A failed, B succeeded, C skipped
(because A is failed). Parent is in `analyze_failures`. AGENT submits
`retry_failed` naming a nonexistent child.

```bash
koto next coord --with-data '{"retry_failed": {"children": ["coord.ghost"]}}'
```

KOTO response (`error.batch`):

```json
{
  "kind": "invalid_batch_definition",
  "reason": "invalid_retry_request",
  "retry_reason": {
    "reason": "unknown_children",
    "children": ["coord.ghost"]
  }
}
```

Hmm — the outer envelope. `BatchError::InvalidRetryRequest { reason:
InvalidRetryReason }` (line 3204) is a top-level `BatchError`
variant, sibling of `InvalidBatchDefinition`. So on the wire it's:

```json
{
  "kind": "invalid_retry_request",
  "reason": "unknown_children",
  "children": ["coord.ghost"]
}
```

where `kind` is the `BatchError` discriminator (`invalid_retry_request`)
and `reason` is the `InvalidRetryReason` discriminator
(`unknown_children`). No outer/inner collision because the outer
`BatchError` uses `tag = "kind"` and the inner `InvalidRetryReason`
uses `tag = "reason"` (line 3264).

Verify:

- `error.batch.kind == "invalid_retry_request"` — matches
  `BatchError::InvalidRetryRequest` snake_case serialization.
- `error.batch.reason == "unknown_children"` — the new typed variant
  from DESIGN line 3274, not a repurposed `ChildNotEligible` with a
  sentinel outcome.
- `children: ["coord.ghost"]` — names echoed back exactly as
  submitted. The bare child names (no parent prefix stripping) let
  the agent see their own input.
- `ChildEligibility.current_outcome` is NOT used here (that field
  explicitly rejects the `"unknown"` sentinel per DESIGN line 3298-3301).
  Unknown names go to `UnknownChildren`; mis-statused known names go
  to `ChildNotEligible`. Clean split.

---

### Step 10 — `SpawnedTaskMutated` with redaction

Setup: prior submission spawned `coord.A` with
`vars.GITHUB_TOKEN = "ghp_realsecret"`. AGENT resubmits with
`vars.GITHUB_TOKEN = "ghp_newsecret"`.

```json
{
  "kind": "invalid_batch_definition",
  "reason": "spawned_task_mutated",
  "task": "A",
  "changed_fields": [
    {
      "field": "vars.GITHUB_TOKEN",
      "spawned_value": "[REDACTED]",
      "submitted_value": "[REDACTED]"
    }
  ]
}
```

Verify:

- `spawned_value` and `submitted_value` are the literal string
  `"[REDACTED]"`. Not `{redacted: true}`, not an empty string, not
  `null`. DESIGN line 1983-1986 pins this exact sentinel.
- BOTH values are redacted, not just the submitted one. The agent
  cannot cross-reference against a leaked spawned value.
- The `field` path still names the var (`vars.GITHUB_TOKEN`) so the
  agent knows which secret collided. The `field` is not redacted; only
  the `spawned_value` and `submitted_value` cells are.
- Redaction applies based on the top-level `vars` key matching a
  secret-pattern (`*_TOKEN`, `*_SECRET`, `*_KEY`, case-insensitive);
  non-secret `vars.X` changes emit the real old/new values. This
  keeps the envelope debuggable for routine mutations while scrubbing
  credentials.

Verified: shape is byte-identical to round-2 B1's R8 probe except
the values are the `"[REDACTED]"` literal.

---

### Step 11 — `SchedulerWarning::OmittedPriorTask`

Setup: AGENT previously submitted `[A, B, C]`. Scheduler spawned all
three; `coord.A` and `coord.C` are in-flight, `coord.B` is already
complete. AGENT resubmits `[A, C]` (omits B).

KOTO response (valid submission, scheduler tick runs):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "advanced": false,
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [...],
    "errored": [],
    "warnings": [
      {"kind": "omitted_prior_task", "task": "B"}
    ]
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {...}
  }]
}
```

Verify:

- `action: "gate_blocked"` — the submission is VALID. Omission is not
  a rejection per Decision 10. Parent proceeds as if B is still part
  of the batch (it is, via the prior log entries).
- `scheduler.warnings[0].kind == "omitted_prior_task"` — typed
  variant from DESIGN line 3081. `SchedulerWarning` uses
  `#[serde(tag = "kind")]` (line 3065).
- `task: "B"` — the single omitted name.
- Multiple omissions emit multiple warning entries (one per task)
  rather than a single aggregated `tasks: Vec<String>` warning. Lets
  agents pattern-match on individual names without array traversal.
- The warning is informational. Parent state does not change, no
  event is appended beyond the normal `EvidenceSubmitted` +
  `SchedulerRan`. If AGENT genuinely wants to cancel B, they need
  `cancel_tasks` (v1.1 per DESIGN line 2002).

AGENT dispatch can ignore the warning or surface it in a log line.
**No way to silently drop a task.**

---

## Section 2: Decision tree — can an agent dispatch on typed fields only?

Round-3's test: walk all 11 scenarios using ONLY `error.code`,
`error.batch.kind`, `error.batch.reason` (when present),
`error.batch.which`, `scheduler.errored[].kind`,
`scheduler.errored[].compile_error.kind`, and
`scheduler.warnings[].kind` — no free-string parsing of `.message`.

```
match response.action:
  "error" =>
    match response.error.batch.kind:
      "limit_exceeded" =>
        match response.error.batch.which:
          "tasks"         => [Step 1]
          "waits_on"      => [Step 2] (with .task)
          "depth"         => [Step 3]
          "payload_bytes" => [not exercised; expected]
      "concurrent_tick" =>
        # Step 4. Read .holder_pid; retry with backoff.
      "invalid_batch_definition" =>
        match response.error.batch.reason:
          "empty_task_list"          => ...
          "cycle"                    => read .cycle
          "dangling_refs"            => read .entries
          "duplicate_names"          => read .duplicates
          "reserved_name_collision"  => [Step 5] read .reserved
          "invalid_name"             =>
            match response.error.batch.<outer-field>.kind:  # AFTER A3-1 rename
              "empty"          => ...
              "invalid_chars"  => [Step 6] read .pattern
              "too_long"       => read .limit, .actual
          "spawned_task_mutated"     => [Step 10] read .changed_fields
          "trigger_rule_unsupported" => ...
      "invalid_retry_request" =>
        match response.error.batch.reason:
          "unknown_children"          => [Step 9] read .children
          "child_not_eligible"        => read .children[].current_outcome
          "child_is_batch_parent"     => ...
          "empty_child_list"          => ...
          "no_batch_materialized"     => ...
          "mixed_with_other_evidence" => ...
          "retry_already_in_progress" => ...   # reserved; never fires under flock
  "gate_blocked" =>
    for err in response.scheduler.errored:
      match err.kind:
        "template_not_found"      => [Step 8] read .paths_tried
        "template_compile_failed" => [Step 7] read .compile_error.kind
        "collision" | "backend_unavailable" | "permission_denied" | "io_error" => ...
    for warn in response.scheduler.warnings:
      match warn.kind:
        "omitted_prior_task"          => [Step 11] read .task
        "missing_template_source_dir" => ...
        "stale_template_source_dir"   => read .falling_back_to
```

**Every branch lands on a snake_case enum tag.** No `.message` string
scraping in any branch. Closed set of values per enum. AGENT dispatch
is fully typed.

The ONE sharp edge is A3-1 above (outer field name `kind` on
`InvalidName` colliding with the envelope's own `kind` key). Pin the
outer field to `name_rule` (or similar) and the decision tree lands
cleanly.

---

## Section 3: Findings

### Finding A3-1 — `InvalidName { task, kind }` outer field name collision

**Severity: medium.** Round-3 renamed the INNER serde tag on
`InvalidNameDetail` from `detail` to `kind` (DESIGN line 3249). That
eliminates the B1-1 double-nesting problem. But `InvalidBatchReason`
uses `#[serde(tag = "reason")]` and flattens variant fields alongside
the tag, so `InvalidName`'s outer field `kind: InvalidNameDetail`
would serialize at the same JSON object level as the existing envelope
key `kind: "invalid_batch_definition"`. Depending on serde's handling:
either the derive fails at compile time (duplicate key) or one of the
two `kind` keys wins silently. Neither is acceptable.

Proposed resolution: rename the outer field on
`InvalidBatchReason::InvalidName` from `kind: InvalidNameDetail` to
`name_rule: InvalidNameDetail` (or `detail`, or `rule`). Any rename
that doesn't collide with the envelope's `kind` works. The inner tag
stays as `kind` per round-3's cleanup.

### Finding A3-2 — `paths_tried: null` vs. omitted on compile failures

**Severity: low.** `TaskSpawnError.paths_tried` is
`Option<Vec<String>>` but DESIGN line 3090 does not apply
`skip_serializing_if = "Option::is_none"`. So for a
`template_compile_failed` error, the serialized shape is
`"paths_tried": null` rather than an absent key. Minor UX concern:
agents iterating `for p in err.paths_tried` must null-check rather
than default-to-empty. Add `skip_serializing_if` to normalize (every
other optional field in the enum does).

### Finding A3-3 — `holder_pid` on `ConcurrentTick` when holder is on another machine

**Severity: low.** Under `CloudBackend`, a concurrent tick from
another machine cannot report a meaningful local PID. DESIGN line
3208 has `Option<u32>`, which is correct, but the design doesn't spec
what the cross-machine case populates. Proposed: carry `holder_pid:
None` and add an optional sibling `holder_machine: Option<String>`
(the `machine_id` of the other holder) so the agent can distinguish
"retry in 30ms because local contention" from "retry in 30s because a
peer machine is mid-tick." Not a round-3 regression; a gap the
typed-variant refactor exposes.

### Finding A3-4 — Envelope consistency across kinds

Verified: every `error.batch` in this simulation is one of:

```
{ kind: "limit_exceeded", which, limit, actual, task? }
{ kind: "concurrent_tick", holder_pid? }
{ kind: "invalid_batch_definition", reason, ...reason-specific fields }
{ kind: "invalid_retry_request", reason, ...reason-specific fields }
```

Four top-level shapes, each with a fixed tag at `error.batch.kind`.
No free strings at the discriminator positions. No duplicate
representations. **The canonical-path goal holds** modulo A3-1.

### Finding A3-5 — Redaction is field-list-driven, not type-driven

**Severity: informational.** DESIGN commits to the `"[REDACTED]"`
literal but does not enumerate the field-name patterns that trigger
redaction. Implementation freedom; but two agents on the same payload
could disagree on whether `vars.API_KEY` vs. `vars.apiKey` gets
redacted. Not a round-3 regression. Propose a named list in Key
Interfaces: `DEFAULT_REDACT_PATTERNS = ["*_TOKEN", "*_SECRET",
"*_KEY", "*_PASSWORD"]` case-insensitive.

### Finding A3-6 — No path can emit both `scheduler.warnings` and `error.batch`

**Severity: none (confirmation).** Every error response has
`scheduler: null` (DESIGN line 3150). Every successful scheduling
response has `action != "error"`. The two blocks are mutually
exclusive, so an agent never has to reconcile a warning from the
success path with an error from the validation path. Envelope
invariant holds.

### Finding A3-7 — `details[]` stays a free-string array

**Severity: low (carryover).** `error.details: [{field, reason}]` is
still human-readable string pairs (not a typed enum). The typed
information for agent dispatch lives in `error.batch`; `details`
duplicates the signal in a backwards-compatible shape for pre-CD11
consumers. As round-2 Pair B1 noted, this is intentional — don't
break the existing `NextError` struct. Flagging for awareness:
`details[].reason` strings track `error.batch.*` tags but are not
guaranteed to stay in sync variant-for-variant. Agents MUST dispatch
on `error.batch.*`, not on `details[].reason`.

---

## Section 4: Answers to probe questions

**Can an agent pattern-match through all 11 scenarios using only
`kind` fields (top-level + nested), without string-scraping messages?**
Yes, with one caveat: fix A3-1. Once `InvalidName`'s outer field is
renamed away from `kind`, the decision tree in Section 2 covers every
scenario with a closed set of snake_case tags. No `.message` parsing
required. No sentinel values. No string-inside-string escaping.

**What if a response carries BOTH `scheduler.warnings` and a
validation error?** It can't. `action: "error"` responses have
`scheduler: null` (DESIGN line 3150); warnings live inside the
`scheduler` block. The two states are disjoint. Agents implement one
handler per `action` value. **A3-6 confirms the invariant.**

**Is there any path where `error.details[]` carries structured data
rather than human strings?** No. `details[]` is legacy
`NextError.details` shape: `[{field: String, reason: String}]`. All
structured data moves to `error.batch`. This is a deliberate
bifurcation: `details` for legacy consumers, `batch` for CD11-aware
agents. **Don't dispatch on `details`.**

**Does every discriminator use a named enum variant?** Almost.
Exceptions:
- `CompileError.kind` is `String` (DESIGN line 3217), not a Rust enum
  — "short, machine-parseable discriminator" is a norm not a type.
  Values like `yaml_parse` / `missing_field` / `state_reference` are
  listed in the doc but not pinned in a `#[derive]`. **Finding A3-8
  (low):** promote to a typed `CompileErrorKind` enum.
- `ChildEligibility.current_outcome` is `String` (DESIGN line 3301)
  with a comment enumerating the allowed values. Same class as A3-8.

**Is redaction a literal string vs. object?** Literal string. DESIGN
line 1984 pins `"[REDACTED]"`. Verified in Step 10.

---

## Section 5: What round-3 closed vs. what remains

Round-2 B1 findings status under round-3:

| Finding | Status under round 3 |
|---------|----------------------|
| B1-1 `detail: {detail: ...}` double-nesting | **Closed** (inner tag renamed to `kind`) — but new collision flagged as A3-1 |
| B1-2 Dual `LimitExceeded` representation | **Closed** (only `BatchError::LimitExceeded` remains; `InvalidBatchReason::LimitExceeded*` variants deleted) |
| B1-3 `LimitExceeded` missing `task` field | **Closed** (`task: Option<String>` added) |
| B1-4 `TaskSpawnError` compile error is free string | **Closed** (typed `compile_error: Option<CompileError>` added; shape shared with `BatchError::TemplateCompileFailed`) |
| B1-5 `concurrent_tick` is free string | **Closed** (typed `BatchError::ConcurrentTick { holder_pid }`; wire exposes it via `error.batch.kind = "concurrent_tick"`) |
| B1-6 `DanglingRef` struct unspecified | Not addressed; carryover |
| B1-7 prompt vs. design `cycle_path` | Not a design issue |

New round-3 findings (this pair):

| Finding | Severity | Class |
|---------|----------|-------|
| A3-1 `InvalidName.kind` outer/envelope collision | medium | Regression introduced by B1-1 fix |
| A3-2 `paths_tried: null` vs. omitted | low | Consistency |
| A3-3 `holder_pid` cross-machine semantics | low | Gap exposed by typed variant |
| A3-4 Envelope kinds are disjoint | none | Confirmation |
| A3-5 Redaction field-list unspecified | info | Carryover clarification |
| A3-6 warnings + error are mutually exclusive | none | Confirmation |
| A3-7 `details[]` stays a free-string shape | low | Intentional; documented |
| A3-8 `CompileError.kind` / `current_outcome` remain `String` | low | Remaining free-string positions |

Net: round-3 closed the five B1 findings it targeted. One regression
(A3-1) needs one more rename. Three existing free-string positions
(A3-3, A3-8, A3-5) remain as known soft spots; none block AGENT
dispatch, but promoting them to typed enums would make CD11's "typed
discriminators throughout" guarantee airtight.
