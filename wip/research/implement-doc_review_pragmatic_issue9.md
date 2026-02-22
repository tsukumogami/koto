# Pragmatic Review: Issue #9 - Add Remaining CLI Subcommands

**Files reviewed:**
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go` (major rewrite)
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main_test.go` (new, 23 tests)
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go` (signature change)
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller_test.go` (updated)
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go` (minor)

**Issue scope**: Wire template parsing into init/next, add query/status/rewind/cancel/validate/workflows subcommands, implement --state flag with auto-selection. CLI is a thin translation layer.

## Summary

The implementation is clean and stays within scope. The CLI is genuinely a thin layer -- each command is 15-30 lines of flag parsing, package calls, and JSON output. The shared helpers (`resolveStatePath`, `loadTemplateFromState`, `parseFlags`) are justified by being called from 6+ commands each. The `controller.New` signature change from the walking skeleton (`*template.Template` instead of `string` hash) aligns with the design doc's prescribed API. No blocking findings.

## Findings

### 1. [Advisory] `cmdValidate` duplicates state file reading instead of using `loadTemplateFromState`

`cmd/koto/main.go:361-396` -- `cmdValidate` manually reads the state file with `os.ReadFile` + `json.Unmarshal` into `engine.State` to extract both `template_path` and `template_hash`. Meanwhile, `loadTemplateFromState` (line 454) already reads the state file and extracts `template_path`. The validate command needs the stored hash too, which `loadTemplateFromState` doesn't expose, so the duplication is functionally justified. But consider whether `loadTemplateFromState` should return the stored hash as a second value, collapsing the two paths. Not blocking because `cmdValidate` is a leaf command with no interaction with other commands, and the duplication is ~10 lines.

### 2. [Advisory] `cmdStatus` mixes output modes

`cmd/koto/main.go:267-293` -- `cmdStatus` uses `fmt.Printf` for human-readable output while every other command (except `cancel`) uses `printJSON`. The design doc explicitly prescribes this split ("JSON output formatter for agent-facing commands, text output formatter for human-facing commands"), so this is correct behavior. Noting it only because `status` is the one command that doesn't fit the JSON-everywhere pattern -- future callers wanting machine-parseable status output would need to use `query` instead. No action needed.

### 3. [Advisory] `parseFlags` treats any string starting with `-` as a flag

`cmd/koto/main.go:482-484` -- `isFlag` matches any string starting with `-`, including negative numbers or values like `-rf`. If a `--var` value started with a dash (e.g., `--var KEY=-value`), parsing would fail with "requires a value" because `-value` would be interpreted as a flag. This is an edge case with a simple workaround (use `KEY=-value` with `=` in the value portion, which the `--var` parser handles differently). Not blocking because koto's variable values are workflow metadata, not arbitrary user input, and the failure mode is a clear error message rather than silent misbehavior.

### 4. [Advisory] `TestCmdNext_ReturnsDirective` doesn't verify output content

`cmd/koto/main_test.go:205-229` -- Lines 222-223 parse the template and assign it to `_ = tmpl` with a comment "used only to verify the test is meaningful." This is dead code -- the parsed template isn't checked against anything. The test only verifies `cmdNext` doesn't error. The integration test (Scenario 23) covers the full lifecycle, so this test's value is limited to "next doesn't crash on a freshly initialized workflow." The dead assignment should be removed. Not blocking because it's inert test code.

### 5. [Advisory] `controller.New` accepts nil template with fallback behavior

`pkg/controller/controller.go:31-44` -- When `tmpl` is nil, hash verification is skipped and `Next` returns a generic stub directive. This nil-template path exists to support the walking skeleton (issue #4) where templates weren't implemented yet. Now that issue #9 wires real templates into every CLI command, no caller passes nil. The nil path is tested (`TestNew_NilTemplateSkipsVerification`, `TestNext_NonTerminalState`) but serves no production caller. Not blocking because the code is small (one `if` check) and removing it would break the existing tests without functional benefit -- it's effectively a "library consumer who doesn't use templates" path that's reasonable to keep.
