---
name: koto-user
description: |
  How to run koto-backed workflows. Use when a SKILL.md tells you to call koto init or koto next.
---

# koto-user

koto is a workflow orchestration engine for AI coding agents. It enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

You use koto by calling `koto next` in a loop. Each call returns a JSON object that tells you what to do next. You do it, then call `koto next` again.

This skill is for koto-backed workflows only. If the SKILL.md you're following doesn't mention `koto init` or `koto next`, this skill doesn't apply. For authoring new koto templates, use koto-author instead.

## Prerequisites

- koto >= 0.7.1 must be installed and on PATH (`koto version` to verify)
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
| `gate_blocked` | One or more gates failed and the state has no evidence fallback. | Read `directive` and `blocking_conditions`. Check `category` to distinguish temporal blocks (retry later) from corrective ones (fix something). Check `agent_actionable` on each item — override if possible, otherwise escalate to the user. |
| `integration` | An integration ran and returned output. | Read `directive` and `integration.output`. Follow the directive's instructions for handling the output. |
| `integration_unavailable` | An integration is declared but not configured. | Read `directive`. Follow any manual fallback instructions it provides. |
| `done` | The workflow reached a terminal state. | Stop. The workflow is complete. |
| `confirm` | A default action ran and requires your confirmation before advancing. | Read `directive` and `action_output` (command, exit code, stdout, stderr). Confirm if correct, or submit evidence to redirect. |

Note: `directive` is absent on `done` responses. Don't expect it.

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

Returns `name`, `current_state`, `template_path`, `template_hash`, and `is_terminal`. No gates are evaluated, no state changes happen.

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

## Reference material

Read these on demand, not upfront. The sections above cover the common path. Consult a reference file only when you hit the specific situation it describes.

- [**Command reference**](references/command-reference.md) — full CLI syntax, flags, and output shapes for all subcommands. Follow this when you need exact flag names or want to check an unfamiliar command.
- [**Response shapes**](references/response-shapes.md) — annotated JSON examples for every `action` value, sub-object schemas for `expects` and `blocking_conditions`, and field-level annotations. Follow this when a field's presence or shape is unclear.
- [**Error handling**](references/error-handling.md) — exit code table, error code meanings, and agent actions for each error type. Follow this when a command fails or returns a non-zero exit code.
- [**Batch workflows**](references/batch-workflows.md) — coordinator/worker partition, `materialized_children` dispatch, `retry_failed` mechanics, `reserved_actions`, `batch_final_view`, cloud `sync_status`, and skip-marker `synthetic: true`. Follow this when the workflow uses `materialize_children` or the response carries a `scheduler` field.

## Troubleshooting

**"koto: command not found"** — koto isn't on PATH. Install it or add its directory to PATH.

**"workflow_not_initialized"** — the workflow name doesn't exist. Run `koto workflows` to see what's active, or re-run `koto init` if the session was cleaned up.

**"session already exists"** — a previous session with this name is still active. Call `koto next <name>` to resume. If you don't need it, cancel first with `koto cancel <name>` then re-initialize.

**Gate blocked, `agent_actionable` is `false`** — you can't override this gate yourself. Escalate to the user so they can resolve the underlying condition (for example, a required deployment that only they can trigger).

**Evidence rejected (`invalid_submission`)** — one or more fields didn't pass validation. The error includes a `details` array with per-field reasons. Fix the field values and resubmit. Call `koto next <name>` without `--with-data` to re-read the `expects` schema if needed.

**`koto next` returns the same state repeatedly** — check `advanced` in the response. If it's `false`, the engine stopped where it already was (gates still blocking, or evidence still missing). Re-read `blocking_conditions` and `directive`.
