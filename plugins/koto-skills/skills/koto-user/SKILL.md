---
name: koto-user
description: Guides agents through running koto-backed workflows — init, execute the action loop, handle gates, submit evidence, and reach completion
---

# koto-user

koto is a workflow orchestration engine for AI coding agents. It enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

You use koto by calling `koto next` in a loop. Each call returns a JSON object that tells you what to do next. You do it, then call `koto next` again.

## Prerequisites

- koto >= 0.5.0 must be installed and on PATH (`koto version` to verify)
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
| `gate_blocked` | One or more gates failed and the state has no evidence fallback. | Read `directive` and `blocking_conditions`. Check `agent_actionable` on each item — override if possible, otherwise escalate to the user. |
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

### Sub-case B: Gates failed, evidence fallback available

**Signals:** `blocking_conditions` is non-empty, `expects.fields` is non-empty

One or more gates failed, but the state still accepts evidence. You can either fix the gates and re-run `koto next`, record a gate override (when `agent_actionable` is `true`), or submit evidence directly to proceed.

Check each item in `blocking_conditions`:

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

## Recording decisions

Use `koto decisions record` to capture key choices mid-workflow. This doesn't advance the state — it just appends a decision record.

```bash
koto decisions record <name> --with-data '{"choice": "option-a", "rationale": "because X", "alternatives_considered": ["option-b"]}'
```

`choice` and `rationale` are required. `alternatives_considered` is optional.

## Reference material

- [**Command reference**](references/command-reference.md) — full CLI syntax, flags, and output shapes for all subcommands. Follow this when you need exact flag names or want to check an unfamiliar command.
- [**Response shapes**](references/response-shapes.md) — annotated JSON examples for every `action` value, sub-object schemas for `expects` and `blocking_conditions`, and field-level annotations. Follow this when a field's presence or shape is unclear.
- [**Error handling**](references/error-handling.md) — exit code table, error code meanings, and agent actions for each error type. Follow this when a command fails or returns a non-zero exit code.

## Troubleshooting

**"koto: command not found"** — koto isn't on PATH. Install it or add its directory to PATH.

**"workflow_not_initialized"** — the workflow name doesn't exist. Run `koto workflows` to see what's active, or re-run `koto init` if the session was cleaned up.

**"session already exists"** — a previous session with this name is still active. Call `koto next <name>` to resume. If you don't need it, cancel first with `koto cancel <name>` then re-initialize.

**Gate blocked, `agent_actionable` is `false`** — you can't override this gate yourself. Escalate to the user so they can resolve the underlying condition (for example, a required deployment that only they can trigger).

**`koto next` returns the same state repeatedly** — check `advanced` in the response. If it's `false`, the engine stopped where it already was (gates still blocking, or evidence still missing). Re-read `blocking_conditions` and `directive`.
