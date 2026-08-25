---
name: koto-user
description: >-
  Drive a workflow session koto is running, and get it unstuck when it stalls.
  Load this whenever a koto next call has put something in front of you that
  you cannot confidently act on: a directive or action you are unsure how to
  dispatch on, a blocking condition you are about to try to override, a run
  that keeps returning the same state, an anchor or nested-invocation refusal,
  or a coordinator that has been waiting on its children longer than it
  should. It is also the answer to "where did we leave off on that?", "resume
  the thing from yesterday", "we did that out of order, undo the last step",
  and to a ~/.koto that keeps growing or discovery scans that have got slow.
  Guessing through these is expensive and quiet: an anchor refusal names a
  repair that is right only when the checkout really moved, and reaching for
  it otherwise binds the session to the wrong tree; some blocking conditions
  cannot be overridden at all and the attempt just fails; and a session's
  execution anchor is not a sandbox, so treating it as one tells a user
  something untrue about what a workflow can reach. Do NOT load it to design a state machine for a business domain -
  order lifecycles, request status models and the like are ordinary software
  design with nothing to do with koto. To write a durable template or a
  workflow-backed skill use koto-author; to decompose a fresh one-off task
  that has no template yet use koto-adhoc, which hands the run loop back here
  once the session is started.
---

# koto-user

koto is a workflow orchestration engine for AI coding agents. It enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

You use koto by calling `koto next` in a loop. Each call returns a JSON object that tells you what to do next. You do it, then call `koto next` again.

This skill is for koto-backed workflows only -- a session koto is already running, or one you're about to start from a template. If no koto session is involved, this skill doesn't apply. For authoring a durable template or a workflow-backed skill, use koto-author instead; for a one-off task that has no template yet, koto-adhoc.

## Prerequisites

- koto >= 0.12.3 must be installed and on PATH (`koto version` to verify)
- You need a compiled koto template (`.md` file with YAML frontmatter)

If koto is not installed or the version is too old, install the latest release:

```bash
# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m); [ "$ARCH" = "x86_64" ] && ARCH="amd64"; [ "$ARCH" = "aarch64" ] && ARCH="arm64"

# Download and install
gh release download -R tsukumogami/koto -p "koto-${OS}-${ARCH}" -D /tmp
chmod +x "/tmp/koto-${OS}-${ARCH}"
mv "/tmp/koto-${OS}-${ARCH}" ~/.local/bin/koto
```

## Session lifecycle

Every koto session follows the same three-step pattern:

**1. Initialize**

```bash
koto init <name> --template <path>
```

- `<name>` is the workflow name you choose — used in every subsequent call
- `<path>` is the path to the template file (e.g., `${CLAUDE_SKILL_DIR}/koto-templates/my-workflow.md`)
- Supply template variables with `--var KEY=VALUE` (repeatable)
- Returns `{"name": "<name>", "state": "<initial_state>"}` on success
- For a novel one-off task with no template, pipe a definition inline with `koto init <name> --from-stdin` (strict-only; mutually exclusive with `--template`). See the [command reference](references/command-reference.md#koto-init) for the full contract. The run loop below is identical once the session starts.

**2. Execute the action loop**

```bash
koto next <name>
```

Check the `action` field in the JSON response and act accordingly (see the [action dispatch table](#action-dispatch-table) below). Repeat until you see `action: "done"`.

**3. Reach completion**

When `action` is `"done"`, the workflow has reached a terminal state. No further `koto next` calls are needed.

## Action dispatch table

Every `koto next` response includes an `action` field. Dispatch on this field only — don't use other fields like `advanced` to decide what to do.

| `action` | What it means | What you do |
|---|---|---|
| `evidence_required` | The state needs input. May have gates blocking too. | Read `directive`. Check `blocking_conditions` and `expects.fields` to determine the sub-case — see below. |
| `gate_blocked` | One or more gates failed and the state has no evidence fallback. Also how a failed `default_action` arrives. | Read `directive` and `blocking_conditions`. A condition named `__action__` means the state's command failed — see [When a default action fails](#when-a-default-action-fails). Otherwise check `category` to distinguish temporal blocks (retry later) from corrective ones (fix something), and `agent_actionable` on each item — override if possible, otherwise escalate to the user. |
| `integration` | An integration ran and returned output. | Read `directive` and `integration.output`. Follow the directive's instructions for handling the output. |
| `integration_unavailable` | An integration is declared but not configured. | Read `directive`. Follow any manual fallback instructions it provides. |
| `done` | The workflow reached a terminal state. | Stop. The workflow is complete. |
| `confirm` | A default action ran **successfully** and requires your confirmation before advancing. | Read `directive` and `action_output` (command, exit code, stdout, stderr). Confirm if correct, or submit evidence to redirect. |

Note: `directive` is absent on `done` responses. Don't expect it.

A session bound to a request leg also carries a top-level `leg` object on every `koto next` response, and a `leg_abandoned` sibling once the requester stops waiting — see [Requests and legs](#requests-and-legs). Both are informational. The abandonment signal that matters rides `directive`; `action` gains no value for it, so nothing in the table above changes.

Directive-bearing responses also include a top-level `unassigned_children` array. It lists child workflows that name this coordinator as their `coordinator_of_record` and need agent dispatch; each element carries `child_session_id`, `role`, `template`, optional `inputs`, `requested_by`, `created_at`, and `dispatch_epoch`. The array stays empty unless the workspace contains unassigned children for this coordinator. Treat the field as informational alongside the directive — the current state's directive is still authoritative for what to do next.

## Handling `evidence_required`

This action covers three distinct situations. Distinguish them by examining `blocking_conditions` and `expects.fields` together.

### Sub-case A: Submit evidence directly

**Signals:** `blocking_conditions` is empty (`[]`), `expects.fields` is non-empty

No gates are blocking. The state is waiting for you to submit evidence.

```bash
koto next <name> --with-data '{"field_name": "value"}'
```

Use `expects.fields` to know what keys to include. Match the keys exactly (they're already snake_case). Check `expects.options` if present — it shows which target state each field value routes to.

Example: if `expects.fields` contains `{"outcome": {"type": "enum", "required": true, "values": ["success", "failure"]}}`, submit:

```bash
koto next <name> --with-data '{"outcome": "success"}'
```

For large or pre-built JSON payloads, prefix the value with `@` to read from a file:

```bash
koto next <name> --with-data @evidence.json
```

The file must contain the JSON payload directly (no shell quoting needed) and must be at most 1 MB.

**Dispatched-agent writes (`SubagentStop` hooks):** if you are a dispatched subagent writing back to a child workflow you were spawned to fulfil (one started with `--needs-agent`), you MUST pass `--dispatch-epoch <n>` with the epoch baked into your spawn. Example: `koto next <child> --dispatch-epoch 0 --with-data '{"status":"completed"}'`. The koto CLI validates `presented == header.dispatch_epoch` before any persistence call and rejects mismatches with `epoch_fence_violation` (exit code 65). Operator-driven `koto next <coord_workflow>` calls on the parent workflow do NOT require the flag. The same epoch goes on `koto request progress` / `resolve` / `abandon` when your session is bound to a request leg — there it's checked against the epoch recorded when the leg was bound, and the failure exits 2, not 65.

### Sub-case B: Gates failed, evidence fallback available

**Signals:** `blocking_conditions` is non-empty, `expects.fields` is non-empty

One or more gates failed, but the state still accepts evidence. You can either fix the gates and re-run `koto next`, record a gate override (when `agent_actionable` is `true`), or submit evidence directly to proceed.

Check each item in `blocking_conditions`:

- Check `category`: `"temporal"` means the condition will resolve on its own (e.g., child workflows finishing) — retry later. `"corrective"` (the default) means you or the user must fix something.
- If `agent_actionable` is `true`: record an override (see [Override flow](#override-flow)), then re-query
- If `agent_actionable` is `false`: you can't override this gate; submit evidence to bypass if the template allows it, or escalate to the user

### Sub-case C: Auto-advance candidate

**Signals:** `blocking_conditions` is empty (`[]`), `expects.fields` is empty (`{}`)

The state has no evidence schema, no integration, and no blocking gates. Call `koto next <name>` without `--with-data` to let it auto-advance.

In practice, the engine's advancement loop usually handles these states before returning to you — but if you do receive this shape, just call `koto next` again.

## Override flow

When a gate blocks and `agent_actionable` is `true`, you can override it:

**Step 1** — Record the override with a rationale:

```bash
koto overrides record <name> --gate <gate_name> --rationale "<why this override is justified>"
```

- `<gate_name>` is the `name` field from the `blocking_conditions` item
- `--with-data '<json>'` is optional; if omitted, the gate's `override_default` or the built-in default applies

**Step 2** — Re-query the workflow:

```bash
koto next <name>
```

The overridden gate is now treated as passed.

For `children-complete` gates, the override pretends all children are done. The default value mirrors the extended gate output schema: all aggregate counters are zero, `all_complete` and `all_success` are `true`, the `any_*` and `needs_attention` booleans are `false`, and `children` is empty. Use this when you know children are finished but the gate hasn't picked it up, or when you need to proceed regardless.

When `agent_actionable` is `false`, the gate has no override mechanism. Don't call `koto overrides record` for it — the command will fail. Escalate to the user instead.

## When a default action fails

A state can declare a `default_action` — a command koto runs itself on entering the state, before that state's gates. When it fails, the tick stops at that state and tells you in the same response. There's no second call to make and no error envelope to catch: `koto next` still exits 0 and answers `action: "gate_blocked"`, carrying one blocking condition under the reserved name `__action__`:

```json
{"action":"gate_blocked","advanced":false,"state":"detect",
 "blocking_conditions":[{"name":"__action__","type":"action","status":"failed",
   "agent_actionable":false,"category":"corrective",
   "output":{"state":"detect","command":"git rev-parse --abbrev-ref HEAD",
             "failure_kind":"nonzero_exit","exit_code":128,
             "stdout":"","stderr":"fatal: not a git repository (or any of the parent directories): .git\n",
             "truncated":false}}],
 "directive":"koto could not read the branch name. Run `git rev-parse --abbrev-ref HEAD` yourself...\n\nReading the current branch."}
```

**Route on `failure_kind`, never on message wording.** Two kinds share `status: "failed"`, so `status` doesn't discriminate.

| `failure_kind` | Meaning | What you do |
|---|---|---|
| `nonzero_exit` | The command ran and exited non-zero. The only kind carrying a real `exit_code`. | Read `stderr` and fix what the command is complaining about, then re-tick. |
| `spawn_failed` | No child process started — the tool isn't installed, the path doesn't resolve, or the action's `working_dir` was rejected. | Fix the environment or escalate; re-ticking unchanged won't help. |
| `timed_out` | The command exceeded its 30-second timeout and its process group was killed. Whatever it printed before the kill is still reported. | Check whether the command is hung on something external before retrying. |
| `wait_failed` | The child started but waiting on it failed, so no exit status was obtained. | Treat as infrastructure; report it. |
| `capture_failed` | The command exited zero, but its stdout couldn't be delivered under the state's `capture_stdout_as` name. A `capture_error` object names the case: `empty`, `too_large`, or `disallowed_character`. | The command produced the wrong shape of output. This is a template problem — report it rather than working around it. |

Three things to know:

- **`exit_code` is present only for `nonzero_exit`.** The other kinds omit it rather than reporting a synthetic `-1`. Don't read it unconditionally.
- **The state's gates did not run.** The tick returns before gate evaluation, so a state whose action failed reports exactly one condition and no gate result. Nothing advanced and nothing later in the workflow executed.
- **`agent_actionable` is `false` and there is no override.** Don't call `koto overrides record` against `__action__` — it isn't a gate, and the compiler won't let a template declare one by that name.

If the template's author wrote a `fallback`, its text opens the `directive`, ahead of the state's own instructions. That's the author telling you how to do the step by hand. Do that, and carry on.

## Where a session's commands run

A session records the directory it was created in — its **execution anchor** — and every tick is checked against it. Every gate and action of an accepted tick runs there, not in whatever directory you typed `koto next` in, so a command means the same thing wherever in the tree you're standing.

Standing in a subdirectory of the anchor is fine. Ticking from a *different* tree is refused, before the template is read and before any gate or action exists — so a refusal means nothing ran, nothing was evaluated, and nothing moved.

| Error code | Exit | What happened | What you do |
|---|---|---|---|
| `execution_anchor_mismatch` | 2 | The tick ran from a directory that is neither the anchor nor beneath it. The message names the bound directory. | `cd` to the directory the message names and re-run. |
| `execution_anchor_unresolvable` | 3 | The recorded anchor names nothing on this machine — the checkout was deleted, or the session moved machines. | Put the checkout back where the message names, or rebind the session to where the tree is now. |

When the checkout genuinely moved, `koto session rebind <session> [--to <dir>]` moves the anchor to match; `--to` defaults to the directory you run it from. It's the only verb that changes an anchor, and it records the move as an `execution_anchor_rebound` event.

Reach for it deliberately. A `execution_anchor_mismatch` usually means you're standing in the wrong place, not that the checkout moved, and rebinding then points the session at the wrong tree. Route on the error code rather than the message text.

A session created before anchoring existed has no recorded directory. Its first tick adopts the directory it's ticked from, records the binding, and says so once on the `directive`:

```
[koto] Session 'demo' had no recorded directory; it is now bound to /home/dev/repo. Later ticks must run there or below it -- `koto session rebind demo` moves it.
```

It doesn't refuse and it doesn't adopt silently. Check that the directory it names is the one you meant before you keep ticking — the adopted directory is simply whatever tree was current. The notice appears once; the next tick takes the ordinary path.

**Anchoring is not containment, sandboxing, or isolation.** It guarantees the directory a workflow's commands *start* in. It does not bound what a command can reach once running: a command can name absolute paths or change directory, and nothing here stops it. Don't rely on it as a safety boundary, and don't describe it to a user as one.

## Resuming a session

koto preserves state across interruptions. To resume:

```bash
koto next <name>
```

If you don't remember the session name, list active sessions:

```bash
koto workflows
```

If you're in the wrong state (for example, a prior action completed outside the workflow), roll back with:

```bash
koto rewind <name>
```

`koto rewind` walks back one state. Repeated calls walk back further. It can't go past the initial state.

## Hierarchy

A parent workflow can spawn child workflows and wait for them to finish. koto tracks the relationship but doesn't launch child agents — you do that yourself (Agent tool, subprocess, etc.).

### Creating child workflows

Link a child to its parent at init time:

```bash
koto init <child-name> --parent <parent-name> --template <path>
```

The `--parent` flag validates that the parent workflow exists and records the link in the child's state file. The naming convention `parent.child` is recommended but not enforced — the metadata link is what matters.

### Requesting agent dispatch on a new child

When the child you're spawning needs a separate agent to pick it up later, use `koto session start` instead of `koto init`. It marks the child as awaiting dispatch so a coordinator can later dispatch the right agent:

```bash
koto session start <child-name> \
  --parent <parent-name> \
  --needs-agent \
  --role <role-name> \
  --template <template-name> \
  --inputs '<json>'
```

- `--needs-agent` marks the child as awaiting dispatch and **requires** the `--role`, `--template`, and `--inputs` companions. Any of those without `--needs-agent`, or `--needs-agent` without the full set, rejects at parse time.
- `--inputs` is a JSON blob (max 1 MiB, max 128 nesting levels).
- `--coordinator-of-record <c>` is optional; it defaults to the parent's effective coordinator.
- Omit all four to start a plain child session without a dispatch marker — useful when the child is launched in-process by the same agent.

The session id (`--parent`) and coordinator id (`--coordinator-of-record`) are validated against `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` (max 255 chars) before any path operation, so paths like `../etc/passwd` or shell-metacharacter ids are rejected up front.

`--needs-agent` and the `koto request` noun group below are orthogonal, and both are supported. `--needs-agent` is how a child session that wants an agent comes into existence; it's still the only way. A request is a separate object that records what you asked for and holds the answer — `koto request create` spawns nothing. Starting a child without ever binding it to a leg is exactly today's behavior and stays fine.

### Checking children

List a parent's children:

```bash
koto workflows --children <parent-name>
```

Other useful filters:

```bash
koto workflows --roots        # only parentless workflows
koto workflows --orphaned     # children whose parent was cleaned up
```

### Reading child state

Check where a child is without side effects:

```bash
koto status <child-name>
```

Returns `name`, `current_state`, `template_path`, `template_hash`, and `is_terminal`, plus a `leg` object when the session is bound to a request leg. No gates are evaluated, no state changes happen.

Read a child's stored results:

```bash
koto context get <child-name> <key>
```

### Temporal blocking

When a parent has a `children-complete` gate, `koto next` returns `gate_blocked` or `evidence_required` with a blocking condition whose `category` is `"temporal"`. The `output` field carries aggregate counters, derived booleans, and per-child entries:

```json
{
  "total": 3,
  "completed": 2,
  "pending": 1,
  "success": 2,
  "failed": 0,
  "skipped": 0,
  "blocked": 0,
  "spawn_failed": 0,
  "all_complete": false,
  "all_success": false,
  "any_failed": false,
  "any_skipped": false,
  "any_spawn_failed": false,
  "needs_attention": false,
  "children": [
    {"name": "plan.issue-1", "state": "done", "complete": true, "outcome": "success"},
    {"name": "plan.issue-2", "state": "done", "complete": true, "outcome": "success"},
    {"name": "plan.issue-3", "state": "implement", "complete": false, "outcome": "pending"}
  ],
  "error": ""
}
```

Route on the derived booleans rather than raw counts:

- `all_complete` — `pending == 0 AND blocked == 0 AND spawn_failed == 0`. Passes the gate.
- `all_success` — every child finished successfully; the clean "no retries needed" branch.
- `any_failed`, `any_skipped`, `any_spawn_failed` — individual signals for templates that need finer control.
- `needs_attention` — `any_failed OR any_skipped OR any_spawn_failed`. One boolean routes the parent into its retry/escalation branch.

Per-child entries carry an `outcome` enum (`success | failure | skipped | pending | blocked | spawn_failed`). Failed children include a `failure_mode` string; skipped children include a `skipped_because` name and `skipped_because_chain` listing the failed ancestors; blocked children include `blocked_by` with the non-terminal `waits_on` names. A `reason_source` field (`failure_reason | state_name | skipped | not_spawned`) tells agents where the failure explanation came from.

Temporal blocks with `needs_attention: false` resolve on their own — poll `koto next` periodically. When `needs_attention: true` the parent's template typically routes to a retry or analysis state.

### Advisory lifecycle

When you cancel, clean up, or rewind a parent, the response includes a `children` array listing affected child workflows. koto doesn't cascade these operations — it tells you which children exist so you can decide what to do with them.

## Requests and legs

A request is a durable answer slot. `koto request create` records what you're asking for as one or more named **legs**; each leg is later bound to the child session that fulfils it, and the leg keeps that child's result after the child is gone. The record lives at `~/.koto/requests/<request-id>/`, outside every session, so it outlives the child's cleanup and your own restart.

A request is a container, not a spawner — `koto request create` starts no sessions. A fan-out is three steps:

```bash
# 1. Declare what you're asking for. Prints the generated request id.
koto request create \
  --with-data '{"legs":[{"name":"review","role":"reviewer","template":"review.md","inputs":{"pr":443}}],"inputs":{"repo":"koto"}}' \
  --requested-by <your-session-id> \
  --coordinator-of-record <coordinator-id>

# 2. Start the child the usual way (see "Requesting agent dispatch on a new child").
koto session start review-443 --parent <your-session-id> --needs-agent \
  --role reviewer --template review.md --inputs '{"pr":443}'

# 3. Connect the two.
koto request bind <request-id> review --child review-443
```

For a single leg there's a shorthand: `koto request create --role reviewer --template review.md --inputs '<json>' --requested-by ID --coordinator-of-record ID`. The leg is named after the role, so the role has to satisfy the leg-name grammar. `--with-data` and the `--role` / `--template` / `--inputs` triple are mutually exclusive.

`bind` only accepts a child started with `--needs-agent` under a parent — that's what makes the dispatch fence meaningful — and a child fulfils at most one leg. Rebinding the same leg to the same child is a no-op success; rebinding it elsewhere is rejected.

Output is JSON on stdout, always, with no format flag. Every verb prints the same envelope: `request_id`, `request_state`, `close_disposition`, `leg_counts`, `revision`, `legs`, and `cli_contract`. Full flags and the response shape are in the [command reference](references/command-reference.md#koto-request); the closed error-code set and its exit statuses are in [error handling](references/error-handling.md#request-command-errors).

### Where you read a leg's result

**For a bound leg, read the result from your own `koto next` directive, exactly as you always have.** Don't tick the child, don't query it, and don't poll `koto request get` waiting for the answer — your own gate is what advances your workflow, so watching the request view leaves you sitting still while the gate that would have moved you has already cleared.

The request view is for the four things your own directive can't give you:

- mid-flight progress on a leg that hasn't finished,
- partial state across a fan-out — which legs are in, which are still open,
- a read from some session other than the coordinator holding the gate,
- recovery after a restart, when the directive that carried the result is gone.

### Reporting progress on a leg you're filling

```bash
koto request progress <request-id> <leg> --with-data '{"note":"parser rewritten, tests next"}' --dispatch-epoch <n>
```

`koto session update --intent` is one sentence about what your whole workflow is for and each write replaces the last; a leg progress append is one entry about what you just finished for the request someone asked you to fill, and every entry is kept in order.

Progress goes on the request, not on your own session log, and it never advances your workflow. `--dispatch-epoch` is required once the leg is bound: present the epoch baked into your spawn, the same value `koto next` wants. Up to 256 appends per leg, 16 KiB each.

### Recording a result explicitly

`koto request resolve` applies **only to a leg with no bound child.** A bound leg resolves by promotion: when its child reaches a terminal state, koto writes that child's result onto the leg on the same tick, with no extra action from either side. Resolving a bound leg explicitly is rejected (`explicit_resolve_on_bound_leg`) — and if it weren't, your explicit answer would permanently block the real one, since a leg accepts at most one result.

One sentence to hold: if something else is doing the work, let it report; if nothing is, report it yourself.

```bash
koto request resolve <request-id> <leg> --with-data '{"status":"success","summary":"...","payload":{}}'
```

`status` is `success`, `failure`, or `skipped` — the same envelope a child's terminal tick produces.

### Reading, waiting, and closing

| Command | What it does |
|---|---|
| `koto request get <request-id>` | Read one request. Exits 0 for open, partially resolved, and closed alike — an unfinished request is a successful read, not an error. Two reads of an unchanged request are byte-equal. |
| `koto request list [--requested-by ID \| --coordinator-of-record ID] [--state open\|closed] [--unresolved-legs]` | Summaries only. Advances no cursor and writes nothing. |
| `koto request wait <request-id> <predicate> --timeout-secs N [--interval-secs N]` | Poll the same read path until a predicate holds. Exactly one predicate: `--leg <name>`, `--all-legs`, `--closed`, or `--resolved-count <N>`. |
| `koto request abandon <request-id> <leg> --rationale TEXT` | Stop waiting on one leg. The others stay open. |
| `koto request abandon-request <request-id> --rationale TEXT` | Abandon every open leg and close the request. A separate verb, so an unset shell variable can't escalate a leg abandonment into the whole request's. |
| `koto request close <request-id>` | Close, recording a disposition derived from the legs. Closing twice is rejected. |

`wait` is where readiness lives, so `get` can stay exit-zero. A satisfied predicate exits 0; a deadline with the predicate still unsatisfied exits 1 (transient, retry); a predicate that could never hold — five resolved legs on a three-leg request — exits 2 before polling starts; one that stopped being reachable while you waited exits 2 with a distinct code. `--timeout-secs` is required, and `--interval-secs` defaults to 2 with a floor of 1.

`--issued-by ID` is accepted on the six mutating verbs — `bind`, `progress`, `resolve`, `abandon`, `abandon-request`, `close` — and recorded for audit; `create` carries the same attribution as `--requested-by`. `--cli-contract MAJOR.MINOR` is accepted on every subcommand and checked before any read or write; this build serves `1.0`.

### Learning your own leg

If your session was bound to a leg, every `koto next` response carries a top-level `leg` object:

```json
{"leg": {"request_id": "req-9f1c2b7a-4d3e-4c5f-8a1b-2c3d4e5f6071", "leg_name": "review"}}
```

You read it off your own tick — it is never passed in your prompt. `koto status <name>` mirrors it read-only. It deliberately carries no `dispatch_epoch`: the epoch you present on writes is the one baked into your spawn, not something you can look up. A leg bound after your session started shows up on your next tick with no restart.

If there's no `leg` object, your session isn't bound to one. Don't guess a request id and don't call `koto request` verbs against a request you can't see.

### When the requester stops waiting

If your leg is abandoned, your next `koto next` tells you twice.

`directive` opens with a notice from koto — explicitly not from your coordinator — saying the leg was abandoned and nobody is waiting for your result. The state's own directive is still underneath it, retained for context only. The response also carries a top-level `leg_abandoned` object with `request_id`, `leg_name`, and the requester's verbatim `rationale`; the rationale is never spliced into `directive`, so read it from that sibling or from `koto request get <request-id>`.

**What to do:** stop the work in progress, start nothing new, and wind the session down without producing further output. Report what happened to whoever spawned you.

**What doesn't change:** the notice adds no `action` value — keep dispatching on `action` exactly as the table above says — and it doesn't touch `blocking_conditions` or your workflow state. koto isn't cancelling you; the advance loop still works, which is why the wind-down is yours to do.

Two response variants carry no `directive` to splice into: `done` and an error response. So a tick that fails validation gets no notice and you'll see it on the next successful tick. A `koto next --to <state>` directed transition carries the notice in `directive` but no `leg_abandoned` sibling and no `leg` object.

## Batch workflows

A batch workflow is a hierarchy variant where the parent submits a structured task list once, and koto's scheduler materializes and tracks per-task children automatically. The parent declares a `materialize_children` hook plus a `children-complete` gate; each `koto next <parent>` tick runs the scheduler, reports per-task feedback, and aggregates child outcomes for the gate.

The response shape includes batch-specific fields:

- `scheduler.materialized_children` — the per-child dispatch ledger (use this for idempotent dispatch, not `spawned_this_tick`).
- `scheduler.feedback.entries` — per-task outcome keyed by short name (`accepted`, `blocked`, `errored`, `already_running`, etc.).
- `reserved_actions` — ready-to-run retry invocations, synthesized when the gate reports `any_failed`, `any_skipped`, or `any_spawn_failed`.
- `batch_final_view` — frozen snapshot attached to the terminal `done` response.
- `synthetic: true` — marker on skip-marker children whose state was materialized directly (no worker ran).

Cloud-backend freshness indicators (`sync_status`, `machine_id`) are **not** attached to batch `koto next` responses. They surface only on `koto session resolve` output — use that command when you need to check or reconcile cross-machine divergence.

The canonical rule for worker dispatch:

> Dispatch a worker for every entry in `scheduler.materialized_children` where `ready_to_drive == true AND outcome != "spawn_failed"`, excluding children already dispatched this session.

Full coverage lives in [**batch-workflows.md**](references/batch-workflows.md). Read it when the SKILL.md you're following mentions `materialize_children`, task submission via `--with-data @tasks.json`, or `retry_failed`.

## Recording decisions

Use `koto decisions record` to capture key choices mid-workflow. This doesn't advance the state — it just appends a decision record.

```bash
koto decisions record <name> --with-data '{"choice": "option-a", "rationale": "because X", "alternatives_considered": ["option-b"]}'
```

`choice` and `rationale` are required. `alternatives_considered` is optional.

## Updating session intent

Use `koto session update --intent` to record a human-readable description of what the workflow is trying to accomplish. This doesn't advance the state — it appends an `intent_updated` event to the log, visible in the dashboard's Summary tab.

```bash
koto session update <name> --intent "investigate the flaky CI failure in the auth module"
```

Intent strings over 1024 characters are rejected. The command exits non-zero if the session doesn't exist.

Don't reach for this to report progress on a request leg. `--intent` is one sentence about what your whole workflow is for and each write replaces the last; a leg progress append is one entry about what you just finished for the request someone asked you to fill, and every entry is kept in order. See [Requests and legs](#requests-and-legs).

## Periodic maintenance: koto workspace prune

`koto workspace prune` reclaims the derived files the dispatch substrate accumulates over time — stale scan cursors (`~/.koto/coordinators/<id>/scan_cursor.toml`), stale compaction locks, and stale claim sidecars (`claim.lock`). It does NOT reclaim session bodies under `~/.koto/sessions/`; per-session cleanup still routes through `koto session cleanup <session-id>`. It also leaves request records under `~/.koto/requests/` alone — a closed request stays readable, which is what makes it an audit trail.

Suggest the verb when the user reports growing `~/.koto/` disk usage, when the discovery scan starts noticeably slowing at year-2 scale, or when stale-claim recovery events show up in the audit log.

Recommended cadence is **weekly to monthly** for typical workloads. See `docs/workspace-layout.md` ("Sizing your prune cadence") for the per-workload sizing math and cron snippets.

Flags worth knowing: `--root <session-id>` (required; terminal-state root to prune), `--dry-run` (preview without reclaiming), `--yes` (cron-friendly; skip the confirmation prompt), `--force` (bypass the terminal-state safety gate — dangerous). Full flag set lives in `docs/guides/cli-usage.md`.

```bash
koto workspace prune --root <session-id> --dry-run
```

## Native Claude Code `/workflows` rendering

When a koto session runs inside a Claude Code session, it can appear as a native entry in Claude Code's `/workflows` screen — no separate command or window. This is opt-in and rides the state-commit path: on each advance, koto writes its own `koto-<uuid>.json` into the Claude Code session's workflows directory, and the operator sees it on the next `/workflows` reopen. The entry shows the session's real structure — its phases in order with the active one marked, the active phase's directive, each completed phase's evidence/gate outcome, and a running / blocked / done status (a session blocked on a failed gate reads *blocked*). No agent action shapes this file; koto derives it from the session's own state.

Enabling it takes one thing: koto must know the target directory. The koto-skills plugin ships a `SessionStart` hook that derives the directory (`<projectDir>/<sessionId>/workflows`) and announces it. koto sessions render into it when **`KOTO_WORKFLOWS_DIR`** is set in their environment:

```bash
export KOTO_WORKFLOWS_DIR="<projectDir>/<sessionId>/workflows"   # the hook announces the exact path
```

With that set, no further action is needed — `koto next`, `koto rewind`, and directed transitions all refresh the entry. With it unset (and no published location), koto writes nothing and its default behavior is unchanged.

You can also publish a location explicitly (e.g. for a specific session id):

```bash
koto workflows publish --dir <workflows-dir> --session <session-id>
```

`workflows publish` records the directory in the session's context store under the reserved key `workflows/publish-location`; it writes no event. koto resolves the directory on each commit by walking from the session up its parent chain to the nearest published location, so a child session renders into an ancestor's published directory automatically.

To verify the path end-to-end without a live Claude Code TUI, run `scripts/verify-native-workflows.sh`; the manual TUI procedure is in `docs/guides/native-workflows-verification.md`.

## Reference material

Read these on demand, not upfront. The sections above cover the common path. Consult a reference file only when you hit the specific situation it describes.

- [**Command reference**](references/command-reference.md) — full CLI syntax, flags, and output shapes for all subcommands, including the whole `koto request` group. Follow this when you need exact flag names or want to check an unfamiliar command.
- [**Response shapes**](references/response-shapes.md) — annotated JSON examples for every `action` value, sub-object schemas for `expects` and `blocking_conditions`, and field-level annotations. Follow this when a field's presence or shape is unclear.
- [**Error handling**](references/error-handling.md) — exit code table, error code meanings (including the closed `koto request` code set), and agent actions for each error type. Follow this when a command fails or returns a non-zero exit code.
- [**Batch workflows**](references/batch-workflows.md) — coordinator/worker partition, `materialized_children` dispatch, `retry_failed` mechanics, `reserved_actions`, `batch_final_view`, cloud `sync_status`, and skip-marker `synthetic: true`. Follow this when the workflow uses `materialize_children` or the response carries a `scheduler` field.

## Troubleshooting

**"koto: command not found"** — koto isn't on PATH. Install it or add its directory to PATH.

**"workflow_not_initialized"** — the workflow name doesn't exist. Run `koto workflows` to see what's active, or re-run `koto init` if the session was cleaned up.

**"session already exists"** — a previous session with this name is still active. Call `koto next <name>` to resume. If you don't need it, cancel first with `koto cancel <name>` then re-initialize.

**"execution_anchor_mismatch"** — you're ticking from a different tree than the one the session is bound to. The message names the bound directory; `cd` there (or into a subdirectory of it) and re-run. Nothing ran on the refused tick. See [Where a session's commands run](#where-a-sessions-commands-run).

**"execution_anchor_unresolvable"** — the directory the session is bound to doesn't exist on this machine. Restore the checkout at the path the message names, or, if it moved, run `koto session rebind <session> --to <dir>` to point the session at where the tree is now.

**"capture_unset"** — a state read a `{{NAME}}` that a `capture_stdout_as` was supposed to deliver, and this run never entered the state that produces it. It covers the state's instructions, its `default_action`'s `command` and `working_dir`, and every field of its gates — a gate's `command`, `key`, `pattern` and `name_filter` alike; the message names which of them, along with the gate where one is involved, the value and the producing state. Nothing ran on either the action path or the gate path — the check happens before the command is spawned, the context key is read or the regex is compiled. One case reads differently: a gate on a state whose own *polling* action delivers the name is refused because that value cannot exist while the command is still running, and the message says so rather than naming a state you are standing in. This is a template routing problem, not something you can fix by re-ticking — report it to whoever authored the workflow.

**"nested_invocation"** — a `koto next` ran from inside a command koto itself was running. koto refuses it: a nested tick would advance the session while the tick that spawned it kept reporting the state it started with. If you hit this from a template's `default_action` or command gate, that call has to come out — the enclosing tick is what advances the session.

**"nested_invocation" when no tick is running** — the marker is an inherited environment variable with no liveness behind it, so a process that outlived its tick keeps it. A command that detaches (`setsid`, a backgrounded subshell) escapes the process-group kill koto uses at timeout and carries `KOTO_TICK_SESSION` for as long as it lives, so a `koto next` it runs minutes later is refused in the name of a tick that exited long ago. The message names the session, which is your clue: if `koto status` on that session shows nothing in progress, clear the marker and re-run — `KOTO_TICK_SESSION= koto next <name>`. Don't clear it reflexively. Inside a command that really is running under a tick, clearing it re-opens the defect the refusal exists to stop.

**A blocking condition named `__action__`** — the state's own `default_action` command failed; the state's gates never ran. Read `output.failure_kind` to decide what to do, and the front of `directive` for the author's fallback instructions. See [When a default action fails](#when-a-default-action-fails).

**Gate blocked, `agent_actionable` is `false`** — you can't override this gate yourself. Escalate to the user so they can resolve the underlying condition (for example, a required deployment that only they can trigger).

**Evidence rejected (`invalid_submission`)** — one or more fields didn't pass validation. The error includes a `details` array with per-field reasons. Fix the field values and resubmit. Call `koto next <name>` without `--with-data` to re-read the `expects` schema if needed.

**"reserved audit-event kind"** — your `--with-data` payload included a `fields.kind` value that collides with the request-store audit family. Four literal kinds (`ChildDispatched`, `ChildRedelegated`, `RequesterWoken`, `RequesterRespawn`) and anything starting with the `request_store.` prefix are reserved for the engine — template authors can't use them. Rename the field value to something workflow-specific (e.g., `"verdict"`, `"scrutineer"`) and resubmit.

**"explicit_resolve_on_bound_leg"** — you called `koto request resolve` on a leg that already has a bound child. Let the child's terminal tick promote its result instead; if you *are* that child, just finish your workflow normally and the leg resolves itself.

**"epoch_fence_violation" from a `koto request` write** — the leg is bound and you either omitted `--dispatch-epoch` or presented something other than the epoch recorded when the leg was bound. Present the epoch baked into your spawn. It's deliberately not readable from the `leg` object, so don't go looking for it there.

**"request_not_found" or "leg_not_found"** — check the id and leg name against your own `leg` object, or run `koto request list --coordinator-of-record <id>`. Both are caller errors (exit 2); retrying unchanged won't help.

**`koto next` returns the same state repeatedly** — check `advanced` in the response. If it's `false`, the engine stopped where it already was (gates still blocking, or evidence still missing). Re-read `blocking_conditions` and `directive`.
