# /scope Handoff: session-view

## Provenance

Written by `/explore` on 2026-10-10 from
`wip/explore_session-view-surfaces_crystallize.md`. Research files:
`wip/explore_session-view-surfaces_findings.md`,
`wip/explore_session-view-surfaces_decisions.md`, and
`wip/research/explore_session-view-surfaces_r1_lead-*.md`. The exploration ran
one discover-converge round over four leads (the workflow-view render path, the
dashboard, the context store, and large-value handling precedents); the
narrowing was the maintainers' standing constraint that the view rides the two
existing surfaces only.

## Problem Statement

A person cannot read what a koto session holds. `koto context list` prints key
names only, `koto context get` dumps one key's raw bytes, and `status` and the
dashboard show the state machine's position and directive but no content. An
operator reading a long-running session's working state — every context key
with its content and size, the state and directive, the execution anchor, the
store origin — has no surface that answers it, and large or binary values have
no legible form anywhere.

## Scope Boundary

### In scope

- A per-session content view on the local dashboard: the TUI's existing detail
  pane and a single-session detail mode on the existing `--once` invocation.
- A compact additive enrichment of the workflow-view projection
  (`src/workflows_surface/`): key names with sizes, counts, anchor, store kind.
- The data layer those need: a library-callable session snapshot (state and
  interpolated directive, today trapped in `handle_status`), per-key metadata
  from the manifest, bounded excerpts of content with text/binary
  classification and size formatting.
- Correct behavior on local, cloud-backed and migrated sessions (a migrated
  session names its target instead of failing or showing empty keys).

### Out of scope

- Any new top-level verb, new subcommand tree, or third rendering surface (a
  web page, a report file) — the maintainers' ruling of 2026-10-10.
- Session hygiene: listing by execution directory, prune, the terminal-state
  signal (koto#308, koto#162, koto#234), cleanup of `migrated.json` markers,
  run-journal retention. If the view meets a migrated marker it shows it; it
  clears nothing.
- Changes to the `--once` feed's existing 8-column contract (the first six
  positions are load-bearing for scripts).
- Key content in the workflow-view file: its contract is a status tree with
  240-character previews, and it is written by default under the hosting
  Claude Code session's directory, where session content does not belong.

## Decisions Already Settled

From `wip/explore_session-view-surfaces_decisions.md` (Round 1):

- Surface split: the dashboard carries the full content view; the workflow
  view carries at most a compact additive summary (names, sizes, counts,
  anchor, store kind), never content.
- "Remote" is read as the session's store origin (`origin.store` kind and
  base, cloud sync presence), not a git remote — nothing in the header records
  a git remote.
- The `--once` 8-column contract stays untouched; detail output goes behind a
  new flag on the existing invocation.
- The view's reads must not silently mutate: avoid the CLI `get` path's
  `context_read` logging for bulk rendering; remote-only (unpulled) keys are a
  presentation state; the exact pull policy is the design's call.
- A migrated session is detected up front via `check_not_migrated` and
  presented by naming its target and workspace.
- Bounded display reuses the repo's cut-and-flag precedents
  (`redact::safe_cut_len`, `*_truncated` flags, lossy decode, the 240-char
  preview); the excerpt bound, binary criterion and size-unit style are the
  design's to set.

## Coverage Notes

- What Claude Code's `/workflows` screen renders beyond
  name/status/phases/previews is empirically unverified; the chain should
  treat unknown fields as invisible and keep any contract change additive.
- The exact excerpt bound, the binary-detection rule (invalid UTF-8 vs NUL),
  and the size-unit style are undecided anywhere in the repo; the PRD/design
  must set them.
- Whether the dashboard detail loads content eagerly or on demand under the
  500ms poll, and how the `ContextStore` capability reaches the dashboard
  (today it receives `&dyn SessionBackend` and reads state files by path), are
  open architecture questions.
- The `--once` sanitizer (`sanitize_field`) strips only tab/newline/CR;
  excerpts need control-character handling the repo does not have yet.
- Two doc/code contradictions were found in passing: `koto workflows publish`
  is documented as writing no event but appends a best-effort `context_added`;
  the dashboard guide's "filters to the named session only" matches `--once`
  but not the TUI, where the name argument is ignored.

## Upstream Observations

The exploration read koto's current designs for the adjacent shipped work:
`DESIGN-session-legibility.md` (dashboard liveness/labels — adjacent, not this
feature), `DESIGN-native-workflows-render.md` and
`DESIGN-native-workflows-phase-detail.md` (the workflow-view contract and its
versioning), `DESIGN-session-migration.md` (the `session_migrated` refusal the
view must present), and `docs/reference/session-feed.md` (the `--once` column
contract). No upstream document is passed on the command; these are
observations the chain should re-read in place.

## Framing-Shift Answer

**Pre-supplied answer:** no signal surfaced.
**Evidence:** the exploration confirmed the problem as framed — no surface
shows session content — and narrowed only the solution space (which surfaces
may carry it), per the maintainers' ruling of 2026-10-10. The problem shape,
audience and success criterion (a person reads a session's state through the
view and finds it enough) did not move.

## Shape Signals

### Architectural alternatives left open

- Dashboard detail: a fourth `DashboardTab` rendering a content view vs
  extending `DetailData`/`read_detail` with context data shown in the existing
  Summary tab. Tab: clearer separation, one more keybinding; extension: less
  new UI, risks crowding the summary.
- `--once` single-session detail: a new flag (e.g. a detail mode keyed on the
  existing positional name) emitting a non-columnar document vs JSON output.
  Text: human-first; JSON: machine-consumable, matches koto's other CLI
  output contracts.
- Content loading: eager read of all keys at detail-open vs on-demand per key
  with a size cap, under the dashboard's 500ms refresh and the cloud `get`'s
  pull-and-log side effects.
- Workflow-view enrichment: where key names/sizes/anchor ride — the `koto`
  extension block (invisible to today's screen but additive and safe) vs
  encoding a one-line summary into existing `detail`/preview strings (visible
  today, cramped).
- Plumbing: widen `dashboard::run`'s parameter from `&dyn SessionBackend` to
  the concrete `Backend` (which already implements `ContextStore`) vs reading
  `ctx/manifest.json` directly by path (no signature change, bypasses the
  backend's migration/cloud awareness).

### Complexity signals

- The work spans three layers (data helpers, dashboard, workflows surface)
  with contested trade-offs in each — more than one PR's worth, but one
  coherent feature.
- The cloud read path has side effects (a `get` pulls and writes locally and
  appends events), so "show content" is not a pure read decision; a wrong
  default quietly mutates sessions being viewed.
- Terminal safety for arbitrary bytes (control characters in `--once` output)
  has no existing guard; getting it wrong is a correctness and injection
  concern, not polish.
