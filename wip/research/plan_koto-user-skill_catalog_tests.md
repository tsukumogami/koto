# Functional Test Catalog: Ground-Truth Behavior for koto-user Skill

This catalog extracts ground-truth behavior from all functional test fixtures.
Use these exact response shapes, payload formats, and edge cases when authoring
response-shapes.md and other skill reference files.

---

## 1. Fixture Templates Inventory

Templates live in `test/functional/fixtures/templates/`. The `hello-koto`
template is looked up from `plugins/koto-skills/skills/hello-koto/hello-koto.md`
(does not exist yet — it's missing from the repo).

| Fixture name | File | Tests |
|---|---|---|
| `hello-koto` | plugins path (missing) | workflow-lifecycle, template-compile, name-validation |
| `simple-gates` | `simple-gates.md` | gate-with-evidence-fallback |
| `structured-gates` | `structured-gates.md` | structured-gate-output |
| `structured-routing` | `structured-routing.md` | structured-gate-output |
| `context-gate` | `context-gate.md` | context-gate-output |
| `mixed-routing` | `mixed-routing.md` | mixed-gate-routing |
| `multi-state` | `multi-state.md` | rewind |
| `decisions` | `decisions.md` | decisions, rewind |
| `legacy-gates` | `legacy-gates.md` | legacy-gate-backward-compat |
| `var-substitution` | `var-substitution.md` | var-substitution |

---

## 2. Response Shapes — Ground-Truth Field Observations

The test harness (steps_test.go) parses the last JSON line of stdout and checks
individual fields. The following shapes are derived from what the tests assert
exists and what they assert equals specific values.

### 2.1 `koto init` response

From `workflow-lifecycle.feature` scenario "Init creates state file":

```json
{"name": "hello", "state": "awakening"}
```

Fields confirmed:
- `state` — string, the initial_state value from the template

### 2.2 `koto next` — action: "evidence_required"

From `gate-with-evidence-fallback.feature` scenario "Gate fails and evidence is required":

```json
{
  "action": "evidence_required",
  "state": "start",
  "expects": { ... }
}
```

Fields confirmed by test:
- `action` = `"evidence_required"`
- `state` = current state name (string)
- `expects` — field present (non-null)

From `workflow-lifecycle.feature` scenario "Next returns directive when gate fails":

```json
{
  "action": "evidence_required",
  "state": "awakening",
  "directive": "..."
}
```

Fields confirmed:
- `action` = `"evidence_required"`
- `state` = `"awakening"`
- `directive` — field present

### 2.3 `koto next` — action: "done" (terminal state reached)

From `gate-with-evidence-fallback.feature` scenario "Gate passes and auto-advances":

```json
{
  "action": "done",
  "state": "done",
  "advanced": true
}
```

From `workflow-lifecycle.feature` scenario "Next advances when gate passes":

```json
{
  "action": "done",
  "state": "eternal",
  "advanced": true
}
```

Note: `advanced` is a **boolean** (`true`/`false`), not a string. Some test steps check
it as a string (`"true"`) and some as a native bool (`true`) — the underlying JSON type
is boolean.

From `structured-gate-output.feature` scenario "Gate passes and auto-advances via gates.* routing":

```json
{
  "action": "done",
  "state": "pass",
  "advanced": true
}
```

From `structured-gate-output.feature` scenario "Gate fails and routes to fix state":

```json
{
  "action": "done",
  "state": "fix",
  "advanced": true
}
```

Key insight: when `gates.*` routing directs to a terminal state, the action is
`"done"` and `advanced` is `true` — even when the gate "failed" (exit code 1).
The gate failure itself isn't an error condition; it's the routing signal.

### 2.4 `koto next` — action: "gate_blocked"

From `structured-gate-output.feature` scenario "Failing command gate returns structured output":

```json
{
  "action": "gate_blocked",
  "state": "check",
  "blocking_conditions": [
    {
      "name": "ci_check",
      "type": "command",
      "status": "failed",
      "output": {
        "exit_code": 1,
        "error": ""
      }
    }
  ]
}
```

Fields confirmed:
- `action` = `"gate_blocked"`
- `state` = current state name
- `blocking_conditions` — array, present
- `blocking_conditions[0].name` = gate name as declared in template
- `blocking_conditions[0].type` = `"command"`
- `blocking_conditions[0].status` = `"failed"`
- `blocking_conditions[0].output.exit_code` = integer (1 in this test)
- `blocking_conditions[0].output.error` = string (empty string `""` when no error message)

From `context-gate-output.feature` scenario "Missing context key produces structured output":

```json
{
  "action": "gate_blocked",
  "state": "check",
  "blocking_conditions": [
    {
      "name": "ctx_check",
      "type": "context-exists",
      "status": "failed",
      "output": {
        "exists": false,
        "error": ""
      }
    }
  ]
}
```

Fields confirmed for context-exists gate:
- `blocking_conditions[0].type` = `"context-exists"`
- `blocking_conditions[0].output.exists` = boolean `false`
- `blocking_conditions[0].output.error` = string `""`

**Gate type output shapes differ by gate type:**

| Gate type | `output` fields |
|---|---|
| `command` | `exit_code` (int), `error` (string) |
| `context-exists` | `exists` (bool), `error` (string) |

### 2.5 `koto next` with `--with-data` evidence submission

From `gate-with-evidence-fallback.feature` scenario "Gate fails then evidence advances":

```bash
koto next test-wf --with-data '{"status": "completed", "detail": "manual"}'
```

Result:
```json
{
  "action": "done",
  "state": "done",
  "advanced": true
}
```

From `mixed-gate-routing.feature` scenarios:

```bash
koto next test-wf --with-data '{"decision": "approve"}'
```

```json
{
  "action": "done",
  "state": "approved",
  "advanced": true
}
```

```bash
koto next test-wf --with-data '{"decision": "reject"}'
```

```json
{
  "action": "done",
  "state": "rejected",
  "advanced": true
}
```

From `rewind.feature` scenario "Rewind returns to previous state":

```bash
koto next test-wf --with-data '{"route": "setup"}'
```

From `decisions.feature` scenarios:

```bash
koto next test-wf --with-data '{"status": "completed"}'
```

### 2.6 `koto rewind` response

From `rewind.feature` scenario "Rewind returns to previous state":

```json
{"state": "entry"}
```

Field confirmed:
- `state` — string, the name of the state rewound to

Note: AGENTS.md shows the shape as `{"name": "myworkflow", "state": "analysis"}`.
Tests only assert `state` field, not `name`. The AGENTS.md may include `name` but tests
don't confirm it — do not rely on `name` being present in rewind response.

### 2.7 `koto decisions record` response

From `decisions.feature` scenario "Record and list decisions":

After first record:
```json
{"decisions_recorded": 1}
```

After second record:
```json
{"decisions_recorded": 2}
```

Fields confirmed:
- `decisions_recorded` — integer, running count of decisions recorded for this session

### 2.8 `koto decisions list` response

From `decisions.feature` scenario "Record and list decisions":

```json
{"decisions": {"count": 2}}
```

Fields confirmed:
- `decisions.count` — integer

Note: AGENTS.md documents `koto decisions list` as returning a JSON array. Tests only
assert `decisions.count`. There may be a discrepancy between the AGENTS.md docs and
the actual response shape — the test confirms a `decisions` object with a `count`
field, not a top-level array.

After rewind (from `rewind.feature` scenario "Decisions cleared after rewind"):

```json
{"decisions": {"count": 0}}
```

Confirmed behavior: rewind clears decisions recorded in the state being rewound away from.

---

## 3. `--with-data` Payload Formats

All `--with-data` values are JSON objects passed as a shell-quoted string argument.
The keys must match field names declared in the template's `accepts` block.

### Observed payload shapes

From `simple-gates.md` template (accepts: `status` enum, `detail` string):
```json
{"status": "completed", "detail": "manual"}
```

From `multi-state.md` template entry state (accepts: `route` enum):
```json
{"route": "setup"}
```

From `decisions.md` template work state (accepts: `status` enum):
```json
{"status": "completed"}
```

From `decisions.feature` (decisions record command):
```json
{"choice": "A", "rationale": "because"}
{"choice": "B", "rationale": "also"}
```

From `mixed-routing.md` template (accepts: `decision` enum):
```json
{"decision": "approve"}
{"decision": "reject"}
```

### Invalid payload examples (schema validation errors)

From `decisions.feature` scenario "Invalid decision schema rejected":

```bash
koto decisions record test-wf --with-data '{"not_choice": "A"}'
```

Exit code: 2
Output contains: `"missing required field"`

---

## 4. Template Format Ground Truth

### 4.1 Template frontmatter — required fields

From all fixture templates:
- `name` — string
- `version` — string (quoted: `"1.0"`)
- `description` — string
- `initial_state` — string, must match a key in `states`

### 4.2 Template frontmatter — optional fields

- `variables` — map of variable name to `{description, required}` object

### 4.3 Variable declaration and interpolation

From `var-substitution.md`:

```yaml
variables:
  MY_VAR:
    description: Variable to substitute in gate command
    required: true
```

Used in gate command:
```yaml
command: "test -f wip/{{MY_VAR}}.txt"
```

Supplied at init:
```bash
koto init test-wf --template .koto/templates/var-substitution.md --var MY_VAR=expected_value
```

### 4.4 State structure

From `simple-gates.md`:

```yaml
states:
  start:
    gates:
      check_file:
        type: command
        command: "test -f wip/check.txt"
    accepts:
      status:
        type: enum
        values: [completed, override, blocked]
        required: true
      detail:
        type: string
        required: false
    transitions:
      - target: done
        when:
          status: completed
      - target: done
        when:
          status: override
      - target: done       # fallback (no when)
  done:
    terminal: true
```

### 4.5 `gates.*` routing in `when` clauses

From `structured-routing.md`:

```yaml
transitions:
  - target: pass
    when:
      gates.ci_check.exit_code: 0
  - target: fix
    when:
      gates.ci_check.exit_code: 1
```

Key: the `when` clause references `gates.<gate_name>.exit_code` (for command gates).
This is the structured routing pattern. Templates without this pattern are "legacy mode."

### 4.6 Mixed gate output + agent evidence in `when`

From `mixed-routing.md`:

```yaml
transitions:
  - target: approved
    when:
      gates.lint.exit_code: 0     # gate output
      decision: approve            # agent evidence
  - target: rejected
    when:
      decision: reject             # agent evidence only
  - target: done                   # fallback
```

Key insight: a single `when` clause can mix `gates.*` keys and plain evidence keys.
Both conditions must match for the transition to fire (AND semantics).

### 4.7 Legacy gate templates

From `legacy-gates.md` — a template with gates but NO `gates.*` when-clause references:

```yaml
states:
  verify:
    gates:
      ci_check:
        type: command
        command: "true"
    accepts:
      status:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: complete
        when:
          status: done
      - target: complete    # fallback
```

Behavior:
- `koto init` accepts it with a warning printed to stderr containing `"legacy behavior"`
- `koto template compile` rejects it (exit nonzero), output mentions `"gates."` and `"--allow-legacy-gates"`
- `koto template compile --allow-legacy-gates` accepts it (exit 0)
- At runtime, gate passes and workflow advances normally (`action: "done"`, `advanced: true`)

---

## 5. CLI Behavior — Edge Cases

### 5.1 Workflow name validation

From `name-validation.feature`:

| Input | Exit code | Error message |
|---|---|---|
| `my-workflow` | 0 | (success) |
| `../escape` | 2 | `"invalid characters"` |
| `.hidden` | 2 | `"invalid characters"` |
| `''` (empty) | nonzero | (unspecified) |

Valid characters: alphanumeric, dots, hyphens, underscores (from AGENTS.md).
Path traversal (`..`) and leading dots are both rejected with exit code 2 and the
message `"invalid characters"`.

### 5.2 Duplicate init rejected

From `workflow-lifecycle.feature` scenario "Duplicate init rejected":

Running `koto init` with a name that already has a state file:
- Exit code: nonzero
- Output contains: `"already exists"`

### 5.3 Missing required variable at init

From `var-substitution.feature` scenario "Missing required variable rejected":

```bash
koto init test-wf --template .koto/templates/var-substitution.md
# (no --var)
```

- Exit code: nonzero
- Output contains: `"missing required variable"`

### 5.4 Rewind at initial state

From `rewind.feature` scenario "Rewind at initial state fails":

```bash
koto rewind test-wf
# (immediately after init, no transitions yet)
```

- Exit code: nonzero
- Output contains: `"initial state"`

### 5.5 Template compilation

From `template-compile.feature`:

- Valid template: `koto template compile <path>` exits 0
- Invalid YAML frontmatter: exits nonzero, output contains `"error"`
- Legacy-gate template: exits nonzero, output mentions `"gates."` and `"--allow-legacy-gates"`
- Legacy-gate template with flag: `koto template compile --allow-legacy-gates <path>` exits 0

### 5.6 Decision schema validation

From `decisions.feature` scenario "Invalid decision schema rejected":

- Submitting a payload that's missing a required field (`choice`) to `koto decisions record`
- Exit code: 2
- Output contains: `"missing required field"`

### 5.7 State file naming convention

From `steps_test.go` `theStateFileExists()`:

State files are named: `koto-<workflow-name>.state.jsonl`

Example: workflow named `hello` → `koto-hello.state.jsonl`

The file lives in the working directory (the git repo root where `koto init` was run).

---

## 6. Behaviors Tested But Not Yet in Skills

### 6.1 Structured `blocking_conditions` output (AGENTS.md has partial coverage)

AGENTS.md documents `blocking_conditions` at the array level and mentions `type`,
`status`, and `agent_actionable` fields. Tests confirm a different set of fields:

**What tests confirm exists:**
- `blocking_conditions[0].name` — the gate's declared name
- `blocking_conditions[0].type` — `"command"` or `"context-exists"`
- `blocking_conditions[0].status` — `"failed"`
- `blocking_conditions[0].output.exit_code` — int (command gates)
- `blocking_conditions[0].output.error` — string (both gate types)
- `blocking_conditions[0].output.exists` — bool (context-exists gates)

**What AGENTS.md documents that tests don't confirm:**
- `agent_actionable` — field mentioned in AGENTS.md example but not tested

The `output` subobject is entirely missing from AGENTS.md's examples for
`gate_blocked`. This is a documentation gap.

### 6.2 `gates.*` routing behavior explained from the user perspective

Neither AGENTS.md nor any current skill content explains:
- What happens when all matching `gates.*` routing transitions point to terminal states
  (the action becomes `"done"` immediately, not `"gate_blocked"`)
- That a gate "failing" is not necessarily an error — it can be a routing signal
- That `gates.*` and evidence keys can appear in the same `when` clause

### 6.3 Decisions `record` response shape

AGENTS.md doesn't document the `koto decisions record` response at all. Tests confirm:
```json
{"decisions_recorded": <integer>}
```

### 6.4 Decisions `list` response shape

AGENTS.md documents `koto decisions list` returning a JSON array. Tests confirm:
```json
{"decisions": {"count": <integer>}}
```

There may be additional fields (the full array of decision objects) that tests don't
check, but the shape is at minimum an object with a `decisions.count` field.

### 6.5 Rewind clears decisions

Tests confirm: after `koto rewind`, `koto decisions list` returns `{"decisions": {"count": 0}}`.
This behavior is not documented in AGENTS.md.

### 6.6 Legacy gate behavior at runtime

AGENTS.md and koto-author don't explain what happens at runtime with legacy-gate
templates from a user perspective: the gate still evaluates and the workflow still
advances — legacy mode only affects compile-time validation and emits a warning at
init time.

### 6.7 `koto init` emits warning to stderr (not stdout) for legacy templates

The test checks `error output contains "legacy behavior"` (stderr). This means legacy
warnings appear on stderr, not stdout. Skills should clarify this channel distinction.

---

## 7. Fixture Template Annotated Shapes

Complete annotated view of each fixture template for reference when writing examples.

### simple-gates.md

- initial_state: `start`
- States: `start` (gates + accepts), `done` (terminal)
- Gate: `check_file`, type `command`, checks `test -f wip/check.txt`
- Accepts: `status` (enum: completed/override/blocked, required), `detail` (string, optional)
- Routing: three transitions from start — two conditional on status, one fallback
- Test pattern: "file exists → auto-advance", "file absent → evidence_required", "submit evidence → advance"

### structured-gates.md

- initial_state: `check`
- States: `check` (gates only, no accepts), `done` (terminal)
- Gate: `ci_check`, type `command`, command `"exit 1"` (always fails)
- No accepts block → `gate_blocked` action when gate fails
- Test pattern: "always-failing gate produces structured blocking_conditions output"

### structured-routing.md

- initial_state: `check`
- States: `check` (gates + `gates.*` routing), `pass` (terminal), `fix` (terminal)
- Gate: `ci_check`, type `command`, `test -f wip/flag.txt`
- Routing: `gates.ci_check.exit_code: 0` → `pass`, `gates.ci_check.exit_code: 1` → `fix`
- No accepts block — routing is entirely driven by gate output
- Test pattern: "file present → routes to pass (done, advanced=true)", "file absent → routes to fix (done, advanced=true)"

### context-gate.md

- initial_state: `check`
- States: `check` (context-exists gate), `done` (terminal)
- Gate: `ctx_check`, type `context-exists`, key `required_key`
- Test pattern: "missing key → gate_blocked with output.exists=false"

### mixed-routing.md

- initial_state: `check`
- States: `check` (gate + accepts + mixed routing), `approved` (terminal), `rejected` (terminal), `done` (terminal)
- Gate: `lint`, type `command`, `"exit 0"` (always passes)
- Accepts: `decision` (enum: approve/reject, required)
- Routing: `gates.lint.exit_code: 0` + `decision: approve` → `approved`; `decision: reject` → `rejected`; fallback → `done`
- Test pattern: "gate passes + evidence matches both conditions → approved"; "evidence reject alone → rejected"

### multi-state.md

- initial_state: `entry`
- States: `entry` (accepts, evidence routing), `setup` (gates + accepts), `work` (accepts), `done` (terminal)
- Used for rewind testing: advance to `setup`, then rewind back to `entry`

### decisions.md

- initial_state: `work`
- States: `work` (accepts: `status` enum), `done` (terminal)
- Used for decisions recording tests and rewind-clears-decisions test
- Note: decisions schema requires `choice` and `rationale` fields — submitting
  `{"not_choice": "A"}` is rejected with exit code 2

### legacy-gates.md

- initial_state: `verify`
- States: `verify` (command gate + accepts), `complete` (terminal)
- Gate: `ci_check`, type `command`, command `"true"` (always passes)
- Key: no `gates.*` references in any `when` clause → classified as legacy
- Transitions use only evidence routing (`when: {status: done}`) plus fallback

### var-substitution.md

- initial_state: `check`
- Variables: `MY_VAR` (required: true)
- Gate command: `"test -f wip/{{MY_VAR}}.txt"` — variable substituted before execution
- Test: init with `--var MY_VAR=expected_value`, create `wip/expected_value.txt`, gate passes

---

## 8. AGENTS.md / koto-author Gaps Summary

These are items confirmed by tests that are absent or incorrect in current documentation:

| Gap | Location | Correct behavior |
|---|---|---|
| `blocking_conditions[N].output` subobject | AGENTS.md gate_blocked example | `command` gates: `{exit_code, error}`. `context-exists` gates: `{exists, error}`. |
| `blocking_conditions[N].name` field | AGENTS.md | Gate's declared name appears in blocking output |
| `koto decisions record` response shape | AGENTS.md | `{"decisions_recorded": N}` |
| `koto decisions list` response shape | AGENTS.md | `{"decisions": {"count": N}}` (not a raw array) |
| Rewind clears decisions | AGENTS.md | After rewind, decisions.count = 0 |
| Legacy warning on stderr | Neither | `koto init` with legacy template: warning goes to stderr |
| `gates.*` routing to terminal → action "done" | Neither | Not a blocking error; routing to terminal state produces `action: "done"` |
| State file naming convention | AGENTS.md | `koto-<name>.state.jsonl` in CWD |
| Name validation rules | AGENTS.md (partial) | `..` and leading `.` → exit code 2, message "invalid characters" |

---

## 9. What the koto-user Skill Needs That Current Docs Don't Provide

Based on this catalog, the koto-user skill specifically needs:

1. **response-shapes.md** — complete shapes for all `action` values, with the
   `blocking_conditions[N].output` subobject correctly documented for each gate type.
   Use the shapes from section 2 above.

2. **decisions-workflow.md** (or inline in response shapes) — correct shapes for
   `koto decisions record` and `koto decisions list` responses, plus the
   rewind-clears-decisions behavior.

3. **gates-user-perspective.md** — explains what a user/agent sees when gates run,
   without needing to understand template authoring. Covers:
   - `gate_blocked` vs auto-advance via `gates.*` routing
   - The `output` subobject differs by gate type
   - Gate failure is not always an error (it can be a routing signal)

4. **operational-patterns.md** — covers:
   - State file naming and location
   - Resume from interrupted session (`koto workflows`, then `koto next`)
   - When to use `koto rewind` vs fix-and-retry
   - Name validation rules (what names are valid, what errors to expect)
   - Legacy template warnings (stderr, not stdout)
