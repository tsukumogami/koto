# Workflow-Tool Reference Implementation Analysis

Phase 2 research for koto engine design. Analyzes the existing workflow-tool source code that koto is being extracted from.

## Source Overview

- **Binary**: Statically linked Go ELF x86-64, ~4MB (not stripped, with debug info)
- **Module**: `github.com/tsukumogami/workflow-tool` (Go 1.25)
- **Location**: `private/tools/.claude/command_assets/tools/`
- **Total code**: ~10,000 lines across 17 source files (13 `.go` + 1 `.md` template + 3 test files)

### Package Structure

```
cmd/workflow-tool/main.go          (923 lines) - CLI entry point, flag parsing, design doc parsing
internal/
  state/
    state.go                       (~524 lines) - State file types, Load/Save, CRUD, queries
    transitions.go                 (~529 lines) - State machines, evidence gates, transition validation
    hash.go                        (~128 lines) - Integrity hash (SHA-256, jq-compatible)
    state_test.go                  (~350 lines)
    transitions_test.go            (~1130 lines)
  controller/
    controller.go                  (~615 lines) - Dependency graph, directive generation, template interpolation
    controller_test.go
    dependency_test.go
    integration_test.go
  template/
    template.go                    (~118 lines) - Section extraction, variable interpolation
    template_test.go
  discover/
    discover.go                    (~48 lines)  - Auto-discover wip/*-state.json
    discover_test.go
  bookkeeping/
    verify.go                      (~204 lines) - PR checkbox, Mermaid class, strikethrough verification
    verify_test.go
```

## State File Schema

The state file is a flat JSON document stored at `wip/implement-doc-state.json`. It uses a two-level state machine (issue-level + PR-level).

### Complete Schema

```json
{
  "pr_status": "implementing|completing|qa_validated|docs_validated",
  "design_doc": "path/to/DESIGN-*.md",
  "branch": "impl/branch-name",
  "pr_number": null | 123,
  "integrity_hash": "sha256:hexdigest...",
  "issues": [
    {
      "number": 42,
      "title": "issue title from design doc link",
      "status": "pending|planning|planned|in_progress|implemented|scrutinized|pushed|ci_fixing|ci_blocked|completed",
      "ci_status": "pending|running|passed|failed|fixing",
      "ci_fix_attempts": 0,
      "issue_type": "code|task",
      "dependencies": [41, 40],
      "commits": ["sha1", "sha2"],
      "agent_type": "coder|webdev",
      "summary": {
        "narrative": "",
        "files_changed": ["pkg/foo.go"],
        "tests_added": 3,
        "key_decisions": "Used existing interface",
        "requirements_map": [{"ac": "...", "status": "implemented"}]
      },
      "reviewer_results": {},
      "testable_scenarios": ["scenario-1", "scenario-2"]
    }
  ],
  "skipped_issues": [
    {
      "number": 99,
      "reason": "needs-design|dependency_blocked:#50",
      "label": "needsDesign|blocked|dependency_blocked",
      "blocked_by": [50]
    }
  ],
  "test_plan_file": null | "wip/implement-doc_name_test_plan.md",
  "doc_plan_file": null | "wip/implement-doc_name_doc_plan.md"
}
```

### Key Schema Observations

1. **No transition history**: Unlike koto's proposed design, the current state file has no `history` array. The only audit trail is the issue's current `status` field and the `summary` data accumulated during transitions.

2. **Flat evidence model**: Evidence is stored as direct fields on the issue struct (`commits`, `reviewer_results`, `ci_status`, `summary`), not in a generic `evidence: {}` map. This is more type-safe but less flexible.

3. **Two-level state machine**: `pr_status` tracks the PR lifecycle independently of individual issue statuses. Issue transitions are locked when `pr_status` moves past `"implementing"`.

4. **Nullable fields via json.RawMessage**: `pr_number`, `test_plan_file`, `doc_plan_file`, and `summary` use `json.RawMessage` to support `null` JSON values while preserving arbitrary JSON structures.

5. **No template hash**: The current state file has no `template_hash` field. The embedded template can't change between invocations (it's compiled in), so TOCTOU protection isn't needed. Koto's filesystem-based templates introduce this requirement.

## State Machine Transition Logic

### Issue-Level State Machines

Three separate transition tables exist, selected by `issue_type`:

**Generic (untyped, fallback)**:
```
pending     -> in_progress, planning
planning    -> planned
planned     -> in_progress
in_progress -> implemented
implemented -> scrutinized, pushed
scrutinized -> pushed, implemented
pushed      -> completed, ci_fixing
ci_fixing   -> pushed, ci_blocked
```

**Code issues** (`issue_type: "code"`):
```
pending     -> in_progress
in_progress -> implemented
implemented -> scrutinized, pushed
scrutinized -> pushed, implemented
pushed      -> completed, ci_fixing
ci_fixing   -> pushed, ci_blocked
```

**Task issues** (`issue_type: "task"`):
```
pending     -> in_progress
in_progress -> completed
```

Terminal states: `completed`, `ci_blocked`.

### PR-Level State Machine

```
implementing  -> completing
completing    -> qa_validated
qa_validated  -> docs_validated (terminal)
```

The PR state machine is strictly linear. The `implementing -> completing` transition is auto-triggered by the controller when all issues resolve (completed, ci_blocked, or skipped).

### Issue Transition Guard

A critical guard prevents issue transitions when the PR is past the implementing phase:

```go
if effectivePRStatus != PRStatusImplementing {
    return 0, fmt.Errorf("issue transitions are locked: PR is in %q state")
}
```

This prevents agents from modifying issue state during Phase 2 (QA, docs, finalization).

## Evidence Gate Implementation

Evidence gates are defined as a static map keyed by `"from_status:to_status"`:

```go
type evidenceGate struct {
    fieldName     string  // issue field to check
    requiredValue string  // if non-empty, field must equal this; otherwise field must be non-empty
}

var transitionEvidence = map[string]evidenceGate{
    "in_progress:implemented": {fieldName: "commits"},
    "implemented:pushed":      {fieldName: "reviewer_results"},
    "implemented:scrutinized": {},  // no evidence required
    "scrutinized:pushed":      {fieldName: "reviewer_results"},
    "pushed:completed":        {fieldName: "ci_status", requiredValue: "passed"},
    "pushed:ci_fixing":        {fieldName: "ci_status", requiredValue: "failed"},
}
```

### Evidence Gate Types (Current)

Only two gate types exist in the current implementation:

1. **field_not_empty**: Checks that a named field on the issue struct has meaningful content. Uses a switch on field name with type-specific checks:
   - `commits`: `len(issue.Commits) > 0`
   - `reviewer_results`: `len(issue.ReviewerResults) > 0 && string(issue.ReviewerResults) != "null"`
   - `summary`: `len(issue.Summary) > 0 && string(issue.Summary) != "null"`

2. **field_equals**: Checks that a named field equals a specific string value. Only used for `ci_status` ("passed" or "failed").

There is **no `command` gate type** in the current implementation. The design doc proposes this for koto, but workflow-tool does not execute shell commands as evidence checks. All evidence is provided via CLI flags on the `transition` command.

### Two Evidence Validation Paths

The codebase has a subtle but important distinction between two validation approaches:

**Path 1: Stored evidence** (`CheckEvidence` function). Validates evidence already stored on the issue struct. Used by the older `SetIssueStatus` method. Checks whether the issue's fields already have the required values before allowing the transition.

**Path 2: Flag-based evidence** (`Transition` method + `validateCodeTransitionFlags`/`validateTaskTransitionFlags`). Validates the CLI flags provided with the transition command. The evidence is carried in the call itself, making the transition atomic: validate flags -> apply all changes -> done. This is the active path used by the CLI.

The flag-based validation checks include:
- `in_progress -> implemented` (code): requires `--commits`
- `implemented -> pushed` (code): requires `--reviewer-results-file`
- `scrutinized -> pushed` (code): requires `--reviewer-results-file`
- `pushed -> completed` (code): requires `--ci-status passed`
- `pushed -> ci_fixing` (code): requires `--ci-status failed`
- `in_progress -> completed` (task): requires `--key-decisions`
- `pending -> in_progress` (all): requires `--issue-type`

### Bookkeeping Verification

Before the `pushed -> completed` transition, a pre-check reads external artifacts and verifies:

1. **PR checkbox**: `[x] #N` exists in PR body (via `gh pr view`)
2. **PR Fixes line**: `Fixes #N` exists in PR body (case-insensitive)
3. **Mermaid class**: `class I<N> done` in design doc
4. **Table strikethrough**: `~~[#N]...~~` in design doc table
5. **Test scenarios**: Each testable scenario has `[x]` in test plan

This is a tsuku-specific compound evidence gate that has no equivalent in koto's initial scope. It reads from the filesystem and runs `gh` CLI commands.

## Transition Validation Flow

The full validation chain for `workflow-tool state transition <status> [flags]`:

1. **Guard check**: Is `pr_status == "implementing"`? If not, reject.
2. **Find current issue**: Scan issues for first `in_progress`, then first `pending` (auto-resolve, no explicit issue number).
3. **Pending-specific check**: If `pending -> in_progress`, require `--issue-type`.
4. **Transition validation**: Check against the type-specific transition table (`codeTransitions`, `taskTransitions`, or `validTransitions`).
5. **Flag validation**: Check required flags per type and transition pair.
6. **Bookkeeping pre-check**: If target is `completed` and not `--force`, verify PR/design doc/test plan updates via external reads.
7. **Apply atomically**: Set status, issue_type, agent_type, append commits, set CI status, build summary, store reviewer results.
8. **Auto-increment**: If transitioning to `ci_fixing`, increment `ci_fix_attempts`.
9. **Save**: Compute integrity hash, marshal to JSON, write to disk.

The `--force` flag bypasses steps 4, 5, and 6.

## State Persistence

### Save Operation

```go
func Save(s *StateFile, path string) error {
    hash, err := ComputeHash(s)
    s.IntegrityHash = hash
    data, err := json.MarshalIndent(s, "", "  ")
    data = append(data, '\n')
    err := os.WriteFile(path, data, 0644)
}
```

**This is NOT atomic.** The current implementation uses `os.WriteFile` directly, which can leave a corrupted file if the process is killed mid-write. The koto design doc specifies write-to-temp + rename for atomic writes -- this is a known improvement over the current implementation.

### Integrity Hash

The hash is `sha256` of the JSON with `integrity_hash` removed, using `jq -cS` compatible output (recursively sorted keys, compact format). The custom `marshalSorted` function produces byte-identical output to `jq -cS 'del(.integrity_hash)'`.

Key implementation detail: The hash is computed by:
1. Marshal struct to JSON
2. Unmarshal to `map[string]interface{}`
3. Delete `integrity_hash` key
4. Re-marshal with sorted keys (recursive)
5. SHA-256 hash the result

The hash is auto-updated on every `Save()` call. A `Validate --fix` command can recompute a mismatched hash.

### Auto-Discovery

The discover package scans `wip/*-state.json` in the working directory. It requires exactly one match -- multiple state files cause an error. This means only one workflow can be active per working directory. The `--state-file` flag overrides discovery for explicit targeting.

## Controller Logic

### Directive Generation

The controller follows this decision tree on each `controller next` call:

1. If `pr_status != "implementing"`: emit Phase 2 directive for the current PR state.
2. Compute dependency graph from issue `dependencies` arrays.
3. Search for current issue:
   a. First scan for any issue in `in_progress`, `implemented`, `scrutinized`, `pushed`, or `ci_fixing` status.
   b. If none, scan for first `pending` issue with satisfied dependencies.
   c. For pending issues, check dependency status:
      - `depCompleted`: all deps completed -- issue is actionable.
      - `depNotReady`: some dep is pending/in_progress -- skip past (don't add to skipped).
      - `depBlocked`: some dep is `ci_blocked` or in `skipped_issues` -- auto-skip this issue and persist.
4. If all issues resolved and `pr_status == "implementing"`: auto-transition to `completing`.
5. Extract `## STATE: <status>` section from embedded template.
6. Build variable map from state file fields.
7. Compute design doc updates (status changes for Mermaid diagram).
8. Interpolate variables into template section.
9. Return JSON response.

### Response Structure

```json
{
  "action": "execute|skipped|error",
  "issue": 42,
  "status": "pending",
  "directive": "<interpolated template section>",
  "design_doc_updates": [...],
  "message": "",
  "reason": ""
}
```

The controller auto-persists state changes in two cases:
- Auto-skip a dependency-blocked issue (adds to `skipped_issues` and saves).
- Auto-transition `pr_status` from `implementing` to `completing` (saves).

### Template System

The template is embedded via `go:embed` at compile time. Template sections are delimited by `## STATE: <name>` headings. The `ExtractSection` function does a simple line-by-line scan:

```go
func ExtractSection(templateContent, stateName string) (string, error) {
    heading := "## STATE: " + stateName
    // Scan lines until heading found, then collect until next ## STATE:
}
```

Variable interpolation is pure string replacement:

```go
func Interpolate(content string, vars map[string]string) string {
    for name := range AllowedVariables {
        placeholder := "{{" + name + "}}"
        content = strings.ReplaceAll(content, placeholder, val)
    }
}
```

There is an explicit allowlist of 18 variable names. Unrecognized `{{VAR}}` patterns are left as-is. Long values are truncated (TITLE: 200 chars, SCENARIOS: 1000, PREVIOUS_SUMMARY: 2000, KEY_DECISIONS: 2000).

This is intentionally simple: no template logic, no conditionals, no loops. Single-pass literal replacement prevents injection through user-controlled values.

## CLI Command Structure

```
workflow-tool state init <design-doc> [--branch name] [--pr N] [--state-file path]
workflow-tool state transition <status> [flags]
workflow-tool state pr-transition <status> [flags]
workflow-tool state rewind <issue-number>
workflow-tool state query issue <N>
workflow-tool state query history
workflow-tool state query progress
workflow-tool state query blocked-by <N>
workflow-tool state validate [--fix]
workflow-tool controller next [--state-file <path>]
```

### Key Differences from koto's Proposed CLI

| workflow-tool | koto (proposed) | Notes |
|--------------|-----------------|-------|
| `state init <design-doc>` | `koto init --template <name> --var KEY=VALUE` | workflow-tool parses design docs; koto uses templates |
| `controller next` | `koto next` | Same concept, different command path |
| `state transition <status>` | `koto transition <state> --evidence KEY=VALUE` | workflow-tool uses typed flags; koto uses generic evidence |
| `state pr-transition <status>` | (no equivalent) | Two-level state machine is tsuku-specific |
| `state query issue/history/progress/blocked-by` | `koto query state/evidence/history` | Similar, different subcommand names |
| `state rewind <N>` | `koto rewind [--to <state>]` | workflow-tool rewinds per-issue; koto rewinds the single workflow |
| `state validate [--fix]` | `koto validate` | Same concept |
| (none) | `koto status` | Human-readable display (new in koto) |
| (none) | `koto cancel` | Abandon workflow (new in koto) |
| (none) | `koto template list/validate/show` | Template management (new in koto) |
| (none) | `koto generate claude-code/agents-md` | Agent integration (new in koto) |
| (none) | `koto workflows --json` | Discovery (new in koto) |

### Error Handling

Errors go to stderr as text, exit code 1. The controller returns errors as JSON on stdout (so the agent can parse them). The exit code convention: 0 = success, 1 = operation error, 2 = invalid arguments.

Note that the `skipped` action (auto-skip due to blocked dependency) exits 0. The agent is expected to call `controller next` again to get the next actionable issue.

## Known Issues and Anti-Patterns

### From the Design Doc

1. **Non-atomic writes**: `os.WriteFile` instead of write-to-temp + rename. Power failure or kill during write corrupts the state file. Koto's design specifies atomic writes.

2. **No transition history**: The state file doesn't record when transitions happened or what evidence was provided. Makes debugging failed workflows difficult. Koto adds a `history` array.

3. **No template hash verification**: Since the template is `go:embed`'d, it can't change between invocations. But filesystem-based templates in koto require hash verification to detect mid-workflow tampering.

4. **No concurrent access protection**: No file locking or optimistic version counter. If two processes (agent + human, or a retry + rewind) access the state file simultaneously, one write silently overwrites the other. Koto's design defers this to DESIGN-koto-engine.md.

5. **State file tampering**: The integrity hash detects accidental corruption but not intentional tampering (the attacker can recompute the hash). No hash chain or signing. Koto notes this as a known residual risk.

6. **Bookkeeping verification couples to external tools**: The `pushed -> completed` pre-check calls `gh pr view` and reads design doc files. This introduces I/O into the transition path and requires GitHub CLI availability. Koto's `command` evidence gate generalizes this.

### From the Code

7. **Implicit current issue resolution**: The `transition` command doesn't take an issue number. It auto-resolves by finding the first non-terminal, non-pending issue. This works for the linear controller loop but would be confusing in a generic tool. Only `rewind` takes an explicit issue number.

8. **Two evidence validation paths**: Both `CheckEvidence` (stored-field gates) and `validateCodeTransitionFlags` (flag-based gates) exist. The flag-based path is the one used by the CLI, but the stored-field path remains for `SetIssueStatus` (which is still called with `--force`). This dual-path creates maintenance confusion.

9. **Hardcoded variable allowlist**: The template system has an explicit `AllowedVariables` map. Adding a new variable requires changing Go code and recompiling. Koto's template system should derive allowed variables from the template's `variables:` section.

10. **Design doc parsing in the binary**: The `init` command contains ~250 lines of markdown table parsing, issue link extraction, and dependency resolution -- all deeply coupled to tsuku's design doc format. This is the single largest tsuku-specific component and will not exist in koto.

11. **No `done` terminal action**: The controller response has `"action": "execute"` and `"action": "skipped"` and `"action": "error"`, but no explicit `"action": "done"`. When all issues resolve, the controller auto-transitions `pr_status` and emits Phase 2 directives. The workflow ends when the `docs_validated` state's directive tells the agent to finalize. Koto adds an explicit `"action": "done"` response for terminal states.

## Workflow Continuation Hook

The Stop hook (`workflow-continue.sh`) runs on Claude Code Stop events. It:

1. Reads hook input JSON from stdin
2. Checks for `wip/*-state.json` files
3. Parses the state file to find issues not in `completed` or `ci_blocked` status
4. If incomplete work exists, blocks the stop with a nudge message
5. Uses `jq` for JSON parsing (external dependency)

This is the "safety net" that prevents agents from quitting mid-workflow. The hook is advisory (the agent can still stop after being nudged) but effective in practice.

## Tsuku-Specific vs Generic Components

### Clearly Generic (direct koto equivalents)

| Component | Lines | Description |
|-----------|-------|-------------|
| `state.Load/Save` | ~30 | Read/write JSON state files |
| `ComputeHash` | ~128 | Integrity hashing |
| `CheckTransition` | ~30 | Transition table lookup |
| `CheckEvidence` | ~30 | Evidence gate evaluation |
| `ExtractSection` | ~35 | Template section extraction |
| `Interpolate` | ~30 | Variable replacement |
| `discover.StateFile` | ~48 | Auto-discover state files |
| `Rewind` | ~10 | Reset to previous state |
| `QueryProgress` | ~35 | Progress summary |
| `Validate` | ~55 | Structure validation |

### Clearly tsuku-Specific (will not exist in koto)

| Component | Lines | Description |
|-----------|-------|-------------|
| `parseDesignDoc` | ~170 | Markdown table parsing for design docs |
| `parseIssueLink` | ~35 | `[#N: title](url)` extraction |
| `parseDependencies` | ~30 | Dependency cell parsing |
| `bookkeeping.VerifyBookkeeping` | ~204 | PR checkbox, Mermaid, strikethrough checks |
| `verifyBookkeepingPreCheck` | ~65 | External artifact reads + gh CLI |
| `computeDesignDocUpdates` | ~90 | Mermaid class change computation |
| `formatDesignDocUpdates` | ~15 | Human-readable update instructions |
| Type-specific transition tables | ~20 | `codeTransitions`, `taskTransitions` |

### Needs Refactoring for koto

| Component | Lines | What Changes |
|-----------|-------|-------------|
| `Transition` method | ~115 | Remove type-specific validation; use template-declared evidence gates |
| `controller.Next` | ~90 | Remove auto-skip and dependency graph (Phase 1 is linear) |
| `buildVars` | ~60 | Derive from template `variables:` section, not hardcoded |
| `findCurrentIssue` | ~70 | Single-workflow koto has one "current state", not multi-issue search |
| State file struct | ~50 | Replace multi-issue schema with single-workflow schema |

## Implications for koto Engine Design

### What to Keep

1. **Transition table as the source of truth**: `map[string][]string` for valid transitions. Simple, fast, debuggable.
2. **Evidence gate pattern**: Check required evidence before allowing transition. But generalize from struct fields to a key-value evidence map.
3. **Template section extraction**: `## STATE:` heading pattern works well.
4. **Single-pass string interpolation**: Simple and injection-safe.
5. **Integrity hashing**: SHA-256 with sorted JSON. Extend to include template hash.
6. **Auto-discovery**: `wip/*-state.json` glob pattern.
7. **JSON response format**: Agent-parseable, with `action` field for dispatch.

### What to Change

1. **Single-workflow state model**: Replace multi-issue array with single `current_state` + `evidence` map + `history` array.
2. **Template-driven state machine**: Move transition tables and evidence gates into the template YAML header. No hardcoded state machines in Go code.
3. **Atomic writes**: Write-to-temp + rename instead of `os.WriteFile`.
4. **Template hash verification**: Hash stored at init, verified on every `next` and `transition`.
5. **Explicit `done` action**: Terminal states return `{"action": "done"}` instead of auto-transitioning.
6. **Generic evidence**: `--evidence KEY=VALUE` pairs instead of typed flags (`--commits`, `--ci-status`, etc.).
7. **Transition history**: Record each transition with timestamp and evidence snapshot.
8. **`command` evidence gate**: New gate type that runs a shell command and checks exit code. Not in workflow-tool.
9. **No implicit issue resolution**: Koto has one current state, not a multi-issue search.
10. **Variables from template**: Derive allowed variables from template `variables:` section, not a hardcoded allowlist.

### What to Add (New in koto)

1. `koto cancel` -- abandon workflow, remove state file
2. `koto status` -- human-readable progress display
3. `koto template list/validate/show` -- template management
4. `koto generate claude-code/agents-md` -- agent integration file generation
5. `koto workflows --json` -- structured discovery for agents
6. Error responses as JSON on stdout (workflow-tool already does this for controller, but not for state commands)
