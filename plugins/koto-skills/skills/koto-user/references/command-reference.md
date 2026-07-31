# koto Command Reference (Workflow Runner)

This file covers the subcommands a workflow-running agent uses. Template authoring
commands (`koto template compile`, `koto template validate`, `koto template export`)
are documented in the koto-author skill instead; brief entries appear here only to
orient agents who encounter them.

Subcommands confirmed from `src/cli/mod.rs`:

| Subcommand | Audience |
|---|---|
| `koto init` | Runner — primary |
| `koto next` | Runner — primary |
| `koto cancel` | Runner — primary |
| `koto rewind` | Runner — primary |
| `koto workflows` | Runner — primary |
| `koto overrides record` | Runner — primary |
| `koto overrides list` | Runner — primary |
| `koto decisions record` | Runner — primary |
| `koto decisions list` | Runner — primary |
| `koto session dir` | Runner — primary |
| `koto session list` | Runner — primary |
| `koto session cleanup` | Runner — primary |
| `koto session resolve` | Runner — cloud backend only |
| `koto status` | Runner — primary |
| `koto request create/bind/get/wait/list` | Runner — coordinator |
| `koto request progress/resolve/abandon` | Runner — coordinator and delegate |
| `koto request abandon-request/close` | Runner — coordinator |
| `koto context add` | Runner — primary |
| `koto context get` | Runner — primary |
| `koto context exists` | Runner — primary |
| `koto context list` | Runner — primary |
| `koto config get/set/unset/list` | Runner — setup only |
| `koto template compile` | Author (brief mention) |
| `koto template validate` | Author only |
| `koto template export` | Author only |
| `koto version` | Utility |

---

## koto init

```
koto init <name> --template <path> [--parent <parent-name>] [--var KEY=VALUE ...]
koto init <name> --from-stdin [--var KEY=VALUE ...]
```

Initializes a new workflow. Provide the definition one of two ways:

- `--template <path>` — compile a durable template source file (the standard path).
- `--from-stdin` — pipe an inline workflow definition on standard input and run it in one invocation, with no scratch file to manage. Use this for a novel, one-off task you've decomposed yourself; for a workflow you'll author repeatedly, write a durable template and use `--template`.

| Argument/Flag | Required | Description |
|---|---|---|
| `<name>` | Yes | Workflow name. Must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`. Dots, underscores, and hyphens are allowed after the first character. |
| `--template <path>` | One of `--template` / `--from-stdin` | Path to the template `.md` source file. Compiled automatically on first use. |
| `--from-stdin` | One of `--template` / `--from-stdin` | Read the workflow definition from stdin, strict-compile it into the session directory, and start the session. **Mutually exclusive** with `--template`. **Rejects** `--allow-legacy-gates` (the inline path is strict-only). Does not support `--parent`. |
| `--parent <parent-name>` | No | Link this workflow as a child of an existing parent workflow. Fails if the parent doesn't exist. Not available with `--from-stdin`. |
| `--var KEY=VALUE` | No | Set a template variable. Repeatable. Required variables must be supplied; unknown keys are rejected. VALUE is checked against an allowlist (see Notes). |

**Success output:**
```json
{"name": "my-workflow", "state": "initial_state_name"}
```

**Error cases:**
- Exit 2: name format violation, workflow already exists, missing required variable, unknown variable; passing both `--from-stdin` and `--template`; passing `--allow-legacy-gates` with `--from-stdin`.
- Exit 3: template parse failure, template hash mismatch (file path).
- Exit 1: a `--from-stdin` definition that fails strict validation. No session is created, the process exits non-zero, and the error **names the failing element** (state / transition / gate) — for example `state "start" references undefined transition target "nowhere"` — so you can correct the definition and re-pipe it.

**Notes:**
- `--var` values are validated against an allowlist because a substituted `{{KEY}}` can land in a gate command (run via `sh -c`) or an agent instruction. Allowed characters: letters, digits, `. _ - /`, `:`, `@`, and spaces. This covers structured data values such as Gmail filters (`newer_than:90d`, `from:user@example.com`) and names with spaces (a calendar title). Shell metacharacters -- `;` `|` `&` `$` `(` `)` `<` `>` `*` `?`, quotes, backticks, and newlines -- are **rejected** so a value cannot inject a command. A space is allowed but is not shell-quoted for you: when a value may contain spaces, quote the reference in the template (e.g. `--calendar "{{CALENDAR}}"`) so it stays a single argument. An empty value (an optional variable left unset, or one with an empty default) is safe unquoted: `--flag {{VAR}}` renders `--flag ''` rather than dropping the token, so the next flag isn't consumed as the value.
- Reserved variable names `SESSION_DIR` and `SESSION_NAME` cannot be declared in templates. They are injected automatically.
- If a `--template` source uses legacy-mode gates (no `gates.*` when-clause routing), `koto init` emits a warning to **stderr** and still succeeds. The `--from-stdin` path is strict: a legacy gate is **rejected**, naming the offending state and gate.
- `--from-stdin` writes both the compiled artifact and the human-readable source into the session directory (the source under a fixed filename), so the workflow survives global cache eviction and the authored definition stays recoverable for audit. Do not embed secrets in the definition; reference `$VAR` / files read at gate-evaluation time instead.

---

## koto next

```
koto next <name>
koto next <name> --with-data '<json>'
koto next <name> --to <state>
koto next <name> [--no-cleanup] [--full]
```

Gets the current state directive. Submits evidence when `--with-data` is provided. Forces a directed transition when `--to` is provided.

| Flag | Description |
|---|---|
| `--with-data <json>` | Submit evidence as a JSON object. Must conform to the state's `accepts` schema. Max 1 MB. The `"gates"` key is reserved and rejected. Mutually exclusive with `--to`. Prefix with `@` to read the payload from a file (e.g. `--with-data @evidence.json`); the file is also capped at 1 MB. |
| `--to <state>` | Force a directed transition to a named state. Must be a valid transition target from the current state. Mutually exclusive with `--with-data`. |
| `--no-cleanup` | Skip automatic session cleanup when the workflow reaches a terminal state. Useful for debugging artifacts after a workflow ends. |
| `--full` | Always include the `details` field, even on repeat visits to a state. By default `details` is omitted after the first visit. |
| `--dispatch-epoch <n>` | The epoch this writer was dispatched with. Required for `--with-data` writes against a child workflow's log; validated before any persistence call and rejected with `epoch_fence_violation` (exit 65) on a mismatch. Parent-workflow ticks don't need it. The same value goes on `koto request progress` / `resolve` / `abandon` for a bound leg. |

**Output shapes** are determined by the `action` field. See `response-shapes.md` for all nine annotated scenarios.

**Classification priority (highest wins):**
1. Terminal state → `action: "done"`
2. Gate(s) failed + no `accepts` block → `action: "gate_blocked"`
3. Gate(s) failed + `accepts` block present → `action: "evidence_required"` with non-empty `blocking_conditions`
4. Integration declared → `action: "integration_unavailable"` (or `"integration"` when available)
5. `accepts` block present → `action: "evidence_required"`
6. Fallback (no `accepts`, no integration, not terminal) → `action: "evidence_required"` with empty `expects.fields`

**Error output:** structured `NextError` JSON on stderr (see `error-handling.md`).

### Converging a fan-out (reading child results)

When a coordinator has fanned work out to children (a state with a
`materialize_children` hook), `koto next <parent>` is also the converge point.
There is no separate converge command and no new response shape — convergence
rides the existing `children-complete` gate and the `gate_blocked` directive.

**Blocked until results are in.** While any non-skipped child has not yet
produced a result, `koto next <parent>` returns `action: "gate_blocked"` and the
parent is **not** advanced past the state. The gate output lives at
`blocking_conditions[0].output` and carries three converge fields alongside the
existing batch aggregate:

| Field | Meaning |
|---|---|
| `results_in` | `true` once every non-skipped child's result is available to read. |
| `converge_blocked` | `true` while the batch is terminal-complete but at least one non-skipped child's result is still missing. This is the converge-specific block — distinct from "children still running". |
| `outstanding` | Array naming the children still missing a result, by their fan-out identity (`<parent>.<task>`). Empty once `results_in` is `true`. |

The `children-complete` blocking condition is in the `temporal` (retry-later)
category: re-tick `koto next <parent>` after the named `outstanding` children
finish. A `skipped` child never appears in `outstanding` — it carries a
synthesized skipped-status default result and does not hold the gate.

**Cleared: results inline, no child log opened.** When the last result lands,
`results_in` becomes `true`, the gate passes, the parent advances, and the
cleared directive carries every child's result inline. Each entry in the gate
output's `children[]` array gains a `result` object:

```json
{
  "children": [
    {
      "name": "coord.task-1", "outcome": "success",
      "state": "done", "complete": true,
      "result": {
        "status": "success",
        "summary": "implemented and tested the parser change",
        "payload": { "files_changed": 3 }
      }
    }
  ]
}
```

The coordinator reads each child's `status` / `summary` / optional `payload`
straight from its own directive — it never runs `koto query` or `koto next`
against a child to learn what the child produced. `status` is one of `success`
/ `failure` / `skipped` (the same classification as the child's terminal
outcome); `summary` is always present (a default is synthesized when the child's
terminal state declares no summary field); `payload` is omitted when the child
recorded none.

A child that is itself a coordinator converges its own children through this
same gate and then carries its own result up to its parent identically — the
read is uniform at every depth.

**This stays true when the child also fills a request leg.** The leg records the
same result — koto promotes it from the child's terminal tick — but that record
exists for progress, partial fan-out state, cross-session reads, and restart
recovery. The coordinator holding the gate reads the answer from its own
directive as it always has. Polling `koto request get` for a result your own
gate is about to hand you means never advancing.

### Fields a request leg adds

| Field | When it appears |
|---|---|
| `leg` | On every advance-loop response for a session bound to a leg, `done` included. Carries `request_id` and `leg_name`, never `dispatch_epoch`. Absent on the `koto next --to <state>` directed-transition path. |
| `leg_abandoned` | On the same responses, once the requester abandoned the leg. Carries `request_id`, `leg_name`, and the verbatim `rationale`. Also absent on the `--to` path. |

When a leg is abandoned, koto also prepends a stop notice to `directive` — see
the skill's "When the requester stops waiting". The notice adds no `action`
value and changes no `blocking_conditions`.

---

## koto cancel

```
koto cancel <name> [--cleanup]
```

Marks a workflow as cancelled, preventing any further advancement.

**Success output:**
```json
{"name": "my-workflow", "state": "current_state", "cancelled": true, "cleaned_up": false}
```

**Flags:**
- `--cleanup` — also remove the session directory after writing the cancel event. `cleaned_up: true` in the response. Use this when you want to reuse the workflow name immediately (e.g., restart during development). Without `--cleanup`, the session stays on disk so the history remains auditable.

**After cancellation:**
- `koto next` returns exit 2 with `error.code = "terminal_state"`.
- A second `koto cancel` call returns exit 2.
- Without `--cleanup`, the session directory is preserved. Use `koto session cleanup <name>` separately to remove it (or pass `--cleanup` up front).

**Error cases:**
- Exit 2: already cancelled, workflow already in a terminal state
- Exit 3: cancel event was written but the subsequent cleanup failed (rare; filesystem error)

---

## koto rewind

```
koto rewind <name>
```

Rolls back the workflow to the previous state by appending a `Rewound` event. Non-destructive — the event log is preserved.

**Batch parent behavior:** When rewinding past a state with a `materialize_children` hook, the rewind relocates all spawned children to a superseded branch. Children are renamed from `<parent>.<task>` to `<parent>~N.<task>` (where N is the epoch counter), freeing the names for a fresh batch submission. Superseded children remain fully queryable via standard commands (`koto status <parent>~N.<task>`, `koto workflows --children <parent>~N`).

**Success output:**
```json
{"name": "my-workflow", "state": "previous_state_name", "superseded_branch": "my-workflow~1", "children_relocated": 3}
```

The `superseded_branch` and `children_relocated` fields appear only when a batch rewind occurs. For non-batch rewinds they are null/0.

**After batch rewind:** `koto status <parent>` includes a `superseded_branches` array listing all prior branch names. Use `koto workflows --children <parent>~N` to inspect children from any prior attempt.

**Error cases:**
- Exit 1: already at the initial state (cannot rewind further)

---

## koto workflows

```
koto workflows [--roots] [--children <name>] [--orphaned]
```

Lists active workflows in the current directory as a JSON array.

| Flag | Description |
|---|---|
| `--roots` | Show only workflows with no parent (top-level workflows) |
| `--children <name>` | Show only children of the named parent workflow |
| `--orphaned` | Show only workflows whose parent no longer exists |

Flags are mutually exclusive. When none are provided, all workflows are listed.

**Success output:**
```json
[
  {
    "name": "my-workflow",
    "created_at": "2026-01-15T10:30:00Z",
    "template_hash": "abc123...",
    "parent_workflow": null
  }
]
```

The `parent_workflow` field is `null` for parentless workflows and a string with the parent's name for children. Returns `[]` when no workflows exist or no workflows match the filter. The `name` field is the workflow identifier (not `id` — that field name appears only in `koto session list`).

---

## koto status

```
koto status <name>
```

Returns read-only state metadata for a workflow. No gates are evaluated, no actions run, no state changes happen. Useful for checking child workflow progress from a parent agent.

| Argument | Required | Description |
|---|---|---|
| `<name>` | Yes | Workflow name |

**Success output:**
```json
{
  "name": "design.research-agent",
  "current_state": "synthesize",
  "template_path": ".koto/research.template.json",
  "template_hash": "a1b2c3...",
  "is_terminal": false
}
```

A session bound to a request leg also carries a read-only `leg` section mirroring what the session's own `koto next` returns:

```json
{
  "name": "review-443",
  "current_state": "review",
  "is_terminal": false,
  "leg": {"request_id": "req-9f1c2b7a-4d3e-4c5f-8a1b-2c3d4e5f6071", "leg_name": "review"}
}
```

The key is absent when the session isn't bound to a leg. It never carries `dispatch_epoch` — a readable epoch would let a displaced agent present the current value and defeat the fence.

**Error cases:**
- Exit 2: workflow not found

---

## koto request

Ten subcommands over the request store — the durable record of what a coordinator asked for and what came back. See the skill's "Requests and legs" section for when to use it; this section is the flag surface.

```
koto request create   [--with-data '{"legs":[…],"inputs":{…}}' | --role R --template T --inputs J]
                      --requested-by ID --coordinator-of-record ID
koto request bind     <request-id> <leg> --child SESSION_ID [--dispatch-epoch N] [--issued-by ID]
koto request get      <request-id>
koto request wait     <request-id> (--leg NAME | --all-legs | --closed | --resolved-count N)
                      --timeout-secs N [--interval-secs N]
koto request list     [--requested-by ID | --coordinator-of-record ID] [--state open|closed] [--unresolved-legs]
koto request progress <request-id> <leg> --with-data J [--dispatch-epoch N] [--issued-by ID]
koto request resolve  <request-id> <leg> --with-data J [--dispatch-epoch N] [--issued-by ID]
koto request abandon  <request-id> <leg> --rationale TEXT [--dispatch-epoch N] [--issued-by ID]
koto request abandon-request <request-id> --rationale TEXT [--issued-by ID]
koto request close    <request-id> [--issued-by ID]
```

`--cli-contract MAJOR.MINOR` is accepted on every subcommand and validated before any I/O, so a mismatch has no side effect. This build serves `1.0`; an older minor is served, a newer minor or a different major is refused.

Output is JSON on stdout unconditionally — there is no format flag.

### The response envelope

Every subcommand except `list` prints the same object:

```json
{
  "request_id": "req-9f1c2b7a-4d3e-4c5f-8a1b-2c3d4e5f6071",
  "request_state": "open",
  "close_disposition": null,
  "leg_counts": {"total": 2, "open": 1, "resolved": 1, "abandoned": 0},
  "revision": 7,
  "requested_by": "coord-root",
  "coordinator_of_record": "coord-root",
  "created_at": "2026-07-14T09:12:04.318Z",
  "inputs": {"repo": "koto"},
  "legs": {
    "review": {
      "name": "review",
      "declaration": {"role": "reviewer", "template": "review.md", "inputs": {"pr": 443}},
      "disposition": "resolved",
      "bound_child": "review-443",
      "bound_epoch": 0,
      "result": {"status": "success", "summary": "no blocking findings", "payload": {}},
      "result_source": "promoted",
      "abandoned_rationale": null,
      "progress": [
        {"seq": 5, "timestamp": "2026-07-14T09:20:00Z", "content": {"note": "parser rewritten"}, "issued_by": "review-443"}
      ]
    }
  },
  "written": true,
  "cli_contract": {"major": 1, "minor": 0}
}
```

| Field | Notes |
|---|---|
| `request_state` | `open` or `closed`. |
| `close_disposition` | `all_resolved`, `closed_with_abandoned_legs`, or `request_abandoned`. Always present; `null` while open, so you never distinguish "absent" from "not yet closed". |
| `leg_counts` | `total`, `open`, `resolved`, `abandoned`. Derived, not stored. |
| `revision` | Sequence number of the last event on the log. After a write, this is how you tell your own append from a concurrent one. |
| `inputs` | The request-level shared context recorded at `create`. Omitted when none was supplied. |
| `legs` | Keyed by leg name, in name order — that ordering is what makes two `get` calls byte-equal. |
| `legs[*].disposition` | `open`, `resolved`, or `abandoned`. |
| `legs[*].result_source` | `promoted` (from the bound child's terminal tick) or `explicit` (recorded through `resolve`). |
| `written` | Present on mutating verbs only, so you can tell a real append from a no-op success — a rebind to the child the leg already has, a re-abandon, or a retry the idempotency hash recognized. Absent on reads, which keeps `get` byte-stable. |
| `cli_contract` | Two integers, never a string — `"1.10"` sorts below `"1.9"` as a string, and that mistake is unavailable here. |

`list` prints `{"requests": [...], "cli_contract": {...}}`, where each row carries `request_id`, `requested_by`, `coordinator_of_record`, `created_at`, `request_state`, and `leg_counts`. It advances no coordinator cursor and writes nothing.

### create

`--with-data` takes `{"legs":[{"name","role","template","inputs"}],"inputs":{...}}`; the per-leg `inputs` is that leg's own ask, the top-level `inputs` is context shared by the whole request. Unknown keys are rejected. The flat `--role` / `--template` / `--inputs` triple is the one-leg shorthand and names the leg after the role; it is mutually exclusive with `--with-data`, and all three are required together.

`--requested-by` and `--coordinator-of-record` are both required. Attribution, not authorization — anything running as the same user can read any request it can name.

The generated id is `req-` plus a v4 UUID, printed in the envelope. Creation is a single atomic write, so a crash never leaves a request with a header and no log.

### bind

Three checks run against the child before the append: the child's session must be readable (`child_not_found`), its header must satisfy the dispatch fence — a `--parent` child started with `--needs-agent` (`child_not_fenceable`) — and it must not already point at a different request-and-leg pair (`child_bound_to_different_leg`). Rebinding the same leg to the same child succeeds and reports `written: false`.

The epoch recorded on the bind event is read from the child's header, not from your flag; passing `--dispatch-epoch` here asserts a value and is rejected if it disagrees. After the bind is durable, koto writes a pointer into the child's session directory so the child can read its own leg back. A failed pointer write warns and does not fail the bind — re-run `bind` to repair it.

### wait

Exactly one predicate is required, enforced at parse time. `--timeout-secs` is required; a wait with no deadline is a hang. `--interval-secs` defaults to 2 and is clamped to a floor of 1 so zero cannot spin. The deadline is absolute and computed once, and the sleep runs in 100 ms slices so a signal is noticed promptly.

The wait reads through the same path `get` uses and writes nothing. A count predicate is satisfied by a partial fan-out — it does not require every leg.

### progress, resolve, abandon

All three take `--dispatch-epoch`, validated against the epoch recorded on the leg's bind event rather than against the child's header, so the fence still works after the child's session is cleaned up. Equality is strict: a future epoch is rejected alongside a stale one. On an unbound leg the flag is accepted and ignored, so a wrapper that always passes it keeps working.

`progress` and `resolve` carry an idempotency hash derived from the payload, so retrying either after an ambiguous failure short-circuits instead of double-appending. A retry whose payload differs from one already on the log surfaces as `idempotency_conflict`.

`resolve` takes a result envelope: `{"status":"success|failure|skipped","summary":"...","payload":{...}}`. It applies only to a leg with no bound child — see the skill's "Recording a result explicitly".

### abandon-request and close

`abandon-request` abandons every open leg and then closes the request with the `request_abandoned` disposition. It is a separate subcommand rather than `abandon` with an optional leg argument, so an unset shell variable fails argument parsing instead of escalating. `--rationale` is required on both abandon forms, capped at 4 KiB, with control characters replaced and whitespace runs collapsed — this text reaches a delegate's response.

`close` derives its disposition inside the lock from the legs' state, distinguishing all-resolved from closed-with-abandoned-legs. Closing an already-closed request is rejected.

`koto cancel` is unrelated and unchanged: no request operation is reachable from it.

---

## koto overrides record

```
koto overrides record <name> --gate <gate> --rationale <text> [--with-data '<json>']
```

Records an override for a blocked gate so that the next `koto next` call treats the gate as passed.

| Argument/Flag | Required | Description |
|---|---|---|
| `<name>` | Yes | Workflow name |
| `--gate <gate>` | Yes | Name of the gate to override. Must exist in the current template state. |
| `--rationale <text>` | Yes | Explanation for why the override is appropriate. Max 1 MB. |
| `--with-data '<json>'` | No | Override value to substitute as gate output. If omitted, falls back to the gate's `override_default` value, then to the built-in default for the gate type. Fails if no default is available. Supports the `@file` prefix to read the value from a file (e.g. `--with-data @override.json`), with the same 1 MB cap as `koto next`. |

**Override value resolution order:**
1. `--with-data` value
2. Gate's `override_default` (from the template)
3. Built-in default for the gate type (e.g., `{"exit_code": 0, "error": ""}` for `command` gates)
4. Error if none of the above apply

**Success output:**
```json
{"status": "recorded"}
```

**When to use:** only when `blocking_conditions[].agent_actionable` is `true`. When `agent_actionable` is `false`, the blocking condition is externally controlled and cannot be resolved by the agent.

**Error cases:**
- Exit 2: gate not found in current state, no override value available, workflow not found
- Exit 3: template hash mismatch, corrupt state file

---

## koto overrides list

```
koto overrides list <name>
```

Lists all override history for a workflow across all states.

**Success output:**
```json
{
  "state": "current_state",
  "overrides": {
    "count": 1,
    "items": [
      {
        "state": "state_where_override_was_recorded",
        "gate": "gate_name",
        "rationale": "explanation",
        "override_applied": {"exit_code": 0, "error": ""},
        "actual_output": {"exit_code": 1, "error": ""},
        "timestamp": "2026-01-15T10:30:00Z"
      }
    ]
  }
}
```

`actual_output` is the gate's real evaluation result at override time. It is `null` when no gate evaluation event was recorded for that gate. Unlike `koto decisions list`, overrides are **not** scoped to the current epoch — all override history is returned.

---

## koto decisions record

```
koto decisions record <name> --with-data '<json>'
```

Records a structured decision without advancing state.

The `--with-data` JSON must be an object with:
- `choice` (string, required)
- `rationale` (string, required)
- `alternatives_considered` (array of strings, optional)

Supports the `@file` prefix to read the payload from a file (e.g. `--with-data @decision.json`), with the same 1 MB cap as `koto next`.

**Success output:**
```json
{"state": "current_state", "decisions_recorded": 2}
```

`decisions_recorded` is the running count of decisions for the current epoch after this record.

**Error cases:**
- Exit 2: missing `choice` or `rationale` field (message: `"missing required field"`)

---

## koto decisions list

```
koto decisions list <name>
```

Lists decisions recorded in the current epoch.

**Success output:**
```json
{
  "state": "current_state",
  "decisions": {
    "count": 2,
    "items": [
      {"choice": "implement", "rationale": "tests pass"},
      {"choice": "skip", "rationale": "out of scope", "alternatives_considered": ["defer", "remove"]}
    ]
  }
}
```

Returns decisions for the current epoch only. Decisions recorded before the last state reset (rewind) are excluded. After `koto rewind`, `decisions.count` resets to 0.

---

## koto session dir

```
koto session dir <name>
```

Prints the absolute path of the session directory to stdout (plain text, not JSON).

```
/home/user/.koto/sessions/a1b2c3d4e5f6a7b8/my-workflow
```

The path always ends with the session name as the last component. This is the same path that `{{SESSION_DIR}}` resolves to in gate commands and state directives. Always exits 0 — the path is computed from the name, not read from disk.

---

## koto session list

```
koto session list
```

Lists all sessions as a JSON array, sorted alphabetically by `id`.

**Success output:**
```json
[
  {
    "id": "my-workflow",
    "created_at": "2026-01-15T10:30:00Z",
    "template_hash": "abc123..."
  }
]
```

Note: this command uses `id` where `koto workflows` uses `name`. Both refer to the same session identifier. Returns `[]` when no sessions exist.

---

## koto session cleanup

```
koto session cleanup <name>
```

Removes the entire session directory for the named workflow. Idempotent — succeeds even if the session does not exist. Produces no stdout output.

Under normal operation, `koto next` auto-cleans on terminal state unless `--no-cleanup` was passed. Use this command for manual cleanup after abandoned workflows or after using `--no-cleanup` during debugging.

---

## koto session resolve (cloud backend only)

```
koto session resolve <name> --keep <local|remote> [--children <auto|skip|accept-remote|accept-local>]
```

Resolves a version conflict when using the cloud session backend. Only valid when `session.backend = "cloud"` is configured; fails with an error on the local backend.

`--children` (default `auto`) controls how the parent's direct children reconcile alongside the parent log:

| Value | Behavior |
|---|---|
| `auto` | Apply the strict-prefix rule per child: if one side is a byte-prefix of the other, the longer side wins. Divergent logs surface as a `conflict` entry and the command exits non-zero so the caller runs `koto session resolve <child>` on each flagged child. |
| `skip` | Leave child state files untouched. The parent reconciles alone. |
| `accept-remote` | Unconditionally overwrite local child state with remote. |
| `accept-local` | Unconditionally overwrite remote child state with local. |

Response shape (cloud backend only — `sync_status` and `machine_id` are elided under the local backend, which rejects the command anyway):

```json
{
  "name": "parent",
  "keep": "remote",
  "children_policy": "auto",
  "sync_status": "fresh",
  "machine_id": "a1b2c3d4",
  "children": [
    {"name": "parent.task-1", "action": "identical"},
    {"name": "parent.task-2", "action": "accepted_remote"},
    {"name": "parent.task-3", "action": "conflict"},
    {"name": "parent.task-4", "action": "errored", "message": "remote state unreachable: ..."}
  ]
}
```

Per-child `action` values:

| Value | Meaning |
|---|---|
| `identical` | Local and remote bytes matched; nothing touched. |
| `accepted_local` | Local was pushed to remote (either by strict-prefix rule under `auto` or by the explicit `accept-local` policy). |
| `accepted_remote` | Remote was pulled to local (either by strict-prefix rule under `auto` or by the explicit `accept-remote` policy). |
| `skipped` | `skip` policy was applied — neither side was touched. |
| `conflict` | Both sides diverged under `auto`. Run `koto session resolve <child>` on this child. |
| `errored` | A per-child I/O or network failure prevented reconciliation. Sibling children still processed. The `message` field explains the specific failure. This includes the case where remote S3 was unreachable under `auto` — `auto` refuses to overwrite remote when it cannot confirm the remote state, so a transient fetch failure surfaces here rather than silently applying `accepted_local`. |

---

## koto context add

```
koto context add <session> <key> [--from-file <path>]
echo "content" | koto context add <session> <key>
```

Stores content under `<key>` in the session's context store. When `--from-file` is absent, reads from stdin. Overwrites any existing content at that key.

Keys are hierarchical path strings (e.g., `scope.md`, `research/r1/lead.md`). Keys must not start with `.` or contain `..`.

No stdout on success. Exit 3 on infrastructure errors.

---

## koto context get

```
koto context get <session> <key> [--to-file <path>]
```

Retrieves stored content and writes it to stdout, or to `--to-file` if specified. When writing to a file, parent directories are created automatically.

Exit 3 if the key does not exist or an I/O error occurs.

---

## koto context exists

```
koto context exists <session> <key>
```

Checks whether a key exists in the session's context store.

**Exit-code-as-boolean contract:**
- Exit 0 — key is present
- Exit 1 — key is absent

No stdout output. No JSON error on exit 1. This differs from all other context commands, which produce JSON errors on failure. The design is intentional for shell conditional use:

```sh
if koto context exists my-workflow scope.md; then
  koto context get my-workflow scope.md | process_scope
fi
```

This command is also usable directly in `gates: command:` entries in templates.

---

## koto context list

```
koto context list <session> [--prefix <prefix>]
```

Lists all context keys as a JSON array sorted alphabetically.

```json
["alpha.md", "beta.md", "research/r1/lead.md"]
```

`--prefix <prefix>` filters to keys that start with the given prefix. Returns `[]` when no keys exist or no keys match.

---

## koto config

Configuration commands are primarily used during environment setup. Most agents running on the default local backend need no configuration.

```
koto config get <key>            # Print the value of a dotted key; exit 1 if unset
koto config set <key> <value>    # Write to project config (.koto/config.toml)
koto config set <key> <value> --user   # Write to user config (~/.koto/config.toml)
koto config unset <key>          # Remove from project config
koto config unset <key> --user   # Remove from user config
koto config list                 # Print merged config as TOML
koto config list --json          # Print merged config as JSON
```

Valid key paths: `session.backend`, `session.cloud.endpoint`, `session.cloud.bucket`, `session.cloud.region`, `session.cloud.access_key`, `session.cloud.secret_key`, `workflows.native`.

---

## koto template compile (brief)

```
koto template compile <source> [--allow-legacy-gates]
```

Compiles a template source file to a cached JSON file and prints the cache path. `koto init` runs this automatically — you don't need to call it directly. The `--allow-legacy-gates` flag suppresses the D5 error for templates without `gates.*` routing. Full documentation is in the koto-author skill.

---

## Variable substitution

Two variable tokens are available in all gate commands and state directives at runtime without any declaration:

| Token | Value |
|---|---|
| `{{SESSION_DIR}}` | Absolute path to the workflow's session directory |
| `{{SESSION_NAME}}` | The workflow name passed to `koto init` |

User-defined variables declared in the template's `variables:` block and supplied via `koto init --var` use the same `{{KEY}}` syntax. Substitution is non-recursive.

---

For topics not covered here, see `docs/guides/cli-usage.md`.
