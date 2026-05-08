# Decision 4: Module Organization

## Chosen: Option A (with sub-files)
New files under `src/cli/`, following the existing CLI module pattern, but split across three sibling files to separate the dashboard's distinct concerns.

## Rationale

The dashboard is a CLI command. Its natural home is `src/cli/`, which is exactly where every other command handler lives — `next.rs`, `session.rs`, `batch.rs`, `batch_view.rs`. Placing it there keeps the convention intact and signals to contributors that the dashboard is a command, not a domain concept. The alternative of a top-level `src/dashboard/` module implies peer status with `engine/`, `template/`, and `session/` — modules that own durable domain logic, not command implementations. That framing would misrepresent what the dashboard is.

Option A as stated allows all dashboard code to sit in a single `dashboard.rs` file or be split into sibling files. Given the three-concern separation required by the constraints (data reading, application state, rendering), a single-file approach would produce a 700–1000 line file that mixes TUI event loop logic with session file parsing. Instead, the right structure is three sibling files following the `batch.rs` / `batch_view.rs` precedent: `batch.rs` holds core scheduler logic and `batch_view.rs` holds the derived read model, both under `src/cli/`. The dashboard split is analogous. This gives each concern its own file while keeping them adjacent to the command dispatcher in `mod.rs`.

Testing follows naturally from the split. The data layer (`dashboard_data.rs`) reads session headers from a `SessionBackend` trait reference — unit-testable without a PTY. The state layer (`dashboard_state.rs`) operates on plain Rust structs — pure unit tests. Only the render layer (`dashboard_render.rs`) touches ratatui and requires a buffer/backend stub for testing, which ratatui's `TestBackend` supports. A top-level module (Option B) would offer no better testability: the same file-level isolation is achievable within `src/cli/` by simply using multiple files.

## Rejected Options

### Option B: `src/dashboard/` top-level module
A top-level module is appropriate when the code represents a domain concern used by multiple consumers — as `session/`, `engine/`, and `template/` do. The dashboard is consumed only by the CLI command dispatcher. Elevating it to a top-level module would suggest it exposes a public API to other parts of the library, which it doesn't. It also breaks the established pattern where CLI commands live in `src/cli/`, creating an exception that contributors would need to learn. The sub-module structure (mod.rs + data.rs + state.rs + render.rs) is achievable equally well as sibling files under `src/cli/`.

### Option C: Inline in `src/cli/mod.rs`
`src/cli/mod.rs` is already 4000 lines. Adding a full TUI event loop, application state, and ratatui rendering inline would push it past 5000 lines and mix unrelated concerns — command dispatch, JSON output types, and terminal UI — in a single file. The existing codebase already avoids this pattern: `batch.rs` (4484 lines), `next.rs` (814 lines), and `batch_view.rs` (688 lines) all live in separate files rather than in `mod.rs`. Option C contradicts the codebase's own precedent and would make the dashboard code hard to locate and modify.

## Recommended File Structure

```
src/cli/
├── mod.rs               # Add Dashboard variant to Command enum; dispatch to dashboard::run()
├── dashboard_data.rs    # Session reading: scan session backend, derive hierarchy, aggregate counts
├── dashboard_state.rs   # Application state: expand/collapse tree, cursor position, selection
├── dashboard_render.rs  # ratatui widgets: layout, row rendering, footer detail panel
├── batch.rs             # (existing)
├── batch_view.rs        # (existing)
├── next.rs              # (existing)
├── session.rs           # (existing)
└── ...                  # (other existing modules unchanged)
```

Module declarations added to `src/cli/mod.rs`:
```rust
pub mod dashboard_data;
pub mod dashboard_render;
pub mod dashboard_state;
```

The `Dashboard` variant in `Command` dispatches to a `run()` function in `dashboard_data.rs` (or a thin `dashboard.rs` entry point if the three files don't warrant a fourth). Tests live as inline `#[cfg(test)]` modules at the bottom of each file, consistent with how `batch.rs`, `next.rs`, and `batch_view.rs` handle unit tests.

## Assumptions

- Dashboard TUI code will total 500–1000 lines across the three concerns; this is large enough to warrant splitting but not so large that a dedicated top-level module is needed.
- The dashboard does not expose any public API used by non-CLI code. If a future feature (e.g., a watch mode, a web exporter) needs to consume session hierarchy data, `dashboard_data.rs` can be promoted to a top-level module at that point without restructuring the rest.
- ratatui's `TestBackend` is sufficient for render-layer unit tests; no PTY is required.
- The `Dashboard` command variant will be added to the existing `Command` enum in `mod.rs`, following the same pattern as `Status`, `Workflows`, and other leaf commands.
