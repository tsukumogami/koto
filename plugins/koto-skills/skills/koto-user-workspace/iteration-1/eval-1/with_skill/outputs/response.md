# Starting and Running the `review-pr-443` Workflow

Here's exactly what to do to initialize the workflow and complete the first cycle.

---

## Step 1: Verify koto is available

```bash
koto version
```

If this returns an error, koto isn't on PATH. Stop and resolve that first.

---

## Step 2: Initialize the workflow

```bash
koto init review-pr-443 --template .koto/code-review.md
```

This creates a new session named `review-pr-443` using the template at `.koto/code-review.md`. koto compiles the template automatically on first use.

On success, you'll see something like:

```json
{"name": "review-pr-443", "state": "initial_state_name"}
```

The `state` field shows which state the workflow started in.

**If you get an error:**
- Exit 2 with `"workflow already exists"` — a session named `review-pr-443` is already active. Run `koto next review-pr-443` to resume it instead.
- Exit 2 with a variable error — the template declares required variables you haven't supplied. Add `--var KEY=VALUE` flags for each required variable.
- Exit 3 — the template failed to parse. Check the template file for syntax errors.

---

## Step 3: Run the first cycle

```bash
koto next review-pr-443
```

This returns a JSON object. The `action` field tells you what to do next.

---

## Step 4: Act on the response

Read the `action` field and follow the matching path:

### `action: "evidence_required"`

Read `directive` for instructions. Then check `blocking_conditions` and `expects.fields`:

**No gates blocking, fields present** (`blocking_conditions` is `[]`, `expects.fields` is non-empty):

The workflow is waiting for your input. Check `expects.fields` for the schema and submit:

```bash
koto next review-pr-443 --with-data '{"field_name": "value"}'
```

For example, if `expects.fields` has an `outcome` enum with values `["approve", "reject", "defer"]`:

```bash
koto next review-pr-443 --with-data '{"outcome": "approve"}'
```

**Gates blocking, fields present** (`blocking_conditions` is non-empty, `expects.fields` is non-empty):

One or more gates failed. For each item in `blocking_conditions`:
- If `agent_actionable` is `true`: record an override, then re-query:
  ```bash
  koto overrides record review-pr-443 --gate <gate_name> --rationale "<why this is justified>"
  koto next review-pr-443
  ```
- If `agent_actionable` is `false`: you can't override this gate. Report the blocking condition to the user and wait.

**Empty fields** (`blocking_conditions` is `[]`, `expects.fields` is `{}`):

Call `koto next` again without `--with-data`. The engine will auto-advance.

---

### `action: "gate_blocked"`

One or more gates failed and there's no evidence fallback. Read `directive` and `blocking_conditions`.

- If `agent_actionable` is `true`: record an override:
  ```bash
  koto overrides record review-pr-443 --gate <gate_name> --rationale "<why this is justified>"
  koto next review-pr-443
  ```
- If `agent_actionable` is `false`: escalate to the user. Don't retry in a loop — the condition is externally controlled.

---

### `action: "integration_unavailable"`

The template uses an integration runner that isn't configured. Report this to the user — it can't be resolved by the agent alone.

---

### `action: "confirm"`

A default action ran and needs your confirmation. Read `directive` and `action_output` (which contains the command, exit code, stdout, and stderr). If the output looks correct, submit confirmation per the `expects.fields` schema.

---

### `action: "done"`

The workflow reached a terminal state. No further action is needed. Note: `directive` is absent on `done` responses — don't try to read it.

---

## Continuing after the first cycle

Keep calling `koto next review-pr-443` (submitting evidence when required) until you see `action: "done"`. Each call returns the current state directive and tells you what to do next.

If the session is interrupted, just run `koto next review-pr-443` to resume — koto persists all state atomically.

To see all active sessions if you lose track of the name:

```bash
koto workflows
```
