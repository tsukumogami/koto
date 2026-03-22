# Phase 3 Research: Shirabe Invocation Mechanics

## Questions Investigated

- How should the skill call `koto init`? What args? Where does the template live?
- What does `koto next` return when a state has a directive and is waiting for evidence?
- How does the agent submit evidence with `koto next --with-data`? What does koto return on success vs. validation failure?
- How does the agent resume after session interruption?
- Does shirabe or koto use a SessionStart hook for auto-resume?
- Where should the merged work-on template live? What does koto's discovery mechanism support?

## Findings

### 1. Template Initialization

`koto init` takes exactly two arguments: `name` (positional) and `--template <path>` (required flag). There is no `--var` flag in the current CLI implementation — the `variables` field in the init event is hardcoded to an empty `HashMap`. The AGENTS.md in `koto-skills` documents `--var SPIRIT_NAME=<name>` but this is aspirational; it isn't implemented in `src/cli/mod.rs`.

The init command:
1. Resolves the state file path as `<cwd>/koto-<name>.state.jsonl`
2. Compiles the template source file and caches the compiled JSON to `$XDG_CACHE_HOME/koto/<hash>.json` (or `~/.cache/koto/`)
3. Writes a header line + `workflow_initialized` event (seq 1) + `transitioned` event (seq 2, `from: null`)
4. Prints `{"name": "<name>", "state": "<initial_state>"}` on success

So the skill would call:
```
koto init <workflow-name> --template <path-to-template>
```

The `<workflow-name>` can be anything unique per project. A reasonable convention is `work-on-<issue-number>` or `work-on-<slug>`.

### 2. Template Location

koto has no built-in template discovery mechanism. The `--template` flag takes an explicit file path. There is no registry, no `.koto/templates` discovery, no search path. The `discover.rs` module only discovers active *workflow state files* (`koto-*.state.jsonl` in cwd), not template files.

The AGENTS.md for hello-koto establishes a convention: copy the template to `.koto/templates/<name>.md` in the project. This is a documentation convention, not enforced by koto. The path passed to `--template` can be anywhere on disk.

For shirabe, there are three viable options:
- **(a) Bundled with shirabe plugin**: Template lives at a stable path inside the shirabe plugin directory (e.g., `<shirabe-plugin-dir>/koto-templates/work-on.md`). The skill knows this path at runtime because it knows where it's installed.
- **(b) Copied to user project `.koto/templates/`**: The skill checks for `.koto/templates/work-on.md` in cwd and copies it there from the plugin if missing. This is the pattern AGENTS.md documents for hello-koto.
- **(c) Fetched from a registry**: Not supported by koto today.

The `shirabe/koto-templates/` directory exists but is empty (only `.gitkeep`). The design intent is for templates to live in shirabe, but the mechanism for the skill to resolve the template path isn't implemented yet.

### 3. Directive Loop

After `koto init`, the skill calls `koto next <name>` to get the current directive. The output contract is:

For a non-terminal state waiting for evidence (`EvidenceRequired`):
```json
{
  "action": "execute",
  "state": "<state-name>",
  "directive": "<full directive text>",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": { "<field>": { "type": "<type>", "required": true/false } },
    "options": [{ "target": "<state>", "when": { "<field>": "<value>" } }]
  },
  "error": null
}
```

For an auto-advancing state (no `accepts` block), koto returns the same `EvidenceRequired` shape but with `expects.fields` empty. The `options` field is omitted when empty (`skip_serializing_if`). The agent reads `directive` and executes it, then calls `koto next <name>` again (no `--with-data`) to advance through auto-advancing states.

The agent knows which state it's in from `response.state`. The `action` field is `"execute"` for all non-terminal responses and `"done"` for terminal.

Auto-advancement runs inside the `koto next` call itself — the engine loops internally until it hits a stopping condition (terminal, gate blocked, evidence required, integration). One `koto next` call can advance through multiple states.

### 4. Evidence Submission

When a state has an `accepts` block, the agent must call:
```
koto next <name> --with-data '{"field": "value"}'
```

The `--with-data` payload is validated against the `accepts` schema before being written. Validation covers: required fields present, type matching (string/boolean/enum with allowed values), no extra fields.

On success: koto appends an `evidence_submitted` event, then runs the advancement loop from the current state. Returns a `NextResponse` (typically advancing to the next state and stopping at the next evidence gate or terminal).

On schema validation failure: exits with code 2 and returns:
```json
{
  "error": {
    "code": "invalid_submission",
    "message": "evidence validation failed",
    "details": [{ "field": "<field>", "reason": "<why>" }]
  }
}
```

The `--with-data` and `--to` flags are mutually exclusive. `--to` performs a directed transition without evidence validation, useful for exception paths (e.g., early termination).

### 5. Session Resume

There is no `koto status` or `koto query` command in the current implementation. The `Command` enum in `src/cli/mod.rs` defines: `Version`, `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`. No status/query subcommand exists.

Resume is done with two commands:
1. `koto workflows` — scans cwd for `koto-*.state.jsonl` files, returns array of `{name, created_at, template_hash}`. Used to find which workflows are active.
2. `koto next <name>` — reads current state from the event log and returns the directive. This is the resume command. It tells the agent both where it is (`state`) and what to do next (`directive`).

So the resume flow is: call `koto workflows` to discover the workflow name, then call `koto next <name>` to get the current directive. If the agent already knows the workflow name (e.g., from a wip state file), it can skip `koto workflows` and call `koto next` directly.

### 6. SessionStart Hook (Auto-resume)

koto's `koto-skills` plugin defines hooks in `plugins/koto-skills/hooks.json`. The current hook is a **Stop** hook (fires when the agent session ends), not a SessionStart hook:

```json
{
  "hooks": {
    "Stop": [{
      "type": "command",
      "command": "WORKFLOWS=$(koto workflows 2>/dev/null); if [ \"$WORKFLOWS\" != \"[]\" ] && [ -n \"$WORKFLOWS\" ] && echo \"$WORKFLOWS\" | grep -qE '^\\[.*\\]$'; then echo 'Active koto workflow detected. Run `koto next <name>` to resume.'; fi"
    }]
  }
}
```

This Stop hook checks for active workflows and prints a reminder. There is **no SessionStart hook** in the current koto-skills implementation. Auto-resume on session start is not implemented.

If multiple workflows are active, the Stop hook fires once and lists them all (the `koto workflows` output is an array). There's no per-workflow disambiguation in the hook.

For shirabe, a SessionStart hook would need to identify the active work-on workflow by name convention (e.g., match `koto-work-on-*.state.jsonl`). Without a SessionStart hook in Claude Code's hook system, resume is triggered by the agent reading the Stop hook reminder from the previous session and acting on it, or the user explicitly asking to resume.

### 7. The `koto init` Name Argument Discrepancy

The AGENTS.md for hello-koto documents `koto init --template ... --name hello` but the actual CLI signature is `koto init <name> --template <path>` (positional name, not `--name` flag). The documented `--var` flag also doesn't exist. AGENTS.md appears to be written for a planned CLI design that wasn't yet implemented when it was written.

## Implications for Design

**Template path resolution**: The skill must know where its template file is. Since shirabe's `koto-templates/` directory is the designated location, the skill needs a reliable way to resolve this path at runtime. The most practical approach: the skill checks for the template at `<skill-dir>/../koto-templates/work-on.md` (relative to the skill file's location), then copies it to `.koto/templates/work-on.md` in the project if it doesn't exist, and passes the project-local copy to `koto init`. This mirrors the hello-koto convention and ensures the template path is stable even if the plugin moves.

**Workflow naming**: The skill needs a deterministic workflow name so it can call `koto next <name>` without first running `koto workflows`. Convention: `work-on-<issue-number>` for GitHub issue mode, `just-do-it-<slug>` for free-form mode. This avoids collisions and makes `koto next` directly callable on resume.

**Variables not supported**: Template variable substitution (`{{ISSUE_NUMBER}}`, `{{DESCRIPTION}}`) isn't implemented in the CLI yet. The skill can't pass variables at init time. Directive text must be static or the skill must pre-process the template with a different mechanism. This is a gap that needs either CLI work (add `--var` flag) or a workaround (e.g., write a customized template per invocation).

**Evidence submission error handling**: On `invalid_submission` (exit code 2), the agent should not retry the same payload. It must re-read the `details` array, fix the evidence, and resubmit. The skill loop needs to handle this case explicitly.

**No `koto status`/`koto query`**: The skill can't ask "what state am I in?" without calling `koto next`, which also runs the advancement loop. If the skill only needs to inspect state without advancing, it must parse the state file directly or use `koto workflows` and then `koto next`. Designing around this: always use `koto next` for resume, accept that it may advance through auto-advancing states.

**SessionStart hook gap**: Claude Code doesn't currently support SessionStart hooks in `hooks.json` (only Stop). Auto-resume requires either a workaround (skill instructions tell the agent to check for active workflows on first invocation) or the Stop hook reminder pattern already in koto-skills. The skill's own instructions can include: "at the start of a session, run `koto workflows` and if a `work-on-*` workflow exists, resume it."

## Surprises

- **No `koto status` or `koto query` command**: The CLAUDE.local.md for koto lists these as key commands but they don't exist in the implementation. This is significant: resume must go through `koto next`, not a read-only inspection command.
- **`--var` flag is not implemented**: The hello-koto AGENTS.md documents `--var` as if it exists, but `koto init` only takes `name` and `--template`. Template variables can't be passed at init time today.
- **AGENTS.md uses `--name` flag syntax**: The actual CLI uses a positional argument for `name`, not `--name`. The AGENTS.md docs are ahead of the implementation.
- **Stop hook, not SessionStart**: The hook system fires on session *end*, not start. Auto-resume reminders work as hints but the agent must act on them in the next session.
- **Template discovery is only for state files**: `discover.rs` only finds `koto-*.state.jsonl` workflow state files. There is no template discovery. Template paths are always explicit.

## Summary

`koto init <name> --template <path>` initializes a workflow with a state file in cwd; the template is compiled and cached automatically. After init, `koto next <name>` is the single command for both getting directives and advancing state — it returns `{"action": "execute", "state": "...", "directive": "..."}` for non-terminal states and `{"action": "done"}` for terminal. Evidence submission uses `koto next <name> --with-data '{"field": "value"}'` and returns `{"error": {"code": "invalid_submission", ...}}` on schema failure. There is no SessionStart hook or `koto status`/`koto query` command; resume is handled by calling `koto next <name>` directly after finding active workflows via `koto workflows`. Two significant gaps affect the design: template variable substitution (`--var`) is not implemented, and the AGENTS.md docs describe a CLI that doesn't yet exist.
