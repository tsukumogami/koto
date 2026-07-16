---
schema: prd/v1
status: Done
upstream: docs/briefs/BRIEF-native-workflows-render.md
problem: |
  An operator driving a koto session inside Claude Code cannot see the
  session's state in Claude Code's own `/workflows` screen; they must leave the
  TUI and open koto's separate dashboard. One session, two attention surfaces.
goals: |
  A single koto session driven inside a Claude Code session appears as a native
  `/workflows` entry showing its name and current state, refreshing on reopen
  and settling to done on completion -- with no new skill or surface, opt-in,
  and koto's default path untouched when no host is participating. The slice
  also lands the extensible foundation (file contract, commit-funnel hook,
  context-store publish/discover) that Features 2-5 build on.
---

# PRD: koto sessions render natively in Claude Code's `/workflows` (walking skeleton)

## Status

Done

Requirements for the walking-skeleton slice of koto's native Claude Code
`/workflows` rendering, derived from the Accepted `BRIEF-native-workflows-render`.
The surface decision is settled (koto produces the native artifact; no skill or
reader). Mechanism choices -- the exact hook seam, the file shape, the
context-store key, terminal inference -- are settled downstream in
`DESIGN-native-workflows-render`; this PRD fixes the WHAT and the
acceptance contract.

## Problem Statement

Operators run long-form agentic work inside Claude Code and watch its TUI,
including the `/workflows` screen, which renders multi-agent runs as a tree of
phases and agents. When the work is driven through a koto workflow, that
workflow's state appears nowhere in `/workflows`. To see it, the operator
leaves the surface they are in and opens koto's dashboard -- a second window
and a second attention context for the same session. koto already has the
state; what is missing is legibility where the operator already is. This
matters now because the reference fleet runs koto workflows inside Claude Code
daily, and the per-glance context switch is a standing tax on exactly the
observability koto commits to.

This is the walking skeleton: the thinnest end-to-end slice that proves the
whole path (host publishes a location, koto writes its state there as it
advances, `/workflows` renders it) for one session, before the richer slices
are built on top.

## Goals

- A single, non-hierarchical koto session driven inside a Claude Code session
  is visible as a native `/workflows` entry -- no skill, reader, or second
  window.
- The entry carries a minimal but legible projection: the session's name, its
  current state, and whether it is running or done.
- The entry stays current: it reflects the session's latest committed state on
  the next `/workflows` reopen, and settles to *done* (not a stuck *running*)
  when the session finishes.
- The slice is opt-in and safe: when no host location is published, koto writes
  nothing and its default behavior and cost are unchanged.
- The slice lands the shared foundation the later features extend without
  reopening it: an extensible file contract, a single commit-funnel
  materialization point, and a context-store publish/discover mechanism whose
  key schema already supports Feature 3's ancestor walk.

## User Stories

- As an operator driving a koto session inside Claude Code, I want the session
  to appear in `/workflows`, so that I can see its state without leaving the
  TUI for the dashboard.
- As that operator, I want the entry to show the newer state after the session
  advances (on reopen), so that `/workflows` tells me where the session
  actually is.
- As that operator, I want the entry to read *done* when the session finishes,
  so that a completed run is not misrepresented as perpetually running.
- As an operator running koto outside any participating Claude Code session, I
  want koto to behave exactly as before, so that enabling this feature for some
  sessions never changes the default experience.
- As a koto maintainer, I want the file contract, hook point, and context-store
  key chosen for extension, so that Features 2-5 add to the foundation instead
  of reworking it.

## Requirements

### Functional

- **R1 -- Materialize on every state-commit.** When a participating koto
  session commits a state change through koto's single low-level commit funnel
  (the session backend's event-append, through which `koto next`, directed
  `--to`, `koto rewind`, and error/limit exits all pass), koto writes/refreshes
  that session's `/workflows` artifact. Materialization rides the commit; it is
  not a daemon, watcher, or background process.
- **R2 -- One file per session, UUID-named.** Each session writes exactly its
  own file, named `koto-<session-uuid>.json`, keyed off the session's stable
  init-time UUID so it never collides with Claude Code's own `wf_*.json` files
  in the same directory and is identifiable as koto's.
- **R3 -- Minimal projection.** The file carries at least: the session's
  display name, its current state, and a running/done (and failure-terminal)
  status. The projection is derived from koto's existing per-session read seam,
  re-derived from the log on each commit.
- **R4 -- Extensible file contract.** The file is a minimal *valid* shape that
  later slices add fields to without breaking F1's readers. A koto-namespaced
  block identifies the file as koto's and carries a contract version.
- **R5 -- Publish/discover via the context store.** The target `/workflows`
  directory is made known to koto by publishing it into koto's existing
  per-session context store under a reserved, namespaced key. On commit a
  session resolves its target directory by walking from itself up the
  `parent_workflow` chain and taking the nearest ancestor that has published a
  location (for F1, that resolves to the session's own published location). The
  key schema is chosen so Feature 3's nearest-published-ancestor walk needs no
  change to it.
- **R6 -- Opt-in by published-location presence.** A session materializes
  exactly when a published location is discoverable for it; otherwise it writes
  nothing. Opt-in is not per-session config (children do not inherit config) --
  it is the presence of a published location.
- **R7 -- Create the directory.** koto creates the target `/workflows`
  directory when it is absent (Claude Code does not guarantee it exists), then
  writes the file into it.
- **R8 -- SessionStart publish path.** A Claude Code `SessionStart` hook,
  shipped with koto's plugin, derives the hosting session's `/workflows`
  directory from the hook payload and makes it the published location for koto
  sessions driven in that Claude Code session.
- **R9 -- Terminal renders as done.** A session that reaches a (non-failure)
  terminal state renders as *done*; a failure-terminal renders as *failed*; a
  session writes its terminal state before koto cleans it up, so the finished
  entry persists rather than reverting to a stuck *running*.

### Non-functional

- **R10 -- Default path untouched.** When no location is published for a
  session, materialization performs no write and imposes no behavior change on
  koto's default path; its added cost on the default path is at most a cheap
  presence probe.
- **R11 -- Atomic, non-torn write.** The file is written so a concurrent
  `/workflows` reopen never observes a half-written or invalid file (write to a
  temp file, then rename into place). (Atomic write is delivered here as a
  correctness floor; the broader hardening guard is Feature 4.)
- **R12 -- No regression.** Existing koto behavior, CLI output, and tests are
  unaffected; `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt` all
  pass.

## Acceptance Criteria

- [ ] **AC1 (F1 verified-when #1).** Driving a real koto session inside a real
  Claude Code session, opening `/workflows` shows an entry for it with its
  current state. (Where a live-TUI check is not automatable in CI, a scripted
  verification exercises the same property: publish a location, advance a koto
  session, assert `koto-<uuid>.json` renders the name and current state.)
- [ ] **AC2 (F1 verified-when #2).** After the session advances and the
  operator reopens `/workflows`, the entry shows the new state. (Scripted: a
  second `koto next` updates the file's current-state field.)
- [ ] **AC3 (F1 verified-when #3).** On completion the entry reads *done*, not
  a stuck *running*. (Scripted: driving the session to a terminal state leaves
  the file at a done/failed status.)
- [ ] **AC4 (F1 verified-when #4).** With no published location for the
  hierarchy, koto writes nothing and `/workflows` is unaffected -- koto's
  default path is untouched. (Scripted: with no location published, no
  `koto-*.json` is written and no existing file is modified.)
- [ ] **AC5.** The written file is named `koto-<session-uuid>.json` and does
  not collide with Claude Code's `wf_*.json` naming.
- [ ] **AC6.** The published location lives in the context store under the
  reserved namespaced key, and discovery walks self-then-ancestors; the key
  schema is documented as supporting Feature 3's walk with no change.
- [ ] **AC7.** The target directory is created when absent.
- [ ] **AC8.** The write is atomic (temp-then-rename); a reader never sees a
  partial file.
- [ ] **AC9.** `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, and
  `cargo fmt --check` all pass; koto-user / koto-author skills are updated for
  any new CLI surface.

## Out of Scope

- Richer per-phase / per-agent detail in the entry (Feature 2).
- Hierarchies: coordinator and delegates each rendering (Feature 3).
- The version/fixture guard over the three undocumented surfaces and the
  rendered smoke check (Feature 4). F1 ships atomic write as a correctness
  floor but not the guard.
- Retention/rotation and crash-staleness reconciliation (Feature 5).
- Any koto skill, reader, MCP server, or parallel surface (settled out by the
  ADR).
- Cross-machine delivery (the write target is a local directory) and nested
  single-run rendering (delegates as agents under one run).

## Decisions and Trade-offs

- **Minimal projection now, richness later.** The BRIEF's scope fixes F1 at
  name + current state + running/done. Per-phase detail is deliberately
  deferred to Feature 2 so the skeleton proves the path cheaply; the file
  contract is extensible so F2 adds fields rather than reshaping.
- **Atomic write lands in F1, the guard lands in F4.** Atomic write is cheap
  and prevents torn reads, so it is a correctness floor here; the version/
  fixture guard and rendered smoke check are heavier hardening and stay in the
  later hardening slice.
- **Opt-in is presence of a published location, not config.** koto config is
  not inherited by child sessions, so a config flag could not enable a whole
  hierarchy; the presence of a published location is the per-hierarchy enable.
  This is a requirement (R6), settled here so the DESIGN does not reopen it.
- **The context-store key schema is chosen up front for Feature 3.** Even
  though F1 has a single session, the key is namespaced and discovery is an
  ancestor walk, so Feature 3 adds hierarchy without changing the shipped key
  (R5) -- the explicit "don't box the hierarchy slice out" obligation.
