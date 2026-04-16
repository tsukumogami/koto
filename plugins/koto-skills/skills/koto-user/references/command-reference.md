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
```

Initializes a new workflow from a template source file.

| Argument/Flag | Required | Description |
|---|---|---|
| `<name>` | Yes | Workflow name. Must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`. Dots, underscores, and hyphens are allowed after the first character. |
| `--template <path>` | Yes | Path to the template `.md` source file. Compiled automatically on first use. |
| `--parent <parent-name>` | No | Link this workflow as a child of an existing parent workflow. Fails if the parent doesn't exist. |
| `--var KEY=VALUE` | No | Set a template variable. Repeatable. Required variables must be supplied; unknown keys are rejected. |

**Success output:**
```json
{"name": "my-workflow", "state": "initial_state_name"}
```

**Error cases:**
- Exit 2: name format violation, workflow already exists, missing required variable, unknown variable
- Exit 3: template parse failure, template hash mismatch

**Notes:**
- Reserved variable names `SESSION_DIR` and `SESSION_NAME` cannot be declared in templates. They are injected automatically.
- If the template uses legacy-mode gates (no `gates.*` when-clause routing), `koto init` emits a warning to **stderr** and still succeeds.

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

**Output shapes** are determined by the `action` field. See `response-shapes.md` for all nine annotated scenarios.

**Classification priority (highest wins):**
1. Terminal state → `action: "done"`
2. Gate(s) failed + no `accepts` block → `action: "gate_blocked"`
3. Gate(s) failed + `accepts` block present → `action: "evidence_required"` with non-empty `blocking_conditions`
4. Integration declared → `action: "integration_unavailable"` (or `"integration"` when available)
5. `accepts` block present → `action: "evidence_required"`
6. Fallback (no `accepts`, no integration, not terminal) → `action: "evidence_required"` with empty `expects.fields`

**Error output:** structured `NextError` JSON on stderr (see `error-handling.md`).

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

**Success output:**
```json
{"name": "my-workflow", "state": "previous_state_name"}
```

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

**Error cases:**
- Exit 2: workflow not found

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

Valid key paths: `session.backend`, `session.cloud.endpoint`, `session.cloud.bucket`, `session.cloud.region`, `session.cloud.access_key`, `session.cloud.secret_key`.

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
