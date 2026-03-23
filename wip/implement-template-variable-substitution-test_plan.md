# Test Plan: Template variable substitution

Generated from: docs/plans/PLAN-template-variable-substitution.md
Issues covered: 3

---

## Scenario 1: substitute() replaces defined variables in a string
**ID**: scenario-1
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- Build the project: `cargo build`
- Run unit tests: `cargo test substitute`
**Expected**: `Variables::substitute("check {{ISSUE_NUMBER}} status")` with `ISSUE_NUMBER=42` returns `"check 42 status"`. Multiple variables in one string are all replaced. The substitution is single-pass (output of substitution is not re-processed).
**Status**: pending

---

## Scenario 2: substitute() panics on undefined variable references
**ID**: scenario-2
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test substitute`
**Expected**: Calling `substitute("{{UNDEFINED}}")` with a Variables map that does not contain `UNDEFINED` panics with a descriptive message indicating a corrupted state file or compiler bug.
**Status**: pending

---

## Scenario 3: substitute() passes through unclosed and non-matching patterns
**ID**: scenario-3
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test substitute`
**Expected**: Input like `"unclosed {{ brace"`, `"{{lowercase}}"`, and `"no braces"` all pass through unchanged. Only `{{[A-Z][A-Z0-9_]*}}` patterns trigger substitution.
**Status**: pending

---

## Scenario 4: Variables::from_events() re-validates values from state file
**ID**: scenario-4
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test from_events`
**Expected**: `from_events` with a WorkflowInitialized event containing a value like `"; rm -rf /"` (characters outside `[a-zA-Z0-9._/-]`) returns an error, not a valid Variables struct. Values matching the allowlist succeed.
**Status**: pending

---

## Scenario 5: compile-time validation rejects undeclared variable references
**ID**: scenario-5
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test validate`
**Expected**: A template with a gate command containing `{{UNDECLARED}}` where `UNDECLARED` is not in the `variables` block fails `CompiledTemplate::validate()` with an error naming the undeclared variable and the state where it appears. A template where all `{{KEY}}` references match declared variables passes validation.
**Status**: pending

---

## Scenario 6: compile-time validation catches references in directive text
**ID**: scenario-6
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test validate`
**Expected**: A template with a directive string containing `{{UNDECLARED}}` that is not in the `variables` block fails validation. Directives with only declared variable references pass.
**Status**: pending

---

## Scenario 7: event type narrowing preserves round-trip serialization
**ID**: scenario-7
**Testable after**: Issue 1
**Category**: infrastructure (automatable)
**Commands**:
- `cargo test`
**Expected**: Existing serialization and round-trip tests for `EventPayload::WorkflowInitialized` and `WorkflowInitializedPayload` pass after the type change from `HashMap<String, serde_json::Value>` to `HashMap<String, String>`. Empty variables maps deserialize correctly. Non-empty `HashMap<String, String>` round-trips through JSON.
**Status**: pending

---

## Scenario 8: koto init accepts --var KEY=VALUE flags
**ID**: scenario-8
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- Create a template YAML with `variables:` block declaring `ISSUE_NUMBER` (required) and `PREFIX` (optional, default: `wip`)
- `koto template compile template.yaml`
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42`
**Expected**: Init succeeds with exit code 0. The state file's WorkflowInitialized event contains `variables: {"ISSUE_NUMBER": "42", "PREFIX": "wip"}` (default applied for PREFIX). JSON output shows the workflow name and initial state.
**Status**: pending

---

## Scenario 9: koto init rejects missing required variables
**ID**: scenario-9
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with `ISSUE_NUMBER` declared as required
- `koto init test-wf --template template.yaml`
**Expected**: Init fails with a non-zero exit code and an error message indicating that required variable `ISSUE_NUMBER` is missing.
**Status**: pending

---

## Scenario 10: koto init rejects unknown variable keys
**ID**: scenario-10
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with only `ISSUE_NUMBER` declared
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42 --var UNKNOWN=foo`
**Expected**: Init fails with a non-zero exit code and an error message indicating that `UNKNOWN` is not declared in the template.
**Status**: pending

---

## Scenario 11: koto init rejects duplicate --var keys
**ID**: scenario-11
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42 --var ISSUE_NUMBER=99`
**Expected**: Init fails with a non-zero exit code and an error message about duplicate variable key `ISSUE_NUMBER`.
**Status**: pending

---

## Scenario 12: koto init rejects values with forbidden characters
**ID**: scenario-12
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER="42; rm -rf /"`
**Expected**: Init fails with a non-zero exit code and an error message naming the forbidden character (`;`) and the variable (`ISSUE_NUMBER`). Values like `42`, `my-project`, `path/to/file`, and `v1.2.3` are accepted.
**Status**: pending

---

## Scenario 13: koto init rejects malformed --var syntax
**ID**: scenario-13
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- `koto init test-wf --template template.yaml --var NOEQUALS`
- `koto init test-wf --template template.yaml --var =value`
**Expected**: Both fail with non-zero exit codes. First errors on missing `=`. Second errors on empty key.
**Status**: pending

---

## Scenario 14: koto init works with templates that have no variables block
**ID**: scenario-14
**Testable after**: Issue 2
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with no `variables:` section
- `koto init test-wf --template template.yaml`
**Expected**: Init succeeds with exit code 0. No variables stored in event. Existing workflows without variables continue to work.
**Status**: pending

---

## Scenario 15: koto next substitutes variables in gate commands
**ID**: scenario-15
**Testable after**: Issue 3
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with a gate command `test -f wip/issue_{{ISSUE_NUMBER}}_context.md` and variable `ISSUE_NUMBER` (required)
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42`
- `koto next test-wf`
**Expected**: The gate evaluates the substituted command `test -f wip/issue_42_context.md`, not the raw template string. If the file doesn't exist, the gate fails with a blocking condition referencing the actual path.
**Status**: pending

---

## Scenario 16: koto next substitutes variables in directive text
**ID**: scenario-16
**Testable after**: Issue 3
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with directive text `"Work on issue {{ISSUE_NUMBER}} using prefix {{PREFIX}}"` and both variables declared
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42`
- `koto next test-wf`
**Expected**: The JSON response's `directive` field contains `"Work on issue 42 using prefix wip"` (with both variables substituted, including the default value for PREFIX).
**Status**: pending

---

## Scenario 17: koto next --to substitutes directive text in target state
**ID**: scenario-17
**Testable after**: Issue 3
**Category**: infrastructure (automatable)
**Commands**:
- Create a template with multiple states, target state has directive `"Review issue {{ISSUE_NUMBER}}"`
- `koto init test-wf --template template.yaml --var ISSUE_NUMBER=42`
- `koto next test-wf --to target-state`
**Expected**: The JSON response's `directive` field contains `"Review issue 42"`, not the raw `{{ISSUE_NUMBER}}` reference.
**Status**: pending

---

## Scenario 18: koto next returns error on tampered state file values
**ID**: scenario-18
**Testable after**: Issue 3
**Category**: infrastructure (automatable)
**Commands**:
- Init a workflow with valid variables
- Manually edit the state file JSONL to change a variable value to `"bad;value"`
- `koto next test-wf`
**Expected**: koto next fails with a structured error indicating re-validation failure (value contains forbidden characters). The gate command is never executed with the tampered value.
**Status**: pending

---

## Scenario 19: end-to-end variable substitution through a multi-state workflow
**ID**: scenario-19
**Testable after**: Issue 3
**Category**: use-case (automatable)
**Commands**:
- Create a template that models a simplified work-on workflow with 3 states: `start` (gate: `test -f wip/issue_{{ISSUE_NUMBER}}_plan.md`), `implement` (directive: `"Implement issue {{ISSUE_NUMBER}}"`), `done` (terminal)
- Declare `ISSUE_NUMBER` as required variable
- `koto init work --template template.yaml --var ISSUE_NUMBER=55`
- `touch wip/issue_55_plan.md` (satisfy the gate)
- `koto next work` (should advance past start, show implement directive)
- Verify the directive says `"Implement issue 55"`
- Submit evidence and advance to done
- `koto next work` (terminal state)
**Expected**: The entire workflow completes with variables correctly substituted at every point: gate commands use `55` for file path checks, directive text shows `55` instead of `{{ISSUE_NUMBER}}`. This validates the full data flow from init through runtime substitution.
**Status**: pending

---

## Scenario 20: compile-time validation catches variable typos before init
**ID**: scenario-20
**Testable after**: Issue 1
**Category**: use-case (automatable)
**Commands**:
- Create a template that declares `ISSUE_NUMBER` but references `{{ISSUE_NUMBR}}` (typo) in a gate command
- `koto template compile template.yaml`
**Expected**: Compilation fails with an error naming `ISSUE_NUMBR` as an undeclared variable reference and identifying the state where it appears. The user can fix the typo before ever running `koto init`. This validates the core user value: catching template authoring mistakes early.
**Status**: pending
