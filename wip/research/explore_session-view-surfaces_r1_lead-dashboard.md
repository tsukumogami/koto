# Lead: What does the dashboard show today, through which modules, and where would a per-session content view fit?

## Findings

The dashboard is a purely local, read-only view over the event log, and it
never touches context keys. `koto dashboard [name] [--once --interval --status
--needs-you --all]` runs on `dashboard_data.rs`, `dashboard_state.rs` and
`dashboard_render.rs` (all under `src/cli/`). A per-session detail pane already
exists, but it only has Summary, History and Remaining tabs. It shows state,
directive (from the compiled template), gate, intent, template, evidence capped
at 3, and history. It shows no header fields beyond template and intent, and no
anchor (`header.origin.anchor` / `execution_dir` exist but are unused). It
shows no context keys, and no remote: there is no "remote" field in
`src/engine/types.rs`.

### Where a per-session view would fit

- **TUI:** add a fourth tab to `DashboardTab` (`dashboard_state.rs:19`, cycled
  by Tab at ~303 and drawn in `render_detail` in `dashboard_render.rs`).
  Alternatively extend `DetailData` / `read_detail` (`dashboard_data.rs:273`
  and 655), which is already loaded on demand for the focused session with an
  mtime guard.
- **`--once`:** the column contract (`docs/reference/session-feed.md`) is
  fixed at 8 tab-separated columns, with the first six positionally stable.
  Full content does not fit that shape, so it needs a new flag on the existing
  command instead, such as a single-session mode keyed on the `name` argument.
  `format_once_line` and `run_once` in `dashboard.rs` are where it would go.
- **Context reading:** the dashboard needs a `ContextStore` handle. `Backend`
  implements both `SessionBackend` and `ContextStore` (`src/session/mod.rs:728`),
  and `dashboard::run` is called with the concrete backend (`mod.rs:1894`) but
  takes `&dyn SessionBackend`. The signature would need widening, or the code
  could read `ctx/manifest.json` directly. `KeyMeta` already carries size,
  hash, writer and created_at (`src/session/context.rs:7`), and
  `ContextStore::meta`, `list_keys` and `get` exist.
- **Claude Code workflow view:** `src/workflows_surface/` renders a session as
  a `koto-<uuid>.json` file. `derive_enriched_projection` in `project.rs:168`
  supplies phases, directives and outcomes. `WorkflowFile` has a `koto`
  extension block in `contract.rs`, which is the likely place for anchor and
  context-key content.

### What a row shows and how it is read

- **Row:** a name, built by `derive_label`, with a `parent ▸ leaf` prefix for
  children and a batch/leg badge. Then state, a liveness glyph plus idle time,
  and a Children count. Rows are attention-sorted; the receded set (done and
  abandoned) is hidden behind `a` or `--all`.
- **Data read:** `read_session` reads the whole event log once per mtime
  change and keeps a `CachedSession` (`dashboard_data.rs:170`), with no cost
  model beyond that. Its fields are the header, current state, terminal and
  blocked flags, intent, `last_event_at` and one salient variable. It also
  checks for a leg-pointer sidecar in the session directory.
- **Liveness:** thresholds are active 5m, stalled 2h, abandoned 7d; precedence
  as documented in `DESIGN-session-legibility.md`.

### Children and trees

- The TUI shows parent/child trees: roots by parent workflow, and `l` or the
  right arrow expands a row.
- `--once` is flat, and the design deferred folding children under their parent.

### Backend

- Works against both local and cloud backends, but only through the local
  cache.
- `Backend::Cloud` `list()` also calls S3 and adds remote-only sessions as
  placeholders (`cloud.rs:868`). The local read of those fails, so they show
  up as unreadable rows.
- `session_dir` delegates to the local directory, so nothing is read from S3
  on the render path.

## Implications

A per-session content view fits the existing detail pane (a new tab or an
extension of `DetailData`), and a single-session mode on `--once` needs a new
flag rather than new columns, keeping the 8-column contract stable. The data
layer (`ContextStore::list_keys`/`meta`/`get`) already answers sizes and
content; the plumbing gap is that the dashboard receives `&dyn SessionBackend`
and would need the `ContextStore` capability widened through, or a direct
manifest read.

## Surprises

- `render_frame` never reads `view_mode`. The detail pane is just the right
  60% whenever the terminal is at least 80 columns wide. Below 80 columns
  Enter and the detail view do nothing visible.
- `DashboardArgs.name` is ignored in TUI mode. In `--once` it is an exact id
  match, so children are excluded. The guide's text ("filters to the named
  session only") matches `--once` but not the TUI.
- The README says the dashboard shows "sessions for the current repo", but the
  design made the dashboard global (`~/.koto/sessions`).
- The Summary tab shows only the first 3 evidence entries
  (`EVIDENCE_DISPLAY_CAP`) and has no truncation or binary handling for values.

## Open Questions

1. What does "remote" mean for a session? It is not a header field. It could
   be the cloud sync presence that `parent_remote_presence_label` in
   `src/cli/session.rs` already computes.
2. Should single-session `--once` output be a new flag such as `--show` or
   `--detail`, or an extension of the positional `name`? The strict 8-column
   count has to stay stable for existing scripts.
3. Should the TUI detail tab load context content eagerly, or only on demand
   with a size cap, given the 500ms poll?
4. Should the <80-column behaviour be fixed as part of this work?

## Summary

The dashboard already has an on-demand per-session detail pane (Summary,
History, Remaining tabs) but reads no context keys, no anchor and no remote;
the data it needs exists behind `ContextStore`, which the dashboard isn't
handed today. A content view fits as a new detail tab plus a single-session
flag on `--once`, keeping the 8-column feed contract intact. The biggest open
question is what "remote" should mean on this surface and whether content loads
eagerly or on demand under the 500ms poll.
