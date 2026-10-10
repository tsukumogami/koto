# Exploration Findings: session-view-surfaces

## Core Question

koto is about to gain a human-readable view of a session: the state and
directive, the execution anchor and remote, and every context key with its
content and size, with large or binary values shown legibly. The view must ride
koto's two existing visualization surfaces — the Claude Code workflow view's
session rendering and the local dashboard (TUI and its `--once` CLI mode) — and
no new surface may be added. What can each surface show today, what can each
carry, and which parts of the requirement belong on which surface?

## Round 1

### Key Insights

- **The workflow view is a status tree, not a document view**
  (lead-workflows-render). `src/workflows_surface/` writes
  `koto-<uuid>.json` (contract v2) per commit, best-effort, into the hosting
  Claude Code session's `/workflows` directory, on by default
  (`workflows.native`). It carries name, status, phases, and directive/outcome
  previews hard-capped at 240 one-line characters, plus a `koto` extension
  block. It has no slot for per-key content; Claude Code's reader is
  undocumented and defaults every field, so additions must be additive
  (contract bump). Context keys, sizes, anchor and remote are all derivable
  cheaply (log events carry key/hash/size/writer) but none are projected today.
- **The dashboard already has a per-session detail pane** (lead-dashboard):
  Summary / History / Remaining tabs, loaded on demand with an mtime guard
  (`DetailData` / `read_detail`). A content view fits as a new detail tab; the
  `--once` feed's 8-column contract is fixed, so single-session detail output
  needs a new flag on the existing command rather than new columns. The
  dashboard never reads context keys today and is handed `&dyn SessionBackend`
  (not `ContextStore`), so plumbing must widen.
- **The data layer mostly exists** (lead-context-store). The manifest's
  `KeyMeta` gives size, hash, created_at and writer per key without reading
  content, cheap even on cloud (one manifest GET, 5s cache). A migrated
  session is detectable up front via `Backend::check_not_migrated`, which
  names the target and workspace. Missing pieces: the directive derivation is
  trapped inside `handle_status` (which exits the process on errors), there is
  no text/binary classifier, and a cloud `get` is not a pure read (it pulls
  into the local store and appends `context_added`; the CLI `get` also logs
  `context_read`).
- **Bounded-display precedents exist and are consistent**
  (lead-large-values): 64 KiB capture caps with `*_truncated` flags,
  character-boundary cuts (`redact::safe_cut_len`), lossy UTF-8 decode
  (`decode_capture`), the 240-char one-line preview, and the "↓ N more"
  pattern. Missing: a human size formatter, binary/NUL detection, and a
  control-character sanitizer for the `--once` path (today's `sanitize_field`
  strips only tab/newline/CR). Binary test fixtures already exist
  (`tests/support/migration_carrier.rs` `carrier_keys()`).

### Tensions

- **Content under `~/.claude/projects/`**: the workflow render is on by
  default, so projecting key content there would write session content to
  every hosting Claude session's directory by default. Names, counts and sizes
  are safe; content is a deliberate call.
- **Cloud reads vs purity**: showing a remote-only key's content requires a
  pull that writes locally and logs events; showing metadata only keeps the
  view pure but shows "remote, not pulled" instead of content.
- **"Remote" is not a stored field**: no git remote exists anywhere in the
  header; the session's `origin.store` (kind local|cloud, base) and the cloud
  sync presence are what koto can truthfully show.
- **Migration awareness is path-dependent**: the backend read paths refuse
  with `session_migrated`, but the dashboard and workflows surface read state
  files by path and would not see it; `ctx_exists`/`meta` hide it as
  `false`/`None`. The view must check explicitly.
- **Doc/code contradiction found**: `koto workflows publish` is documented as
  writing no event but appends a best-effort `context_added`
  (`discover.rs::publish_location`).

### Gaps

- What Claude Code's `/workflows` screen renders beyond
  name/status/phases/previews is empirical and unverified; treat unknown
  fields as invisible and keep any addition additive.
- The exact excerpt bound, binary criterion and size-unit style are decided
  nowhere in the repo; they are the design's to set.

### Decisions

See `wip/explore_session-view-surfaces_decisions.md` (Round 1).

### User Focus

Auto mode; the standing constraints are the maintainers' recorded rulings:
two existing surfaces only, no new top-level verb, no new subcommand tree, no
third rendering surface; new flags on existing invocations are permitted. The
split this round grounds: the dashboard carries the full content view; the
workflow view carries at most a compact additive summary.

## Accumulated Understanding

The feature decomposes into three layers, two of which largely exist.

**Data layer (mostly exists):** header fields (workflow, session_id,
created_at, intent, lineage, `execution_dir`, `origin{anchor, store}`),
derived current state (`derive_machine_state`), per-key metadata from the
manifest (`KeyMeta`: size, hash, created_at, writer) and per-key content
(`ContextStore::get`). New work: extract a library-callable session-snapshot
(state + interpolated directive) out of `handle_status`; a bounded excerpt
helper (UTF-8/binary classification, character-boundary cut, control-character
escaping, size formatting); an up-front `check_not_migrated` so a migrated
session is named, not mangled; a deliberate choice about cloud pulls
(metadata-only for unpulled remote keys vs side-effectful content pulls).

**Dashboard surface (the full view):** a new detail tab (fourth
`DashboardTab`) rendering the session's header facts, anchor, store origin,
state, directive, and every context key with size and bounded content; and a
single-session detail mode on the existing `--once` invocation behind a new
flag, leaving the 8-column feed contract untouched. The dashboard needs the
`ContextStore` capability threaded through (today it gets
`&dyn SessionBackend` and reads state files by path).

**Workflow view surface (the compact summary):** an additive contract bump
projecting what fits a status tree — key names with sizes (and counts/total),
anchor, store kind — never content, both for shape and because the render is
default-on under `~/.claude/projects/`. The 240-char preview cap is the
surface's own precedent for any string it carries.

Out of scope confirmed: session hygiene (prune, listing by directory,
terminal signal), anything outside koto. The carrier-test fixtures give the
view a ready-made real session with several keys including a binary one.

## Decision: Crystallize
