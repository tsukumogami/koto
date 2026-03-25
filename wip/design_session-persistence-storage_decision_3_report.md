# Decision 3: Template path substitution for session directories

## Context

Templates reference `wip/` in gate commands and directives. Example gate command today:

```
test -f wip/issue_{{ISSUE_NUMBER}}_context.md
```

When sessions move out of `wip/` (to `~/.koto/sessions/<name>/` or a cloud-synced local path), these hardcoded paths break. The session directory path needs to be available for substitution so templates can reference session artifacts regardless of the configured storage backend.

The PRD requirement R12 states: "Templates that reference wip/ in gate commands or directives must work with the session directory instead."

## Current codebase state

**Variable infrastructure.** The `CompiledTemplate` struct has a `variables: BTreeMap<String, VariableDecl>` field. `VariableDecl` carries `description`, `required` (bool), and `default` (string). The `WorkflowInitialized` event payload has `variables: HashMap<String, serde_json::Value>` for storing resolved variable values at init time. Currently, this field is always `HashMap::new()` -- no `--var` CLI flag exists yet on `koto init`, and no substitution logic runs against gate commands or directives.

**Gate execution.** Gate commands are spawned as `sh -c <command>` in `src/gate.rs` with `current_dir` set to the working directory. The `command` string from the template is passed directly to the shell -- no variable substitution is applied before execution.

**Directive output.** Directives are returned as-is in the `NextResponse` JSON. No substitution is applied.

**MachineState.** The derived state struct holds `current_state`, `template_path`, and `template_hash`. It doesn't carry variables.

**No existing substitution code.** Despite the background mentioning a `Variables::substitute()` function, no such code exists in the codebase today. The `{{KEY}}` pattern replacement would be new work regardless of which option is chosen.

## Options evaluated

### Option A: Built-in template variable {{SESSION_DIR}}

koto injects `SESSION_DIR` as a variable at init time, stored in the `WorkflowInitialized` event alongside user-provided variables. Templates declare it in their variables block (no default, not required). Gate commands become `test -f {{SESSION_DIR}}/issue_{{ISSUE_NUMBER}}_context.md`.

**Strengths:**
- Single substitution mechanism for all variable types. Template authors learn one pattern.
- SESSION_DIR value is persisted in the event log, making state files self-describing. You can replay the log and see exactly what path was used.
- Uses the already-designed (but not yet implemented) `--var` infrastructure. Implementation builds naturally on that work.

**Weaknesses:**
- SESSION_DIR appears in the template's variables block alongside user variables, but behaves differently -- users don't provide it, the engine does. This blurs the user/engine boundary.
- If the session directory changes (e.g., user migrates from local to cloud backend), the persisted value in the init event becomes stale. The engine would need to handle re-resolution.
- Requires all templates that use session paths to declare SESSION_DIR in their variables block, adding boilerplate.

### Option B: Environment variable during command execution

koto sets `KOTO_SESSION_DIR` in the environment before spawning gate commands. Gate commands use shell expansion: `test -f $KOTO_SESSION_DIR/issue_${ISSUE_NUMBER}_context.md`.

**Strengths:**
- No template schema changes. Existing templates don't need a variables block update.
- Environment variables are a well-understood mechanism for passing context to subprocesses.
- The value is always current -- computed at execution time, not persisted at init time.

**Weaknesses:**
- Mixes two substitution systems: `{{VAR}}` for template variables and `$VAR` for environment variables. Template authors must know which system provides which variable.
- Directives can't use environment variables. Directives are returned as JSON strings to agents, not shell-executed. An agent reading "Write findings to $KOTO_SESSION_DIR/research/" would need to resolve the env var itself, which agents can't reliably do.
- Gate commands that use `{{ISSUE_NUMBER}}` alongside `$KOTO_SESSION_DIR` look inconsistent and confusing.

### Option C: Engine-provided variables (separate from user --var)

Distinguish between user variables (from `--var`) and engine variables (SESSION_DIR, WORKFLOW_NAME, etc.). Engine variables are injected automatically at substitution time, not declared in templates. Same `{{KEY}}` syntax. The substitution function merges user and engine variables, with engine variables taking precedence.

**Strengths:**
- Clean conceptual separation. Template authors know: "my custom variables go in the variables block, engine variables are always available."
- Extensible. Adding WORKFLOW_NAME, TEMPLATE_NAME, or PROJECT_DIR later follows the same pattern with no template changes.
- No template boilerplate. SESSION_DIR is available without declaring it.
- Always current. Engine variables are computed at substitution time (during `koto next`), not stored at init time. If the session directory moves, the next substitution picks up the new path.

**Weaknesses:**
- Adds a new concept: "engine variables" vs "user variables." The substitution function needs to know about both sources.
- Risk of name collision: a user `--var SESSION_DIR=foo` would conflict with the engine-provided value. Needs a precedence rule or reserved-name validation.
- Engine variables aren't visible in the event log. Debugging requires knowing that the engine injects certain names.

## Analysis

The decisive factor is directives. Directives are not shell-executed -- they're returned as text to the agent. Option B fails here because `$KOTO_SESSION_DIR` in a directive is a literal string the agent would see but can't expand. Both Option A and Option C handle this through `{{KEY}}` substitution applied before the directive is returned.

Between A and C, the key trade-off is simplicity vs extensibility:

**Option A** is simpler. One variable pool, one substitution pass. But it requires every template to declare SESSION_DIR, and the value persisted at init time can go stale if the backend changes.

**Option C** is cleaner long-term. Engine variables are always available, always current, and don't pollute the template's user-facing variables block. The "new concept" cost is modest -- the substitution function takes two maps instead of one, and documentation lists the available engine variables. The name collision risk is addressable: engine variables use a reserved prefix or the engine rejects `--var` keys that shadow engine names.

The staleness problem in Option A is the real concern. If a user runs `koto init` with the local backend (SESSION_DIR = `~/.koto/sessions/my-workflow/`), then switches to the git backend (SESSION_DIR = `wip/`), Option A would return the old path until the workflow is re-initialized. Option C computes the path fresh each time.

## Recommendation

**Option C: Engine-provided variables.**

Implementation approach:
1. Build the `{{KEY}}` substitution function as shared infrastructure (used by both user vars and engine vars).
2. At substitution time (in `advance_until_stop` and the `koto next` response construction), merge engine-provided variables with user variables. Engine variables take precedence.
3. Start with one engine variable: `SESSION_DIR`. Document it. Add `WORKFLOW_NAME` later if useful.
4. Reject `--var` keys that match engine variable names at `koto init` time, with a clear error message.
5. Apply substitution to both gate commands (before shell execution) and directives (before JSON output).

The substitution function itself is straightforward: scan for `{{KEY}}`, look up in the merged map, replace. Unresolved `{{KEY}}` patterns should produce a warning (not an error) to avoid breaking templates during incremental adoption.

## Impact on other decisions

- **Decision 1 (storage backend trait):** The storage backend must expose a method like `session_dir(&self, workflow_name: &str) -> PathBuf` that the engine calls at substitution time. This is consistent with the PRD's `koto session dir <name>` command.
- **Decision 2 (config system):** No direct impact. The config system determines which backend is active; the backend determines the SESSION_DIR value.
