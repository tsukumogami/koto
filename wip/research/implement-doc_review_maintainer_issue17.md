# Maintainer Review: Issue #17 (feat(engine): implement command gate execution)

## Review Scope

New code in this issue:
- `pkg/engine/engine.go`: `evaluateCommandGate()` (lines 603-649), `gitRepoRoot()` (lines 651-660), `"command"` case in `evaluateGates` switch (line 586-589), new imports (`context`, `errors`, `os/exec`, `syscall`, `strings`, `path/filepath`)
- `pkg/engine/engine_test.go`: `commandGateMachine()` helper and 7 test functions (`TestGate_Command_ExitZero_Passes`, `TestGate_Command_NonZeroExit_Fails`, `TestGate_Command_Timeout`, `TestGate_Command_NoInterpolation`, `TestGate_Command_DefaultTimeout`, `TestGate_Command_CWD`, `TestGate_Command_StdoutNotCaptured`)

## Findings

### 1. Gate type strings remain bare literals across three packages (Advisory)

**File**: `pkg/engine/engine.go:556,567,586` and `pkg/template/compiled.go:81,85,92`

The strings `"field_not_empty"`, `"field_equals"`, and `"command"` now appear as bare literals in both the engine's `evaluateGates` switch and the template's `ParseJSON` validation switch. With issue #17, `"command"` became load-bearing in the engine (it triggers real shell execution). A typo in either location silently falls through to the `default` case.

This was flagged as advisory in prior reviews (#13, #16). It remains advisory: the test suite would catch a typo because tests use the same strings. But the surface area has grown -- these strings now appear in `pkg/engine/engine.go`, `pkg/template/compiled.go`, `pkg/template/compile/compile.go`, and ~40 test locations across three packages. Defining `GateTypeFieldNotEmpty`, `GateTypeFieldEquals`, `GateTypeCommand` constants on `engine.GateDecl` (or in `engine/errors.go` next to the error constants) would centralize the single source of truth.

Not blocking because the tests would catch a mismatch. But the next developer adding a gate type (e.g., `"prompt"` for LLM gates) will need to update both switch statements manually, and nothing in the compiler or tests would catch if they only updated one.

**Severity**: Advisory

---

### 2. Non-zero exit error message omits the exit code (Advisory)

**File**: `pkg/engine/engine.go:641-642`

```go
Message: fmt.Sprintf("gate %q failed: command gate returned non-zero exit status", name),
```

When a command gate fails with a non-zero exit code, the error message says "non-zero exit status" but doesn't include the actual exit code. The `exec.ExitError` type from `cmd.Run()` carries the exit code. Including it (e.g., "command gate returned exit status 2") gives the next developer or agent actionable information -- exit code 1 vs 2 vs 127 (command not found) vs 126 (permission denied) tells very different stories about what went wrong.

The test at line 2836 only checks for the substring "command", so adding the exit code wouldn't break tests.

**Severity**: Advisory

---

### 3. `gitRepoRoot()` is called once per command gate evaluation (Advisory)

**File**: `pkg/engine/engine.go:622`

```go
cmd.Dir = gitRepoRoot()
```

`gitRepoRoot()` shells out to `git rev-parse --show-toplevel` on every command gate evaluation. If a state has multiple command gates, each one spawns a separate `git` process for the same answer. The result won't change between gate evaluations in the same `Transition()` call.

This isn't a correctness issue -- the function is idempotent. But the next developer reading `evaluateGates` won't see that the CWD resolution happens inside `evaluateCommandGate`, and if they add a second command gate to a test state, they might be surprised by the extra `git` invocation in race-condition-sensitive environments.

Could cache the result in `evaluateGates` and pass it into `evaluateCommandGate`, but this is a minor inefficiency, not a misread risk.

**Severity**: Advisory (minor inefficiency, not a clarity issue)

---

### 4. Test names accurately describe behavior

The test naming follows a consistent pattern (`TestGate_Command_<scenario>`) that clearly communicates what each test verifies:
- `ExitZero_Passes` -- exit 0 means gate passes
- `NonZeroExit_Fails` -- non-zero means gate fails
- `Timeout` -- long command with short timeout
- `NoInterpolation` -- security boundary test
- `DefaultTimeout` -- timeout=0 uses 30s default
- `CWD` -- command runs from git root
- `StdoutNotCaptured` -- output doesn't leak into state

The `NoInterpolation` test at line 2880 is particularly well done -- it writes the literal `{{TASK}}` to a file and verifies it wasn't expanded, which directly tests the security boundary the design doc requires.

### 5. `evaluateCommandGate` function is well-structured

The function clearly separates concerns:
1. Timeout resolution (lines 608-611)
2. Context creation (lines 613-614)
3. Command setup (lines 616-622)
4. Execution (line 624)
5. Error classification: timeout vs non-zero exit (lines 625-646)

The `SysProcAttr` with `Setpgid: true` (line 617) and the process group kill on timeout (lines 629-631) show careful handling of child process cleanup. The godoc at line 603-606 accurately describes the behavior.

### 6. `cmd.Stdout = nil` and `cmd.Stderr = nil` are explicit (Good)

**File**: `pkg/engine/engine.go:618-619`

Setting these explicitly to nil makes the design intent clear: stdout/stderr are deliberately discarded. The next developer won't wonder whether this was an oversight. This matches the design doc's statement that "Exit codes only. stdout/stderr not captured or stored."

## Summary

The implementation is clean and well-tested. The `evaluateCommandGate` function is appropriately sized, clearly documented, and handles the key concerns (timeout, process group cleanup, no interpolation) that the design doc requires. Test coverage is thorough with good scenario naming.

No blocking findings. The gate type string literals remain advisory (same as prior reviews). The only new advisory finding specific to this issue is the missing exit code in the non-zero exit error message, which would improve debuggability.
