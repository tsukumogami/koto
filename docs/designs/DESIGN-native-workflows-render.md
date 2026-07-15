---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-native-workflows-render.md
problem: |
  koto must render a single, non-hierarchical session as a native entry in
  Claude Code's `/workflows` screen: write its own `koto-<uuid>.json` on every
  state-commit, carrying name/current-state/status, into a directory a hosting
  Claude Code session publishes -- opt-in, atomic, directory-creating, and
  untouched when no location is published. The slice must also fix the shared
  foundation (file contract, hook seam, context-store publish/discover key) so
  Features 2-5 extend it rather than reopen it.
decision: |
  Materialize off the single trait-level commit funnel
  (`SessionBackend::append_event`), re-deriving a minimal projection from the
  existing dashboard read seam. Publish the target `/workflows` directory into
  koto's per-session context store under the reserved, F3-ready key
  `workflows/publish-location`; resolve the target on commit by walking
  self-then-`parent_workflow`-ancestors and taking the nearest published
  location. Learn the directory from the hosting Claude Code session via a
  `SessionStart` hook that sets `KOTO_WORKFLOWS_DIR`; koto self-publishes that
  into its own context store so descendants (F3) discover it by the same walk.
  Write `koto-<session-uuid>.json` atomically (temp-then-rename), creating the
  directory when absent. No published location resolvable -> no write.
rationale: |
  The trait-level append is the one point every commit path funnels through and
  where a cross-cutting post-commit side effect already lives (the cloud
  backend's S3 push), so hooking it covers all commit paths without
  instrumenting N commands. Reusing the read seam keeps koto's model the single
  source of truth. A namespaced context-store key plus an ancestor walk is the
  minimal shape that is already correct for one session (the walk degenerates to
  self) and for Feature 3's tree (nearest published ancestor) with no key
  change. Opt-in by published-location presence, not config, is forced by koto
  config not being inherited by children.
---

# DESIGN: koto sessions render natively in Claude Code's `/workflows` (walking skeleton)

## Status

Planned

Mechanism design for Feature 1 of `ROADMAP-koto-agent-surface-legibility`,
settling the forks left open by the Accepted `PRD-native-workflows-render`. The
surface decision (koto produces the native artifact; no skill or reader) is
settled upstream in `ADR-koto-native-workflows-rendering` and is not reopened
here. This design pins the shared foundation the whole roadmap rides on: the
extensible file contract, the commit-funnel hook seam, and the context-store
publish/discover key schema.

## Context and Problem Statement

Claude Code's `/workflows` screen reads `<projectDir>/<sessionId>/workflows/*.json`
on open, `JSON.parse`s each file, applies field defaults, sorts by `startTime`,
and renders each as a run entry -- with no registry, signature, or validation
(established empirically against Claude Code v2.1.209 and recorded in the ADR).
The directory is keyed by the *viewing* session's id, which an external koto
process cannot self-discover; the verified way to learn it is a `SessionStart`
hook. The screen is refresh-on-open, not live.

koto is event-sourced. Every state mutation -- `koto next`, directed `--to`,
`koto rewind`, and error/limit exits -- funnels through one low-level
primitive, the session backend's event-append. koto already reconstructs a
UI-free per-session projection at read time (the dashboard's read seam, which
`koto dashboard --once` consumes). The problem is to connect these: on each
commit, re-derive the session's state and write it, in Claude Code's file
shape, into the directory a hosting session published -- for one session now,
extensibly enough that Features 2-5 (richer detail, hierarchies, hardening,
lifecycle) add to it rather than rework it.

Concrete seams this design builds on (koto working tree):

- Commit funnel: `SessionBackend::append_event` (`src/session/mod.rs:231`),
  implemented by `LocalBackend::append_event` (`src/session/local.rs:176`) which
  delegates to `persistence::append_event` (`src/engine/persistence.rs:91`).
  The cloud backend already attaches a post-append side effect here
  (`CloudBackend::append_event` -> `sync_push_state`, `src/session/cloud.rs:711`).
- Read seam: `read_session` / `read_detail` and the pure derivations
  `derive_state_from_log`, `is_terminal_state`, `classify_status`
  (`src/cli/dashboard_data.rs`, `src/cli/dashboard.rs:38`).
- Session identity: `StateFileHeader.session_id` -- an init-time, rename-stable
  UUID v4 (`src/engine/types.rs:246`, generated at `src/engine/types.rs:1273`);
  `StateFileHeader.parent_workflow` for the ancestry walk (`types.rs:221`); the
  canonical walk pattern `measure_depth_from_parent` (`src/engine/caps.rs:167`).
- Context store: the `ContextStore` trait (`src/session/context.rs:26`), keyed
  by `(session, key)`, backed by `~/.koto/sessions/<session>/ctx/<key>` with a
  flock-guarded, atomically-rewritten manifest (`src/session/local.rs:505`);
  key charset validated by `validate_context_key` (`src/session/validate.rs:48`).
- Atomic write precedent: `write_cursor_atomic` (temp-then-rename-then-fsync,
  `src/engine/discovery.rs:150`).

## Decision Drivers

- **Cover every commit path uniformly** without instrumenting individual
  commands (the strategy's explicit "hook the one funnel" rule).
- **koto's model is the source of truth**: the projection is a derivation, not a
  second store; reuse the read seam rather than reimplement it.
- **Do not box out Feature 3**: the context-store key and discovery walk must be
  the shape F3's nearest-published-ancestor generalization needs, chosen now.
- **Opt-in, default path untouched**: no location published -> no write, no
  meaningful added cost, no behavior change.
- **Never render a torn entry**: writes are atomic; the directory is created.
- **Isolate the undocumented coupling**: the exact `/workflows` field shape and
  the hook payload are undocumented and version-coupled; confine that coupling
  to a thin projection/plumbing layer so koto's core is untouched.

## Considered Options

### Fork A -- Which commit seam to hook

**Chosen: the trait method `SessionBackend::append_event`.** Every production
commit path funnels through it (the advance loop's injected append closure,
`--to`, `rewind`, terminal epilogue, error/limit exits), and it is already the
layer a cross-cutting post-commit side effect lives at (the cloud backend's S3
push). A post-commit materialization call is added at the end of
`LocalBackend::append_event`, after `persistence::append_event` succeeds; the
cloud backend inherits it through its inner `self.local.append_event(...)` call.

- *Rejected: instrument each command handler.* This is the N-place design the
  strategy warns against -- it would miss paths (`rewind`, error exits) and
  duplicate the trigger at every call site.
- *Rejected: the free function `persistence::append_event`.* It takes a raw
  `&Path` with no session id, backend handle, or context-store access, so it
  cannot resolve a publish location or key a projection; and it is also called
  by internal coordinator writes (respawn/wake/claim) where a projection would
  be premature. Note those same internal sites bypass the trait, so they do not
  trigger materialization -- correct for F1 (mid-coordination writes are not
  session-state commits an operator wants rendered).

### Fork B -- How to derive the projection

**Chosen: reuse the read-time derivation the dashboard already uses.** The
minimal projection needs only the display name (from the header/intent), the
current state (`derive_state_from_log`), and running/done/failed (`is_terminal_state`
plus the failure-name heuristic `classify_status` uses). These are pure
functions over header + events + template. The small subset F1 needs is factored
into a shared, UI-free helper the materializer and the dashboard both call, so
this is a *reuse* (lifting pure helpers to where both callers see them), not an
engine refactor and not a second derivation.

- *Rejected: reimplement a bespoke state read in the materializer.* Duplicates
  logic that must stay in lockstep with koto's terminal/blocked semantics and
  invites drift -- the exact failure the "single source of truth" driver forbids.
- *Rejected: call the full `read_detail` deep projection.* F1's minimal shape
  does not need evidence/gates/history/remaining; pulling the deep seam now
  couples F1 to fields it does not render. F2 opts into `read_detail` when it
  adds per-phase detail.

### Fork C -- How the hosting directory reaches koto (the addressing problem)

The `/workflows` directory is private to the Claude Code session; koto cannot
self-discover it, and at `SessionStart` time no koto session exists yet, so the
hook cannot publish "into the koto session's context store" directly (the koto
session is created later, by `koto init`).

**Chosen: the hook exposes the directory via `KOTO_WORKFLOWS_DIR`; koto
self-publishes it into its own context store on first commit.** The shipped
`SessionStart` hook derives `workflows_dir = <projectDir>/<sessionId>/workflows`
from the payload and makes it available to koto processes in the session as the
environment variable `KOTO_WORKFLOWS_DIR`. On commit, if that variable is set
and this session has not yet published a location, koto writes the location into
its *own* context store (Fork D's key). Materialization then resolves the
directory through the context-store walk (Fork D), which for the host finds its
own freshly-published key. Self-publishing (rather than reading the env var
directly at write time) is what makes the location durable and walkable by
descendants: a Feature 3 child inherits no env var but discovers the location by
walking to this ancestor's published key -- so the same mechanism serves F1's
single session and F3's tree, and the opt-in stays "presence of a published
location," not config.

- *Rejected: the hook shells out `koto context add <koto-session> ...`.* This is
  the ADR's literal phrasing, but it presumes the hook knows the koto session id,
  which does not exist at `SessionStart`. It also appends a `context_added` event
  (a state commit) into a possibly-nonexistent session, creating a malformed
  session dir. The env-var handoff resolves the bootstrapping gap the ADR left
  implicit while preserving its intent (the location lives in the context store).
- *Rejected: a fixed pseudo-session key (e.g. `__claude_host__`).* Collides
  across concurrent Claude Code sessions, each of which has its own `/workflows`
  directory. Keying discovery off the session's own store (seeded from the
  per-session env var) avoids the collision.

An explicit `koto workflows publish --dir <dir> [--session <id>]` subcommand is
*also* provided (Solution Architecture) for the case where a caller does know
the target session and for scripted verification; it writes the same key without
appending an event. The env-var handoff is the default F1 path; the subcommand is
the explicit escape hatch and the ADR-literal publish surface.

The `KOTO_WORKFLOWS_DIR` handoff is the one accepted point of coupling to Claude
Code's (undocumented) session model, consistent with the ADR's accepted cost. If
Claude Code cannot make the variable visible to a koto process, the documented
fallback is the explicit `koto workflows publish` call from the hook; either way
the location ends up in the context store, and koto core reads only the store.

### Fork D -- The context-store key schema and discovery walk (F3-ready)

**Chosen: reserved namespaced key `workflows/publish-location`, content = the
absolute directory path; discovery walks self-then-ancestors and takes the
nearest published location.** The `workflows/` prefix namespaces koto's
agent-surface reserved keys away from user context keys and passes
`validate_context_key` (the directory path is the *content*, never the key, so
the leading-`/` and `..` key restrictions do not bite). Discovery reuses the
`measure_depth_from_parent` walk shape -- cycle guard, hop cap, missing-header-
as-root, empty-parent-as-root -- probing each session's store with
`ctx_exists(session, "workflows/publish-location")` and returning the nearest
hit's content. For F1 the walk starts and ends at the session itself. Feature 3
adds nothing to the key or the probe: it is already the nearest-published-
ancestor walk, just exercised over a real tree.

- *Rejected: an unprefixed key (`workflows-dir`).* Risks colliding with user
  context keys and does not group the reserved surface; harder to reason about
  when F3/F4/F5 add sibling reserved keys.
- *Rejected: publish only at the root and compose downward.* The strategy
  already rejected root-only composition (it leaves deep progress stale); the
  per-session-writes-to-nearest-published-ancestor model is the settled shape.

### Fork E -- The file contract shape

**Chosen: a minimal, extensible JSON object carrying the top-level fields
Claude Code's `/workflows` renders (an id, a human name, a status, a
`startTime`) plus a koto-namespaced block (`koto: { sessionId, workflow,
currentState, contractVersion }`) that identifies the file and versions the
contract.** Claude Code applies defaults to every field and renders even `{}`,
so a conservative minimal top-level set renders as an entry showing the name and
status; the current state is surfaced both in the koto block and reflected into
the rendered entry. The struct is serde-modeled with room to grow: Feature 2
adds phase/agent detail as additional fields, Feature 4's guard pins the exact
shape against a fixture, and the `contractVersion` gives that guard a stable
anchor. The file is named `koto-<session-uuid>.json` (R2), non-colliding with
`wf_*.json`.

- *Rejected: emit koto's internal detail struct verbatim.* Couples the on-disk
  contract to koto's internal field names and would churn every time the read
  seam changes; the projection is a deliberate, versioned mapping instead.
- *Rejected: reverse-engineer and pin the full Claude Code `wf_*.json` schema
  now.* That is Feature 4's guard/fixture work. F1 emits the minimal renderable
  shape and documents the coupling; over-specifying now would front-load F4.

### Fork F -- Terminal / done inference

**Chosen: reuse koto's terminal derivation for the common path, map the rest
conservatively.** A session whose current state is terminal and not a failure
renders `completed`; terminal-and-failure (name matches the `classify_status`
failure heuristic, or the template's `failure` flag) renders `failed`; anything
else renders `running`. A session writes its terminal state on the terminal
commit, before koto's cleanup deletes its state file, so the finished entry
persists (R9). The three genuinely ambiguous cases the strategy calls out
(missing/changed template, an unnamed failure state, a cancelled session) are
handled at F1 only to the extent the common path requires -- a cancelled session
(a `WorkflowCancelled` event) renders terminal rather than a stuck `running`;
the finer inference (Block 2's net-new work) is Feature 2/4/5 scope and is noted
as a known limitation here, not silently assumed solved.

- *Rejected: solve all three ambiguous cases now.* The ADR assigns that
  net-new inference to Block 2 (Feature 2's mapping and Feature 4's guard);
  pulling it into F1 widens the skeleton past its purpose. F1 must only satisfy
  "graceful completion reads done," which the common-path mapping does.

### Fork G -- Write atomicity and directory creation

**Chosen: temp-then-rename into the target directory, creating the directory
first.** The materializer `create_dir_all`s the target directory (R7), writes
the JSON to a temp file in the same directory, fsyncs, and renames it over
`koto-<uuid>.json` (R11) -- the `write_cursor_atomic` pattern
(`src/engine/discovery.rs:150`) applied to the external directory. Same-directory
rename is atomic on the local filesystem, so a concurrent `/workflows` reopen
never sees a partial file.

- *Rejected: write in place.* Risks a torn read if `/workflows` opens mid-write;
  cheap to avoid, so avoided even though the broader hardening guard is F4.

## Decision Outcome

koto materializes a per-session `/workflows` artifact off the single
trait-level commit funnel. On each `SessionBackend::append_event`:

1. **Resolve the target directory.** Walk from the committing session up its
   `parent_workflow` chain, probing each session's context store for
   `workflows/publish-location`; take the nearest hit. Before the walk, if
   `KOTO_WORKFLOWS_DIR` is set and this session has no published location yet,
   self-publish it into this session's store (so the walk finds it and
   descendants can too). No location resolvable -> return, writing nothing
   (R6/R10).
2. **Derive the minimal projection** from the read seam: display name, current
   state, running/done/failed status.
3. **Build the file** -- the extensible JSON with the koto-namespaced block and
   `contractVersion`.
4. **Write atomically** -- `create_dir_all` the directory, temp-write, fsync,
   rename over `koto-<session-uuid>.json`.

A Claude Code `SessionStart` hook, shipped in koto's plugin, derives the
`/workflows` directory from its payload and exposes it as `KOTO_WORKFLOWS_DIR`
(with `koto workflows publish` as the explicit fallback/escape hatch).

This satisfies every PRD requirement: R1 (funnel), R2 (UUID file), R3 (minimal
projection via the read seam), R4 (extensible contract), R5/R6 (context-store
publish/discover, opt-in by presence), R7 (mkdir), R8 (SessionStart), R9
(terminal-before-cleanup), R10 (untouched default path), R11 (atomic), R12 (no
regression).

## Solution Architecture

New module `src/workflows_surface/` (koto core, below `cli/`):

- `contract.rs` -- the serde `WorkflowFile` struct (top-level `id`, `name`,
  `status`, `startTime`; nested `koto` block with `sessionId`, `workflow`,
  `currentState`, `contractVersion`) and its JSON serialization. This is the
  extensible file contract (R4); `contractVersion` starts at 1.
- `discover.rs` -- `resolve_publish_location(backend, session_id) -> Option<PathBuf>`:
  the self-then-ancestor context-store walk (Fork D), reusing the
  `measure_depth_from_parent` structure; and `publish_location(store, session_id, dir)`:
  write the reserved key without appending an event.
- `project.rs` -- `derive_minimal_projection(backend, session_id) -> Projection`:
  the UI-free derivation (Fork B), reusing the dashboard's pure state/terminal
  helpers (lifted to a shared spot if currently private to `cli/`).
- `materialize.rs` -- `materialize_after_commit(backend, session_id)`: the
  post-commit entry point (Fork A) tying resolve -> self-publish-if-needed ->
  derive -> atomic-write; and the atomic writer (Fork G). Opt-in gate first:
  if neither `KOTO_WORKFLOWS_DIR` nor a published key is present, return
  immediately.

Wiring:

- `LocalBackend::append_event` calls `materialize_after_commit(self, id)` after
  a successful `persistence::append_event`. `self` is `&LocalBackend`, which
  implements both `SessionBackend` and `ContextStore`, so the materializer needs
  no `cli/` dependency and there is no post-commit callback plumbing. The
  materializer's writes go to the external `/workflows` directory and (for
  self-publish) the context store via `ContextStore::add`, which does *not*
  append an event -- so there is no re-entrancy into `append_event`.
- CLI: `koto workflows publish --dir <dir> [--session <id>]`. The existing
  `Command::Workflows { roots, children, orphaned }` flat variant becomes
  `Command::Workflows(WorkflowsArgs)` carrying the same flags plus an optional
  `#[command(subcommand)] WorkflowsSubcommand::Publish { dir, session }`, so
  bare `koto workflows` (used by the plugin's `Stop` hook) is unchanged and the
  new verb is additive.
- Plugin: a `SessionStart` entry in `plugins/koto-skills/hooks.json` runs a
  small script (`plugins/koto-skills/hooks/session-start-workflows.sh`) that
  reads `session_id` / `transcript_path` from stdin JSON, computes the
  `/workflows` directory, and exposes `KOTO_WORKFLOWS_DIR` (emitting the
  documented Claude Code `SessionStart` env/context output).

Data/control flow on a commit:

```
koto next / --to / rewind / exit
  -> SessionBackend::append_event (LocalBackend)
       -> persistence::append_event  (state file written)
       -> materialize_after_commit(self, id)
            gate: KOTO_WORKFLOWS_DIR set OR key published?  no -> return
            self-publish KOTO_WORKFLOWS_DIR into own ctx store (idempotent)
            dir = resolve_publish_location(self, id)   (self-then-ancestor walk)
            proj = derive_minimal_projection(self, id) (read seam)
            create_dir_all(dir); atomic-write koto-<uuid>.json
```

## Implementation Approach

1. **File contract** (`contract.rs`): the `WorkflowFile` struct + a builder from
   a `Projection`; unit tests asserting the serialized JSON shape and the
   `koto-<uuid>.json` name. No behavior change to koto yet.
2. **Discovery + publish** (`discover.rs`): the ancestor walk and the
   event-free `publish_location`; the `koto workflows publish` CLI verb (the
   `WorkflowsArgs` restructure) and its dispatch; koto-user/koto-author skill
   updates for the new surface. Unit + a small CLI integration test.
3. **Projection derivation** (`project.rs`): the minimal read-seam derivation,
   lifting any dashboard helper that is currently private so both callers share
   one implementation. Unit tests over fixture logs (running, terminal-success,
   terminal-failure, cancelled).
4. **Materialize + wiring** (`materialize.rs` + `LocalBackend::append_event`):
   the atomic writer, the opt-in gate, and the post-commit call. Unit tests for
   the atomic writer; an integration test that drives a real koto session under
   `KOTO_SESSIONS_BASE` + `KOTO_WORKFLOWS_DIR`, asserting AC1-AC4 (file appears
   with current state; updates on advance; reads done on terminal; no file when
   no location published).
5. **End-to-end verification harness**: a scripted check (committed under
   `tests/` or `scripts/`) that exercises publish -> advance -> render without a
   live Claude Code, plus a documented manual procedure for the live-TUI check
   (per the PRD's allowance where CI cannot drive the TUI).

The five map onto the PLAN's issue outlines; they are dependency-ordered
(contract -> discovery -> projection -> materialize -> verify) with the first
three independent enough to land in any order behind the fourth.

## Security Considerations

- **Untrusted directory value.** `KOTO_WORKFLOWS_DIR` and any published-location
  content are operator-supplied paths. They are used only as a write target
  (`create_dir_all` + rename); the value is never interpolated into a shell, and
  the filename is fixed (`koto-<uuid>.json`, uuid from koto's own header). A
  malicious value can at worst direct koto's own write to a directory the
  invoking user can already write to -- no privilege boundary is crossed.
- **Context-store key charset.** The reserved key is a constant
  (`workflows/publish-location`) that passes `validate_context_key`; the
  directory path is stored as *content*, never as a key, so no path-traversal
  surface opens through the key.
- **No new event on publish.** Self-publish and `koto workflows publish` write
  the context key via `ContextStore::add` (no `append_event`), so they neither
  perturb the session's event log nor re-enter the commit funnel.
- **Opt-in containment.** With no location published and no env var, the
  materializer returns after a single cheap probe -- no directory is created, no
  file written, no existing `/workflows` file touched (AC4/R10).
- **Undocumented-surface isolation.** The coupling to Claude Code's file shape
  and hook payload is confined to `contract.rs` and the hook script; koto's
  event log, engine, and dashboard do not depend on any of it, so a future
  Claude Code change touches only the projection layer (the ADR's core-isolation
  property). The guard that makes such a change fail loudly is Feature 4.

## Consequences

- koto gains a per-commit, opt-in materialization side effect on the local
  commit funnel. Cost on the default (no-location) path is one env read plus, at
  most, one `ctx_exists` probe; on the enabled path it is a read-seam
  re-derivation plus an atomic write per commit (the strategy's accepted
  per-advance cost).
- The context-store key `workflows/publish-location` and the self-then-ancestor
  walk are now a shipped contract Feature 3 extends without change -- the "don't
  box out F3" obligation is discharged in F1.
- Internal coordinator writes that bypass the trait (respawn/wake/claim, via
  `persistence::append_event` directly) do not materialize. Harmless for F1
  (they are not operator-facing session commits); Feature 3 revisits whether any
  need to, when hierarchy rendering is in scope.
- Known limitations carried forward (not regressions): the finer terminal
  inference for the three ambiguous cases is Feature 2/4/5; the version/fixture
  guard and rendered smoke check are Feature 4; retention and crash-staleness
  are Feature 5. Each is named in the roadmap and out of F1 scope.
- If Claude Code cannot expose `KOTO_WORKFLOWS_DIR` to a koto process, the
  documented fallback (`koto workflows publish` from the hook) keeps the design
  intact, since koto core reads only the context store.

## References

- `PRD-native-workflows-render` (upstream) -- the requirements this design
  satisfies.
- `ADR-koto-native-workflows-rendering` (upstream, Accepted) -- the settled
  surface decision and empirical addendum (Claude Code v2.1.209 render
  mechanics).
- `STRATEGY-koto-agent-surface-legibility` (upstream, Accepted) -- Blocks 1-3
  (materialize / mapping / publish-discover).
- koto seams: `src/session/mod.rs` (`SessionBackend`), `src/session/local.rs`
  (`append_event`, context store), `src/engine/types.rs` (`StateFileHeader`),
  `src/engine/caps.rs` (`measure_depth_from_parent`), `src/cli/dashboard_data.rs`
  (read seam), `src/engine/discovery.rs` (`write_cursor_atomic`).
