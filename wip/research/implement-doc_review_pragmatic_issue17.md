# Pragmatic Review: Issue #17 (feat(engine): implement command gate execution)

## Summary

The implementation correctly adds command gate execution to the engine. It matches all design doc requirements: `sh -c` execution, git root CWD, configurable timeout with 30s default, no variable interpolation (verified by explicit test), and timed-out commands fail the gate. Test coverage is solid across happy path, error path, timeout, no-interpolation security boundary, CWD, and stdout/stderr handling.

No blocking findings.

## Findings

### Advisory-1: Dead assignments to nil zero values

**File:** `pkg/engine/engine.go:618-619`
**Severity:** Advisory

```go
cmd.Stdout = nil
cmd.Stderr = nil
```

These are already the zero values for `exec.Cmd`. The assignments are no-ops. They do serve as documentation that stdout/stderr are intentionally not captured, so this is defensible as intent signaling. Minor.

**Suggestion:** Could add a comment like `// Intentionally not captured (exit code only)` instead, or keep as-is.

### Advisory-2: gitRepoRoot() shells out on every command gate

**File:** `pkg/engine/engine.go:622`
**Severity:** Advisory

`gitRepoRoot()` runs `git rev-parse --show-toplevel` on every command gate evaluation. If a state has multiple command gates, this shells out multiple times within one `Transition()` call. The result won't change between calls.

In practice, states with multiple command gates are rare, and the `git` call is fast (~5ms). Not worth caching unless profiling shows it matters.

### Advisory-3: Platform portability (syscall.Setpgid, syscall.Kill)

**File:** `pkg/engine/engine.go:617, 630`
**Severity:** Advisory

`syscall.SysProcAttr{Setpgid: true}` and `syscall.Kill(-pid, SIGKILL)` are Unix-specific. This code won't compile on Windows. CI runs on ubuntu-latest only, so this is fine for now. If Windows support becomes a goal, this needs a build-tagged abstraction.

## Verification

| Requirement | Status | Evidence |
|------------|--------|----------|
| `sh -c` execution | Met | Line 616: `exec.CommandContext(ctx, "sh", "-c", gate.Command)` |
| CWD = git root or process CWD | Met | Line 622: `cmd.Dir = gitRepoRoot()`, with fallback on line 656 |
| Default 30s timeout | Met | Lines 608-611: `if gate.Timeout <= 0 { timeout = 30 * time.Second }` |
| Configurable timeout | Met | Line 608: `time.Duration(gate.Timeout) * time.Second` |
| No variable interpolation | Met | Command string passed directly to `sh -c`; `TestGate_Command_NoInterpolation` verifies |
| Timed-out commands fail gate | Met | Lines 627-638: checks `ctx.Err()` for `DeadlineExceeded` |
| Process group cleanup on timeout | Met | Lines 617, 629-630: `Setpgid` + `Kill(-pid, SIGKILL)` |
| Exit code only, no stdout capture | Met | Lines 618-619; `TestGate_Command_StdoutNotCaptured` |

## Test Coverage

| Test | What it covers |
|------|---------------|
| `TestGate_Command_ExitZero_Passes` | Happy path: exit 0 passes gate |
| `TestGate_Command_NonZeroExit_Fails` | Exit 1 fails gate with correct error code |
| `TestGate_Command_Timeout` | 1s timeout on `sleep 60`; verifies timing |
| `TestGate_Command_NoInterpolation` | Security: `{{TASK}}` literal survives to shell |
| `TestGate_Command_DefaultTimeout` | Timeout=0 uses default (indirect: fast command passes) |
| `TestGate_Command_CWD` | Verifies CWD matches git root |
| `TestGate_Command_StdoutNotCaptured` | stdout/stderr don't leak into state |

## Overall Assessment

Clean implementation. The scope matches the issue exactly -- no scope creep, no unnecessary abstractions. The `evaluateCommandGate` function is the right level of extraction (it handles timeout, context, process group cleanup). The `gitRepoRoot` helper is the only other addition, and it's well-scoped. Test coverage hits all the important cases including the security boundary (no interpolation).
