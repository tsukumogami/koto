# Starting a koto Workflow Session

Here's exactly what to do to initialize the `review-pr-443` session and run the first cycle.

## Step 1: Initialize the session

```bash
koto init review-pr-443 --template .koto/code-review.md
```

This creates a new session named `review-pr-443` using your template. On success, koto prints a JSON object like:

```json
{"name": "review-pr-443", "state": "<initial_state_name>"}
```

The session is now persisted. If anything interrupts you before the workflow completes, you can always resume by running `koto next review-pr-443`.

## Step 2: Get the first directive

```bash
koto next review-pr-443
```

koto returns a JSON object. The most important field is `action`. Here's what each value means and what to do:

| `action` value | Meaning | What to do |
|---|---|---|
| `evidence_required` | The state needs input from you. | Read `directive`. Check `blocking_conditions` and `expects.fields` — see below. |
| `gate_blocked` | One or more gates failed and the state has no evidence fallback. | Read `directive` and `blocking_conditions`. Override if possible, otherwise stop and resolve the gate condition. |
| `integration` | An integration ran and returned output. | Read `directive` and `integration.output`. Follow the directive's instructions. |
| `confirm` | A default action ran and needs your confirmation. | Read `directive` and `action_output`. Confirm if correct. |
| `done` | Workflow complete. | Stop — no further `koto next` calls needed. |

## Step 3: Act on the first directive

The response will contain a `directive` field with instructions for this state. Read it and follow it.

**If `action` is `evidence_required`:**

Check `blocking_conditions` and `expects.fields` together:

- `blocking_conditions` is empty, `expects.fields` is non-empty — no gates are blocking. Submit evidence directly:
  ```bash
  koto next review-pr-443 --with-data '{"field_name": "value"}'
  ```
  Use `expects.fields` to know what keys to provide. If `expects.options` is present, it shows which target state each value routes to.

- `blocking_conditions` is non-empty — one or more gates failed. Check `agent_actionable` on each item:
  - `true`: you can override it. Record the override and re-query:
    ```bash
    koto overrides record review-pr-443 --gate <gate_name> --rationale "reason the override is justified"
    koto next review-pr-443
    ```
  - `false`: you can't override this gate programmatically. Resolve the underlying condition it checks (for example, a file that needs to exist, or a check that needs to pass), then re-run `koto next review-pr-443`.

- Both `blocking_conditions` and `expects.fields` are empty — the state auto-advances. Call `koto next review-pr-443` again.

**If `action` is `gate_blocked`:**

The state has no evidence fallback, so you can't submit your way past it. Check `blocking_conditions` the same way as above (check `agent_actionable`). Either record an override or fix the underlying condition, then re-query.

## Continuing the workflow

Keep calling `koto next review-pr-443` after each action until the response contains `"action": "done"`. That signals the workflow has reached a terminal state.

If you lose track of what sessions are active:

```bash
koto workflows
```

If you need to roll back a state (for example, a prior action completed incorrectly):

```bash
koto rewind review-pr-443
```

## Common errors on init

**"session already exists"** — a previous session named `review-pr-443` is still active. Either resume it with `koto next review-pr-443`, or cancel it first and re-initialize:

```bash
koto cancel review-pr-443
koto init review-pr-443 --template .koto/code-review.md
```

**Template variables** — if your template declares variables (e.g., `{{pr_number}}`), pass them at init time:

```bash
koto init review-pr-443 --template .koto/code-review.md --var pr_number=443
```
