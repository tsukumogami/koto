# Error Code Reference

Every error from the koto CLI is a JSON object with an `error` field and a `command` field:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

The `error` field is a human-readable message. The `command` field identifies which subcommand produced the error. Both fields are always present in this flat shape.

Three surfaces use a structured envelope instead, where `error` is an object carrying a machine-readable `code`: `koto next`'s domain errors, the batch-scoped errors, and the whole `koto request` noun group. Those envelopes carry no `command` field — the code identifies the condition, so there's nothing to string-match.

## Error conditions by command

### init

**Workflow already exists** — a session for this name already exists under `~/.koto/sessions/<name>/` (its `koto-<name>.state.jsonl` state file is present):

```json
{"error":"workflow 'my-workflow' already exists","command":"init"}
```

Rename the workflow or delete the existing state file.

**Invalid template** — the template file can't be compiled:

```json
{"error":"failed to parse template: missing required field 'initial_state'","command":"init"}
```

Run `koto template compile <path>` to see the full compilation error.

---

### next

The `next` command has two error paths:

1. **Pre-dispatch I/O errors** use the flat format (`{"error": "...", "command": "next"}`). These fire before the dispatcher runs -- corrupt state files, missing templates, hash mismatches.
2. **Domain errors** use a structured format with a code, message, and optional field-level details. These come from the dispatcher and validation logic.

#### Structured domain errors

Domain errors use this shape:

```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence validation failed",
    "details": [
      {"field": "decision", "reason": "required field missing"}
    ]
  }
}
```

The `details` array is empty when the error isn't field-specific. The thirteen error codes:

| Code | Exit | Meaning |
|------|:----:|---------|
| `gate_blocked` | 1 | One or more command gates failed or timed out. Transient -- may resolve on retry. |
| `integration_unavailable` | 1 | The state declares an integration but no runner is available. Transient. |
| `concurrent_access` | 1 | Another `koto next` invocation is already running on this workflow. Transient -- wait and retry. |
| `invalid_submission` | 2 | The `--with-data` payload is malformed, too large, or fails schema validation. Caller must fix the payload. |
| `precondition_failed` | 2 | A logical precondition wasn't met: `--with-data` and `--to` used together, `--to` targets an invalid state, or the state has no `accepts` block. |
| `terminal_state` | 2 | Evidence was submitted to a terminal state. The workflow is already done. |
| `workflow_not_initialized` | 2 | No state file found for the given workflow name. |
| `template_error` | 3 | A structural template problem: cycle detected, chain limit reached, ambiguous transition, dead-end state, unresolvable transition, or unknown state. |
| `persistence_error` | 3 | A disk I/O failure while reading or writing the state file. |
| `execution_anchor_mismatch` | 2 | The tick ran from a directory that is neither the session's execution anchor nor beneath it. The message names the bound directory. Run `koto next` from there, or rebind the session. |
| `execution_anchor_unresolvable` | 3 | The session's recorded execution anchor names nothing on this machine -- the checkout was deleted or the session moved machines. Rebind the session to where the tree is now. |
| `capture_unset` | 3 | A state's instruction text reads a `capture_stdout_as` name that no state delivered on this run. The message names the value and the state that produces it. |
| `nested_invocation` | 2 | The tick was started from inside a command koto is running. Take the `koto next` call out of the command. |

Exit code 1 means transient -- the agent can retry without changing its behavior. Exit code 2 means the agent must change something (fix the payload, pick a different target, etc.).

#### Exit code mapping

| Exit code | Category | When |
|:---------:|----------|------|
| 0 | Success | Normal response (any variant) |
| 1 | Transient | `gate_blocked`, `integration_unavailable`, engine I/O errors |
| 2 | Caller error | `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized`, `execution_anchor_mismatch`, `nested_invocation` |
| 3 | Infrastructure | Corrupt state file, template hash mismatch, template parse failure, `execution_anchor_unresolvable`, `capture_unset` |

#### Execution anchoring

A session records the directory it was created in and every tick is checked against it. Both refusals below happen before the template is read and before any gate or action closure exists, so neither runs a command, evaluates a gate, or moves the workflow.

**Wrong tree (exit code 2)** -- the tick ran from a directory that is neither the anchor nor beneath it:

```json
{"error":{"code":"execution_anchor_mismatch","message":"workflow 'my-workflow' is bound to /home/dev/repo; `koto next` must run from that directory or one beneath it, not /tmp/elsewhere. Run `koto session rebind my-workflow --to <dir>` if the checkout moved","details":[]}}
```

Standing in a subdirectory of the anchor is fine -- the tick is accepted and its commands still run at the anchor itself, so a command means the same thing from anywhere in the tree.

**Anchor does not resolve (exit code 3)** -- the recorded directory names nothing on this machine:

```json
{"error":{"code":"execution_anchor_unresolvable","message":"workflow 'my-workflow' is bound to /home/dev/repo, which does not resolve on this machine (host-7); run `koto session rebind my-workflow --to <dir>` if the checkout moved","details":[]}}
```

The two codes are separate because the repairs differ: change directory for the first, rebind for the second.

Both messages name `koto session rebind <session> --to <dir>`, which moves the session's anchor to the directory you name (or, with `--to` omitted, to the directory you run it from). It's the only verb that changes an anchor, and it records the move as an `execution_anchor_rebound` event. The wrong-tree refusal can also be repaired by simply running from the anchor; the unresolvable one cannot, so rebinding is its only repair short of restoring the tree. Route on the code rather than the message text.

Anchor comparison is byte-exact over `fs::canonicalize` output. That resolves `.`, `..`, and symlinks and strips trailing slashes, so a symlinked path and a trailing-slash variant of the anchor both satisfy it. It does not case-fold: a path differing only in case names a different directory and is refused, on every platform including case-insensitive filesystems.

Anchoring binds where a session's commands *start*. It does not bound what a command reaches once running -- a command can name absolute paths or change directory, and nothing here stops it.

#### An undelivered capture (exit code 3)

A state can declare a name for its command's output with `capture_stdout_as`, and a later state can read that name in its instruction text. The name is not an init-time variable, so nothing materializes it: if the run reaches the reading state without passing through the state that produces it, there is no value.

```json
{"error":{"code":"capture_unset","message":"state 'report' reads {{BRANCH}}, which state 'detect' delivers with capture_stdout_as; this run has not entered that state, so the value is unset","details":[]}}
```

The tick stops rather than rendering an empty string or the raw `{{BRANCH}}` token, either of which would put a placeholder into an agent's instructions. A name no state declares at all is caught earlier, when the template compiles. The fix is usually in the template: route through the producing state, or move the reference to a state that always follows it.

A failed *delivery* is a different thing and does not use this code. When the command runs but its output cannot be delivered -- it is empty, it exceeds 4096 bytes, or it holds a character the value allowlist forbids -- the tick stops at the state that ran the command, through the ordinary blocked response with an `__action__` condition whose `failure_kind` is `capture_failed`.

#### A tick inside a tick (exit code 2)

`koto next` runs a state's `default_action` and its command gates as child processes, and those children inherit the tick's environment. Before it runs anything, the tick exports `KOTO_TICK_SESSION` naming the session it is advancing. A `koto next` that finds that variable already set was started from inside one of those commands, and refuses:

```json
{"error":{"code":"nested_invocation","message":"koto next cannot run inside a command koto is running: this process inherited KOTO_TICK_SESSION from a tick on session 'my-workflow'. A nested tick advances the workflow while that tick goes on reporting the state it started with, so the caller is told the session is somewhere it has already left. If this is a template's command, remove the `koto next my-workflow` call -- the enclosing tick is what advances the session. If that tick has already exited and this process outlived it (a command that detaches with setsid or backgrounds itself does), clear the marker: `KOTO_TICK_SESSION= koto next my-workflow`.","details":[]}}
```

The refusal is not about wasted work. A nested tick appends to the same event log the outer tick is halfway through processing: it really does advance the session, and the outer tick then finishes against the snapshot it started with and reports a state the workflow has already left. That is a wrong answer rather than a missing one, so nothing else surfaces it.

Two things this does *not* cover. It refuses `koto next` only -- `koto context`, `koto status`, and the rest run from inside a command as before, and reading or writing context on the loop-back edge of a state stays a supported pattern. And it is scoped to the process tree, not to the session name: a tick on some *other* workflow is refused too, because a chain that ticks back into the outer session through a second one lands on the same defect.

`KOTO_TICK_SESSION` is a marker, not an input. Nothing reads it to decide which session to act on.

It also carries no liveness, and the message deliberately does not claim otherwise. koto kills a timed-out command by its process group, so a command that detached itself first -- `setsid`, or a backgrounded subshell -- survives, and keeps the marker for as long as it lives. A `koto next` that process runs after the tick exits is refused in the name of a tick that is already gone. Adding a pid and a liveness probe would trade that for pid-reuse ambiguity, and no shipped template detaches, so the marker stays a plain inherited flag and the way out is documented instead: a blank value counts as absent, so `KOTO_TICK_SESSION= koto next <name>` clears it for one invocation. The refusal message says so itself, because whoever hits this cannot read this page from inside the failing process. Check `koto status` on the session the message names before reaching for it -- inside a command that really is under a live tick, clearing the marker re-opens the defect.

#### Command failure kinds

A command koto runs -- a state's `default_action` or a command gate -- reports *why* it failed under a `failure_kind` key. This is not a `next` error code: the tick answers with an ordinary blocked response rather than an error envelope. It is still the machine-readable discriminator to read, because three of these kinds share `exit_code: -1` and telling them apart by searching stderr for "timed out" is exactly what the key exists to replace.

| Kind | Meaning |
|------|---------|
| `nonzero_exit` | The command ran to completion and exited non-zero. |
| `timed_out` | The command did not finish within its timeout, so its process group was killed. Whatever it wrote before the kill is still reported. |
| `spawn_failed` | No child process was ever started. Also covers an action refused before the spawn: a `working_dir` that is absolute, or one that resolves outside the session's execution anchor. |
| `wait_failed` | The child started but waiting on it failed, so no exit status was ever obtained. |
| `capture_failed` | The command exited zero but its stdout could not be delivered under the state's `capture_stdout_as` name. Action failures only; a gate has nothing to capture. The `capture_error` object alongside it names the case: `empty`, `too_large`, or `disallowed_character`. |

The vocabulary is the same on the two surfaces it appears on. What sits beside it is not.

**On a blocked `koto next` response**, inside the `__action__` blocking condition an action failure produces:

```json
{"name":"__action__","type":"action","status":"failed","category":"corrective","agent_actionable":false,
 "output":{"state":"lint","command":"cargo clippy","failure_kind":"nonzero_exit","exit_code":1,"stdout":"","stderr":"...","truncated":false}}
```

`exit_code` is present here only for `nonzero_exit`. Three of the others never obtained a status at all, and reporting the synthetic `-1` would be the conflation `failure_kind` exists to end; a `capture_failed` command exited zero, so it has no failing status to report either. The condition's `status` narrows the same way: `failed` for `nonzero_exit` and `capture_failed`, `timed_out` for a timeout, `error` for a spawn or wait failure. Route on `failure_kind`, not on `status` -- two kinds share `failed`.

**In command-gate evidence**, on the `output` object of a `gate_evaluated` event and of a recorded override. `exit_code` is always present there, `-1` for the three kinds that never got a status. The key is additive: the passing and plain-failing shapes are unchanged, and the `{"exit_code": -1, "error": "timed_out"}` a timeout has always produced still carries its `error` field, so nothing reading `error` moves.

An action failure that got as far as running writes a `default_action_executed` event before the tick stops. A `working_dir` rejection does not, because no command ran.

#### Pre-dispatch I/O errors

These still use the flat format and aren't domain errors:

**Corrupt state file (exit code 3)** -- the state file exists but can't be parsed. This covers empty files, invalid JSON, and sequence number gaps:

```json
{"error":"state file corrupted: sequence gap at line 4: expected seq 3, got 5","command":"next"}
```

Inspect the file directly. The first line should be a header with `schema_version`, and each subsequent line should be a valid event with a monotonic `seq` number. A truncated final line (e.g., from a crash) is recovered automatically -- only interior corruption triggers this error.

**Template hash mismatch (exit code 3)** -- the compiled template on disk doesn't match the hash recorded at init time:

```json
{"error":"template hash mismatch: header says abc123 but cached template hashes to def456","command":"next"}
```

Reinitialize the workflow to pick up the new template.

**No events in state file** -- the state file has a header but no event lines:

```json
{"error":"state file has no events","command":"next"}
```

---

### rewind

**Corrupt state file (exit code 3)** -- same as `next` above.

**Already at initial state** -- only one state-changing event exists, so there's nothing to rewind to:

```json
{"error":"already at initial state, cannot rewind","command":"rewind"}
```

**Workflow not found:**

```json
{"error":"workflow 'my-workflow' not found","command":"rewind"}
```

---

### template compile

**Compilation failed** — invalid YAML, missing required fields, or unknown gate type:

```json
{"error":"missing required field 'initial_state'","command":"template compile"}
```

#### skip_if diagnostic codes

The `skip_if` field has four compile-time diagnostics. Two are errors (compilation fails) and one is a warning (compilation succeeds with output to stderr).

**E-SKIP-TERMINAL (error)** — `skip_if` is declared on a terminal state. The terminal check fires before `skip_if`, so the auto-advance condition can never match.

```
E-SKIP-TERMINAL: state "done": skip_if cannot be declared on a terminal state; the terminal check fires before skip_if, making it unreachable
  remedy: remove skip_if or make the state non-terminal
```

Fix: remove the `skip_if` field, or remove `terminal: true` from the state.

**E-SKIP-NO-TRANSITIONS (error)** — `skip_if` is declared but the state has no transitions. There's nowhere for the auto-advance to go.

```
E-SKIP-NO-TRANSITIONS: state "check": skip_if requires at least one declared transition
  remedy: add a transition target, or remove skip_if
```

Fix: add at least one transition, or remove `skip_if`.

**E-SKIP-AMBIGUOUS (error)** — When all of a state's transitions are conditional (`when` clauses present on every transition), the `skip_if` values must match exactly one of them. Zero matches or more than one match is an error.

Zero matches:

```
E-SKIP-AMBIGUOUS: state "decide": skip_if values match zero conditional transitions; exactly one must match
  remedy: ensure skip_if values match the when clause of exactly one transition
```

More than one match:

```
E-SKIP-AMBIGUOUS: state "decide": skip_if values match more than one conditional transition ["fast_path", "slow_path"]; exactly one must match
  remedy: refine skip_if values or when clauses so exactly one transition matches
```

Fix: adjust `skip_if` values or the `when` clauses on transitions so exactly one conditional transition matches.

E-SKIP-AMBIGUOUS doesn't apply when the state has a mix of conditional and unconditional transitions. An unconditional transition acts as the fallback, so there's always a valid route.

**W-SKIP-GATE-ABSENT (warning)** — A `skip_if` key of the form `gates.NAME.*` references a gate name that isn't declared on the state. The condition will never match at runtime. Compilation succeeds, but a diagnostic goes to stderr.

```
warning: W-SKIP-GATE-ABSENT: state "check": skip_if key "gates.ci.exit_code" references gate "ci" which is not declared on this state; the condition will be silently unmatchable at runtime
  remedy: declare a gate named "ci" on this state, or correct the key
```

Fix: add the referenced gate name to the state's `gates` block, or correct the `skip_if` key.

---

### template validate

**Schema invalid** — the compiled JSON doesn't match the expected schema:

```json
{"error":"invalid JSON: missing field `format_version`","command":"template validate"}
```

---

## Batch errors

Batch-scoped ticks (parents with `materialize_children`, or `retry_failed` submissions) emit a dedicated envelope with `action: "error"` and a typed `error.batch` field carrying a `BatchError` variant. Each variant uses a snake_case `kind` discriminator so agents can dispatch without string-matching on `message`.

| `batch.kind` | Exit | Meaning |
|---|:---:|---|
| `concurrent_tick` | 1 | Another `koto next` invocation holds the advisory flock on this batch parent. Retryable after backoff. Carries `holder_pid` (may be `null`). |
| `invalid_batch_definition` | 2 | A pre-append structural rule rejected the submission. Carries a nested `InvalidBatchReason` (`empty_task_list`, `cycle`, `dangling_refs`, `duplicate_names`, `spawned_task_mutated`, `invalid_name`, `reserved_name_collision`). |
| `limit_exceeded` | 2 | A pre-append hard limit (R6) was violated. Carries `which` (`tasks`, `waits_on`, `depth`, `payload_bytes`), `limit`, `actual`, and optional `task`. |
| `template_not_found` | 2 | A task's child template path did not resolve against any configured search base. Carries `task`, `path`, `paths_tried`. |
| `template_compile_failed` | 2 | A task's child template was found but failed to compile. Carries `task`, `path`, typed `compile_error`. |
| `backend_error` | 1 or 3 | Backend list/read failed during classification. Exit code 1 when `retryable: true`, else 3. Tick-wide. |
| `spawn_failed` | 3 | Per-task spawn failure after validation passed (`init_state_file` I/O, collision, compile). Carries `task`, `spawn_kind`, `message`. |
| `invalid_retry_request` | 2 | A `retry_failed` submission failed validation. Carries a nested `InvalidRetryReason` with pinned precedence: `unknown_children` → `child_is_batch_parent` → `child_not_eligible` → `mixed_with_other_evidence` → `retry_already_in_progress`. |

Example envelope:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "batch": {
      "kind": "limit_exceeded",
      "which": "tasks",
      "limit": 1000,
      "actual": 1500
    }
  }
}
```

All batch validation runs pre-append — rejected submissions leave no events on the parent's state file.

---

## Request errors

Every subcommand under `koto request` — `create`, `bind`, `get`, `wait`, `list`, `progress`, `resolve`, `abandon`, `abandon-request`, and `close` — reports failure through one nested envelope, the same shape `koto next`'s domain errors use:

```json
{
  "error": {
    "code": "epoch_fence_violation",
    "message": "leg 'review' was bound at dispatch epoch 3; this writer presented 2",
    "details": [
      {"field": "--dispatch-epoch", "reason": "expected 3, got 2"}
    ]
  }
}
```

The code set is closed. A consumer that had to match on `message` to tell "this leg already has a result" from "this request is closed" would have no contract at all, so each condition gets its own code and each code binds to exactly one exit class. `details` is empty when the rejection isn't tied to a specific flag or bound.

| Code | Exit | Meaning |
|------|:----:|---------|
| `wait_timeout` | 1 | `wait` hit its `--timeout-secs` deadline with the predicate still unsatisfied. |
| `wait_interrupted` | 1 | A signal arrived while `wait` was polling. |
| `lock_contention` | 1 | The per-request write lock wasn't acquired within its five-second deadline. Retryable after backoff. |
| `request_not_found` | 2 | No request record exists at that identifier. |
| `leg_not_found` | 2 | The request has no leg by that name. |
| `invalid_identifier` | 2 | A request id, leg name, session id, or coordinator id failed its grammar. Never worth retrying. |
| `invalid_submission` | 2 | A flag payload was malformed, or the flag combination was — `--with-data` together with the `--role`/`--template`/`--inputs` triple, a creation payload with no legs, a duplicate leg name, a value that isn't a JSON object. |
| `contract_mismatch` | 2 | `--cli-contract` named a contract this build doesn't serve. Checked before any read or write, so a mismatch has no side effect. |
| `request_closed` | 2 | A leg mutation, or a second `close`, on a closed request. |
| `leg_already_resolved` | 2 | A second result, or any mutation, on a leg that already answered. |
| `leg_abandoned` | 2 | A mutation on a leg the requester stopped waiting on. |
| `leg_bound_to_different_child` | 2 | A rebind that would point an already-bound leg at a different child. Rebinding to the same child is an idempotent success, not this. |
| `explicit_resolve_on_bound_leg` | 2 | `resolve` on a bound leg. A bound leg's result is promoted from its child's terminal tick; accepting an explicit one here would block the real one. |
| `child_not_found` | 2 | `bind` named a child whose session could not be read. |
| `child_not_fenceable` | 2 | `bind` named a child whose header does not satisfy the dispatch-fence predicate. Binding it would produce a leg that could never be fenced, so the bind is refused instead. |
| `child_bound_to_different_leg` | 2 | `bind` named a child that already points at a different request-and-leg pair. A child fulfils at most one leg, and this is the only place that can be checked — the lock is per-request, so two binds in different requests never serialize against each other. |
| `request_id_collision` | 2 | The generated identifier already had a record on disk. |
| `idempotency_conflict` | 2 | A retry presented a known idempotency hash with a different payload, so it isn't the same logical write. |
| `bound_exceeded` | 2 | One of the bounds below rejected the call. `details` names the dimension. |
| `epoch_fence_violation` | 2 | The presented `--dispatch-epoch` doesn't match the epoch recorded on the leg's bind event, or was omitted on a leg that is bound. Equality is strict, so a future epoch rejects alongside a stale one. |
| `predicate_impossible` | 2 | The `wait` predicate could never hold, caught before polling began — asking for five resolved legs on a three-leg request, for instance. |
| `predicate_became_impossible` | 2 | The predicate stopped being reachable while the wait was running, through abandonment or close. Distinct from a timeout so a caller can tell "not yet" from "never". |
| `persistence_error` | 3 | The filesystem refused, or the log disagrees with itself. |

An unsatisfiable predicate is a caller error rather than a transient one on purpose: telling a shell loop to retry forever on a condition that can never become true is worse than failing it.

`progress` and `resolve` appends carry an idempotency hash derived from the payload, so retrying either after an ambiguous failure is safe — an identical retry short-circuits instead of double-appending, and a payload that differs under a hash already on the log surfaces as `idempotency_conflict` rather than as a phantom second result.

### Bounds behind `bound_exceeded`

Rejecting is chosen over truncating: truncation silently drops the newest information, which is the most valuable, and rolling over would break the log's ordering guarantee.

| Dimension | Limit | Reported in `details` as | Tunable |
|-----------|-------|--------------------------|---------|
| Progress appends per leg | 256 | `progress_appends_per_leg` | `request_store.request_leg_append_cap` |
| Bytes per progress append | 16 KiB | `append_bytes` | fixed |
| Legs per request, at `create` | 256 | `legs_per_request` | `request_store.request_leg_cap` |
| Bytes in any JSON flag value | 1 MiB | the flag name, e.g. `--with-data` | fixed |
| Nesting depth in any JSON flag value | 128 | the flag name, or `json_depth` | fixed |
| Bytes in a stored leg or request `inputs` | 1 MiB | `leg_inputs_bytes`, `request_inputs_bytes` | fixed |
| Bytes in `--rationale` | 4 KiB | `rationale_bytes` | fixed |

The rationale cap is much tighter than koto's 1 MiB rationale precedent elsewhere because this text is prepended to a delegate's directive on every tick until it terminates. A large one is a context-exhaustion problem, not merely a large string. Control characters in an accepted rationale are replaced with spaces and runs of whitespace collapse, so the notice stays one line.

Every bound is checked inside the per-request lock, so none can be raced past, and a rejected `create` leaves no directory behind.

---

## Exit classes

The four classes are the same everywhere koto reports an error, whatever the envelope shape:

| Exit code | Class | What the caller should do |
|:---------:|-------|---------------------------|
| 0 | Success | Nothing. A read that succeeded is a success even when it reports an unfinished request. |
| 1 | Transient | Retry without changing anything, after a backoff. |
| 2 | Caller error | Change something — fix the payload, pick a different target, stop asking for a condition that can't hold. |
| 3 | Infrastructure | Inspect the workspace. Retrying the same call won't help. |

`koto request` uses only these four. The sysexits values that appear elsewhere in the CLI (64, 65, 66, 75) never surface from that group, so a wrapper can treat any status above 3 as coming from somewhere else.
