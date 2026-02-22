# Test Plan: koto-engine

Generated from: docs/designs/DESIGN-koto-engine.md
Issues covered: 7
Total scenarios: 25

---

## Scenario 1: Go module builds and vets cleanly
**ID**: scenario-1
**Testable after**: #4
**Commands**:
- `go build ./...`
- `go vet ./...`
**Expected**: Both commands exit 0 with no output. No external dependencies beyond the Go standard library.
**Status**: passed

---

## Scenario 2: Init creates a valid state file
**ID**: scenario-2
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
- `cat /tmp/test-dir/koto-test-workflow.state.json`
**Expected**: State file exists at the expected path. JSON contains `schema_version: 1`, `version: 1`, `current_state` set to the machine's initial state, empty `history` array, and a `workflow` object with `name`, `template_hash`, `template_path`, and `created_at` fields.
**Status**: passed

---

## Scenario 3: Init output is valid JSON with state and path
**ID**: scenario-3
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
**Expected**: stdout is valid JSON matching `{"state": "<initial-state>", "path": "<state-file-path>"}`. Exit code 0.
**Status**: passed

---

## Scenario 4: Transition advances state and increments version
**ID**: scenario-4
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
- `koto transition <next-state> --state /tmp/test-dir/koto-test-workflow.state.json`
**Expected**: stdout is valid JSON matching `{"state": "<next-state>", "version": 2}`. The state file on disk reflects `current_state` as the target state, `version` as 2, and history has one entry with `type: "transition"`.
**Status**: passed

---

## Scenario 5: Transition from terminal state returns terminal_state error
**ID**: scenario-5
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
- `koto transition <intermediate-states...> --state <path>` (advance to terminal)
- `koto transition <any-state> --state <path>`
**Expected**: Exit code 1. stdout is JSON: `{"error": {"code": "terminal_state", "message": "...", "current_state": "<terminal>"}}`.
**Status**: passed

---

## Scenario 6: Transition to invalid target returns invalid_transition error
**ID**: scenario-6
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
- `koto transition nonexistent-target --state <path>`
**Expected**: Exit code 1. stdout is JSON with `error.code` equal to `"invalid_transition"` and `error.valid_transitions` listing the allowed targets from the current state.
**Status**: passed

---

## Scenario 7: Next returns execute directive for non-terminal state
**ID**: scenario-7
**Testable after**: #4
**Commands**:
- `koto init --name test-workflow --template /tmp/test.md --state-dir /tmp/test-dir`
- `koto next --state <path>`
**Expected**: stdout is valid JSON with `action: "execute"`, `state` matching the current state, and a non-empty `directive` string. Exit code 0.
**Status**: passed

---

## Scenario 8: Next returns done directive for terminal state
**ID**: scenario-8
**Testable after**: #4
**Commands**:
- (Advance workflow to terminal state via init + transitions)
- `koto next --state <path>`
**Expected**: stdout is valid JSON with `action: "done"`, `state` matching the terminal state, and a `message` field. Exit code 0.
**Status**: passed

---

## Scenario 9: Stub subcommands return not_implemented
**ID**: scenario-9
**Testable after**: #4
**Commands**:
- `koto rewind`
- `koto cancel`
- `koto query`
- `koto status`
- `koto validate`
- `koto workflows`
**Expected**: Each command outputs JSON with `error.code` equal to `"not_implemented"`. These stubs are placeholders until later issues implement them.
**Status**: passed

---

## Scenario 10: Rewind to previously visited state succeeds
**ID**: scenario-10
**Testable after**: #5
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- `koto transition state-b --state <path>`
- `koto transition state-c --state <path>`
- `koto rewind --to state-b --state <path>`
**Expected**: Exit code 0. State file shows `current_state: "state-b"`. History contains a new entry with `type: "rewind"`, `from: "state-c"`, `to: "state-b"`. Version incremented.
**Status**: passed

---

## Scenario 11: Rewind to initial state succeeds even without prior visit in history
**ID**: scenario-11
**Testable after**: #5
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- `koto transition state-b --state <path>`
- `koto rewind --to <initial-state> --state <path>`
**Expected**: Exit code 0. `current_state` reverts to the machine's initial state. The initial state need not appear as a `to` field in history because it is always a valid rewind target.
**Status**: passed

---

## Scenario 12: Rewind from terminal state succeeds (error recovery)
**ID**: scenario-12
**Testable after**: #5
**Commands**:
- (Advance workflow to terminal state)
- `koto rewind --to <non-terminal-state> --state <path>`
**Expected**: Exit code 0. `current_state` is now the non-terminal target. This is the recovery path when a workflow reaches an undesired terminal state.
**Status**: passed

---

## Scenario 13: Rewind to unvisited non-initial state fails
**ID**: scenario-13
**Testable after**: #5
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- `koto rewind --to never-visited-state --state <path>`
**Expected**: Exit code 1. Error JSON with `code: "rewind_failed"`.
**Status**: passed

---

## Scenario 14: Rewind to terminal state fails
**ID**: scenario-14
**Testable after**: #5
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- `koto transition state-b --state <path>`
- `koto rewind --to <terminal-state> --state <path>`
**Expected**: Exit code 1. Error JSON with `code: "rewind_failed"`. Rewinding to a terminal state would leave the workflow stuck.
**Status**: passed

---

## Scenario 15: Cancel removes the state file
**ID**: scenario-15
**Testable after**: #5
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- `koto cancel --state <path>`
- `ls <state-file-path>`
**Expected**: Cancel exits 0 with confirmation message. The state file no longer exists on disk. No other files in the directory are removed.
**Status**: passed

---

## Scenario 16: Query methods return independent copies
**ID**: scenario-16
**Testable after**: #5
**Commands**:
- `go test ./pkg/engine/... -run TestCopySafety -v`
**Expected**: Unit tests confirm that `Variables()`, `History()`, and `Snapshot()` return copies. Mutating returned values does not affect engine internal state.
**Status**: passed

---

## Scenario 17: TransitionError serializes to expected JSON shape
**ID**: scenario-17
**Testable after**: #6
**Commands**:
- `koto transition invalid-target --state <path>`
**Expected**: stdout JSON matches `{"error": {"code": "invalid_transition", "message": "...", "current_state": "...", "target_state": "invalid-target", "valid_transitions": [...]}}`. Fields with zero values are omitted (omitempty). All six error codes (`terminal_state`, `invalid_transition`, `unknown_state`, `template_mismatch`, `version_conflict`, `rewind_failed`) serialize correctly.
**Status**: passed

---

## Scenario 18: Version conflict detected on concurrent modification
**ID**: scenario-18
**Testable after**: #6
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- (Externally modify the state file to increment `version` field)
- `koto transition <target> --state <path>`
**Expected**: Exit code 1. Error JSON with `code: "version_conflict"`. The engine detects that the on-disk version changed between its load and write operations.
**Status**: passed

---

## Scenario 19: Template hash mismatch blocks operations
**ID**: scenario-19
**Testable after**: #6
**Commands**:
- `koto init --name test-workflow --template <path> --state-dir <dir>`
- (Modify the template file content after init)
- `koto next --state <path>` or `koto transition <target> --state <path>`
**Expected**: Exit code 1. Error JSON with `code: "template_mismatch"`. No override flag exists to bypass this check.
**Status**: passed

---

## Scenario 20: Template parsing produces correct Machine
**ID**: scenario-20
**Testable after**: #7
**Commands**:
- `go test ./pkg/template/... -v`
**Expected**: Tests confirm that `Parse` reads a template file and returns a `Template` with correct `Name`, `Machine.InitialState`, `Sections` map, `Variables` map, and `Hash` (SHA-256 formatted as `sha256:<hex>`). Invalid templates return parse errors. Undefined transition targets cause errors.
**Status**: pending

---

## Scenario 21: Interpolation replaces placeholders and leaves unresolved ones
**ID**: scenario-21
**Testable after**: #7
**Commands**:
- `go test ./pkg/template/... -run TestInterpolate -v`
**Expected**: `Interpolate("Hello {{NAME}}, task: {{TASK}}", {"NAME": "agent"})` returns `"Hello agent, task: {{TASK}}"`. Unresolved `{{TASK}}` is left as-is. Single-pass replacement.
**Status**: pending

---

## Scenario 22: State file discovery finds workflows in directory
**ID**: scenario-22
**Testable after**: #8
**Commands**:
- `go test ./pkg/discover/... -v`
**Expected**: `Find` returns an empty slice for empty directories. Returns correct `Workflow` structs for directories with one or more `koto-*.state.json` files. Non-matching files are ignored. Corrupted files produce partial results plus a non-nil error.
**Status**: pending

---

## Scenario 23: Full lifecycle from init through terminal state via CLI
**ID**: scenario-23
**Testable after**: #9
**Environment**: automatable
**Commands**:
- `koto init --name lifecycle --template <real-template> --state-dir <dir> --var TASK="build feature"`
- `koto next --state <path>` (verify execute directive with interpolated TASK variable)
- `koto transition <state-2> --state <path>`
- `koto query --state <path>` (verify full state snapshot as JSON)
- `koto status --state <path>` (verify human-readable output)
- `koto transition <terminal-state> --state <path>`
- `koto next --state <path>` (verify done directive)
**Expected**: Complete lifecycle works end-to-end through the CLI. JSON output is valid at every step. Variables are interpolated in directives. State file version increments on each mutation. History records all transitions.
**Status**: pending

---

## Scenario 24: Multi-workflow auto-selection and explicit --state flag
**ID**: scenario-24
**Testable after**: #9
**Commands**:
- `koto init --name workflow-a --template <path-a> --state-dir <dir>`
- `koto init --name workflow-b --template <path-b> --state-dir <dir>`
- `koto next` (without --state, in dir with two state files)
- `koto next --state <path-to-workflow-a>`
- `koto workflows --state-dir <dir>`
**Expected**: `koto next` without `--state` fails with a clear error listing available state files when multiple exist. `koto next --state <specific-path>` succeeds. `koto workflows` lists both active workflows as JSON array. When only one state file exists, commands auto-select it without `--state`.
**Status**: pending

---

## Scenario 25: Agent workflow end-to-end with real template
**ID**: scenario-25
**Testable after**: #10
**Environment**: manual
**Commands**:
- Create a template file defining a 4-state workflow (assess, plan, implement, done)
- `koto init --name agent-task --template workflow.md --state-dir wip/ --var TASK="Add retry logic"`
- `koto next --state wip/koto-agent-task.state.json` (read directive, verify interpolation)
- `koto transition plan --state wip/koto-agent-task.state.json`
- `koto next --state wip/koto-agent-task.state.json`
- `koto transition implement --state wip/koto-agent-task.state.json`
- Simulate error: `koto rewind --to plan --state wip/koto-agent-task.state.json`
- `koto next --state wip/koto-agent-task.state.json` (back in plan state)
- `koto transition implement --state wip/koto-agent-task.state.json`
- `koto transition done --state wip/koto-agent-task.state.json`
- `koto next --state wip/koto-agent-task.state.json` (verify action: done)
- `koto query --state wip/koto-agent-task.state.json` (verify full history including rewind)
**Expected**: The full agent workflow completes. Directives contain interpolated `{{TASK}}` values. Rewind preserves history and re-advancing works. Terminal state returns `action: "done"`. The state file is a self-contained record of everything that happened. This validates the design's core promise: an agent can init a workflow, follow directives, recover from errors via rewind, and reach completion with a full audit trail.
**Status**: pending

---
