# Decision 1: Event Loop Architecture

## Chosen: Option A
Single-threaded tick loop using `crossterm::event::poll(Duration::from_millis(50))`, with keyboard events checked on each tick and file polling performed every 10th tick (~500ms).

## Rationale

Option A is the canonical ratatui event loop pattern because `crossterm::event::poll` does exactly what a TUI tick loop needs: it surrenders the CPU to the OS scheduler for up to 50ms and returns early the moment a keyboard event arrives. When no user is typing, the thread stays parked at the OS level and consumes no CPU. When the user presses a key, the loop wakes immediately and processes it — giving ≤50ms input latency with no busy-waiting. This matches R18 (events visible within 2× poll interval) and the startup performance requirements without any special treatment.

File polling on every Nth tick is a natural fit. A 50ms tick with N=10 gives a 500ms poll cycle, matching the PRD's default interval exactly. The tick counter is a single integer in the loop state; there is no cross-thread coordination or synchronization to reason about. The `AtomicBool` shutdown flag from `signal-hook` is checked after each `poll()` returns, so Ctrl+C is handled at most 50ms after the signal fires — well within any perceptible delay. This integrates cleanly with the existing shutdown pattern in `src/engine/advance.rs`.

Single-threaded design also means the session state model (whichever option is chosen in Decision 2) is straightforward to mutate: no `Arc<Mutex<_>>` wrappers, no channel receivers to drain on shutdown, and no risk of a background thread holding a read lock while the main thread tries to exit. The entire loop — event handling, file polling, rendering — is sequentially ordered, which makes it easy to test and reason about. Windows support is also straightforward: `crossterm::event::poll` is implemented for Windows via its console input backend, so no platform-specific branching is needed.

## Rejected Options

### Option B: Two threads with mpsc channel
The fundamental problem with Option B is shutdown. The main thread blocks indefinitely in `crossterm::event::read()`, which has no timeout. When the user presses Ctrl+C, `signal-hook` sets the `AtomicBool` — but the main thread may never check it because it's stuck waiting for a keyboard event that never arrives. Unblocking a thread from a blocking `read()` call requires either: (a) posting a synthetic crossterm event from the background thread, which means the background thread needs its own crossterm handle and careful platform-specific sequencing; (b) adding a timeout to `read()`, which reintroduces a poll loop and negates the claimed benefit of blocking-read simplicity; or (c) using `unsafe` platform primitives to interrupt the thread. None of these are simpler than Option A. The two-thread approach also forces the state model to be shared across threads — at minimum, a channel receiver and Arc-wrapped state — adding coordination surface that the rest of the synchronous koto codebase deliberately avoids.

### Option C: Non-blocking try_read() with explicit sleep
Option C is structurally identical to Option A but with worse CPU characteristics and more verbose code. `crossterm::event::poll(Duration::ZERO)` followed by `crossterm::event::read()` is roughly equivalent to a hypothetical `try_read()`, but it requires two API calls instead of one. More importantly, a raw `std::thread::sleep(50ms)` wakes the thread unconditionally every 50ms regardless of whether any events are pending, whereas `poll(50ms)` lets the OS wake the thread early when events arrive. The result is the same input latency upper bound (50ms), but Option C burns one unnecessary wakeup per tick when the user is idle. For a dashboard that runs for hours while the user watches a long workflow, this is a needlessly less efficient idle pattern. There is no scenario where Option C is preferable to Option A.

## Implementation Notes

The tick loop structure should look like:

```rust
let tick_rate = Duration::from_millis(50);
let poll_every_n_ticks: u32 = (poll_interval_ms / 50).max(1);
let mut tick_count: u32 = 0;
let shutdown = Arc::clone(&shutdown_flag); // existing AtomicBool from signal-hook

loop {
    if crossterm::event::poll(tick_rate)? {
        match crossterm::event::read()? {
            Event::Key(key) => handle_key(key, &mut state),
            Event::Resize(w, h) => handle_resize(w, h, &mut state),
            _ => {}
        }
    }

    tick_count = tick_count.wrapping_add(1);
    if tick_count % poll_every_n_ticks == 0 {
        poll_session_files(&mut state)?;
    }

    terminal.draw(|f| render(f, &state))?;

    if shutdown.load(Ordering::Relaxed) || state.should_quit {
        break;
    }
}
```

Key implementation details:

- **Poll interval as a CLI flag**: The `--interval <ms>` flag from R16/R17 maps to `poll_every_n_ticks = interval_ms / tick_rate_ms`. The tick rate (50ms) is fixed; only the file poll cadence changes. Changing the tick rate to match the poll interval directly would hurt input responsiveness.

- **Tick rate vs. poll interval**: Keep the tick rate fixed at 50ms. The tick rate controls input latency; the poll interval controls file I/O cadence. They are independent. Do not couple them.

- **Render on every tick only when dirty**: To avoid unnecessary terminal writes, track a `dirty` flag. Set it when keyboard input is processed or session state changes. Skip the `terminal.draw()` call when `!dirty`. This reduces terminal I/O during idle periods without changing the tick rate.

- **Signal handling**: The existing `signal-hook` + `AtomicBool` pattern registers SIGINT and SIGTERM. Check the flag at the bottom of the loop (after the draw call) rather than inside the poll block. This ensures a clean terminal state is restored before the loop exits regardless of when the signal fires.

- **crossterm raw mode and alternate screen**: Wrap the loop in the standard ratatui setup: `enable_raw_mode()`, `execute!(stdout, EnterAlternateScreen)`, then the loop, then a `finally`-style cleanup (best done with a RAII guard or explicit cleanup block that runs even on `?`-propagated errors). This prevents broken terminal state if `poll_session_files` returns an error.

- **Windows compatibility**: `crossterm::event::poll` works on Windows without changes. The file polling side-path uses `std::fs` which is also cross-platform. No conditional compilation is needed.

- **File I/O budget**: The PRD notes that 1000-child discovery may exceed 500ms. If `poll_session_files` takes longer than the tick interval, subsequent ticks will simply be delayed rather than dropped. Consider capping discovery to a fixed time budget (e.g., 400ms) and resuming from where it left off on the next poll tick if that becomes an issue.

## Assumptions

- `crossterm::event::poll` is available and behaves as documented on Linux, macOS, and Windows. This is a stable API in crossterm 0.27+.
- The project adds `crossterm` and `ratatui` as dependencies (neither is currently in `Cargo.toml`). Both are pure Rust with no system library requirements.
- A 50ms tick rate (20 Hz rendering) is sufficient for the dashboard use case. Session data changes at most every 500ms; keyboard latency at 50ms is imperceptible for navigation keys.
- The `signal-hook` `AtomicBool` pattern from the advance loop can be reused directly or a new registration added for the dashboard subcommand's lifetime.
- File polling is synchronous and performed in the main thread. If future profiling shows that file I/O blocks the tick loop noticeably, the polling step can be moved to a background thread at that point — Option B's threading model can be adopted incrementally without changing the event loop contract.
