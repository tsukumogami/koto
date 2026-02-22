# Validation Report: Issue #17 - Command Gate Execution

**Date**: 2026-02-22
**Issue**: #17 (feat(engine): implement command gate execution)
**Scenarios validated**: scenario-19, scenario-20
**Result**: ALL PASSED (7/7 tests, 2/2 scenarios)

## Test Execution

```
$ go test ./pkg/engine/... -v -count=1 -run "TestGate_Command"
=== RUN   TestGate_Command_ExitZero_Passes
--- PASS: TestGate_Command_ExitZero_Passes (0.01s)
=== RUN   TestGate_Command_NonZeroExit_Fails
--- PASS: TestGate_Command_NonZeroExit_Fails (0.01s)
=== RUN   TestGate_Command_Timeout
--- PASS: TestGate_Command_Timeout (1.01s)
=== RUN   TestGate_Command_NoInterpolation
--- PASS: TestGate_Command_NoInterpolation (0.01s)
=== RUN   TestGate_Command_DefaultTimeout
--- PASS: TestGate_Command_DefaultTimeout (0.01s)
=== RUN   TestGate_Command_CWD
--- PASS: TestGate_Command_CWD (0.01s)
=== RUN   TestGate_Command_StdoutNotCaptured
--- PASS: TestGate_Command_StdoutNotCaptured (0.01s)
PASS
ok  github.com/tsukumogami/koto/pkg/engine  1.079s
```

Full project regression check also passed:
```
$ go test ./... -count=1
ok  github.com/tsukumogami/koto/cmd/koto        1.913s
ok  github.com/tsukumogami/koto/internal/buildinfo 0.002s
ok  github.com/tsukumogami/koto/pkg/controller   0.044s
ok  github.com/tsukumogami/koto/pkg/discover      0.004s
ok  github.com/tsukumogami/koto/pkg/engine         2.003s
ok  github.com/tsukumogami/koto/pkg/template       0.024s
ok  github.com/tsukumogami/koto/pkg/template/compile 0.002s
```

## Scenario 19: Command gate executes shell command and blocks on failure

**Status**: PASSED

### Test-to-requirement mapping

| Requirement | Test | Verified |
|---|---|---|
| `exit 1` fails with `gate_failed` | TestGate_Command_NonZeroExit_Fails | Yes |
| Error code is `ErrGateFailed` | TestGate_Command_NonZeroExit_Fails | Yes |
| Error message references gate name and "command" | TestGate_Command_NonZeroExit_Fails | Yes |
| State unchanged after gate failure | TestGate_Command_NonZeroExit_Fails | Yes |
| `exit 0` succeeds and transitions | TestGate_Command_ExitZero_Passes | Yes |
| CWD is git repo root | TestGate_Command_CWD | Yes |
| stdout/stderr not captured in error | TestGate_Command_StdoutNotCaptured | Yes |

### Analysis

- **TestGate_Command_NonZeroExit_Fails**: Creates machine with `exit 1` gate. Asserts transition returns `*TransitionError` with `Code == ErrGateFailed`. Checks message contains both the gate name ("check") and "command". Verifies state remains at "start" after failure.

- **TestGate_Command_ExitZero_Passes**: Creates machine with `exit 0` gate. Asserts transition succeeds with no error. Verifies state moved to "done".

- **TestGate_Command_CWD**: Runs `pwd > <file>` as gate command, then reads file and compares to `git rev-parse --show-toplevel`. Validates CWD matches git repo root. Falls back to non-empty check if not in a git repo.

- **TestGate_Command_StdoutNotCaptured**: Runs a command that writes to both stdout and stderr, exits 0. Verifies transition succeeds and no stdout/stderr content appears in engine evidence. Implementation sets `cmd.Stdout = nil` and `cmd.Stderr = nil` to discard output by design.

## Scenario 20: Command gate enforces timeout and does not interpolate variables

**Status**: PASSED

**Environment note**: scenario-20 is marked `Environment: manual` because timeout behavior may vary by system load. The test passed on this run. The test uses a generous bound (elapsed < 5s for a 1s timeout) to reduce flakiness.

### Test-to-requirement mapping

| Requirement | Test | Verified |
|---|---|---|
| `sleep 60` with `timeout: 1` fails | TestGate_Command_Timeout | Yes |
| Error contains "timed out" | TestGate_Command_Timeout | Yes |
| Completes in ~1s not 60s | TestGate_Command_Timeout | Yes |
| `{{TASK}}` NOT expanded | TestGate_Command_NoInterpolation | Yes |
| Command passed literally to `sh -c` | TestGate_Command_NoInterpolation | Yes |

### Analysis

- **TestGate_Command_Timeout**: Creates machine with `sleep 60` gate, timeout 1 second. Measures wall-clock time. Asserts error is `*TransitionError` with `Code == ErrGateFailed` and message containing "timed out". Confirms elapsed < 5 seconds (generous bound for CI stability). Implementation uses `context.WithTimeout` and kills the process group via `syscall.Kill(-pid, SIGKILL)` on timeout.

- **TestGate_Command_NoInterpolation**: This is the security-critical test. Creates a machine with declared variable `TASK = "should-not-appear"`. The gate command is `printf '{{TASK}}' > <file>`. After transition succeeds, reads the output file. Asserts the file contains the literal string `{{TASK}}`, NOT the variable value. This confirms the command string is passed to `sh -c` without any template interpolation. The implementation achieves this simply by passing `gate.Command` directly to `exec.CommandContext` with no preprocessing.

### Bonus test

- **TestGate_Command_DefaultTimeout**: Verifies that `timeout: 0` uses a 30-second default (implementation: `if gate.Timeout <= 0 { timeout = 30 * time.Second }`). Tests indirectly by running a fast command and confirming it doesn't fail with instant timeout.

## Implementation quality notes

The `evaluateCommandGate` function at `pkg/engine/engine.go:607`:
- Uses `exec.CommandContext` with `context.WithTimeout` for deadline enforcement
- Sets `SysProcAttr{Setpgid: true}` to create a process group for clean kills
- Kills the process group (negative PID) on timeout to handle child processes
- Sets both `Stdout` and `Stderr` to nil (output discarded, not captured)
- Uses `gitRepoRoot()` for CWD with proper fallback to `os.Getwd()`
- No string interpolation of the command string at any point
