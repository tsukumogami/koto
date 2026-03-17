# Phase 3 Research: Gate Evaluation

## Questions Investigated
1. What Rust crates/stdlib APIs are needed for process group spawning and timeout? Do we need `nix` for `setpgid`/`killpg` on Unix?
2. How should gate results be represented? A `GateResult` enum (Passed/Failed/TimedOut/Error)?
3. Should gate evaluation happen in the CLI handler before calling the dispatcher, or should the dispatcher receive raw gate definitions and evaluate them?
4. How does the strategic design specify blocking_conditions in the output? What fields does each blocking condition need?
5. Are there any existing process spawning patterns in the codebase to follow?
6. What happens when multiple gates exist on a state -- all must pass (AND), or any (OR)?

## Findings

### 1. Process group spawning and timeout APIs

The codebase already includes `wait-timeout = "0.2"` as a Unix-only target dependency in `Cargo.toml` (line 26), declared specifically for command gate execution in #48. The migration design (`DESIGN-migrate-koto-go-to-rust.md`, line 198) confirms: "wait-timeout is included now for command gate execution in `koto next` (#48); it is Unix-only and correctly declared as a target-specific dependency."

The `wait-timeout` crate provides `ChildExt::wait_timeout()` on `std::process::Child`, returning `Ok(Some(status))` if the child exits within the timeout, `Ok(None)` if the timeout elapses, or `Err` on failure. This covers the timeout requirement without needing `tokio` or async.

For process group isolation (killing the entire tree on timeout, not just the direct child), the standard library's `std::process::Command` has a `.pre_exec()` method (Unix-only, unsafe) that can call `libc::setpgid(0, 0)` to put the child in its own process group. Killing the group then uses `libc::killpg(pgid, libc::SIGKILL)`. This requires `libc` as a dependency but NOT the full `nix` crate. The `libc` crate is likely already a transitive dependency (it's pulled in by virtually every Rust project on Unix).

Alternatively, `nix` provides safe wrappers (`nix::unistd::setpgid`, `nix::sys::signal::killpg`), but it's a heavier dependency. Since koto only needs two POSIX calls, raw `libc` with a thin safe wrapper is sufficient and matches the codebase's minimal-dependency philosophy (no external time crate, custom ISO 8601 formatting in `engine/types.rs`).

**Recommended approach:**
```rust
use std::process::Command;

#[cfg(unix)]
fn spawn_in_process_group(cmd: &str) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    unsafe {
        command.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
    command.spawn()
}
```

Then for timeout + kill:
```rust
use wait_timeout::ChildExt;
use std::time::Duration;

let timeout = Duration::from_secs(timeout_secs);
match child.wait_timeout(timeout)? {
    Some(status) => { /* exited within timeout */ },
    None => {
        // Timeout: kill the process group
        #[cfg(unix)]
        unsafe { libc::killpg(child.id() as i32, libc::SIGKILL); }
        child.kill().ok(); // fallback
        child.wait().ok(); // reap
    }
}
```

**No `tokio` needed.** The entire gate evaluation is synchronous, matching the rest of the codebase (no async anywhere in `src/`).

### 2. Gate result representation

A `GateResult` enum is the right approach, consistent with the codebase's typed-enum pattern (`EventPayload`, `EngineError`, and the proposed `NextResponse`/`NextError`). The enum should distinguish outcomes that map to different user-facing behavior:

```rust
pub enum GateResult {
    /// Command exited 0.
    Passed,
    /// Command exited non-zero.
    Failed { exit_code: i32 },
    /// Command exceeded its timeout and was killed.
    TimedOut,
    /// Command could not be spawned (e.g., sh not found).
    Error { message: String },
}
```

`Failed` and `TimedOut` are separate because the strategic design treats them differently in the output: a failed gate is "condition not satisfied" (the agent should wait or retry), while a timeout is operationally distinct (the command hung). Both result in `gate_blocked` error code, but the `blocking_conditions` detail should distinguish them.

`Error` covers spawn failures (missing `sh`, permission denied). This maps to exit code 3 (config error), not exit code 1 (transient). A gate that can't be spawned is a configuration problem, not a transient retry situation.

The aggregate result for a state is a `BTreeMap<String, GateResult>` keyed by gate name, since the output needs per-gate detail in `blocking_conditions`.

### 3. Gate evaluation: CLI handler vs. dispatcher

**Gate evaluation should happen in the CLI handler before calling the dispatcher.** This is consistent with the design document's architecture (`DESIGN-koto-cli-output-contract.md`, decision outcome section):

> "Gate evaluation and evidence validation are helper functions called before dispatch"

And:

> "Dispatcher (new module): Pure function that takes loaded state, template, flags, and gate results. Returns `Result<NextResponse, NextError>`. No I/O."

The dispatcher must be I/O-free to be testable as a pure function. Gate evaluation spawns shell commands -- that's I/O. So the CLI handler:
1. Loads state and template (I/O)
2. Evaluates gates for the current state (I/O -- shell commands)
3. Calls the dispatcher with gate results (pure)
4. Serializes and exits

This also means tests for the dispatcher can inject synthetic gate results without spawning processes. The gate evaluation function itself can be tested separately with actual commands in integration tests.

### 4. Blocking conditions in the output

The strategic design (`DESIGN-unified-koto-next.md`, lines 337-349) specifies the gate-blocked output variant:

```json
{
  "action": "execute",
  "state": "wait_for_ci",
  "directive": "Waiting for CI to pass...",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    { "name": "tests_passed", "type": "command", "agent_actionable": false }
  ],
  "error": null
}
```

Each `blocking_condition` has:
- `name`: the gate name from the template (key in `BTreeMap<String, Gate>`)
- `type`: the gate type string (always `"command"` for now, since field gates were removed in #47)
- `agent_actionable`: boolean -- `false` for command gates (koto verifies them, not the agent)

The PRD (R10) adds more detail: "structured detail for each unsatisfied condition: condition name, what it requires, and whether the agent can satisfy it (evidence gate) or koto will verify it independently (integration gate)."

Since only command gates remain (field gates replaced by accepts/when), all `blocking_conditions` entries will have `agent_actionable: false`. The field exists for forward compatibility -- if future gate types allow agent action, this field tells the agent whether to submit data or just wait and retry.

Additional fields worth including for operational usefulness:
- `status`: "failed" | "timed_out" | "error" -- so agents and humans can distinguish why a gate didn't pass
- `command`: the actual command string (useful for debugging; the command is already in the template, not secret)

The `DESIGN-koto-cli-output-contract.md` lists `BlockingCondition` as a supporting type in the response types module. The exact fields should be finalized in that design, but the strategic design's three fields (`name`, `type`, `agent_actionable`) are the minimum.

### 5. Existing process spawning patterns

**There are none.** The codebase has no existing process spawning code. The migration design explicitly deferred command gate execution to #48 (`DESIGN-migrate-koto-go-to-rust.md`, line 260: "Command gate execution: Not implemented in this issue. Command gates (shell execution with timeout) are deferred to #48, where process group isolation and timeout handling will be specified.").

The only process-adjacent code is `std::process::exit()` in `cli/mod.rs` for error exits. No `Command::new()`, no child process management anywhere.

This means the gate evaluation module will be the first OS-coupled code in the Rust codebase. It should be cleanly isolated in its own module (e.g., `src/gate.rs` or `src/gate/mod.rs`) to keep the process spawning code separate from the pure logic.

### 6. Multiple gates: AND or OR semantics

**AND (all must pass).** This is specified in multiple places:

- `DESIGN-koto-template-format.md`, line 323: "Gates on a state are exit conditions: all must pass (AND logic) before leaving that state. OR composition is not supported in Phase 1."
- `DESIGN-koto-template-format.md`, line 583: "Gates are exit conditions: all gates on a state must pass before leaving (AND logic)."
- Strategic design data flow (line 459): "evaluate gates: if any fail -> stop (gate_blocked)"

If any gate fails, the state is blocked. All failing gates should be reported in `blocking_conditions` (not just the first one), so the agent/human gets a complete picture of what's blocking advancement.

The evaluation order doesn't matter semantically (AND is commutative), but for efficiency, gates could be evaluated sequentially and short-circuited. However, since the output should list ALL failing gates (not just the first), all gates must be evaluated even if earlier ones fail. Parallel evaluation is possible but unnecessary given the 30s timeout per gate and the synchronous codebase.

### 7. Execution environment details (from template format design)

- Commands run via `sh -c "<command>"` (DESIGN-koto-template-format.md, line 585)
- Working directory: project root (git repository root, or CWD if not in a git repo) (line 585)
- No variable interpolation in command strings (security boundary) (line 321)
- Default timeout: 30 seconds, configurable per gate via `timeout` field (line 585, 663)
- Timed-out commands fail the gate (line 663)
- Gate execution happens only during advancement, not during template parse or validation (line 665)
- Commands inherit the user's full shell environment (line 661)

The `Gate` type in `src/template/types.rs` already has a `timeout: u32` field (line 74-76) with a default of 0 meaning "use default of 30s".

## Implications for Design

1. **New dependency: `libc`** (for `setpgid`/`killpg`). This should be a `[target.'cfg(unix)'.dependencies]` entry. Since koto targets linux/darwin only (per DESIGN-event-log-format.md line 434), this is fine. `libc` is almost certainly already a transitive dependency.

2. **New module needed:** A `src/gate.rs` (or `src/gate/mod.rs`) containing the `GateResult` enum, the process group spawn function, the timeout/kill logic, and an `evaluate_gates()` function that takes a `BTreeMap<String, Gate>` and returns `BTreeMap<String, GateResult>`.

3. **Dispatcher receives pre-computed results:** The dispatcher function signature should include something like `gate_results: &BTreeMap<String, GateResult>` alongside the state/template/flags. The dispatcher checks if any results are non-Passed and returns the `GateBlocked` variant of `NextResponse`.

4. **All gates evaluated even on failure:** Since the output must list all blocking conditions, the evaluator runs every gate and collects results, rather than short-circuiting on first failure.

5. **`BlockingCondition` struct** in the response types module maps directly from `GateResult` + gate metadata. The dispatcher constructs this from the gate results map.

6. **Platform gating:** The gate evaluation module needs `#[cfg(unix)]` paths for process group management. On non-Unix platforms (if ever supported), the fallback would skip process group isolation and just use `child.kill()` + `child.wait()` on timeout. Since koto currently targets Unix only, a compile error on non-Unix is acceptable.

7. **Working directory resolution:** The evaluator needs to determine the project root (git root or CWD). This is a side-effect that happens before calling the gate command. The `discover.rs` module may already have git root detection; if not, it's a small addition (`git rev-parse --show-toplevel` or walk up looking for `.git`).

## Surprises

1. **No existing process spawning code at all.** The entire Rust codebase has zero process management. This means the gate evaluator is new infrastructure with no local patterns to follow -- but the design documents anticipated this and included `wait-timeout` in the dependency list during the Go-to-Rust migration.

2. **`wait-timeout` is already a dependency.** It was added proactively during the Rust migration (#45) specifically for #48's gate evaluation needs. This is good planning -- no new dependency negotiation needed for the timeout mechanism.

3. **The `timeout` field on `Gate` is `u32`, not `Option<u32>`.** It defaults to 0 via serde, with a helper `is_zero()` for skip-serializing. The evaluator needs to interpret `0` as "use default 30s" rather than "no timeout." This is documented in the comment on line 73-74 of `types.rs`.

4. **`libc` is not an explicit dependency yet.** While it's almost certainly transitive, the gate evaluator will need direct `libc` calls for `setpgid` and `killpg`. It should be added as an explicit `[target.'cfg(unix)'.dependencies]` entry.

## Summary

Gate evaluation is self-contained new infrastructure. The `wait-timeout` crate (already a dependency) handles timeout, `libc::setpgid`/`killpg` via `pre_exec` handles process group isolation, and no async or `nix` crate is needed. The evaluator runs all gates (AND semantics), collects per-gate `GateResult` values, and passes them to the pure dispatcher which constructs `blocking_conditions` output. Gate evaluation happens in the CLI handler layer to keep the dispatcher I/O-free and testable with synthetic gate results.
