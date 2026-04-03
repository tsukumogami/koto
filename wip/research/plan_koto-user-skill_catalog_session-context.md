# koto-user Skill Catalog: Session, Context, Config, Export, and Related Surfaces

Sources read: `src/session/mod.rs`, `src/session/context.rs`, `src/session/local.rs`,
`src/config/mod.rs`, `src/config/resolve.rs`, `src/export/mod.rs`, `src/export/mermaid.rs`,
`src/action.rs`, `src/cli/mod.rs`, `src/cli/session.rs`, `src/cli/context.rs`,
`src/cli/vars.rs`, `tests/integration_test.rs`.

---

## 1. `koto session` Subcommands

### `koto session dir <name>`

Prints the absolute filesystem path of the session directory for `<name>` to stdout (plain text, not JSON). Does not require the session to exist — the path is computed purely from the session name.

```
koto session dir my-workflow
# => /home/user/.koto/sessions/a1b2c3d4e5f6a7b8/my-workflow
```

Exit 0 always (path computation cannot fail). The path always contains the session name as the last component.

**Why an agent needs this:** gate commands and template directives can reference `{{SESSION_DIR}}`, but an agent that wants to write files to the session directory directly (for multi-agent handoffs, artifact storage, etc.) needs the resolved path. `session dir` is the canonical way to get it without parsing `next` output.

**Skill placement:** inline in `koto-user` SKILL.md, one paragraph. Mention that `{{SESSION_DIR}}` substitution in directives/gates is the passive form of the same path.

---

### `koto session list`

Prints a JSON array of session objects to stdout. Returns an empty array `[]` when no sessions exist.

```
koto session list
```

Each object in the array has:
```json
{
  "id": "my-workflow",
  "created_at": "2026-01-15T10:30:00Z",
  "template_hash": "abc123..."
}
```

Sessions are sorted by `id` (alphabetical). Sessions whose state files cannot be read are silently skipped with a warning to stderr.

Exit 0 always.

**Why an agent needs this:** checking whether a named workflow already exists before calling `init`, or auditing what is running. The `workflows` command also lists sessions but includes `name` (not `id`) and is scoped to the current working directory. `session list` is lower-level and shows all sessions under the repo-id bucket regardless of template presence.

Note: `workflows` returns `{ name, created_at, template_hash }` while `session list` returns `{ id, created_at, template_hash }` — the field name differs (`name` vs `id`).

**Skill placement:** inline in `koto-user` SKILL.md, brief table of fields.

---

### `koto session cleanup <name>`

Removes the entire session directory for `<name>` (including state file, context store, and all artifacts). Idempotent: succeeds even if the session does not exist.

```
koto session cleanup my-workflow
```

Produces no stdout output. Exit 0 on success or missing session.

**Why an agent needs this:** manual cleanup when a workflow was abandoned mid-run, or after using `--no-cleanup` during debugging. Under normal operation, `koto next` auto-cleans when a terminal state is reached, so manual cleanup is an edge case.

**Skill placement:** inline in `koto-user` SKILL.md. One sentence. Cross-reference `--no-cleanup` flag on `next`.

---

### `koto session resolve <name> --keep <local|remote>`

Resolves a version conflict when using the cloud backend. Only valid when `session.backend = "cloud"`. Fails with an error if called against the local backend.

```
koto session resolve my-workflow --keep local
koto session resolve my-workflow --keep remote
```

**Why an agent needs this:** cloud sync can produce conflicts. Not relevant to agents using the default local backend.

**Skill placement:** reference file entry only (`references/session-management.md`), guarded by "cloud backend only" note. Do not place inline — most agents will never encounter this.

---

## 2. `koto context` Subcommands

The context store provides a key-value blob store scoped to a session. Keys are hierarchical path strings (e.g., `scope.md`, `research/r1/lead-cli-ux.md`). Content is arbitrary bytes (text or binary). The store is backed by `~/.koto/sessions/<repo-id>/<session>/ctx/` on the local backend.

### `koto context add <session> <key>`

Reads content from stdin and stores it under `<key>` in `<session>`'s context store. Overwrites any existing content at that key.

```
echo "scope content" | koto context add my-workflow scope.md
koto context add my-workflow scope.md --from-file ./scope.md
```

Flags:
- `--from-file <path>`: read from a file instead of stdin.

No stdout output on success. Exit 0 on success, exit 3 on infrastructure errors (key validation failure, I/O errors).

Key constraints (from `src/session/validate.rs` — not read directly but inferred from tests):
- Must not start with `.` or contain `..` (path traversal rejected)
- Maximum 255 characters
- Hierarchical segments separated by `/` are allowed and create subdirectories in the ctx store

**Why an agent needs this:** storing research artifacts, intermediate outputs, or cross-state data that gate commands or subsequent state directives need to access via `{{SESSION_DIR}}/ctx/<key>`.

**Skill placement:** reference file `references/context-store.md` with full syntax. Summarize in SKILL.md: one paragraph covering `add`, `get`, `exists`, `list` as a group.

---

### `koto context get <session> <key>`

Retrieves stored content and writes it to stdout (raw bytes, not JSON-wrapped).

```
koto context get my-workflow scope.md
koto context get my-workflow scope.md --to-file ./local-copy.md
```

Flags:
- `--to-file <path>`: write to a file instead of stdout. Creates parent directories if needed.

Exit 0 on success, exit 3 on errors (key not found, I/O errors).

**Skill placement:** same reference file as `add`. Covered in the same paragraph in SKILL.md.

---

### `koto context exists <session> <key>`

Checks whether a key exists. No stdout output.

```
koto context exists my-workflow scope.md
echo $?   # 0 if present, 1 if absent
```

Exit 0 if present, exit 1 if absent. Never errors on missing keys — absent keys return exit 1, not an error.

**Why an agent needs this:** scripted gate commands or conditional logic before attempting `context get`. Also directly usable in `gates: command:` entries via `koto context exists ...`.

**Skill placement:** inline in `koto-user` SKILL.md as part of the context group paragraph. Flag the exit-code-as-boolean contract explicitly (it differs from other commands which use JSON errors).

---

### `koto context list <session>`

Lists all keys as a JSON array, sorted alphabetically.

```
koto context list my-workflow
# => ["alpha.md","beta.md","research/r1/lead.md"]

koto context list my-workflow --prefix research/
# => ["research/r1/lead.md"]
```

Flags:
- `--prefix <string>`: filter keys to those starting with the given prefix.

Returns `[]` when no keys exist or no keys match the prefix. Exit 0 always.

**Skill placement:** same reference file as `add`. Cover in the same paragraph in SKILL.md.

---

## 3. `koto template export` (Visual Tooling)

### `koto template export <input> [flags]`

Generates a visual diagram of a template's state machine. Accepts either a `.md` source file or a pre-compiled `.json` file as `<input>`.

```
# Print Mermaid stateDiagram-v2 to stdout (raw, for composability)
koto template export workflow.md

# Write Mermaid to file (wrapped in fenced code block for GitHub rendering)
koto template export workflow.md --format mermaid --output diagram.md

# Generate HTML and open in browser
koto template export workflow.md --format html --output diagram.html --open

# Verify existing output matches template (CI freshness check)
koto template export workflow.md --format mermaid --output diagram.md --check
```

Flags:
- `--format <mermaid|html>`: output format. Default: `mermaid`.
- `--output <path>`: write to file. Required for `--format html`. When present with `mermaid`, wraps output in a fenced code block; when absent (stdout), outputs raw mermaid text.
- `--open`: open the generated file in the default browser. Only valid with `--format html`. Mutually exclusive with `--check`.
- `--check`: verify the existing file at `--output` matches what would be generated. Exits 1 if stale or missing, 0 if fresh. Prints a remediation command on failure.

Flag constraint violations exit 2 with a plain-text error to stderr (not JSON).

The Mermaid output is `stateDiagram-v2` format with:
- `direction LR` layout
- `[*] --> <initial_state>` entry arrow
- `<state> --> [*]` exit arrows for terminal states
- Labeled transitions when a `when:` condition is present
- `note left of <state>` annotations for gate names

**Why an agent needs this:** template authors use `export` during development. koto-user agents don't typically call this, but they may encounter `--check` in CI pipelines. The `--check` flag's error output tells the agent exactly what command to run to fix the staleness.

**Skill placement:** `koto-author` SKILL.md (primary owner). Add a one-sentence pointer in `koto-user` SKILL.md under "other commands" or a last-resort `docs/` pointer. koto-user agents rarely need this.

---

## 4. Config File Format

### File locations

| File | Path | Scope |
|------|------|-------|
| User config | `~/.koto/config.toml` | All projects for this user |
| Project config | `.koto/config.toml` (relative to cwd) | Current project only |

Precedence (highest to lowest): environment variables > project config > user config > built-in defaults.

### TOML schema

```toml
[session]
backend = "local"    # or "cloud" (default: "local")

[session.cloud]
endpoint = "https://s3.example.com"
bucket = "my-koto-sessions"
region = "us-east-1"
access_key = "AKIAIOSFODNN7EXAMPLE"   # or set AWS_ACCESS_KEY_ID env var
secret_key = "..."                    # or set AWS_SECRET_ACCESS_KEY env var
```

Only `session.backend` and the `session.cloud.*` keys are recognized. Unknown keys are ignored (TOML parsing is lenient via serde's default).

### `koto config` subcommands

```
koto config get <key>           # Print value of a dotted key; exit 1 if unset
koto config set <key> <value>   # Write to project config (.koto/config.toml)
koto config set <key> <value> --user   # Write to user config (~/.koto/config.toml)
koto config unset <key>         # Remove from project config
koto config unset <key> --user  # Remove from user config
koto config list                # Print merged config as TOML
koto config list --json         # Print merged config as JSON
```

Valid key paths: `session.backend`, `session.cloud.endpoint`, `session.cloud.bucket`, `session.cloud.region`, `session.cloud.access_key`, `session.cloud.secret_key`.

`config list` redacts credentials (`access_key` and `secret_key` shown as `"<set>"` if configured).

**Why an agent needs this:** almost never. Agents using the default local backend don't need to configure anything. An agent setting up the cloud backend for shared session sync would call `koto config set session.backend cloud` and configure the cloud keys. This is infrastructure setup, not workflow execution.

**Skill placement:** reference file `references/configuration.md` covering file paths, schema, and all `koto config` subcommands. Do not place inline in SKILL.md — this is a last-resort surface for most agents.

---

## 5. Session Directory Structure

The local backend stores all sessions at:

```
~/.koto/sessions/<repo-id>/<session-name>/
```

where `<repo-id>` is the first 16 hex characters of the SHA-256 hash of the canonicalized working directory path. Two shells in the same directory produce the same repo-id.

```
~/.koto/sessions/<repo-id>/<session-name>/
├── koto-<session-name>.state.jsonl   # Append-only event log (header line + events)
└── ctx/
    ├── manifest.json                  # Index of context keys (size, hash, created_at)
    ├── manifest.lock                  # Advisory flock coordination file
    ├── scope.md                       # Example flat key
    └── research/
        └── r1/
            └── lead-cli-ux.md         # Example hierarchical key
```

The state file format is JSONL: the first line is the header (`schema_version`, `workflow`, `template_hash`, `created_at`), followed by one event per line (`seq`, `timestamp`, `type`, `payload`).

The `ctx/` directory mirrors the key namespace directly: `context add <session> research/r1/lead.md` creates `ctx/research/r1/lead.md` on disk. `manifest.json` is the authoritative index; files in `ctx/` without a manifest entry are orphaned and not reported by `context list`.

`~/.koto/` is created with mode `0700` (user-only). Session directories inherit the default umask.

**KOTO_SESSIONS_BASE env var:** when set, overrides `~/.koto/sessions/<repo-id>/` entirely. Used in integration tests to control storage location. Agents should not set this.

**Why an agent needs to know the structure:**
- Gate commands that use `{{SESSION_DIR}}` land in `~/.koto/sessions/<repo-id>/<session-name>/`. The agent can write files there for later states to consume.
- `koto session dir <name>` returns this path without needing to know the repo-id.
- If a workflow is abandoned and the agent needs to manually inspect the event log, this is where to look.

**Skill placement:** reference file `references/session-directory.md`. Inline in SKILL.md: one sentence explaining what `{{SESSION_DIR}}` resolves to and that `koto session dir <name>` prints it. Full directory layout belongs in the reference file.

---

## 6. `koto cancel`

### `koto cancel <name>`

Marks a workflow as cancelled, preventing any further advancement.

```
koto cancel my-workflow
```

On success, outputs JSON:
```json
{ "cancelled": true, "name": "my-workflow" }
```

After cancellation:
- `koto next` returns exit 2 with `error.code = "terminal_state"` and a message containing "cancelled".
- A second `koto cancel` returns exit 2 with an error string containing "already cancelled".
- Cancelling an already-terminal workflow returns exit 2 with an error string containing "terminal state".

Session cleanup: cancelling does NOT auto-clean the session directory. The session persists until explicitly cleaned with `koto session cleanup` or `koto next` reaches a terminal state.

**Why an agent needs this:** abandoning a workflow that can't proceed (e.g., the user requested an abort, or a prerequisite failed irrecoverably). Unlike reaching a terminal state via `next`, cancel can be called from any non-terminal state.

**Skill placement:** inline in `koto-user` SKILL.md. One paragraph covering the command, the success JSON shape, and what happens on double-cancel and post-cancel `next`. This is part of the core loop error handling surface that agents need to understand.

---

## 7. Other CLI Surfaces Not Covered by the Core Loop

### `koto version [--json]`

Prints version information. Without `--json`, plain text: `koto <version> (<commit> <built_at>)`. With `--json`: `{ "version": "...", "commit": "...", "built_at": "..." }`.

**Skill placement:** not needed in either skill. Utility command for debugging.

---

### `koto template compile <source> [--allow-legacy-gates]`

Compiles a markdown template source to a cached JSON file. Prints the path to the compiled JSON on stdout. The compiled file is stored in a content-addressed cache (typically `~/.koto/cache/`).

```
koto template compile my-workflow.md
# => /home/user/.koto/cache/abc123def456.json
```

`--allow-legacy-gates`: suppresses the D5 error for gates that lack `gates.*` when-clause routing. Transitory flag for migration.

**Skill placement:** `koto-author` SKILL.md (primary owner). `koto-user` only needs to know that `koto init --template <path>` accepts the raw source directly (compilation is implicit).

---

### `koto template validate <path>`

Validates a compiled template JSON file for structural correctness. Exits 0 if valid, 1 on validation errors.

**Skill placement:** `koto-author` only. Agents running workflows don't validate templates.

---

### `koto decisions record <name> --with-data '<json>'`

Records a structured decision event without advancing state. The `--with-data` JSON must include `"choice"` and `"rationale"` fields.

```
koto decisions record my-workflow --with-data '{"choice":"implement","rationale":"tests pass"}'
```

### `koto decisions list <name>`

Lists accumulated decisions for the current state as a JSON array.

These are covered in the existing koto-user documentation surface (mentioned in the CLAUDE.md Quick Reference table). Worth confirming both skills document the `decisions` subgroup.

**Skill placement:** `koto-user` SKILL.md, reference file entry. Same depth as overrides.

---

### `koto workflows`

Lists active workflows in the current directory as a JSON array. Each entry: `{ "name": "...", "created_at": "...", "template_hash": "..." }`. Returns `[]` when no workflows exist. Already documented in existing koto-user guidance.

**Skill placement:** already covered. Confirm field names (`name`, not `id`) differ from `session list` (`id`).

---

### Template variables (`--var KEY=VALUE` on `koto init`)

Templates can declare variables in the `variables:` frontmatter block. Required variables must be supplied at `koto init` time; optional variables use declared defaults. Reserved names `SESSION_DIR` and `SESSION_NAME` cannot be declared in templates (compiler rejects them; `koto next` returns `error.code = "template_error"` with `exit 3` if a template somehow got through with them).

```
koto init my-workflow --template workflow.md --var OWNER=alice --var ENV=prod
```

Variable values are substituted as `{{KEY}}` tokens in gate commands and state directives at runtime. Substitution is non-recursive (a value containing `{{X}}` does not trigger further substitution).

**Skill placement:** `koto-author` SKILL.md owns declaration syntax. `koto-user` SKILL.md should cover: (1) passing `--var` at init time, (2) that `{{SESSION_DIR}}` is always available without declaration, (3) that `{{SESSION_NAME}}` is also reserved and available. This is relevant because agents calling `koto init` for a template that requires variables must supply them.

---

## Summary: Skill Placement Decisions

| Surface | Skill File | Depth |
|---------|-----------|-------|
| `koto session dir` | `koto-user` SKILL.md | Inline (one paragraph) |
| `koto session list` | `koto-user` SKILL.md | Inline (field table) |
| `koto session cleanup` | `koto-user` SKILL.md | Inline (one sentence + cross-ref `--no-cleanup`) |
| `koto session resolve` | `references/session-management.md` | Reference only |
| `koto context add/get/exists/list` | `koto-user` SKILL.md + `references/context-store.md` | Summary inline; full syntax in reference |
| `koto template export` | `koto-author` SKILL.md (primary); pointer in `koto-user` | Author: inline; user: last-resort pointer |
| Config file format + `koto config` | `references/configuration.md` | Reference only (not inline in SKILL.md) |
| Session directory structure | `koto-user` SKILL.md + `references/session-directory.md` | One sentence inline; layout diagram in reference |
| `koto cancel` | `koto-user` SKILL.md | Inline (one paragraph, error cases included) |
| `koto version` | Neither skill | Omit |
| `koto template compile/validate` | `koto-author` SKILL.md | Author only |
| `koto decisions record/list` | `koto-user` SKILL.md | Reference file entry |
| `koto workflows` | `koto-user` SKILL.md | Already covered; verify `name` vs `id` field naming |
| Template variables (`--var`, `{{SESSION_DIR}}`) | Both skills | `koto-author`: declaration; `koto-user`: consumption at init + substitution behavior |

---

## Key Findings for Skill Authors

1. **`session list` vs `workflows` field naming**: `session list` uses `id`, `workflows` uses `name`. Both identify the session by name — an agent comparing output from both commands must use the right field. This is a gotcha worth calling out explicitly in koto-user.

2. **`context exists` exit code contract**: unlike all other `context` commands (which emit JSON errors on failure), `exists` uses raw exit codes (0/1). This is intentional for scripting composability. Skill must document this clearly.

3. **Auto-cleanup behavior**: `koto next` cleans the session directory by default when reaching a terminal state. `--no-cleanup` suppresses this. `koto cancel` does NOT auto-clean. `koto session cleanup` is always available for manual removal. This interaction is important for agents that need to inspect artifacts after a workflow ends.

4. **`{{SESSION_DIR}}` is the primary cross-state artifact sharing mechanism**: agents write files to the session directory in one state and gate commands (or subsequent state directives) read them in the next. `koto session dir <name>` makes this path accessible programmatically. The full path is `~/.koto/sessions/<repo-id>/<name>/`.

5. **Config is a last-resort surface**: default local backend needs no config. Only mention config in the skill for cloud backend setup. Keep it in a reference file.

6. **`template export` is author tooling**: koto-user agents encounter it only in CI (via `--check`). A one-line pointer to docs is sufficient for the user skill.

7. **`koto cancel` is the agent's abort signal**: document the double-cancel and post-cancel `next` behavior because agents may encounter these error codes and need to know they're not retryable.
