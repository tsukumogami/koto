---
schema: prd/v1
status: In Progress
problem: |
  A person responsible for a koto session — an operator checking a
  long-running run, a teammate taking one over, a maintainer debugging a stuck
  one — cannot read what the session holds. Context keys carry the working
  state, but the surfaces koto ships show names, raw bytes, or state and
  directive only, and a migrated session reads like a broken one.
goals: |
  A person reads one session's working state in the surfaces koto already
  ships: full depth in the dashboard (state, directive, anchor, store origin,
  every key with size and legible content), a compact account in the workflow
  view, and a scriptable one-shot form — with absences, unreadable keys and
  migrations stated plainly, and no raw byte dumps anywhere.
absorbed:
  - docs/briefs/BRIEF-session-view.md
---

# PRD: A human-readable view of a session

## Status

In Progress

Absorbed [BRIEF-session-view](docs/briefs/BRIEF-session-view.md); carried in Absorbed Brief.

## Absorbed Brief

The feature was framed before these requirements existed: a person responsible
for a koto session — an operator checking a long-running run, a teammate
taking one over, a maintainer debugging a stuck one — has no way to read what
the session holds, because context keys carry the working state and the
shipped surfaces show only names, single-key raw bytes, or state and
directive. The framed outcome: that person reads the session like a status
page, in a surface koto already ships, at the depth that fits the surface —
full per-key content in the dashboard, a compact account in the workflow view
— with large and binary values legible, absences stated, and a migrated
session explained by naming its successor. The framing fixed the boundary this
PRD's requirements and Out of Scope operationalize: existing surfaces only, no
new verb or third surface, hygiene work excluded, the view strictly read-only.

## Problem Statement

A koto session accumulates its working state in context keys: notes a
long-running workflow keeps for itself, artifacts one phase leaves for the
next, records a coordinating session maintains about work it oversees. No
surface koto ships lets a person read that state. `koto context list` prints
key names only; `koto context get` prints one key's raw bytes, which is
hazardous for large values and useless for binary ones; `koto status` and the
dashboard show the state machine's position and directive, not what the
session holds; nothing shows a session's execution anchor or store origin
without reading state files by hand.

The people who need the reading — an operator with several runs in flight, a
teammate picking up someone else's session, a maintainer debugging a stuck one
— end up scripting loops over `context get` and hoping nothing wrecks their
terminal. Worse, a teammate pointed at a session that was migrated to another
workspace sees refusals and holes rather than an explanation, so a correct
migration reads like a broken session.

## Goals

- One look answers "what is this session working with?": identity, position,
  location and holdings, in the surface the person is already using.
- Content is always legible: sizes always shown, text excerpted within a
  bound, binary identified rather than dumped, truncation visible.
- Absence is informative: a missing or unreadable key, a field an old session
  never recorded, or a migrated session each produce a plain statement, never
  a silent hole or a stack of errors.
- Existing consumers keep working: the dashboard's multi-session feed and the
  workflow view's file remain compatible for today's readers.

## User Stories

- As an operator with several koto-backed workflows in flight, I want to focus
  one session in the dashboard and read everything it holds — keys, sizes,
  content — so that I can tell what it has accumulated without scripting.
- As a session owner returning to my coding session, I want the workflow view
  to show each koto session's holdings at a glance (how many keys, how large,
  where anchored, which store) so that I can decide whether anything needs a
  closer look.
- As a teammate inspecting a session from a script, I want a one-shot,
  bounded, parseable rendering of one session's facts and keys so that I can
  automate checks without the interactive dashboard.
- As a teammate taking over a migrated session, I want the view to name the
  newer session and its workspace so that I continue there instead of
  debugging an apparent failure.

## Requirements

### Functional

- **R1. Session facts.** The view presents, for one session: its name,
  session id, intent (when recorded), template name, current state, the
  current state's directive, the execution anchor (`execution_dir`), and the
  store origin (kind local or cloud, with the store base). A fact an older
  session never recorded is shown as absent, not omitted or errored.
- **R2. Every key, with metadata.** The view lists every context key the
  session's manifest names, each with its size, its recorded writer (when
  present) and its created-at time. Keys are listed in lexicographic key
  order, the manifest's own order. The listing is complete: no key is
  silently dropped.
- **R3. Legible content.** For each key the view shows content in a bounded,
  terminal-safe form: text values as an excerpt up to a fixed bound with an
  explicit truncation marker when cut (never splitting a UTF-8 character);
  binary values — those whose sampled prefix (a design-set length) is not
  valid UTF-8 or contains a NUL byte — as their type, size and content hash,
  never raw bytes. Control characters never reach the terminal
  unescaped. Sizes are shown in a human-readable unit style fixed by the
  design.
- **R4. Absent and unreadable keys.** A key the manifest names whose content
  cannot be read is shown with an explicit unreadable marker and the reason;
  a key requested but not present is reported as absent. Neither aborts the
  rest of the view.
- **R5. Migrated sessions.** When the session was migrated, the view reports
  the newer session's name and workspace (from the migration marker) in place
  of content, instead of failing or rendering empty sections.
- **R6. Dashboard detail.** The dashboard's per-session detail surface carries
  R1-R5 for the focused session. Context content loads without blocking the
  dashboard's refresh loop, and the detail stays usable for a session holding
  many keys (scroll or equivalent, not truncating the key list).
- **R7. One-shot detail.** The dashboard's non-interactive invocation gains a
  single-session detail mode behind a new flag, emitting R1-R5 as JSON (the
  form koto's other CLI output already uses), with errors and exit codes
  following koto's existing JSON error contract — a missing session and a
  migrated session each produce the documented error object and a non-zero
  exit. The existing multi-session feed's output is
  byte-compatible for existing invocations: same columns, same positions, no
  new columns.
- **R8. Workflow view summary.** The workflow view's per-session file carries
  a compact summary — key names with sizes, key count and total size, the
  execution anchor, the store kind — as an additive change to its contract
  (existing consumers keep parsing). It never carries key content, and it is
  computed from local state only, adding no remote requests to the render
  path.
- **R9. Cloud-backed sessions.** The view works on a cloud-backed session.
  Key metadata reflects the merged local and remote manifests; a key whose
  content is not locally present is shown with its size and an explicit
  not-local state when content is not fetched. Whether the view fetches
  remote content, and when, is the design's call; whichever policy it picks,
  the view's output says when content was not shown and why.
- **R10. Read-only.** The view writes no session content and removes nothing:
  no key writes, no marker cleanup, no state transitions. Incidental
  read-path effects that already exist in koto's backend are not widened by
  the view, and the view adds no event spam proportional to key count.

### Non-functional

- **N1. Bounded output.** No invocation of the view emits unbounded output:
  every content rendering respects the excerpt bound, and the one-shot detail
  mode's output size is bounded by the number of keys times the bound plus
  fixed overhead.
- **N2. Bounded remote cost.** On a cloud-backed session, rendering metadata
  (names, sizes, facts) costs a fixed number of remote requests that does not
  grow with the key count (in today's backend: one manifest fetch and one
  migration-marker check).
- **N3. Compatibility.** Existing dashboard feed consumers and workflow-view
  consumers observe no breaking change: feed columns keep their positions and
  count for existing invocations; the workflow file's existing fields keep
  their names, types and meaning.
- **N4. No new surfaces.** The feature adds no new top-level verb and no new
  subcommand tree; it rides the dashboard and the workflow view.

## Acceptance Criteria

- [ ] AC1. Against a real session holding several keys — including one larger
  than the excerpt bound and one binary — the dashboard detail shows every
  key with size, and content for each: excerpt with a visible truncation
  marker for the long one; type, size and hash (no bytes) for the binary one.
- [ ] AC2. The dashboard detail for that session shows its current state, the
  current directive, the execution anchor, and the store origin; for a
  session predating a field (no intent, no origin), the view prints an
  explicit absent marker for that field.
- [ ] AC3. The one-shot detail mode exists behind a new flag on the existing
  dashboard invocation, emits R1-R5 for one named session in a documented
  machine-readable form, and its output for the fixture session is bounded
  and stable across two consecutive runs with no session activity.
- [ ] AC4. With the new build, running the existing multi-session feed
  invocation (no new flags) against a fixture store produces rows with the
  same column count and positions as the previous release documents; a feed
  consumer script written against the documented columns parses both.
- [ ] AC5. Against a cloud-backed session, the detail view renders metadata
  for every manifest key with a constant number of remote requests, and a
  remote-only key is shown with its size and an explicit not-local state when
  its content was not fetched.
- [ ] AC6. Against a migrated session, the dashboard detail and the one-shot
  detail mode both name the newer session and its workspace, exit without
  error in the one-shot case, and render no empty key list masquerading as
  the session's contents.
- [ ] AC7. The workflow view's file for a session with keys carries key names
  with sizes, the count and total size, the anchor and the store kind;
  the file contains no key content bytes; the shape-guard fixture is updated
  and the existing consumer-known fields are unchanged.
- [ ] AC8. A key made unreadable in a test fixture renders an explicit
  unreadable marker with a reason, and every other key still renders.
- [ ] AC9. `koto --help`'s top-level verb list is unchanged by the feature,
  and no `koto <anything>` subcommand tree was added.
- [ ] AC10. A value containing terminal control sequences (including ESC)
  renders in both the TUI detail and the one-shot mode without the raw
  sequence reaching the output stream unescaped.
- [ ] AC11. Rendering the detail view (TUI and one-shot) against the fixture
  session leaves the session store byte-identical: no key added, removed or
  modified, no migration marker touched, and the session's event log grows by
  at most a fixed number of entries that does not depend on the key count.
- [ ] AC12. Two consecutive renders of the same unchanged session list keys
  in the same, lexicographic order.
- [ ] AC13. The one-shot detail mode invoked for a session name that does not
  exist emits koto's documented JSON error object and exits non-zero, without
  partial detail output.

## Out of Scope

- Session hygiene: listing sessions by execution directory, pruning finished
  sessions, the terminal-cleanup signal (koto#308, koto#162, koto#234), and
  clearing `migrated.json` markers the view encounters.
- Any new top-level verb, subcommand tree, web page or report file (the
  maintainers' surface ruling, 2026-10-10).
- Changing `koto context list`/`get`'s own output: the existing verbs keep
  their contracts; the view is the dashboard's and workflow view's.
- Writing or editing session content; `context add`/`remove` remain the write
  path.
- Key content in the workflow-view file, in any form.
- Changing the dashboard's multi-session feed columns or its attention sort.
- A retention or size cap on context values at write time.

## Decisions and Trade-offs

- **"Remote" means store origin.** The brief left the on-screen vocabulary
  open. Decision: the view labels the session's store origin (`local`, or
  `cloud` with the store base) as "store"; no git remote is recorded in a
  session, so none is displayed. Alternative — deriving a git remote from the
  anchor directory at view time — rejected: it shows a fact koto never
  recorded, which can be wrong for moved checkouts and costs a subprocess per
  render.
- **Binary and excerpt rules are fixed in shape here, in value in the
  design.** Decision: binary means invalid UTF-8 (sampled prefix) or a NUL
  byte; text is excerpted to a single named bound with a visible marker;
  sizes use one human-readable unit style. The exact bound and unit style are
  named constants the design sets, so the requirement stays testable (the
  marker appears exactly when size exceeds the bound) without this document
  pinning numbers the design may tune against the surfaces' widths.
- **Cloud content policy is the design's.** Reading a remote-only key's
  content through koto's backend pulls it into the local store as a side
  effect. Decision deferred to the design with the floor fixed here (R9):
  metadata always renders, and unfetched content is explicitly marked
  not-local. Alternatives the design weighs: metadata-only (pure, less
  informative) vs opt-in or automatic pulls (complete, mutates the local
  store).
- **The feed contract is untouchable.** Alternative — appending detail
  columns to the existing feed — rejected: scripts parse positions, and
  session content does not fit a tab-separated row legibly. The one-shot
  detail is a separate mode behind a new flag instead.

## Known Limitations

- The workflow view updates when the session commits an event and when its
  view is reopened; its summary can lag a session's latest writes. That is
  the surface's existing refresh model, not a regression.
- The excerpt shows the head of a value; a reader needing full content of a
  large key still exports it deliberately (`context get --to-file`), which is
  the existing, intentional path for whole-value access.
- On the local backend nothing records migration, so R5 engages only where a
  migration marker is readable (cloud-backed sessions); a purely local
  session cannot be migrated today.
