# koto Error Handling

This file covers exit codes, error shapes, and how to respond to each category of error.

---

## Exit codes

| Code | Meaning | Agent response |
|---|---|---|
| 0 | Success | Parse and act on the JSON output |
| 1 | Transient — retry when the condition resolves | Wait, then retry; or report to user if externally blocked |
| 2 | Caller error — the agent must change behavior | Fix the request; do not retry without changing input |
| 3 | Infrastructure error — corrupted or misconfigured | Report to user; do not retry automatically |

Some subcommands also return sysexits values (64, 65, 66, 75) — see the `NextErrorCode` table below. The `koto request` group never does: it uses 0, 1, 2, and 3 and nothing else, so a wrapper can treat any status above 3 from a request command as coming from somewhere other than koto.

---

## Two distinct error shapes

koto uses different JSON shapes depending on which command failed.

### Shape 1: structured, nested (`koto next` and `koto request`)

`koto next` writes structured error JSON to **stdout** when a domain error occurs. The whole `koto request` group uses the same shape, on stdout, for every rejection:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence failed validation",
    "details": [
      {"field": "status", "reason": "value 'unknown' is not in allowed values"}
    ]
  }
}
```

Fields:
- `error.code` — snake_case error code string (see table below)
- `error.message` — human-readable explanation
- `error.details` — array of per-field errors; always present but may be empty `[]`. On
  `koto next` it is populated only for `invalid_submission`; on `koto request` it names
  the flag or the bound whenever the rejection is tied to one.

Batch-scoped ticks add a typed `error.batch` sibling to this shape (see below). `koto request` envelopes carry no `command` field — the code identifies the condition, so there's nothing to string-match.

### Shape 2: the remaining subcommands (flat)

Every command other than `koto next` and `koto request` writes a flat error JSON to **stderr** on failure:

```json
{"error": "workflow 'my-workflow' already exists", "command": "init"}
```

Fields:
- `error` — human-readable error string
- `command` — the subcommand name that failed

---

## NextErrorCode table

All `koto next` error codes, their exit codes, and what to do:

| `error.code` | Exit | Retryable | Meaning | Agent action |
|---|---|---|---|---|
| `gate_blocked` | 1 | Yes | One or more gates failed; state has no `accepts` block | Wait for the external condition to change, then retry |
| `integration_unavailable` | 1 | Yes | Integration runner is not configured | Report to user; cannot be resolved by the agent alone |
| `concurrent_access` | 1 | Yes | Another `koto next` call is already running | Wait briefly, then retry |
| `invalid_submission` | 2 | No | Evidence failed schema validation | Check `error.details` for per-field reasons; fix the `--with-data` payload |
| `precondition_failed` | 2 | No | Caller violated a precondition | Read the error message; the workflow state must change before retrying |
| `terminal_state` | 2 | No | Workflow is already in a terminal state (done or cancelled) | Stop; start a new workflow if needed |
| `workflow_not_initialized` | 2 | No | Named workflow does not exist | Run `koto init` first, or check the workflow name |
| `execution_anchor_mismatch` | 2 | No | The tick ran from a directory that is neither the session's execution anchor nor beneath it. The message names the bound directory | `cd` to that directory (or a subdirectory of it) and re-run. Nothing ran on the refused tick |
| `template_error` | 3 | No | Template parse failure, hash mismatch, or cycle detected | Report to user; this requires human intervention |
| `persistence_error` | 3 | No | State file I/O failure or corruption | Report to user; this is an infrastructure problem |
| `execution_anchor_unresolvable` | 3 | No | The session's recorded execution anchor names nothing on this machine — the checkout was deleted, or the session moved machines | Restore the checkout at the path the message names, or escalate |
| `capture_unset` | 3 | No | A state reads a `capture_stdout_as` name that no state delivered on this run — in its instruction text, or in its `default_action`'s `command` or `working_dir`. The message names which, the value, and the state that produces it. On the action path the command is never spawned | A template routing problem: the run never entered the producing state. Report it; re-ticking won't help |
| `nested_invocation` | 2 | No | The tick was started from inside a command koto is running. The message names the session whose tick is in flight | Don't tick koto from a template's command. Take the `koto next` call out and let the enclosing tick advance the session |
| `needs_agent_not_dispatched` | 66 | No | `koto next` was called against a `--needs-agent` child that the coordinator has not yet claimed/dispatched | Stop ticking the child directly; route through the coordinator's `koto next` on the parent root instead |
| `recursion_cap_exceeded` | 64 | No | `koto session start --needs-agent` would push the workflow tree past one of the three recursion caps (`depth`, `fanout`, or `total_unassigned`) | Surface the cap dimension and threshold to the user; restructure the dispatch fanout (collapse a level, batch siblings, or split into separate trees) before retrying |

Note: `gate_blocked` and `integration_unavailable` appear both as `error.code` values
(when `koto next` produces an error response) and as `action` values (when `koto next`
produces a successful response). The successful response shape includes `blocking_conditions`
detail; the error shape does not. Check the exit code to distinguish them.

Exit code 66 corresponds to `EX_NOINPUT` from sysexits.h — the session
has no usable input (no template path yet) because it is awaiting
dispatch. This is distinct from `persistence_error` (exit 3, infra
issue) because the fix is an operator-routing one: tick the parent,
not the child.

Exit code 64 corresponds to `EX_USAGE` from sysexits.h — the caller's
spawn request would violate a hard recursion cap and no retry will
help until the dispatch shape changes. The three dimensions are
`depth` (parent chain length), `fanout` (per-parent unclaimed children),
and `total_unassigned` (workspace-wide unclaimed children). The
thresholds are hard-coded constants at V1 — operators cannot raise
them via config. The reserved `[request_store.recursion]` namespace
exists for a future V1.1 promotion to operator-configurable caps, but
at V1 the fix is always structural: restructure the workflow rather
than chase a config override.

---

## Execution anchor refusals

Both anchor codes are checked before the template is read and before any gate or action
closure exists. A refusal therefore means nothing ran, nothing was evaluated, and nothing
moved — you can retry from the right directory without worrying about a half-applied tick.

```json
{"error":{"code":"execution_anchor_mismatch","message":"workflow 'my-workflow' is bound to /home/dev/repo; `koto next` must run from that directory or one beneath it, not /tmp/elsewhere. Run `koto session rebind my-workflow --to <dir>` if the checkout moved","details":[]}}
```

```json
{"error":{"code":"execution_anchor_unresolvable","message":"workflow 'my-workflow' is bound to /home/dev/repo, which does not resolve on this machine (host-7); run `koto session rebind my-workflow --to <dir>` if the checkout moved","details":[]}}
```

The two codes differ because the repair differs: change directory for the first, put the
checkout back or rebind for the second.

`koto session rebind <session> --to <dir>` moves the session's anchor, and `--to` defaults
to the directory you run it from. It's the only verb that changes an anchor and it records
the move as an `execution_anchor_rebound` event. Rebind when the checkout genuinely moved;
for a mismatch on a checkout that did not move, changing directory is the right repair, not
rebinding. Route on `error.code`, not on the message text.

Paths are compared in canonical form, which resolves `.`, `..`, and symlinks and strips
trailing slashes. Comparison never case-folds, on any platform, and containment is
compared component-wise, so `/home/dev/repo-2` is not beneath `/home/dev/repo`. A working
directory that can't be canonicalized at all is compared as given, which fails closed.

A session with no recorded anchor — written before anchoring existed, or created through
`koto session start` — is not refused. Its first tick adopts the directory it's ticked
from and prefixes the `directive` with a one-time notice naming the directory it bound.
Check that the directory is the one you meant.

---

## Nested tick refusals

`koto next` runs a state's `default_action` and its command gates as child processes, and
they inherit its environment. Before it runs anything, a tick exports
`KOTO_TICK_SESSION` naming the session it is advancing. A `koto next` that finds that
variable already set was started from inside one of those commands, and refuses:

```json
{"error":{"code":"nested_invocation","message":"koto next cannot run inside a command koto is running: the tick on session 'my-workflow' spawned this process and has not finished. ...","details":[]}}
```

Exit 2, and nothing ran — no gate evaluated, no action executed, no event appended.

The refusal exists because a nested tick is not merely redundant. It appends to the same
event log the outer tick is halfway through processing, so it really does advance the
session; the outer tick then finishes against the snapshot it started with and reports a
state the workflow has already left. The caller's view is wrong rather than absent, which
is why koto refuses rather than letting it through.

Two things it does not cover. It refuses `koto next` only — `koto context`, `koto status`,
`koto request`, and the rest all work from inside a command as before. And it keys on the
process tree, not the session name, so a tick on some *other* workflow from inside a
command is refused too.

If you hit this, the fix is usually in the template: take the `koto next` out of the
command. The enclosing tick is what advances the session.

### When the tick named in the message is already gone

The marker is a plain inherited environment variable. Nothing behind it checks whether the
tick still exists, and the message does not claim it does.

That matters for one case. koto kills a timed-out command by its process group, and a
command that detached itself first — `setsid`, or a backgrounded subshell — is no longer in
that group, so it survives. It also still carries `KOTO_TICK_SESSION`. A `koto next` such a
process runs minutes later is refused in the name of a tick that exited long ago.

Tell the two apart with `koto status` on the session the message names. If a tick really is
in flight, the refusal is doing its job and the command is what has to change. If nothing is
running, clear the marker for the one invocation:

```sh
KOTO_TICK_SESSION= koto next <name>
```

A blank value counts as absent, which is why this works. Clear it only after checking —
inside a command that genuinely is running under a tick, clearing it re-opens the defect the
refusal exists to stop.

---

## Command failure kinds

`failure_kind` is **not** an error code. When a command koto runs fails — a state's
`default_action` or a `command` gate — the tick answers with an ordinary response and
exit 0, not an error envelope. `failure_kind` is still the machine-readable discriminator
to route on, because three of these kinds share `exit_code: -1` and telling them apart by
searching stderr for "timed out" is what the key exists to replace.

| Kind | Meaning |
|---|---|
| `nonzero_exit` | The command ran to completion and exited non-zero. |
| `timed_out` | The command did not finish within its timeout, so its process group was killed. Whatever it wrote before the kill is still reported. |
| `spawn_failed` | No child process was ever started. Also covers an action refused before the spawn: a `working_dir` that is absolute, or one that resolves outside the session's execution anchor. |
| `wait_failed` | The child started but waiting on it failed, so no exit status was ever obtained. |
| `capture_failed` | The command exited zero but its stdout could not be delivered under the state's `capture_stdout_as` name. Action failures only; a gate has nothing to capture. The `capture_error` object alongside it names the case: `empty`, `too_large`, or `disallowed_character`. |

The vocabulary is the same on both surfaces it appears on. What sits beside it is not:

- **In the `__action__` blocking condition** of a `gate_blocked` response, `exit_code` is
  present **only** for `nonzero_exit`. The others never obtained a status, and a
  `capture_failed` command exited zero. The condition's `status` narrows the same way —
  `failed` for `nonzero_exit` and `capture_failed`, `timed_out` for a timeout, `error` for
  a spawn or wait failure — which is why you route on `failure_kind` rather than `status`.
- **In command-gate evidence** (a `gate_evaluated` event's `output`, and a recorded
  override's), `exit_code` is always present, `-1` for the three kinds that never got a
  status. The key is additive: the passing and plain-failing shapes are unchanged, and a
  timeout still carries its `{"error": "timed_out"}`.

See `response-shapes.md` scenario (k) for the full failed-action response.

---

## Request command errors

Every subcommand under `koto request` reports failure through the nested envelope, with a code from a closed set. Each code binds to exactly one exit class, so you never have to read `message` to decide what to do.

```json
{
  "error": {
    "code": "epoch_fence_violation",
    "message": "leg 'review' was bound at dispatch epoch 3; this writer presented 2",
    "details": [{"field": "--dispatch-epoch", "reason": "expected 3, got 2"}]
  }
}
```

`details` is empty when the rejection isn't tied to a specific flag or bound.

| `error.code` | Exit | Meaning | Agent action |
|---|---|---|---|
| `wait_timeout` | 1 | `wait` hit `--timeout-secs` with the predicate still unsatisfied | Retry the wait, or go do something else and come back |
| `wait_interrupted` | 1 | A signal arrived while polling | Retry if you still want the answer |
| `lock_contention` | 1 | The per-request write lock wasn't acquired within five seconds | Back off and retry the same call |
| `request_not_found` | 2 | No request record at that identifier | Check the id; `koto request list` if you've lost it |
| `leg_not_found` | 2 | The request has no leg by that name | Read the leg names from `koto request get` |
| `invalid_identifier` | 2 | A request id, leg name, session id, or coordinator id failed its grammar | Fix the identifier; never worth retrying |
| `invalid_submission` | 2 | A flag payload was malformed, or the combination was — `--with-data` together with the flat triple, a creation payload with no legs, a duplicate leg name, a value that isn't a JSON object | Fix the payload |
| `contract_mismatch` | 2 | `--cli-contract` named a contract this build doesn't serve | Drop the pin or match the build. Checked before any I/O, so nothing happened |
| `request_closed` | 2 | A leg mutation, or a second `close`, on a closed request | Stop writing to it |
| `leg_already_resolved` | 2 | A second result, or any mutation, on a leg that already answered | Read the existing result instead |
| `leg_abandoned` | 2 | A mutation on a leg the requester stopped waiting on | Wind down; see the abandonment notice |
| `leg_bound_to_different_child` | 2 | A rebind that would point an already-bound leg at a different child | Rebinding to the *same* child is an idempotent success, not this |
| `child_not_found` | 2 | `bind` named a child whose session couldn't be read | Start the child first, or fix the session id |
| `child_not_fenceable` | 2 | `bind` named a child that isn't a `--needs-agent` child of a parent | Start it with `koto session start --parent <parent> --needs-agent ...` |
| `child_bound_to_different_leg` | 2 | That child already fulfils another leg | A child fulfils at most one leg. Start another child |
| `explicit_resolve_on_bound_leg` | 2 | `resolve` on a bound leg | Don't. The child's terminal tick promotes the result |
| `request_id_collision` | 2 | The generated id already had a record on disk | Retry `create`; a fresh id is minted |
| `idempotency_conflict` | 2 | A retry presented a known hash with a different payload | Decide which write you meant; this isn't the same logical write |
| `bound_exceeded` | 2 | One of the bounds below rejected the call; `details` names the dimension | Send less |
| `epoch_fence_violation` | 2 | The presented `--dispatch-epoch` isn't the epoch recorded on the leg's bind event, or was omitted on a bound leg | Present the epoch baked into your spawn. Equality is strict, so a future epoch fails alongside a stale one |
| `predicate_impossible` | 2 | The `wait` predicate could never hold, caught before polling | Ask for something reachable |
| `predicate_became_impossible` | 2 | The predicate stopped being reachable while waiting, through abandonment or close | Distinct from a timeout on purpose: this is "never", not "not yet" |
| `persistence_error` | 3 | The filesystem refused, or the log disagrees with itself | Report to the user; retrying won't help |

An unsatisfiable predicate is a caller error rather than a transient one on purpose — telling a shell loop to retry forever on a condition that can never become true is worse than failing it.

### Bounds behind `bound_exceeded`

Rejecting is chosen over truncating, since truncation silently drops the newest information.

| Dimension | Limit | Reported in `details` as |
|---|---|---|
| Progress appends per leg | 256 (operator-tunable) | `progress_appends_per_leg` |
| Bytes per progress append | 16 KiB | `append_bytes` |
| Legs per request, at `create` | 256 (operator-tunable) | `legs_per_request` |
| Bytes in any JSON flag value | 1 MiB | the flag name, e.g. `--with-data` |
| Nesting depth in any JSON flag value | 128 | the flag name, or `json_depth` |
| Bytes in a stored leg or request `inputs` | 1 MiB | `leg_inputs_bytes`, `request_inputs_bytes` |
| Bytes in `--rationale` | 4 KiB | `rationale_bytes` |

The rationale cap is much tighter than koto's 1 MiB rationale precedent elsewhere because that text reaches a delegate's response on every tick until it terminates. Every bound is checked inside the per-request lock, so none can be raced past, and a rejected `create` leaves no directory behind.

---

## Handling agent_actionable: false

When `koto next` returns `action: "gate_blocked"` or `action: "evidence_required"` with
a non-empty `blocking_conditions`, check each item's `agent_actionable` field.

**When `agent_actionable: true`:**
The gate has a configured `override_default` value or a built-in default for its type.
The agent can call `koto overrides record` to record an override and unblock the gate:

```
koto overrides record my-workflow --gate ci_check --rationale "verified manually"
```

After recording the override, the next `koto next` call treats that gate as passed.

**When `agent_actionable: false`:**
The gate has no override default. The agent cannot resolve this condition. The right
response is to surface the blocking condition to the user with enough context for them
to act:

- Quote the gate name and its `output` field from `blocking_conditions`
- Explain what the gate checks (from the `directive` text and gate `type`)
- Wait for the user to resolve the external condition before calling `koto next` again

Do not poll `koto next` in a loop when `agent_actionable: false`. The condition is
externally controlled and will not change without user action.

---

## invalid_submission — reading per-field errors

When `error.code` is `invalid_submission` (exit 2), the `error.details` array contains
one entry per field that failed validation:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence failed validation",
    "details": [
      {"field": "status", "reason": "value 'done' is not in allowed values [completed, override]"},
      {"field": "priority", "reason": "unknown field"}
    ]
  }
}
```

Fix each field according to its `reason`, then resubmit with `koto next --with-data`.
The `expects.fields` from the previous successful `koto next` call shows the schema.
If you no longer have that output, call `koto next <name>` without `--with-data` to
get the current state's `expects` schema again.

---

## terminal_state after cancel

After `koto cancel`, all subsequent `koto next` calls return exit 2 with
`error.code = "terminal_state"`. This is expected and not a bug. Use `koto workflows`
to confirm the workflow is no longer listed as active (it will not appear after cleanup).

---

## Checking for errors in shell

Since exit code 0 means success across all subcommands, the simplest check is:

```sh
output=$(koto next my-workflow)
if [ $? -ne 0 ]; then
  echo "koto next failed: $output" >&2
  exit 1
fi
action=$(echo "$output" | jq -r '.action')
```

For `koto next`, parse both the exit code and the `action` field. Exit 0 with
`action: "gate_blocked"` is a successful response indicating a blocked state — it is
not an error. Exit 1 from `koto next` means the `error.code` field explains why.

---

## Typed error envelope (batch tick errors)

Batch-scoped ticks (a parent state with `materialize_children`, or a `retry_failed` submission) can fail with a structured envelope that sits alongside the six domain codes above. The wire shape uses a dedicated `action: "error"` variant with a sibling `error.batch` field:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "message": "...",
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": {"reason": "duplicate_names", "duplicates": ["task-1"]}
    }
  }
}
```

`error.batch` carries a typed `BatchError` variant — agents can dispatch on `batch.kind` instead of string-matching on `message`. All variants use snake_case discriminators.

### Top-level enum families

| Family | Shape | Purpose |
|---|---|---|
| `BatchError` | `{kind, ...}` under `error.batch` | Top-level variant — one of `concurrent_tick`, `invalid_batch_definition`, `limit_exceeded`, `template_not_found`, `template_compile_failed`, `backend_error`, `spawn_failed`, `invalid_retry_request` |
| `InvalidBatchReason` | nested under `invalid_batch_definition.reason` | Structural rejection: `empty_task_list`, `cycle`, `dangling_refs`, `duplicate_names`, `spawned_task_mutated`, `invalid_name`, `reserved_name_collision` |
| `InvalidRetryReason` | nested under `invalid_retry_request.reason` | Retry-submission rejection: `no_batch_materialized`, `empty_child_list`, `child_not_eligible`, `unknown_children`, `child_is_batch_parent`, `retry_already_in_progress`, `mixed_with_other_evidence`, `multiple_reasons` |
| `LimitKind` | under `limit_exceeded.which` | Hard limit that tripped: `tasks`, `waits_on`, `depth`, `payload_bytes` |
| `SpawnErrorKind` | under `spawn_failed.spawn_kind` | Per-task scheduler spawn error classification |
| `CompileErrorKind` | under `template_compile_failed.compile_error` | Typed child-template compile failure |
| `ChildOutcome` | under `child_not_eligible.children[*].current_outcome` | Retryability classification — `failure`, `skipped`, `spawn_failed`, `pending`, `success`, `blocked` |

### InvalidRetryReason precedence

When a `retry_failed` submission violates more than one rule, the engine aggregates them into `multiple_reasons` ordered by this pinned precedence:

1. `unknown_children`
2. `child_is_batch_parent`
3. `child_not_eligible`
4. `mixed_with_other_evidence`
5. `retry_already_in_progress`

`no_batch_materialized` and `empty_child_list` short-circuit before aggregation. The precedence is stable across releases so agents can dispatch on the first reason.

### R0-R9 pre-append validation (summary)

The scheduler runs ten runtime rules on every task-list submission **before** appending any event — rejected submissions leave zero state on the parent's event log. A one-line summary per rule:

| Rule | Summary |
|---|---|
| R0 | Task list is non-empty. |
| R1 | Per-task: child template resolvable and compilable (failures become `spawn_failed`). |
| R2 | Per-task: `vars` resolve against the child template (failures become `spawn_failed`). |
| R3 | `waits_on` graph is a DAG — no cycles. Rejects the whole submission. |
| R4 | No dangling `waits_on` references to names absent from the submission. |
| R5 | Task names are unique within the submission. |
| R6 | Hard limits: `tasks.len() <= 1000`, `waits_on.len() <= 10` per task, DAG depth `<= 50`, payload `<= 1 MB`. |
| R7 | No collision with existing sibling children (enforced at init via `renameat2`). |
| R8 | Spawn-time immutability: for already-spawned tasks, submitted `template` / `vars` / `waits_on` must match the recorded `spawn_entry`. |
| R9 | Task name matches `^[A-Za-z0-9_-]+$`, 1-64 chars, not in the reserved set (`retry_failed`, `cancel_tasks`). |

See [batch-workflows.md](batch-workflows.md) for how the runner dispatches on each rejection, and `docs/designs/current/DESIGN-batch-child-spawning.md` in the koto repository for the full rule definitions and rationale.

---

For the complete error taxonomy and exit code reference, see `docs/guides/cli-usage.md`.
