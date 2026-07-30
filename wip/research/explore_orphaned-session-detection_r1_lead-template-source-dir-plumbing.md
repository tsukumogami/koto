# Lead: How is template_source_dir recorded and read today, and what CLI paths touch session state?

## Findings

### Where it's defined and stored

`template_source_dir` is a field on `StateFileHeader`, not on an event -- `src/engine/types.rs:223-260`. It's `Option<PathBuf>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`, so it's additive and omitted from JSON when `None`. The `StateFileHeader` is the first line of a session's `koto-<name>.state.jsonl` file (a header, "has no `seq` field -- it is not an event", per the doc comment at `src/engine/types.rs:218-221`). The exploration lead's framing ("recorded in the `workflow_initialized` event") is slightly off: the value lives in the header line, separate from the `WorkflowInitialized` event payload (`EventPayload::WorkflowInitialized { template_path, variables, spawn_entry }`, which carries the compiled-template cache path, not the source directory).

### Where it's written (koto init)

Both `koto init <name> --template <path>` and child-spawn (`init_child_from_parent`) go through `src/cli/init_child.rs`. The field is computed at lines 456-467:
- If the template path is absolute, `template_source_dir = template_path.parent()`.
- If relative, it canonicalizes the path against cwd and takes its parent; on any failure it's left `None`.

It's placed into the `StateFileHeader` literal at line 475 and written to disk via `persistence::append_header` (`src/engine/persistence.rs:38-58`), which serializes the header struct and writes it as line 1 of the state file (mode 0o600, create-if-absent).

### Where it's read

There is a single low-level parser both readers funnel through: `parse_header`, called from both `persistence::read_header` (`src/engine/persistence.rs:487-506`, reads just line 1) and `persistence::read_events` (`src/engine/persistence.rs:532-...`, reads header + all events). So header parsing itself is NOT duplicated -- one parse path, two entry points depending on whether the caller needs events too.

Above that shared parser, three call sites diverge:

1. **`koto status`** -- `handle_status` (`src/cli/mod.rs:4387-4423`) calls `backend.read_events(name)`, getting the full `StateFileHeader` (including `template_source_dir`) back as `header`. But `template_source_dir` is never referenced anywhere in the rest of `handle_status` -- the JSON response built at lines 4456-4478 uses `template_hash`, `current_state`, `is_terminal`, batch info, and superseded branches, never the source dir or a liveness check derived from it.

2. **`koto session list`** -- `handle_list` (`src/cli/session.rs:504-508`) just calls `backend.list()` and prints the result. The local backend's `list()` (`src/session/local.rs:84-137`) does read each session's header via `persistence::read_header` (line 115), but immediately projects it down to `SessionInfo` (`src/session/mod.rs:129-141`), which only keeps `id`, `created_at`, `template_hash`, `parent_workflow`. `template_source_dir` is parsed and then discarded -- it's read off disk and thrown away before it ever reaches the CLI output. The cloud backend's `list()` (`src/session/cloud.rs:677-699`) merges in remote-only sessions with placeholder `SessionInfo` values and has the same field set, so parity holds across backends.

3. **`koto init`'s collision check** -- two checks exist, both string-only, neither touches header content:
   - The pre-check at `src/cli/mod.rs:1682-1691`: `if backend.exists(name) { exit_with_error(...) }`. `exists()` (`src/session/local.rs:71-73`) is `self.base_dir.join(id).join(state_file_name(id)).exists()` -- pure filesystem presence, no header parse at all.
   - The authoritative check inside `init_child_from_parent`, surfacing as `SpawnErrorKind::Collision` (`src/cli/task_spawn_error.rs`), handled at `src/cli/mod.rs:1707-1716` with the same "workflow '{name}' already exists" message (deliberately kept byte-identical to the pre-check per the comment at lines 1709-1711).
   
   Neither path opens the *existing* colliding session's state file, so there is no code today that could even check whether that other session's `template_source_dir` still exists -- the error is generated before any header read happens.

### The one existing consumer of template_source_dir

The only place the value is actually read back and *acted on* today is the batch scheduler's relative-child-template-path resolution:
- `resolution_context` in `src/cli/batch.rs:1910-1924` calls `backend.read_events(parent_name)` and pulls `header.template_source_dir` out to use as a resolution base.
- `src/cli/batch.rs:870-876` probes `template_source_dir.exists()` once per scheduler tick and passes the boolean into `resolve_template_path_with_base_status` (`src/engine/path_resolution.rs`).
- `src/engine/path_resolution.rs` (lines 8-260) documents the resolution order: relative target + submitter_cwd, or relative target + template_source_dir, falling back when the base doesn't exist.
- `src/engine/scheduler_warning.rs` already models exactly this staleness question for its own narrow purpose: `SchedulerWarning::MissingTemplateSourceDir` (header has no dir -- pre-existing field, old session) and `SchedulerWarning::StaleTemplateSourceDir { path, machine_id, falling_back_to }` (header has a dir, but `Path::exists()` is false "typically following a cross-machine session migration"). This is prior art for exactly the existence-check the exploration wants, but it's scoped to relative child-template-path resolution during a scheduler tick, not to session-level liveness reporting in `status`/`init`/`session list`.

## Implications

There is no shared "is this session's origin still alive" concept anywhere in the codebase today -- `template_source_dir` existence-checking exists exactly once, inside the batch scheduler, for a narrow path-resolution fallback, and is invisible outside that code path. Wiring orphan detection into `status`, `init`'s collision message, or `session list` would need to newly plumb `header.template_source_dir` (already available from `read_events`/`read_header`) through to those three call sites -- it is not a matter of "the data isn't captured," it's "the data is captured and read by exactly one consumer for an unrelated purpose, then dropped everywhere else that touches the header."

Because header parsing is centralized (`parse_header` underneath both `read_header` and `read_events`), a fix doesn't need to touch parsing logic. It needs to touch three separate, independently-written call sites: `handle_status` (already has the header in hand, just needs to add a check + surface it), `SessionInfo`/`list()` (needs a new field threaded through the struct in `src/session/mod.rs` plus both backend impls in `local.rs` and `cloud.rs`), and the init collision path (needs to newly *read* the colliding session's header at all, since today `exists()` is a directory check that never opens the file).

`SchedulerWarning::StaleTemplateSourceDir` is a reasonable naming/shape precedent (dedicated kind, includes `machine_id` when known, `falling_back_to` path) that a session-level "orphaned" signal could mirror stylistically, though it would need a different home since `scheduler_warning.rs` is explicitly scoped to `SchedulerOutcome.warnings` for scheduler ticks (per its module doc, "Decision 14 in DESIGN-batch-child-spawning.md").

## Surprises

- The exploration lead described the field as living in the `workflow_initialized` event; it actually lives in the header line, which is structurally separate from events (no `seq`, written once via `append_header`, never appended again). This matters for design: any staleness-checking code reads the header once per session, not by scanning the event log.
- The codebase already has a fully-formed "stale template_source_dir" concept (`StaleTemplateSourceDir`) with almost exactly the shape orphan detection would want (recorded path + machine_id + fallback), but it's a scheduler-tick warning, not a session-status signal -- it's easy to assume this already covers the ask and it doesn't; it fires only during batch child spawning, never during `status`/`init`/`list`.
- `koto session list`'s `SessionInfo` projection silently drops `template_source_dir` after reading it off disk -- the I/O cost of reading the header is already paid on every `list()` call; the field is just not kept.

## Open Questions

- What machine-identity mechanism (referenced by `machine_id` in `StaleTemplateSourceDir`) already exists, and should a new orphan-detection signal reuse it to distinguish "directory really gone" from "directory just not visible on this machine" (cross-machine/cloud-backend case)?
- Should `koto init`'s collision check start reading the existing session's header (a new I/O read on a path that's currently a zero-read directory check), and if so, does that change its performance/atomicity guarantees (the comment at `src/cli/mod.rs:1676-1681` explicitly frames the pre-check as best-effort/cheap, with the atomic collision detection happening deeper in `init_state_file`)?
- Does `CloudBackend::list()`'s remote-only placeholder path (`src/session/cloud.rs:688-693`, which fabricates a `SessionInfo` with empty `created_at`/`template_hash` because it can't download the state file) need special-casing for a new `template_source_dir`/orphan field, since it has no header to read in the first place?
- Where should a new "orphaned session" concept be named/modeled -- a new field on `SessionInfo`, a new warning type parallel to `SchedulerWarning` but for status/list, or something else -- and should the check be eager (every `list`/`status` call pays an `exists()` syscall per session) or on-demand (only when explicitly requested)?

## Summary
`template_source_dir` lives on `StateFileHeader` (`src/engine/types.rs:260`), is written once at `koto init`/child-spawn time in `src/cli/init_child.rs:456-475`, and is read back through a single shared parser (`persistence::parse_header`) -- but the only code that actually consumes the value today is the batch scheduler's relative-child-template-path resolver (`src/cli/batch.rs` + `src/engine/path_resolution.rs`), which already has a fully-shaped "stale directory" warning (`SchedulerWarning::StaleTemplateSourceDir`) that nothing outside scheduler ticks can see. `koto status` reads the full header but never looks at this field, `koto session list` reads the header then discards the field when projecting into `SessionInfo`, and `koto init`'s "already exists" collision path never opens the existing session's header at all (its `exists()` check is a pure filesystem-presence test) -- so wiring in orphan detection touches three independent call sites, not one shared choke point. The biggest open question is what machine-identity signal (the `machine_id` field already stubbed in `StaleTemplateSourceDir`) should back a "the directory doesn't exist because it was reaped elsewhere" distinction versus a simple missing-path check.
