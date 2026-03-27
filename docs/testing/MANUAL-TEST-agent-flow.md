# Manual Test Checklist: Agent Flow

Structured manual tests for the koto agent integration. Covers plugin installation, skill invocation, the workflow loop, Stop hook behavior, and failure modes.

## When to Run

Run this checklist before any release that changes files under `plugins/`. If you're only changing engine code and nothing in the plugin directory, you can skip it.

## Prerequisites

- koto binary built from the branch under test (`go build -o koto ./cmd/koto`)
- `koto` on PATH (verify with `koto version`)
- Claude Code v1.0.33 or later (plugin support required)
- A clean test directory with no existing `.koto/` subdirectory and no active koto sessions (`koto session list` returns empty)
- No prior koto marketplace or plugin installation (or remove them first with `/plugin uninstall koto-skills@koto`)

## Test 1: Plugin Installation

Install the koto-skills plugin from the marketplace in a fresh project.

### Steps

1. Open Claude Code in the clean test directory.
2. Add the marketplace:
   ```
   /plugin marketplace add tsukumogami/koto
   ```
3. Install the plugin:
   ```
   /plugin install koto-skills@koto
   ```
4. List installed skills to confirm the plugin registered:
   ```
   /skills
   ```

### Pass Criteria

- [ ] Marketplace add completes without errors
- [ ] Plugin install completes without errors
- [ ] `hello-koto` appears in the skills list
- [ ] No unexpected error messages or warnings during installation

### Fail Criteria

- Marketplace add or plugin install produces an error
- `hello-koto` doesn't appear in the skills list after install
- Claude Code prompts for trust but the source isn't `tsukumogami/koto`

## Test 2: Skill Invocation and Workflow Loop

Trigger the hello-koto skill and verify the full init / next / execute / transition / done cycle.

### Steps

1. With the plugin installed (Test 1), invoke the skill:
   ```
   /hello-koto Hasami
   ```
2. Observe the agent's actions. It should:
   - Copy the template to `.koto/templates/hello-koto.md`
   - Run `koto init --template .koto/templates/hello-koto.md --name hello --var SPIRIT_NAME=Hasami`
   - Run `koto next` and receive the awakening directive
   - Submit a greeting from Hasami via `koto context add hello spirit-greeting.txt`
   - Run `koto transition eternal`
   - Run `koto next` and receive the done response
   - Output a completion message

### Pass Criteria

- [ ] Template file exists at `.koto/templates/hello-koto.md` after the skill runs
- [ ] `koto init` returns `{"state":"awakening"}` (or output containing `"awakening"`)
- [ ] `koto next` returns a directive mentioning the spirit name "Hasami"
- [ ] Greeting content was submitted via `koto context add hello spirit-greeting.txt`
- [ ] `koto context exists hello spirit-greeting.txt` exits 0 (content key exists)
- [ ] `koto transition eternal` succeeds (context-exists gate passes)
- [ ] Final `koto next` returns `{"action":"done"}` (or output containing `"done"`)
- [ ] The agent outputs a completion message to the user
- [ ] Session appears in `koto session list` during the workflow and is cleaned up on completion

### Fail Criteria

- Agent doesn't copy the template to `.koto/templates/` before init
- `koto init` fails (template not found, compilation error)
- Gate check fails even though content was submitted via `koto context add`
- Agent gets stuck in a loop or doesn't reach the terminal state
- Agent skips the `koto transition` step and jumps ahead

## Test 3: Stop Hook Behavior

Verify the Stop hook detects an active workflow and outputs a resume reminder.

### Steps

1. Start a workflow but stop the session before it completes:
   - Run `koto init --template .koto/templates/hello-koto.md --name hook-test --var SPIRIT_NAME=TestSpirit`
   - Confirm the session exists: `koto session list` should show `hook-test`
   - Do NOT submit content via `koto context add` (leave the workflow mid-progress)
2. Stop the Claude Code session (Ctrl+C or `/stop`).
3. Watch the hook output in the terminal.
4. Start a new Claude Code session in the same directory.
5. Check if the agent mentions the active workflow or runs `koto next`.

### Pass Criteria

- [ ] On session stop, the hook outputs: `Active koto workflow detected. Run koto next to continue.`
- [ ] The hook message appears in the terminal output (not swallowed silently)
- [ ] On session restart, the agent acknowledges the active workflow or resumes it
- [ ] The hook doesn't produce any output other than the expected reminder line

### Fail Criteria

- Hook produces no output despite an active session
- Hook outputs an error message instead of the reminder
- Hook produces output even though no workflow is active (false positive)

### Cleanup

Clean up the test session before proceeding:
```bash
koto session cleanup hook-test
```

## Test 4: Hook Silent Failure

Verify the Stop hook fails silently when koto isn't available or no workflow is active.

### Scenario A: koto not on PATH

1. Temporarily remove koto from PATH:
   ```bash
   export ORIGINAL_PATH="$PATH"
   export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v "$(dirname $(which koto))" | tr '\n' ':')
   ```
2. Run the hook command directly:
   ```bash
   koto workflows 2>/dev/null | grep -q '"path"' && echo 'Active koto workflow detected. Run koto next to continue.'
   ```
3. Restore PATH:
   ```bash
   export PATH="$ORIGINAL_PATH"
   ```

### Scenario B: No active workflows

1. Make sure no active sessions exist:
   ```bash
   koto session list   # should return empty
   ```
2. Run the hook command directly:
   ```bash
   koto workflows 2>/dev/null | grep -q '"path"' && echo 'Active koto workflow detected. Run koto next to continue.'
   ```

### Pass Criteria

- [ ] Scenario A: no output at all (no error messages, no warnings, exit silently)
- [ ] Scenario A: exit code is non-zero (grep -q doesn't match), but no text is printed to stdout or stderr
- [ ] Scenario B: no output at all (empty workflow list means grep doesn't match)
- [ ] Neither scenario prints error messages, stack traces, or "command not found" text

### Fail Criteria

- Any visible output in either scenario
- Error messages like "koto: command not found" appearing on stderr (the `2>/dev/null` redirect should prevent this, but check)
- The hook command hangs or takes more than a few seconds

## Notes

- These tests assume session storage at `~/.koto/sessions/<repo-id>/<name>/`. Use `koto session dir <name>` to find the exact path for a given session.
- The Stop hook relies on `koto workflows` checking for active sessions. Run all tests from the project root.
- If Test 1 fails, Tests 2 and 3 can still be run by copying the skill files manually into `.claude/skills/hello-koto/` and running `koto init` directly.
