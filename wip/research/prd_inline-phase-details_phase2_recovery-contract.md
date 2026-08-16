# Lead: the read-only recovery contract

## Findings

### 1. The payload

`NextResponse` (`src/cli/next_types.rs:63-127`) has six variants. The non-terminal ones
(`EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`,
`ActionRequiresConfirmation`) share a common shape: `state`, `directive`, `details:
Option<String>`, `advanced: bool`, plus a variant-specific tail (`expects`,
`blocking_conditions`, `action_output`/`integration`), and every variant carries
`unassigned_children: Vec<UnassignedChild>`.

Field-by-field judgment for what a context-lost agent resuming at the current phase needs:

- **`directive`** — required. This is the instruction text itself; it's the entire point
  of the call. `koto status` never returns it today.
- **`details`** — required when present. Same field, softer content (background/context
  prose). `koto next --full` forces it to always populate (bypassing the "already seen
  it once" suppression at `src/cli/mod.rs:4001-4015`); a recovery call has the same
  justification `--full` does — the agent has no history of having seen it, so the
  suppression heuristic doesn't apply. Recovery should behave like `--full`, not like a
  normal re-entry.
- **`state`** — required. A recovering agent doesn't know where it is; this is the
  anchor fact everything else is relative to. `koto status` already returns this as
  `current_state`.
- **`expects`** — required when the state accepts evidence. It's the schema the agent
  must satisfy to move forward; without it a recovering agent can reconstruct the
  directive's prose intent but not the exact machine-checked shape it must submit.
  Reachable read-only: `derive_expects` (`next_types.rs:774`) is a pure function of
  `TemplateState`, no I/O, already used identically by the live `dispatch_next` path.
- **`blocking_conditions`** — situational, but should be included when derivable.
  `koto next` computes this from **live** gate evaluation (`StopReason::GateBlocked` /
  `EvidenceRequired{failed_gates}` in `advance.rs`, consumed at `mod.rs:4046-4076` via
  `blocking_conditions_from_gates`). A read-only path cannot re-run gates (side effect,
  see §3) but doesn't have to go empty-handed: `derive_last_gate_evaluated`
  (`src/engine/persistence.rs:844-875`) reconstructs the most recent `GateEvaluated`
  event's output per gate, scoped to the current epoch (since the last transition into
  this state), purely from the event log. This is exactly what the existing read-only
  dashboard uses (`src/cli/dashboard_data.rs:690-723`) to show gate PASS/FAIL without
  executing anything. A recovery call can and should surface **last-known** blocking
  conditions this way, explicitly labeled as possibly-stale (reflects the last `koto
  next` tick, not a fresh evaluation).
- **`action`** (the envelope discriminant: `"evidence_required"`, `"gate_blocked"`, etc.)
  — required. It's how the agent knows which of the response shapes it's looking at and
  therefore what's expected of it next. A recovery call needs its own discriminant
  regardless of whether it reuses `NextResponse`'s variant names, since it is a distinct
  action from `next` (see below).
- **`advanced`** — not needed, and actively wrong to include as true/false in the
  `koto next` sense. `advanced` answers "did calling `next` just move the workflow." A
  read-only recovery call by definition never advances anything, so this field would
  either always be `false` (uninformative) or misleadingly imply the call has the same
  semantics as `next`. Better to omit it, or replace it with something honest about the
  call itself (e.g. omit entirely — the response's own identity, e.g. a
  `read_only: true` marker or simply the command name, communicates it without
  overloading a field whose meaning is "a transition happened").
- **`unassigned_children`** — situational, not required for the core recovery case.
  It's populated by `discovery::scan`, which is **not** read-only (see §3, §4) — it
  performs a persistent cursor write and rate-gated compaction as GC side effects. A
  recovery call could still report unassigned children read-only by doing the *filter*
  logic without advancing the cursor (i.e. re-derive candidates from the seen-set state
  as of the last cursor write, or simply omit this field and point the agent back at a
  live `koto next` tick for a fresh scan). Given the field's entire purpose is
  coordinator dispatch bookkeeping, not phase resumption, it is reasonable to leave it
  out of the recovery payload rather than build a parallel non-mutating scan path.
- **`error` / `batch`** — not applicable; recovery has its own error surface (§6), not
  the `next`-specific gate/evidence/batch error taxonomy.

**Verdict on framing**: the right answer is *not* "the same payload minus the side
effects." It's a narrower, purpose-built payload: `state`, `directive` (always
substituted, always present, `--full`-style unsuppressed `details`), `expects` (when
applicable), and *last-known* `blocking_conditions` (explicitly stale-labeled, derived
from the log rather than executed). `advanced` is dropped as meaningless in this
context; `unassigned_children` is out of scope because its only source mutates state.
The action-confirmation and integration variants' extra fields (`action_output`,
`integration`) describe the *outcome of running something* — a recovery call, which
runs nothing, has no analog for them and should not try to reconstruct "what the last
action's stdout was" from the `DefaultActionExecuted` event log entry, since that's
history, not current instruction. (It could optionally be exposed as a distinct,
clearly-historical field if a later design phase wants it, but it isn't part of the
core "what do I do now" payload.)

### 2. Variable substitution

`with_substituted_directive` (`next_types.rs:159-251`) maps a closure `Fn(&str) ->
String` over `directive` and `details` (`Option::map`), returning variants unchanged
for `Terminal` and `Error` (no directive to substitute). The call site
(`mod.rs:3357-3360`, mirrored at `mod.rs:4199` in the directed-transition path) always
composes two layers in order:

```rust
let d = crate::cli::vars::substitute_vars(d, &runtime_vars);   // {{SESSION_DIR}}, {{SESSION_NAME}}
variables.substitute(&d)                                        // template-declared {{VAR}}s
```

Both substitution inputs are reachable from a purely read-only path:

- `runtime_vars` (`mod.rs:3266-3271`) is built from `backend.session_dir(&name)` and
  `name` — two values available to any handler that has the session name and a backend
  handle, no event reads required.
- `variables` comes from `Variables::from_events(&events)` (`substitute.rs:60-75`),
  which scans the already-loaded event log for the `WorkflowInitialized` event's
  `variables` map and re-validates each value against the allowlist regex
  (`VALUE_PATTERN`, `substitute.rs:29`). This is a pure function over data `koto status`
  already reads (`backend.read_events`); it performs no I/O of its own beyond that.

So: yes, a read-only path can fully reach both substitution inputs, and it must — a
recovery call that skipped substitution would hand back literal `{{SESSION_DIR}}` /
`{{VAR}}` tokens, which is strictly worse than what `koto next` gives on first arrival.
The recovery contract should call `with_substituted_directive` with the identical
two-layer closure `koto next` uses, so recovered text is byte-identical to what the
agent would have seen the first time.

### 3. The non-effects

Enumerating every side effect in `handle_next` (`mod.rs:2892-4230`, the unix path;
`mod.rs:4510` is a stub for non-unix that just errors) and `advance_until_stop`
(`engine/advance.rs`), each with its trigger condition:

1. **Startup GC/compaction sweep, unconditional on every invocation**
   (`mod.rs:2948-3038`): `gc_stale_cursors`, `recover_stale_compact_lock`,
   `maybe_compact_terminal_index`, and the wake-candidates pass
   (`crate::engine::wake::wake_candidates_pass`). These run *before* any workflow-name
   validation and can append `RequesterWoken` events to *other* sessions' logs, release
   claim sidecars, and rewrite the terminal index file. None of this touches the target
   workflow's own state file, but it is real cross-session mutation triggered simply by
   invoking the command.
2. **Event log appends** — several distinct triggers:
   - `DirectedTransition` event on `koto next --to <state>` (`mod.rs:3341`,
     unconditional on that code path).
   - `DecisionRecorded` (separate command, not `next` proper, but same handler family;
     `mod.rs:4737`).
   - `GateEvaluated`, one per gate evaluated with no override, emitted inside
     `advance_until_stop`'s gate-evaluation branch (`advance.rs:382`, guarded per-gate
     by "no event for overridden gates" at `advance.rs:364`).
   - `DefaultActionExecuted`, emitted **unconditionally whenever a default action runs**
     (`mod.rs:3937-3945`), regardless of `requires_confirmation` — the event append
     happens before the confirmation branch, so even the "requires confirmation, don't
     auto-advance further" case has already appended the event and already run the
     shell command.
   - The main state-transition `Transitioned` event, appended by `advance_until_stop`'s
     core loop on every successful transition resolution.
   - `RequestStoreResult`/terminal-index/parent-notification appends inside
     `finish_terminal_tick` (`mod.rs:2530-2589`) when the loop stops at a terminal
     state.
3. **Gate evaluation runs shell commands.** `evaluate_command_gate`
   (`src/gate.rs:206-246`) calls `run_shell_command(&gate.command, working_dir,
   gate.timeout)` — a real subprocess spawn (`gate.rs:1-3`: "Command gates spawn shell
   commands in isolated process groups"). This fires for every non-overridden gate on
   the current state as part of `advance_until_stop`'s classification step, which is
   exactly the step that decides `GateBlocked` vs `EvidenceRequired` vs falling through
   — i.e., gate execution is not optional overhead, it's load-bearing for which response
   variant `koto next` returns.
4. **`default_action` execution.** The `action_closure`
   (`mod.rs:3875-3959`) runs `crate::action::run_shell_command(&command, &wd, 30)` (or
   the polling variant, which loops gate evaluation too) whenever the state has a
   `default_action` and no override evidence exists. This is a second, independent
   subprocess-spawning side effect distinct from gate evaluation.
5. **State transitions.** `advance_until_stop` mutates `current_state` in its own loop
   variable and persists each hop via the injected `append_event` closure — this is the
   whole point of `next`, but it's explicitly the thing a read-only recovery call must
   never do even by accident (e.g. via an auto-advancing `skip_if`).
6. **Terminal session cleanup.** `finish_terminal_tick` (`mod.rs:2530-2589`), called
   only when the advance loop's final stop reason is `Terminal`
   (`mod.rs:3383-3395`), can: append a `request_store.result` line to the child's own
   log, promote a leg result onto the parent request record
   (`promote_leg_result`, real cross-file mutation under `~/.koto/`), append to the
   terminal index, append `ChildCompleted` to the parent's log, and finally call
   `backend.cleanup(name)` — which deletes the session directory outright. This is the
   most destructive effect in scope: a read-only recovery call at a terminal state must
   never trigger it.
7. **Request-store writes.** Beyond the terminal-tick promotion above, the discovery
   scan (next point) and the wake-candidates pass (point 1) both write into the
   `~/.koto/` request-store tree independent of the target workflow's own log.
8. **The discovery scan mutates a cursor file.** `crate::engine::discovery::scan`
   (`src/engine/discovery.rs:333`, called at `mod.rs:4025-4038`) is documented as
   walking `~/.koto/sessions/*` against a per-coordinator cursor at
   `~/.koto/coordinators/<coord_id>/scan_cursor.toml`, and explicitly states "After a
   fresh-rescan the next cursor write captures the current scan state" — the scan is
   not read-only; every tick that runs it can advance the cursor, changing what future
   scans consider "already seen." This directly caps what a recovery call can safely
   reuse for `unassigned_children` (see §1).
9. **Lock acquisition.** Only for batch-scoped parent states, `handle_next` takes a
   non-blocking `flock` via `backend.lock_state_file` (`mod.rs:3768-3776`), held for
   the rest of the tick — see §4 for the full locking picture, including why this one
   *is* an effect worth naming (it's exclusive, even if brief) even though it isn't a
   write to the state file itself.
10. **Config-driven warnings and request-store cascade resolution**
    (`mod.rs:2925-2933`): `load_config()` + `warn_if_request_store_recursion_reserved`
    — no persistent effect, but does touch the filesystem (config file reads) and can
    write to stderr; worth naming as "reads config" even though it isn't a mutation, in
    case config resolution has its own I/O cost or failure mode a recovery call would
    want to avoid depending on.

**What a recovery call's contract must therefore explicitly forbid, as checkable
claims**: no event append of any type to any session's log; no shell command execution
(neither gate commands nor `default_action` commands); no state transition (the
returned `state` must equal the workflow's `current_state` on entry, always); no
`backend.cleanup` call under any stop condition; no discovery-scan cursor write; no
request-store `record_result`/`RequestLegBound`-family writes; no exclusive lock
acquisition (contrast with `koto status`, which the code comment at `mod.rs:4830-4832`
already states explicitly: "Read-only: does not evaluate gates, run actions, or modify
the state file" — the recovery call's contract should be that same sentence, extended
to also promise no cursor/request-store mutation and no cleanup, since `koto status`
doesn't reach those code paths in the first place and a recovery call that adds
`directive`/`details`/`expects` might tempt an implementer into reusing more of
`handle_next` than is safe).

### 4. Locking and concurrency

`koto status` takes **no lock** — `handle_status` never calls
`backend.lock_state_file`; it only calls `backend.read_events`
(`mod.rs:4845`), and `read_events` is documented at the one place that matters
(`persistence.rs:312`, in `append_event_idempotent`'s own doc comment) as: "concurrent
readers via `read_events` do NOT take a lock and are unaffected (advisory)."

`koto next` takes a lock **conditionally, not universally**:
- For **non-batch-scoped** states (the common case), `handle_next` acquires no lock at
  all before appending events. `append_event` (`persistence.rs:139`) itself performs no
  locking — it opens the file with `O_APPEND` and writes, relying on the append mode's
  atomicity for the write itself, not on any flock.
- For **batch-scoped** parent states only, `handle_next` acquires a **non-blocking**
  `flock(LOCK_EX | LOCK_NB)` via `backend.lock_state_file` (`mod.rs:3768-3776`,
  implemented at `src/session/local.rs:386`), held for the rest of the tick via RAII
  drop. On contention it returns immediately as `SessionError::Locked { holder_pid }`,
  which `handle_next` maps to `BatchError::ConcurrentTick` (not the generic
  `NextErrorCode::ConcurrentAccess`) — non-blocking, fails fast.
- Separately, `append_event_idempotent` (`persistence.rs:292-390`, used for the
  idempotent evidence-submission path, not applicable to gate/action mutation events)
  takes a **blocking** `flock(LOCK_EX)` with no `LOCK_NB`
  (`persistence.rs:419`, `acquire_state_flock`) — this is the call the parent task
  brief's reference to "issue #171 about `LOCK_EX` being blocking" points at. It can
  stall indefinitely if a holder never releases (e.g. a killed process leaves the flock
  released by the kernel on fd close, so this is bounded by process death, but not by
  any application-level timeout).

**What this means for a recovery call racing a predecessor**: since `koto status`-style
reads already run lock-free against `read_events`, a recovery call built the same way
(read-only, no `append_event`/`append_event_idempotent`/`lock_state_file` calls)
inherits that same lock-free safety — it can run concurrently with an in-flight `koto
next` on the same workflow without blocking or being blocked, and without risking a
torn read, because `read_events` already tolerates a concurrently-appending writer (the
existing tolerance for "a truncated final line... recovered automatically" per
`docs/reference/error-codes.md`'s corrupt-state-file section covers the crash case; an
in-progress append is the same shape of read-time hazard). The recovery call should
**not** attempt to acquire any lock, including a non-blocking one — doing so would be
new behavior beyond what `koto status` does today, and the respawned-agent race
scenario in the task brief (a predecessor may still be mid-tick) is exactly the case
where blocking on a lock would defeat the point of a recovery path meant to be usable
"while the workflow is busy."

### 5. Discoverability

Every `NextResponse` variant's `Serialize` impl (`next_types.rs:360-538`) always emits
`action`, `state`, `advanced`, `expects` (possibly `null`), and `error` (possibly
`null`) — the only fields present on literally every response regardless of variant, by
construction of the custom `Serialize`. `unassigned_children` is present on five of six
variants (`Terminal` has it too, actually — checking: yes, `Terminal`'s serializer at
`next_types.rs:458-471` includes it). `directive` and `details` are present on every
variant **except** `Terminal` and `Error`. So the fields that survive on literally every
response, terminal and error included, are: `action`, `state`, `advanced`, `expects`,
`error`. Of these, only `state` and `action` carry human-legible content that could ride
a pointer; `expects`/`error` are typed/null in the common case and `advanced` is a bare
bool.

**The existing precedent** is the leg-abandonment stop notice. `AbandonedLeg`
(`mod.rs:2699-2745`) is koto-authored text spliced into `directive` via
`NextResponse::with_directive_prefix` (`next_types.rs:268-357`), called unconditionally
whenever `discover_abandoned_leg` finds the bound delegate's leg has been abandoned
(`mod.rs:3374-3380`). The mechanism:

- `with_directive_prefix(prefix: &str)` prepends `prefix` to `directive` only,
  leaving `details` untouched — deliberately, per the doc comment (`next_types.rs:253-267`):
  "the abandonment notice belongs in `directive` and nowhere else: `directive` is the
  one field the agent-facing skill declares authoritative."
- It runs **after** variable substitution (`mod.rs:3357-3380`), specifically so the
  koto-authored prefix text is never itself subject to `{{...}}` expansion — the
  comment at `mod.rs:3361-3368` explains this ordering is load-bearing because the
  substitution helper does a sequential replace, so text substituted early could be
  rescanned by a later key.
- It explicitly does **not** cover `Terminal` or `Error` (`next_types.rs:230,232,354-355`
  return those variants unchanged), which the doc comment (`next_types.rs:264-267`)
  acknowledges as a real coverage gap: "That coverage gap is why the notice also rides
  an envelope sibling (DESIGN-request-lifecycle.md Decision 4)" — i.e. there's a second,
  structured channel (`sibling()`, `mod.rs:2713-2719`) carrying the same information for
  consumers that don't read prose, precisely because the prose channel can't reach every
  variant.

**Model for a phase-info pointer**: the same `directive`-prefix splice mechanism is the
natural fit — a short koto-authored sentence prepended to `directive` on every response
(non-terminal, non-error) telling the agent that if it ever loses this text, `koto
phase-info <name>` (or whatever the recovery command is named) will hand it back. Given
the same coverage gap applies (`Terminal`/`Error` have no `directive` to prepend to),
the design should either (a) accept that a context-loss recovery pointer is
unnecessary at a terminal/error state (there's nothing left to resume), or (b) follow
the existing pattern's own answer and put the pointer on the structured envelope sibling
too — in this case, that's simpler than the abandonment case since there's no
`Terminal`-only Vec every response already carries; `unassigned_children` is present on
`Terminal` too, so a small always-present structured field (not `unassigned_children`,
which has a fixed unrelated shape) is the closest match if prose-on-every-variant isn't
achievable.

### 6. Errors

Grounding each in `docs/reference/error-codes.md` and `handle_status`'s actual code
(`mod.rs:4834-4870`):

- **Workflow does not exist.** `koto status` returns the flat format
  `{"error": "workflow '<name>' not found", "command": "status"}` at exit code 2
  (`mod.rs:4836-4842`). `koto next`'s analogous case is a *structured* domain error,
  `workflow_not_initialized` (exit 2) — see the error-codes table. Recovery should match
  whichever convention it's positioned closer to: since it's a read-only sibling of
  `status`, not a mutating sibling of `next`, the flat `{"error": ..., "command":
  "phase-info"}` shape at exit 2 is the more consistent choice, matching `status`'s
  existing behavior for the identical condition rather than introducing a new
  structured-error surface for a command that (per this doc's own taxonomy: "Three
  surfaces use a structured envelope... `koto next`'s domain errors, the batch-scoped
  errors, and... `koto request`") isn't one of those three surfaces.
- **Session is corrupt.** `handle_status` distinguishes two corruption shapes, both flat
  format:
  - `backend.read_events` failing outright maps through `exit_code_for_engine_error`
    (`mod.rs:4847-4856`) — this is the same path that produces the documented
    "state file corrupted: sequence gap..." (exit 3) and "template hash mismatch" (exit
    3) messages from the error-codes reference's `next` section (the underlying engine
    errors are shared between `status` and `next`, only the `"command"` field differs).
  - `derive_machine_state` returning `None` — `"corrupt state file: cannot derive
    current state"` at `EXIT_INFRASTRUCTURE` (`mod.rs:4859-4870`), for the case where
    events parse individually but don't reduce to a valid current state.
  Recovery should reuse both paths verbatim (same underlying event log, same derivation
  functions), just with `"command": "phase-info"` in the flat envelope.
- **State has no `details`.** This isn't an error at all in either `status` or `next` —
  `details` is `Option<String>`, `None` when `template_state.details.is_empty()`
  (`mod.rs:4001-4003`, mirrored in `dispatch_next`, `next.rs:50-54`), and the
  `Serialize` impls simply omit the JSON key when `None`
  (`map.serialize_entry("details", d)` only inside `if let Some(d) = details`). Recovery
  should do the same: omit the field, not synthesize an error or a placeholder.
- **Workflow is at a terminal state.** `koto status` already handles this as a normal,
  successful response — it returns `is_terminal: true` alongside the usual fields
  (`mod.rs:4898-4909`), not an error. `koto next`, by contrast, treats terminal
  *evidence submission* as an error (`terminal_state`, exit 2, "Evidence was submitted
  to a terminal state. The workflow is already done") — but that error is about
  *submitting evidence* to a done workflow, not about *querying* one. A read-only
  recovery call has no evidence to submit, so the `next`-style `terminal_state` error
  doesn't apply; it should follow `status`'s precedent and return a normal (non-error)
  response reporting the terminal state, with `directive`/`details`/`expects` naturally
  absent the same way `NextResponse::Terminal`'s serializer already omits them
  structurally (`next_types.rs:458-471`) — there's nothing to resume at a terminal
  state, and saying so plainly is more useful than an error the agent has to interpret.

## Implications

- The recovery payload is a genuinely new, narrower shape — not `NextResponse` with
  fields stripped, and not `koto status` with fields added. It borrows `state` +
  `is_terminal`-equivalent framing from `status`, and `directive` + `details` +
  `expects` (fully substituted) from `next`'s non-terminal variants, but drops
  `advanced` and `unassigned_children` as out of scope for a query that runs nothing.
- `blocking_conditions` can be included as a **best-effort, explicitly-stale** field via
  `derive_last_gate_evaluated`, reusing the exact function the read-only dashboard
  already relies on — this closes what looked like a gap (recovery needing gate state
  without running gates) using existing, already-shipped machinery.
- The locking answer is good news for the "respawned agent races its predecessor"
  scenario: modeling the recovery call on `status` (no lock at all) rather than on
  `next` (conditional lock) means it never blocks and never contends, by construction,
  matching the existing advisory-read tolerance the codebase already documents and
  relies on.
- The directive-prefix splice mechanism is reusable machinery, not something to invent:
  `with_directive_prefix` already exists, is already unconditional-on-every-tick for the
  abandonment case, and already establishes the ordering rule (splice after
  substitution) a phase-info pointer would need to follow for the same reason.

## Surprises

- Gate evaluation results are **not** ephemeral the way I initially assumed from reading
  `dispatch_next`'s pure, no-I/O signature — `GateEvaluated` events persist per-gate
  outputs to the log, and there's already a purpose-built, epoch-scoped, read-only
  derivation function (`derive_last_gate_evaluated`) with two existing read-only callers
  (`overrides.rs`, `dashboard_data.rs`). This means the "the read-only call can't know
  about gates without running them" concern the task brief implicitly raises has a
  cleaner answer than "omit gates entirely": surface the last-known result, labeled as
  such.
- The discovery scan is a real, documented state mutation (`scan_cursor.toml` writes),
  not incidental bookkeeping — its own module doc explicitly frames the cursor write as
  necessary for correctness (avoiding lost candidates on tied mtimes), which makes it a
  clean, well-justified exclusion from the recovery payload rather than an oversight to
  work around.
- `koto next`'s non-batch path takes **no lock whatsoever**, even for the destructive
  append/action/cleanup operations — the locking story is much thinner than "next locks,
  status doesn't"; it's closer to "next locks only for batch-parent states, and even
  then non-blocking." This simplifies the concurrency answer for recovery considerably.

## Open Questions

- Should the recovery call's `blocking_conditions` (last-known, from
  `derive_last_gate_evaluated`) be included by default, or only behind an explicit flag
  — given it can be stale and a naive reader might treat it as live? The evidence
  supports either; this is a design call the PRD needs to make explicitly, not something
  derivable from existing code.
- Should the discoverability pointer live only in `directive` (matching the
  abandonment-notice precedent exactly, accepting the `Terminal` gap), or should the
  PRD introduce a small always-present structured sibling field precisely to close that
  gap — the abandonment case tolerates the gap because there's a *different* delivery
  channel (`koto request get`) for the info that's missing at `Terminal`/`Error`; a
  phase-info pointer has no equivalent fallback channel once an agent is at a terminal
  state with a lost context, since by definition it wouldn't know to ask a different
  question of a different command either.
- What should the recovery command be named, and should it take the workflow name as a
  positional arg the way `status` does, given the whole premise is an agent that may not
  remember arguments it wasn't told to memorize? (Likely resolved by the fact that the
  session name is typically baked into the agent's working directory / environment, but
  worth confirming against how other koto commands source the workflow name when not
  passed explicitly.)

## Summary

The recovery payload should be a purpose-built shape — `state`, fully-substituted
`directive`/`details` (unsuppressed, `--full`-style), `expects` when applicable, and
optionally epoch-scoped last-known `blocking_conditions` via the existing
`derive_last_gate_evaluated` — not `next` minus side effects nor `status` plus fields,
since `advanced` and `unassigned_children` don't survive translation into a
nothing-happens call. Locking-wise, modeling the call on `status` (no lock at all,
matching `read_events`'s documented advisory-read tolerance) rather than on `next`
(conditional non-blocking batch-parent lock) makes it safe against a racing predecessor
by construction, and the discoverability pointer has a ready-made mechanism to reuse:
the leg-abandonment `with_directive_prefix` splice, unconditional-after-substitution,
with the same `Terminal`/`Error` coverage gap the existing precedent already accepts and
documents.
