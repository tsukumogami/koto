# Phase 2 Research: Output and Failure Path

Sources: prior research in `koto/wip/research/explore_koto-command-authority_r1_lead-*.md`
and `shirabe/wip/research/explore_koto-runs-commands_r{1,2,3}_lead-*.md` (branch
`origin/docs/koto-runs-commands` in shirabe). All file:line citations below were
re-verified directly against koto source on branch `docs/koto-command-authority`
as of this pass; line numbers may drift by a few lines on future commits but the
structural claims are current.

## Lead A: Output Routing (Current Behavior)

### Findings

**Capture point.** `run_shell_command()` (`src/action.rs:26-107`) is the single
function used by both gates and default_actions. It spawns via `sh -c`, and only
reads `child.stdout`/`child.stderr` after `child.wait_timeout(timeout)` returns
(`src/action.rs:60-84`), producing a `CommandOutput { exit_code, stdout, stderr }`
(`src/action.rs:16-20`).

**Where a default_action's output goes on the happy path (discard point).**
In `src/engine/advance.rs`, step 5 of the advance loop calls `execute_action` and
matches on `ActionResult`:
```rust
// src/engine/advance.rs:286-300 (verified current)
if let Some(action) = &template_state.default_action {
    let has_evidence = !current_evidence.is_empty();
    let result = execute_action(&state, action, has_evidence);
    match result {
        ActionResult::Executed { .. } => {
            // Continue to gate evaluation
        }
        ActionResult::Skipped => {
            // Continue to gate evaluation
        }
        ActionResult::RequiresConfirmation { exit_code, stdout, stderr } => {
            return Ok(AdvanceResult { .. stop_reason: StopReason::ActionRequiresConfirmation { .. } });
        }
    }
}
```
`ActionResult::Executed { .. }` destructures and discards `exit_code`/`stdout`/
`stderr` outright — this is the discard point. It never reaches
`current_evidence`, `gate_evidence_map`, or any evidence structure used by `when`/
`skip_if` routing.

**Where it's persisted regardless.** Before the advance loop's discard, the CLI's
action closure (`src/cli/mod.rs`, around the `action_closure`/action-execution
block near line 4021-4036) truncates output —
`truncate_output(&output.stdout, MAX_ACTION_OUTPUT_BYTES)` /
`truncate_output(&output.stderr, MAX_ACTION_OUTPUT_BYTES)` at `src/cli/mod.rs:4025-4026`
(`MAX_ACTION_OUTPUT_BYTES = 64 * 1024`, `src/cli/mod.rs:61`) — and unconditionally
appends an `EventPayload::DefaultActionExecuted { state, command, exit_code,
stdout, stderr }` event (`src/cli/mod.rs:4028-4036`; payload shape at
`src/engine/types.rs:544-550`). This event write is the only durable trace of a
default_action's output on the happy path — it lands in the session's
`.state.jsonl` event log, not in the `koto next` response.

**What `koto next` actually returns.** `NextResponse` (`src/cli/next_types.rs:63-127`)
has seven variants: `EvidenceRequired`, `GateBlocked`, `Integration`,
`IntegrationUnavailable`, `Terminal`, `ActionRequiresConfirmation`, `Error`. Only
`ActionRequiresConfirmation` carries an `action_output: ActionOutput` field
(`next_types.rs:104-112`), where `ActionOutput { command, exit_code, stdout,
stderr }` is defined at `next_types.rs:788-793`. `GateBlocked` and
`EvidenceRequired` carry `blocking_conditions: Vec<BlockingCondition>` instead —
gate JSON, not action JSON (see Lead B). `Terminal` carries no output at all. So:
an agent only ever sees a default_action's raw stdout/stderr in the `koto next`
JSON when the template author set `requires_confirmation: true` on that action —
and that flag fires unconditionally on success or failure (see Lead B, finding 1),
not as a failure-only mechanism.

**Existing forms of "a later state reads a value."**

1. **Init-time variables / `{{VAR}}` substitution** — the only real precedent for
   text interpolation. `Variables::from_events()` (`src/engine/substitute.rs:57-73`)
   builds the entire binding table by scanning the event log for exactly one event
   type, `EventPayload::WorkflowInitialized { variables, .. }` — a `HashMap<String,
   String>` set once at `koto init --var` time. There is no code path that adds to
   or updates this map after init. Values are re-validated against an allowlist
   regex, `VALUE_PATTERN = r"^[a-zA-Z0-9._/:@ \-]*$"` (`substitute.rs:29`) — no
   shell metacharacters, no newlines. `Variables::substitute()` /
   `substitute_command()` (`substitute.rs:87-106`, `{{KEY}}` token replacement)
   only ever read this frozen map. `{{KEY}}` references are compile-time checked
   against the template's declared `variables:` block
   (`src/template/types.rs`, "when clause references undeclared variable" and
   parallel checks on directive/gate-command/action-command text) — an author
   cannot introduce an ad hoc runtime `{{KEY}}` that wasn't declared up front.
   Substitution is honored in directive text, gate `command`, and action
   `command`/`working_dir` (all pass through `Variables::substitute`/
   `substitute_command`). **Conclusion: a default_action's stdout categorically
   cannot become a `{{VAR}}` value today** — there is no ingestion point into
   `Variables` after init.
2. **Context store** — a separate, byte-blob, agent-write-only system.
   `koto context add/get/exists/remove` (`src/cli/context.rs`) writes/reads raw
   bytes through `ContextStore` (`src/session/context.rs`), content-addressed via
   SHA-256, emitting `ContextAdded`/`ContextRemoved` events. Nothing in
   `src/action.rs` or `src/engine/advance.rs` writes to it — it is written
   exclusively by explicit agent CLI calls. It is read only by two gate types,
   `context-exists` and `context-matches` (`src/gate.rs`) — never by
   `Variables::substitute()`. Context and variables are disjoint: context is
   gate-reachable but not `{{VAR}}`-reachable; variables are `{{VAR}}`-reachable
   but not writable after init.
3. **Gate evidence (`gates.*` namespace)** — the closest real precedent for
   "a command result becomes evidence usable by later routing," but scoped
   narrowly. `evaluate_gates()` runs command-type gates and returns a
   `StructuredGateResult { outcome, output: serde_json::Value }` per gate. This
   `output` is merged into `current_evidence` under a `gates.*` key
   (`gate_evidence_map` in `advance.rs`), and `when`/`skip_if` clauses can route
   on `gates.<name>.exit_code`. But `current_evidence` is reset to
   `BTreeMap::new()` on every transition — it does not survive into a later
   state, only routes the *same* state's own transition decision. And gate
   `output` for command gates never contains stdout (see Lead B finding 4).

**Costed output-routing options (from prior research, four options; names are
this research's own for reference, not koto terminology):**

1. **Populate action output on every `NextResponse` stop reason** ("last_action"
   field). Naive-looking but actually the largest option: the acting state and
   the stopping state are frequently different states once auto-advance chains
   through several states in one `koto next` call, so the value has to live on
   `AdvanceResult` and get threaded through 5 `NextResponse` variants, the
   hand-rolled `Serialize` impl, and three exhaustive-match combinators
   (`with_substituted_directive`, `with_directive_prefix`,
   `with_details_suppressed_unless_full`). Answers "did something run" but not
   the motivating "a later state's directive needs this value" case unless the
   agent manually re-injects it.
2. **`capture_stdout_as:` on the action → new `VariableCaptured` event → folded
   into `Variables::from_events` → consumed by existing `{{VAR}}` substitution.**
   The smallest option that actually satisfies "a later state's directive text
   uses the value." Reuses the existing declared-variable/allowlist/substitution
   machinery; touches no `NextResponse` field, no `Serialize` impl, no baseline
   fixture. Needs one non-obvious fix: `mod.rs` snapshots `Variables` *before*
   the advance loop runs, so a same-tick auto-advance into a state whose
   directive reads the just-captured variable would see a stale/unsubstituted
   value unless the captured value is merged into `variables` after the loop
   returns, before final substitution.
3. **Merge action output into `current_evidence` for `gates.*`-style routing
   within the same state.** Cheap (reuses the existing evidence-merge block),
   but `current_evidence` resets on every transition — cannot reach "a later
   state" on its own; only useful as a same-state routing complement.
4. **Write action stdout into the `ContextStore` under a derived key,
   in-process.** Cheap and already-built read/write plumbing exists, but nothing
   makes context-store content flow into `{{VAR}}` substitution or directive
   text — the agent would still have to run `koto context get` and paste the
   value in, which reintroduces the manual step the design is meant to remove.
   Also: a *shell-level* workaround (piping a command's output into
   `koto context add` from inside an action itself) is not viable at all — a
   nested `koto` invocation touching the session store deadlocks against the
   outer `koto next`'s workspace lock and hangs until the action's 30s timeout
   kills it (only `koto version`, which touches no session state, is safe to
   nest). Any option that writes durable state must do so in-process inside the
   engine, never by re-entering the `koto` binary as a subprocess.

### Implications for Requirements

- Any requirement of the shape "a later state can use a value a command
  produced" must state which of the two existing mechanisms it targets:
  `{{VAR}}` substitution (currently init-time-only, immutable) or gate `gates.*`
  evidence (currently same-state-only, resets on transition). Neither currently
  supports "captured mid-workflow, durable across states" — that would be new
  capability, not a fix to broken plumbing.
- If a requirement calls for command output to reach a later state's directive
  text, cite that the substitution snapshot timing (pre-loop vs. post-loop) is a
  real correctness constraint, not an implementation nicety — a same-tick
  auto-advance is the case most likely to be exercised in the "koto runs commands
  silently" scenario this PRD is presumably motivated by.
- A requirement should not assume the context store is a viable "value handoff"
  path without also requiring new `{{VAR}}`-integration work, since today it is
  read only by two gate types and never by substitution.

### Open Questions

- Should captured/durable values be visible to `when`/`skip_if` routing
  (evidence-style) in addition to `{{VAR}}` prose interpolation, or is one
  sufficient for the PRD's motivating cases?
- Is single-string whole-stdout capture sufficient, or does any known use case
  need structured/multi-value capture (e.g., named regex groups)?

## Lead B: Failure Path (Current Behavior)

### Findings

**1. `requires_confirmation` is not a failure signal — it fires unconditionally.**
`src/cli/mod.rs`'s action-execution code branches on the template's static
`requires_confirmation: bool` flag (`src/template/types.rs:205` area) after
running the command, regardless of exit code. In the advance loop,
`ActionResult::RequiresConfirmation { exit_code, stdout, stderr }` produces
`StopReason::ActionRequiresConfirmation` (`advance.rs:297-311`) whether the
command succeeded or failed. Design intent (`DESIGN-default-action-execution.md`)
frames it as "prevents irreversible actions from running unattended" — an
always-ask flag, not a failure fallback. Using it to catch failures would also
interrupt every successful run.

**2. Non-zero exit alone never stops the loop.** `ActionResult::Executed { .. }`
(returned for *any* exit code when `requires_confirmation` is false) falls
through to gate evaluation unconditionally (`advance.rs:290-292`,
"// Continue to gate evaluation"). `run_shell_command` treats non-zero exit,
spawn failure (`exit_code: -1`, `src/action.rs:50-57`), and timeout
(`exit_code: -1`, `src/action.rs:85-98`) all as ordinary `CommandOutput` values —
none of the three trips any special handling in the advance loop by itself.

**3. Confirmed: a state with `default_action` and no gates has no failure
detection at all.** In `advance.rs`, step 6 (gate evaluation,
`if !template_state.gates.is_empty() { ... }`) is skipped entirely when a state
declares no gates. If a `default_action` state has zero gates, the loop proceeds
straight to transition resolution regardless of the action's exit code — this is
exactly where the exit code gets ignored. (Prior empirical probe: a
default_action exiting 3 with a no-op gate advanced straight to `Terminal`.)
Polling states (`ci_monitor`/`execute_with_polling`, `src/cli/mod.rs` around
993-1053) are unaffected because a polling state always has a gate by
construction — the gate is what tells the poll loop when to stop.

**4. The real arbiter of "did the action succeed" is a paired gate, and gate
output is lossy.** Gate failure genuinely stops the loop: any `Failed`/
`TimedOut`/`Error` outcome from `evaluate_gates()` produces
`StopReason::GateBlocked(BTreeMap<String, StructuredGateResult>)`
(`advance.rs:58`, a tuple variant) unless the state has an `accepts` block, in
which case it falls through to `StopReason::EvidenceRequired { failed_gates:
Option<...> }` (`advance.rs:59-62`). But `evaluate_command_gate`
(`src/gate.rs:206-230`, verified current) only ever produces
`{"exit_code": N, "error": ...}` — it never reads or persists `output.stdout` at
all:
```rust
// src/gate.rs:206-230 (verified)
fn evaluate_command_gate(gate: &Gate, working_dir: &Path) -> StructuredGateResult {
    let output = run_shell_command(&gate.command, working_dir, gate.timeout);
    if output.exit_code == -1 { /* TimedOut or Error, output.stderr copied only on spawn/wait error */ }
    else if output.exit_code == 0 { /* Passed, {"exit_code":0,"error":""} */ }
    else { /* Failed, {"exit_code":N,"error":""} */ }
}
```
So even when a gate correctly detects a failed action, the agent's response
carries only an exit code — no stdout, no stderr — even though the *action's*
richer `CommandOutput` was captured moments earlier by the same
`run_shell_command`. The action's real stdout/stderr only survive in the
`DefaultActionExecuted` event log entry, which the gate-failure response path
does not read.

**5. `koto next`'s response variants and which carry action output.**
`NextResponse` (`src/cli/next_types.rs:63-127`) has 7 variants:
`EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`,
`Terminal`, `ActionRequiresConfirmation`, `Error`. `GateBlocked` and
`EvidenceRequired` carry `blocking_conditions: Vec<BlockingCondition>`, whose
`output` field is the thin gate JSON from finding 4 (no stdout/stderr). Only
`ActionRequiresConfirmation` carries `action_output: ActionOutput { command,
exit_code, stdout, stderr }` (`next_types.rs:104-112`, struct at
`next_types.rs:788-793`) — and per finding 1, that variant is not
failure-specific.

**6. No conditional/branching directive text exists.** `TemplateState.directive`
is a single `String` field authored once per state (`src/template/types.rs`
area). Every response-building match arm in `src/cli/mod.rs` for
`GateBlocked`/`EvidenceRequired`/`ActionRequiresConfirmation` uses the identical
`directive.clone()` regardless of whether the action succeeded, failed, or is
pending confirmation. There is no field or mechanism today for different prose
per stop reason, per exit code, or per gate outcome — "fallback prose" is
whatever the state's one static `directive` string says, shown identically no
matter why `koto next` stopped there. A template author's only way to write
"failure guidance" prose today is to put it in that same single directive body
(e.g., prose like "If the command fails, do X"), not a dedicated field.

**7. Recovery mechanism that does exist: `koto overrides record`.** Not a
default_action-level mechanism — it operates on gates. A template author
declares `override_default` (or relies on a `built_in_default`) on a *gate*; the
agent calls `koto overrides record` with a mandatory `--rationale` to substitute
a passing value for a failed gate. `GateOverrideRecorded` events are read by the
advance loop and injected into `gates.*` evidence. This is how an agent "takes
over" after a detected failure, but it bypasses a gate's evaluation, not a
default_action's own exit code directly.

**8. `src/cli/retry.rs` is unrelated** — it's coordinator/child-workflow retry
machinery for re-dispatching failed *child workflows* in a fan-out, not
single-state action failure recovery.

### Implications for Requirements

- A requirement stating "on command failure, the agent sees the failure detail"
  cannot be satisfied by any currently-implemented path: the only path that
  halts the loop (a paired gate) is also the path that discards stdout/stderr,
  and the only path that carries stdout/stderr (`ActionRequiresConfirmation`)
  fires on success too, not on failure specifically.
- A requirement should explicitly distinguish "gates are the intended arbiter of
  success" (a deliberate design choice — see `touch marker.txt` + `test -f
  marker.txt` as the one real worked example in the test suite) from "a
  gate-less default_action has zero failure detection" (a gap, not a design
  choice) — conflating these would misstate current behavior.
- Any requirement for "different prose depending on outcome" needs new schema
  surface — `TemplateState.directive` is single and unconditional today; there
  is no per-outcome directive field to point a requirement at as already
  existing.

### Open Questions

- Should a gate-less `default_action` state's exit code be treated as an
  implicit gate ("no gate declared" = "exit code is the gate") — this changes
  today's silent-pass-through behavior and needs an explicit requirement,
  since nothing currently synthesizes this.
- Does a `GateBlocked`/`EvidenceRequired` response need a path to the most
  recent `DefaultActionExecuted` event's stdout/stderr (e.g. via `koto status`)
  for post-hoc debugging, independent of whether the primary failure-detail
  requirement is met synchronously in the same `koto next` call?

## Lead C: The Pipe-Buffer Deadlock

### Findings

**Shared function, confirmed current.** `run_shell_command()`
(`src/action.rs:26-107`) is the sole shared command-execution path for both
gates (`src/gate.rs:206`, `evaluate_command_gate`) and default_actions
(`src/cli/mod.rs`, one-shot and polling call sites around lines 1022/4021). No
other non-test code path in `src/` shells out with piped stdio and a wait.

**Mechanism, verified against current source.**
```rust
// src/action.rs:33-38
let mut cmd = Command::new("sh");
cmd.arg("-c").arg(command).current_dir(working_dir)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
...
// src/action.rs:60
match child.wait_timeout(timeout) {
    Ok(Some(status)) => {
        // lines 61-84: pipes are read ONLY here, after wait returns
        let stdout = child.stdout.take().map(|mut s| { read_to_string(...) }).unwrap_or_default();
        let stderr = child.stderr.take().map(|mut s| { read_to_string(...) }).unwrap_or_default();
```
`child.stdout`/`child.stderr` are `Stdio::piped()` and are never read while
`wait_timeout` blocks — read happens only after `wait_timeout` returns. A Linux
pipe buffer is typically 64KB; a child process that writes more than the buffer
holds blocks on its own `write()` syscall once the buffer fills, because nothing
is draining the read end. The child cannot exit (it's blocked mid-write), so
`wait_timeout` never sees it exit and returns `Ok(None)` once the timeout
elapses. At that point (`src/action.rs:85-98`) the process group is SIGKILL'd
and the function returns `exit_code: -1, stdout: "", stderr: "command timed out
after N seconds"` — the observable symptom is a command that actually would
have exited 0 (or with whatever real code) instead reporting a false
`timed_out` after the full timeout duration, with **all output discarded**, not
truncated — truncation never gets a chance to run because the deadlock prevents
the read entirely.

**The 64KB truncation cap is unrelated and narrower than it looks.**
`MAX_ACTION_OUTPUT_BYTES = 64 * 1024` (`src/cli/mod.rs:61`) is applied via
`truncate_output()` (`src/cli/mod.rs:833`, applied at lines 4025-4026) — but
only on the default_action path, and only *after* `run_shell_command` has
already returned successfully. It has nothing to do with the pipe-buffer
deadlock; the two numbers (kernel pipe buffer size and the application-level
truncation cap) are coincidentally similar magnitudes, which makes the failure
read like "truncation broke" when actually truncation never ran.
**`evaluate_command_gate` (`src/gate.rs:206-230`) does not call
`truncate_output` at all** — it never touches `output.stdout` in the first
place (see Lead B finding 4), so the 64KB cap does not apply to gate output at
all, only to default_action stdout/stderr.

**Shipped gate exposure.** Prior research swept all 11 `type: command` gates
shipped in shirabe's `work-on.md` (8) and `execute.md` (3) templates. Structural
finding: 9 of 11 route their potentially-large inner command output through a
shell construct that keeps the *outer* captured stdout empty — `$(...)` command
substitution or piping into a silent sink (`test`, `grep -q`, `[`). Only one gate
(`tests_passing`: `go test ./...`) writes output directly to the captured
stream, and a real measurement on the tsuku monorepo (63 packages) put it at
~3.8KB — well under the 64KB trigger, though this scales with suite size/
verbosity (e.g., `-v`, or Go's per-test output dump on failure) and is not
categorically safe. Two gates (`staleness_fresh`, `ci_passing` ×2) have an
unredirected stderr side-channel from the piped-away upstream command that
wasn't independently measured — a pipe only joins the upstream command's
*stdout* to the next stage; its stderr bypasses the pipe and reaches the outer
captured stderr directly, so a verbose/erroring `gh`/script invocation there is
a plausible, unruled-out risk.

**Independent compounding factor.** A separate, already-tracked defect (koto
issue #193) causes `LocalBackend::new()` to print one `eprintln!` line per
colliding session name on every session-touching koto invocation (up to
~100KB of stderr observed at scale, migration never converges). This is
unrelated to the deadlock in isolation, but if a gate or default_action script
were to shell out to `koto` itself (nested invocation), that ~100KB stderr
payload is sized to trigger the pipe-buffer deadlock directly — turning a
noise issue into a false-timeout failure. No currently-shipped gate or action
does this today (it's the scenario a "koto runs commands" expansion would
newly create exposure to), and separately, a nested `koto` invocation touching
session state deadlocks against the outer `koto next`'s workspace lock
regardless of output volume (see Lead A finding on Option 4) — so nesting koto
inside koto-executed commands is unsafe on two independent axes, not just
this one.

### Implications for Requirements

- Any requirement expanding what commands `default_action`/gates are trusted to
  run should explicitly account for output volume — the deadlock is confirmed
  live in the shared execution path, not hypothetical, even though today's
  shipped gates mostly avoid it by accidental shell composition (substitution/
  silent-sink patterns), not deliberate mitigation.
- A requirement should not conflate "gate stdout is truncated at 64KB" with
  reality — gates never capture stdout at all today, truncation only applies to
  default_action output, and only after a successful (non-deadlocked) read.
- If new authoring guidance or validation is in scope, note that verbose test
  output (`-v` flags, `t.Log`/`fmt.Println` patterns that dump on failure) is
  the most plausible realistic trigger for the one gate that does write output
  directly, not adversarial input.

### Open Questions

- Should the fix (draining stdout/stderr on background threads while
  `wait_timeout` runs, so a deadlocked write is prevented rather than merely
  produced-then-discarded) be treated as a prerequisite for any expansion of
  default_action/gate command trust, given it's confirmed live and the
  compounding nested-koto scenario turns a latent risk into a self-inflicted
  one?
- Should the timeout path, once fixed to drain concurrently, return the partial
  output collected before a genuine timeout kill, or preserve today's
  "empty output on timeout" contract? This is a behavior decision for the
  downstream design, not something current behavior already answers.

## Summary

Today, a `default_action`'s output is captured once by `run_shell_command()`
(`src/action.rs:26-107`, shared with gates), truncated to 64KB, and written
unconditionally to the event log as `DefaultActionExecuted`
(`src/cli/mod.rs:4025-4036`) — but on the normal success path
`advance.rs:290-292` discards the in-memory value outright, so it never reaches
`{{VAR}}` substitution (init-time-only, `substitute.rs:57-73`), the context
store (agent-write-only, gate-read-only), or the `koto next` JSON, which surfaces
raw action output only via `ActionRequiresConfirmation` — a flag that fires on
success or failure alike, not a failure signal. Failure detection itself is
gate-mediated by design (gates are the intended arbiter of success), but a
`default_action` state with no declared gates has zero failure detection at all
(`advance.rs`'s gate-evaluation block is skipped when `gates` is empty), and even
when a gate does catch a failure, `evaluate_command_gate` (`src/gate.rs:206-230`)
discards stdout/stderr and keeps only the exit code — so the richest failure
detail never reaches the agent through the path that actually halts the loop.
There is no per-outcome directive text anywhere; `TemplateState.directive` is one
static string per state. Separately, the shared `run_shell_command` function has
a confirmed, live pipe-buffer deadlock (`action.rs:60-84`: reads happen only
after `wait_timeout` returns, so >64KB of unread output causes the child to block
on write, timeout expires, and all output — not just the excess — is discarded
as a false `timed_out`); today's 11 shipped gates mostly avoid it by shell
composition accident rather than design, and the 64KB truncation cap
(`cli/mod.rs:61`) is an unrelated, coincidentally-similar-sized, default_action-
only, post-hoc bound that never applies to gate output at all. Four costed
output-routing options exist from prior research (populate action output on
every response variant; add a `capture_stdout_as` → new-event → `{{VAR}}`
capture; merge into same-state gate evidence; write to the context store), of
which the `{{VAR}}`-capture option is the smallest that reaches a *later* state,
and any durable-write option must operate in-process inside the engine — nesting
a `koto` subprocess call inside a koto-executed command deadlocks against the
outer session lock independent of the pipe-buffer issue.
