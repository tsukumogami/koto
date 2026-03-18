# Phase 3 Research: Signal Handling and Event Log Atomicity

## Questions Investigated
- How does `append_event` guarantee atomicity? What's the write pattern?
- How does the gate evaluator's process group isolation work? Does it clean up child processes on unexpected termination?
- What happens if SIGTERM arrives during a gate evaluation subprocess?
- What signal handling crates are available in the Rust ecosystem?
- Should the engine return a special response when stopped by signal?
- How should `koto cancel` differ from signal-based shutdown?
- Does `Cargo.toml` already include any signal-related dependencies?

## Findings

### 1. `append_event` Atomicity (`src/engine/persistence.rs`, lines 42-80)

The write pattern is: serialize to JSON string, `writeln!` (single buffered write), then `sync_data()`. This is an append-mode write (`OpenOptions::append(true)`) followed by fsync.

**Partial write risk is real but handled downstream.** A single `writeln!` call does not guarantee an atomic write at the OS level. If the process is killed between `writeln!` completing partially and `sync_data()`, the file could contain a truncated final line. However, `read_events` (lines 147-214) explicitly handles this: a malformed final line is treated as recoverable -- it prints a warning and returns all events up to the last valid one. A malformed non-final line is treated as corruption.

This means the event log is **crash-safe but not crash-proof**: you might lose the in-flight event, but you won't corrupt previously committed events. The sequence gap detection (lines 179-186) adds a second safety net -- if somehow a valid-looking but out-of-sequence event appears, it's caught.

**One concern**: `append_event` reads the entire file (`read_last_seq` at line 44) to determine the next sequence number, then opens for append. If two processes call `append_event` concurrently, they could both read the same `last_seq` and write duplicate sequence numbers. This isn't a signal handling issue per se, but the advancement loop should ensure single-writer semantics. The current codebase has no file locking.

### 2. Gate Evaluator Process Group Isolation (`src/gate.rs`, lines 60-114)

Each gate command is spawned via `sh -c` with a `pre_exec` hook that calls `setpgid(0, 0)` (line 76), placing the child in its own process group. On timeout, the evaluator sends `SIGKILL` to the entire process group via `killpg` (line 104), then reaps the child with `wait()` (line 107).

**Cleanup on unexpected parent termination**: If the parent koto process receives SIGTERM/SIGKILL while a gate subprocess is running, the child process group is NOT automatically cleaned up. The `setpgid(0, 0)` call specifically isolates the child from the parent's process group, which means:
- SIGTERM sent to the parent's process group won't reach the child
- SIGKILL to the parent won't kill the child either
- The child becomes an orphan, reparented to init/systemd

This is intentional for the timeout case (prevents killing the parent when we kill the child group), but it means **orphaned gate processes are possible on unexpected termination**. The gate has its own timeout (default 30s, line 17), so orphans will eventually be reaped by the OS or timeout naturally, but there's no explicit cleanup path in the parent's signal handler.

### 3. SIGTERM During Gate Evaluation

The gate evaluator (`evaluate_single_gate`) blocks on `child.wait_timeout(timeout)` (line 89). If SIGTERM arrives at the parent while blocked here:

- **Without a signal handler**: Default behavior kills the process immediately. The child process group survives (isolated by `setpgid`). The partially-written event log is safe because `append_event` hasn't been called yet (gates are evaluated before event append in the current `handle_next` flow).

- **With a signal handler setting AtomicBool**: The `wait_timeout` call will be interrupted by `EINTR` on some platforms. The `wait-timeout` crate (used at line 13, `wait_timeout::ChildExt`) handles this -- looking at its implementation, it retries on EINTR. So the signal handler sets the flag, but the gate evaluation continues until it finishes or times out. The advancement loop checks the flag **between iterations** (per the design doc), not during gate evaluation.

**Implication**: A gate evaluation could take up to `timeout` seconds (default 30s) after SIGTERM before the loop checks the shutdown flag. If fast shutdown is required, the signal handler would need to also kill the child process group directly. But the design doc says "complete the in-progress atomic append before exiting" (line 540-541), which suggests letting the current iteration finish is the intended behavior.

### 4. Signal Handling Crates

**`signal-hook` (crate)**: The standard choice for async-signal-safe signal handling in Rust. Provides `signal_hook::flag::register()` which atomically sets an `AtomicBool` on signal receipt -- exactly what the design calls for. It handles the tricky parts: only calls async-signal-safe functions from the handler, works with `AtomicBool` directly. Version 0.3.x is current and well-maintained.

**`ctrlc` (crate)**: Simpler API, handles SIGINT/SIGTERM specifically. Calls a closure on signal. Less flexible than `signal-hook` but covers the basic case.

**`tokio::signal`**: Only relevant if using tokio runtime. koto is currently synchronous (no tokio dependency in `Cargo.toml`), so this doesn't apply.

**`nix` (crate)**: Lower-level POSIX bindings. Already using `libc` directly for `setpgid`/`killpg`. `nix` would be a higher-level wrapper but adds a dependency for what's currently 3 lines of unsafe code.

**Recommendation**: `signal-hook` is the right choice. The `signal_hook::flag::register(SIGTERM, Arc::clone(&flag))` pattern maps directly to the design's `AtomicBool` approach. It's battle-tested, has zero unsafe in the flag-setting path, and doesn't pull in an async runtime. The `ctrlc` crate would also work but doesn't support registering a plain `AtomicBool` as cleanly.

### 5. Return Value on Signal-Based Shutdown

The design doc (DESIGN-auto-advancement-engine.md, lines 102-103) says: "Signal handling checks an AtomicBool between iterations; the last fsync'd event is always durable before the check."

This means the engine should return the **last valid state** -- the state corresponding to the last successfully fsync'd event. The `StopReason` enum should include a variant like `SignalReceived { state: String }` so the handler can distinguish between normal stops and signal-induced stops. The CLI can then serialize an appropriate response (probably the same `NextResponse` shape with the current state's directive, since the agent needs to know where things stopped).

The upstream design (DESIGN-unified-koto-next.md, line 541) says: "the chain is not unwound -- only the in-progress transition rolls back (PRD R21)." So: return the last committed state, don't try to undo anything, let the next `koto next` call resume from that point.

### 6. `koto cancel` vs. Signal-Based Shutdown

The design doc (line 106) says: "`koto cancel` is a new subcommand that appends a `workflow_cancelled` event."

Key differences:

| Aspect | Signal shutdown | `koto cancel` |
|--------|----------------|---------------|
| Intent | Process termination, resume later | Workflow abandonment |
| Event log | No new event; last fsync'd state is final | Appends `workflow_cancelled` event |
| Resumability | Yes, next `koto next` picks up where it stopped | No, workflow is done |
| EventPayload | None needed (current types.rs has no cancelled variant) | Needs new `WorkflowCancelled` variant |

`koto cancel` requires:
1. A new `EventPayload::WorkflowCancelled` variant in `src/engine/types.rs`
2. A new `Command::Cancel` variant in `src/cli/mod.rs`
3. `dispatch_next` / the advancement engine recognizing cancelled state as terminal

Signal shutdown requires no schema changes -- it's purely a runtime concern.

### 7. Existing Dependencies (`Cargo.toml`)

No signal-related crates are present. Current dependencies:
- `libc = "0.2"` (unix-only) -- used for `setpgid`/`killpg` in gate evaluator
- `wait-timeout = "0.2"` (unix-only) -- used for gate timeout

`signal-hook` would be a new dependency. It has minimal transitive dependencies (`signal-hook-registry` and `libc`, the latter already present).

## Implications for Design

1. **The append-then-fsync pattern is sufficient for crash safety.** The existing `read_events` truncated-final-line recovery means the advancement loop doesn't need any additional crash protection beyond what `append_event` already provides. The design's requirement to "check the AtomicBool between iterations, after fsync" is the right boundary.

2. **Gate evaluation timeout creates a shutdown delay.** After SIGTERM, the current gate could run for up to 30 seconds before the loop checks the flag. The design should decide whether this is acceptable or whether the signal handler should also kill active child process groups. Given that the design says "complete the in-progress atomic append," letting the gate finish is consistent.

3. **Orphaned gate processes are a real concern.** If the parent is killed (SIGKILL, OOM, etc.) while a gate subprocess is running, the child survives due to process group isolation. Adding a `PR_SET_PDEATHSIG` via `prctl` in the `pre_exec` hook would cause children to receive a signal when the parent dies, but this has portability caveats (Linux-specific, doesn't work across setuid boundaries). For now, the 30s default timeout is a reasonable upper bound on orphan lifetime.

4. **No file locking exists.** The advancement loop must be the single writer during its execution. If a user runs `koto next` while an advancement loop is active, both could append events with duplicate sequence numbers. Consider adding advisory file locking (`flock`) around `append_event`, or at minimum documenting that concurrent `koto next` calls on the same workflow are unsupported.

5. **`koto cancel` needs a new event type.** The `EventPayload` enum and the `Event` deserialization match block both need updating. The `dispatch_next` classifier needs to treat `workflow_cancelled` states as terminal.

6. **`signal-hook` is the right dependency.** It maps directly to the `AtomicBool` design pattern, has zero async requirements, and its only transitive dependency (`libc`) is already present.

## Surprises

1. **`append_event` re-reads the entire file on every call** (`read_last_seq` at line 44 calls `read_to_string`). For a long-running workflow with hundreds of events, this gets progressively slower. The advancement loop should consider passing the expected next seq as a parameter to avoid the file read, or `append_event` should be refactored to accept an explicit seq. This isn't a signal handling issue, but the advancement loop will call `append_event` potentially many times per invocation.

2. **No concurrent access protection at all.** There's no file locking, no PID file, no advisory lock. Two simultaneous `koto next` calls could interleave appends and produce a corrupted log. The sequence gap detection would catch this on the next read, but the damage is done. The advancement loop makes this worse because it holds the file "open" (logically) for the entire chain, which could be seconds or minutes.

3. **The `wait-timeout` crate handles EINTR internally**, which means signal delivery during gate evaluation won't cause the gate to fail spuriously. This is good -- it means the `AtomicBool` check only needs to happen at the loop level, not inside gate evaluation.

## Summary

The existing `append_event` + `read_events` truncated-line recovery provides sufficient crash safety for the advancement loop -- no additional atomicity work is needed. `signal-hook` is the right crate for setting an `AtomicBool` on SIGTERM/SIGINT, and the flag should be checked between loop iterations after each fsync, consistent with the design doc. The main risks are: (1) gate evaluation can delay shutdown by up to 30 seconds, (2) orphaned gate subprocesses survive parent termination due to process group isolation, and (3) there is no file-level locking to prevent concurrent `koto next` calls from interleaving writes -- the advancement loop's longer execution window makes this gap more urgent.
