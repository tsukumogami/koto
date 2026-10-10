# Lead: How does a koto session render in the Claude Code workflow view, and what is that path's data contract?

## Findings

### Who writes, when, where
- Writer: `src/workflows_surface/materialize.rs::materialize_after_commit`. Called from one place, `LocalBackend::append_event` (`src/session/local.rs:246`), after `persistence::append_event` and the run-journal hook. The cloud backend inherits it through its inner local append. It fires on every trait-level commit (next, --to, rewind, error exits, and `context_added`/`context_removed`, since `koto context add` appends an event). Internal coordinator writes calling `persistence::append_event` directly (respawn/wake/claim) do not trigger it (DESIGN-native-workflows-render.md, Fork A).
- Not a tick or timer: per-commit. Claude Code side is refresh-on-open, not live (design Context section).
- Best-effort: errors are `eprintln!`-warned and swallowed. A thread-local re-entrancy guard (`MaterializeGuard`) exists because publishing a location appends a `context_added` event.
- Destination: `<claude projectDir>/<claude sessionId>/workflows/koto-<session-uuid>.json` (`contract.rs::workflow_filename`). Sessions with empty `session_id` (pre-UUID legacy) are skipped.
- Atomic write: `create_dir_all`, tempfile in same dir (`.koto-wf-*.tmp`), `sync_all`, `persist` (`materialize.rs::atomic_write`). `startTime` preserved from existing file (`stable_start_time`), else now.
- Target resolution (`resolve_target_dir`): (1) `KOTO_WORKFLOWS_DIR` env (self-published into the session's store); (2) nearest published ancestor via `discover::resolve_publish_location` (context key `workflows/publish-location`, self then `parent_workflow` chain, cycle guard, 1000-hop cap); (3) if `workflows.native` is true (default true), derive from `CLAUDE_CODE_SESSION_ID` by locating `<id>.jsonl` under `~/.claude/projects/*` and appending `<id>/workflows`. Nothing resolved (headless) or opted out -> no write. A malformed config reads as disabled.
- `koto workflows publish --dir <abs> [--session <id>]` (`src/cli/mod.rs:645-658`, handler `:896-910`) only writes the context key `workflows/publish-location` via `discover::publish_location`; it renders nothing itself. `--session` falls back to `KOTO_WORKFLOWS_HOST_SESSION`. Rendering happens on the target session's next commit.
- Plugin hook `plugins/koto-skills/hooks/session-start-workflows.sh` (in `plugins/koto-skills/hooks.json`) is now optional; it sets `KOTO_WORKFLOWS_DIR`.
- Docs: `docs/guides/native-workflows-verification.md`, `docs/guides/cli-usage.md` (~883-893), `scripts/verify-native-workflows.sh`.

### File format (contract version 2)
`src/workflows_surface/contract.rs`; golden fixture `tests/fixtures/native-workflows/enriched-shape.json`, guard `tests/native_workflows_shape.rs`.
- Top level: `id` (`koto-<uuid>`), `name`, `status` (running|blocked|completed|failed), `startTime` (epoch ms; Claude Code sorts by it), `phases[]` (`{title, detail?}`), `workflowProgress[]`, and a `koto` block `{sessionId, workflow, currentState|null, contractVersion}`.
- `workflowProgress`: `workflow_phase {index,title}` per phase; `workflow_agent {index,label,phaseIndex,phaseTitle,state(done|progress),promptPreview?,resultPreview?}` only for visited/active phases.
- Claude Code side: globs `*.json`, `JSON.parse`, defaults every field, no validation; established empirically against Claude Code v2.1.209. Undocumented, version-coupled; the Claude-side drift guard is "a later slice", not shipped.

### Field inventory and size rules
- Name: header `intent`, else `template_name . current_state`, else `untitled (template)`, else workflow name (`project.rs::derive_display_name`). No cap.
- Phase title: humanized state name. Phase `detail`: `gate <name>: PASS|FAIL`, else `evidence: <field names>`, else "in progress"/"done". Evidence field NAMES only, never values.
- `promptPreview` = state's template directive (compiled template), whitespace-collapsed to one line, capped at `PREVIEW_MAX = 240` chars plus ellipsis (`materialize.rs::preview`). `resultPreview` = outcome line, same cap.
- No other limits: no cap on phase count, name length or file size; pretty JSON rewritten whole each commit.
- Phase order: structural walk of the compiled template from `initial_state`, unreachable states appended; if the compiled template is unreadable, phases are omitted.

### What of a session could ride this path
- State and status: yes.
- Directive: yes, only as a <=240-char one-line preview per phase.
- Context keys: nothing today; the projection never reads the context store (except the publish key). `ContextStore` has `list_keys`/`get`; `context_added` events carry `key`, `hash`, `size`, `writer` (`docs/reference/session-feed.md` ~196), so key + size is derivable cheaply from the log; content needs `get`.
- Execution anchor: not projected. `ExecutionAnchorAdopted {anchor}` / `ExecutionAnchorRebound {from,to}` events (`src/engine/types.rs:1144-1180`) and header `execution_dir` exist, so derivable.
- Cloud remote: not projected; `workflows_surface` has no reference to remote/cloud config.

### What it cannot carry
- It is a status tree, not a document view. Only name/status/phases/workflowProgress previews are known to render; extra keys are probably ignored by the screen (never observed displayed). Large/binary values, per-key content and sizes have no native slot except strings stuffed into `detail`/`resultPreview`.
- Not live: updates on commit and on reopen of `/workflows`.
- Exists only inside a Claude Code session (or explicit `KOTO_WORKFLOWS_DIR`); headless users see nothing.
- No retention/removal logic (later-slice scope).
- Coupled to an undocumented Claude format with no Claude-side drift guard.

## Implications
- The workflow view can credibly carry a compact summary only: state, status, directive preview, and perhaps a one-line context summary (key count, total bytes, `key (size)` lines in a phase `detail`) plus anchor/remote in the `koto` block or name. Full directive, key content, and legible large/binary rendering belong on the dashboard (TUI and `--once`).
- Adding context keys/anchor is an additive contract bump (contractVersion 3, update golden fixture); the funnel already fires on `context_added`. Cost: reading the context store per commit.
- The 240-char one-line preview is the only precedent for bounded display of large values here; reuse it.
- `koto workflows publish` needs no change; it is plumbing, not the view.
- DESIGN-visual-workflow-preview.md covers a different surface (template export to Mermaid/HTML, `--check`, GHA); DESIGN-session-feed-data-contract.md covers the JSONL log contract (`docs/reference/session-feed.md`). Neither is a session render path, but the latter is the source data any view derives from.

## Surprises
- `koto workflows publish` help text (`src/cli/mod.rs:645-647`) and the design say it "writes no event", but `discover.rs::publish_location` appends a `context_added` event best-effort. Doc/code contradiction.
- The design's "opt-in by published location, not config" was superseded: rendering is on by default (`workflows.native` default true) for any koto session inside Claude Code, so any added content (especially context content) would be written by default under `~/.claude/projects/`.
- Directive preview comes from the compiled template, not necessarily the interpolated directive the agent saw (unverified).
- The key `workflows/publish-location` (writer `koto`) appears in a session's context key listing; a context view must decide whether to hide reserved keys.

## Open Questions
- Does Claude Code's `/workflows` display anything beyond name/status/phases/workflowProgress? Needs an empirical check; decides whether context info can be shown natively or must be encoded into preview strings.
- Any Claude-side length limit on `promptPreview`/`resultPreview`/`detail`?
- Should context content (possibly sensitive) be written under `~/.claude/projects` given the default-on gate? Names and sizes only vs truncated content is a maintainer call.
- Where do cloud remote and anchor live for a projection independent of `cli/` (header `execution_dir`, cloud config)? Not investigated.

## Summary
The native path is a per-commit, best-effort, atomic write of `koto-<uuid>.json` (contract v2: name, status, startTime, phases, workflowProgress with directive/outcome previews capped at 240 one-line chars, plus a `koto` block) into the hosting Claude Code session's `/workflows` directory, on by default via `workflows.native`; `koto workflows publish` only records the target directory. Today it carries state, status and a truncated directive but no context keys, sizes, content, execution anchor or cloud remote, and its status-tree shape has no native slot for large or binary values, so the full legible view belongs on the dashboard with this surface getting a compact summary via an additive contract bump. The biggest open question is what the Claude Code screen actually renders beyond name/status/phases/previews, plus the doc/code contradiction that `publish` is documented as writing no event but does append `context_added`.
