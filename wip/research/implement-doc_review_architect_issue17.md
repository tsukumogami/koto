# Architect Review: Issue #17 -- Command Gate Execution

## Review Scope

Issue #17 adds the `command` gate type to the engine's gate evaluation framework. The changes are in `pkg/engine/engine.go` (functions `evaluateCommandGate`, `gitRepoRoot`, and the `case "command"` branch in `evaluateGates`) plus corresponding tests in `pkg/engine/engine_test.go`.

## Design Alignment

The implementation aligns with the design doc (DESIGN-koto-template-format.md) on all key requirements:

- **sh -c execution**: `evaluateCommandGate` uses `exec.CommandContext(ctx, "sh", "-c", gate.Command)` -- matches the design's "sh -c from project root" specification.
- **Configurable timeout with 30s default**: Lines 608-611 apply `gate.Timeout` or fall back to 30 seconds.
- **No variable interpolation**: The command string passes to `sh -c` unmodified. The `TestGate_Command_NoInterpolation` test explicitly verifies this security boundary by writing `{{TASK}}` through the shell and confirming the literal placeholder survives.
- **CWD is git repo root**: `gitRepoRoot()` runs `git rev-parse --show-toplevel`, falling back to `os.Getwd()`.
- **Exit code only**: `cmd.Stdout` and `cmd.Stderr` are explicitly set to nil. The `TestGate_Command_StdoutNotCaptured` test confirms output doesn't leak into state.
- **Timed-out commands fail the gate**: Timeout detection via `context.DeadlineExceeded` with process group kill.

## Pattern Consistency

### Gate evaluation framework (GOOD)

The command gate plugs into the existing `evaluateGates` switch statement established by #16. `evaluateGates` iterates over `map[string]*GateDecl` with AND logic, and the `case "command":` branch delegates to `evaluateCommandGate`. This follows the same dispatch pattern as `field_not_empty` and `field_equals` -- no parallel evaluation path introduced.

### Error types (GOOD)

Command gate failures use the same `ErrGateFailed` error code and `TransitionError` type as field-based gates. No new error codes or types introduced for command-specific failures.

### Process group management (GOOD)

`SysProcAttr = &syscall.SysProcAttr{Setpgid: true}` creates a new process group, and the timeout handler kills the entire group with `syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)`. This prevents orphaned child processes from `sh -c` pipelines.

## Findings

### Finding 1: `gitRepoRoot()` is a package-level function with side effects

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:653-660`
**Severity**: Advisory

`gitRepoRoot()` executes `git rev-parse --show-toplevel` as a side effect every time a command gate evaluates. This is called from inside `evaluateCommandGate`, which is called from `evaluateGates`, which is inside `Engine.Transition`. The function is not mockable or configurable -- it's a hard-coded shell-out inside the engine package.

This is contained for now: command gates are the only consumer, and the design doc explicitly says "CWD is the git repo root if available, otherwise process CWD." But if the engine ever needs to run in environments where shelling out to `git` is undesirable (embedded use, sandboxed environments), this will need to be extracted.

No structural divergence -- the function follows the same pattern as the command execution itself (both shell out). Not blocking because the coupling is contained within the command gate feature and doesn't affect other engine operations.

### Finding 2: `syscall` import ties the engine to Unix platforms

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:12-13`
**Severity**: Advisory

The `syscall` package import (for `Setpgid` and `SIGKILL` via process group kill) is Unix-specific. On Windows, this won't compile. The engine package previously had no platform-specific code.

This is an acceptable trade-off given koto's current Unix-only target. The platform dependency is contained to the command gate code path. If cross-platform support becomes a goal, the process group management would need build-tag separation. Not blocking because there's no stated Windows support requirement.

### Finding 3: CLI `cmdTransition` still does not pass evidence to the engine

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:173-217`
**Severity**: Advisory (pre-existing, not introduced by #17)

`cmdTransition` calls `eng.Transition(target)` with no `WithEvidence()` option, and `parseFlags(args, nil)` declares no multi-value flags for `--evidence`. This means command gates work at the engine level but users can't supply evidence via the CLI to satisfy field-based gates that coexist with command gates on the same state.

This was noted in the #16 review and is tracked separately. Not introduced by this issue.

### Finding 4: Template parsing already handles command gates correctly

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:92-95`
**Severity**: Not a finding -- confirming end-to-end alignment

The compiled template parser (`ParseJSON`) validates `case "command":` gates, checking for non-empty `Command` field. `BuildMachine()` copies `GateDecl` values including `Command` and `Timeout` fields into the engine's `Machine`. The source compiler (`pkg/template/compile/compile.go`) maps `sourceGateDecl.Command` and `Timeout` through to `engine.GateDecl`. The full pipeline from source -> compiled JSON -> engine.Machine -> gate evaluation is connected.

### Finding 5: Dependency direction is correct

The new code in `pkg/engine/` imports only stdlib packages (`context`, `os/exec`, `errors`, `syscall`, `strings`, `time`). No upward dependency on template, controller, or CLI packages. The engine remains the lowest-level package in the dependency graph.

## Overall Assessment

The implementation fits the existing architecture cleanly. Command gate execution plugs into the gate evaluation framework established by #16 without introducing parallel patterns. The `evaluateCommandGate` function is a self-contained addition that follows the same error reporting conventions as field-based gates. The security boundary (no interpolation in command strings) is explicitly tested. Process cleanup via process group kill is well-handled.

No blocking findings. The two advisory observations (platform-specific syscall, non-mockable gitRepoRoot) are contained and don't affect other callers or create patterns that will diverge.
