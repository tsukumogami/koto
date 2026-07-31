---
complexity: testable
complexity_rationale: Adds new, previously-absent header-read I/O to an existing, frequently-hit error path (koto init's collision check) and changes the wording of a documented error string on both collision detectors -- warrants explicit tests beyond a smoke check, but the change is read-only, bounded to a single already-open session's header, and does not cross any new trust boundary.
---

## Goal

Update both of `koto init`'s "already exists" collision paths in `src/cli/mod.rs` -- the pre-check and the `SpawnErrorKind::Collision` handler -- to open the colliding session's header, run the Issue 1 staleness check, and append the same staleness clause to whichever base message each path already emits.

## Context

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

This is the fix for the bug that motivated the whole design: `tsukumogami/koto#189`. A session created inside a working tree that's later torn down (a reaped ephemeral sandbox, a removed git worktree, a container teardown) stays "live" from koto's perspective forever, and a later `koto init <same-name>` from an unrelated environment collides with it and fails with a generic `"already exists"` error that's indistinguishable from a real concurrent session. Today the only way to tell the two apart is to manually open the colliding session's raw state JSONL and check by hand whether `template_source_dir` still resolves.

Direct inspection of `src/cli/mod.rs` confirms the design's Implicit Decision ("both `koto init` collision paths get the staleness clause") is correct as written, including its Phase 6 correction: the two collision paths do **not** emit byte-identical base messages today.

- The pre-check (`src/cli/mod.rs:1682-1691`, inside `handle_init`) emits:
  `"workflow '{}' already exists; run \`koto session cleanup {}\` to reuse the name, or \`koto cancel --cleanup {}\` to stop a running workflow first"`
- The `SpawnErrorKind::Collision` handler (`src/cli/mod.rs:1707-1716`, same function, reached when the atomic `init_child_from_parent` call detects the race case) emits the shorter:
  `"workflow '{}' already exists"`

Only the `"workflow '{}' already exists"` prefix is shared between the two; the pre-check's cleanup guidance is not present in the collision-handler's message. This issue does not unify the two base messages -- that's explicitly out of scope per the design's Implicit Decision -- it only ensures the same new staleness clause is appended to both, so the same underlying condition is diagnosable regardless of which of the two detectors happens to fire (a race-timing detail the caller shouldn't have to reason about).

Note: `handle_init_inline` (`src/cli/mod.rs:1822-1833`) has its own, separate pre-check emitting the same longer message text as `handle_init`'s pre-check, but it has no corresponding `SpawnErrorKind::Collision` match arm in this codebase today -- its error path falls through to a generic `e.to_string()` branch instead. Per the design's explicit scope (Solution Architecture "Components" for `src/cli/mod.rs`, and Implementation Approach "Phase 5"), this issue covers only the two collision paths inside `handle_init` (`~1682` and `~1707`); `handle_init_inline` is not touched here.

## Acceptance Criteria

- [ ] `handle_init`'s pre-check (`src/cli/mod.rs:1682-1691`) opens the colliding session's header (via the same header-read mechanism already used elsewhere, e.g. `backend.read_events(name)` or an equivalent header-only read) before building its error message. This is new I/O: today this branch is a pure `backend.exists(name)` check with no header read.
- [ ] `handle_init`'s pre-check calls the Issue 1 shared helper (`check_template_source_dir`, from `src/engine/template_source_status.rs`) against the freshly-read header, and, when the result is `Some(TemplateSourceStatus { exists: false, .. })`, appends a staleness clause to the existing error message rather than replacing or rewording the existing `"workflow '{}' already exists; run \`koto session cleanup {}\`..."` text.
- [ ] The `SpawnErrorKind::Collision` handler (`src/cli/mod.rs:1707-1716`) is updated in the same way: it opens the colliding session's header, runs the same shared helper, and appends the same staleness clause to its existing (shorter) `"workflow '{}' already exists"` text when the check reports `exists: false`.
- [ ] The staleness clause text and the JSON shape it's carried in (e.g. an added `stale_template_source_dir` field/sub-object alongside the existing `"error"` string, consistent with the wire shape used in `koto status` per Issue 4) are identical between the two paths for the same underlying condition -- verified by a test that forces both detectors to fire against sessions with the same stale `template_source_dir` and asserts the clause content matches, not just its presence.
- [ ] The two paths' base messages are explicitly **not** required to become identical to each other -- the pre-check keeps its cleanup guidance (`run \`koto session cleanup {}\`...`) and the collision handler keeps its shorter text; only the newly-appended staleness clause is required to match between them. A test or comment should make clear this is intentional, not an oversight.
- [ ] Message wording branches on `Backend::is_cloud()` (per Decision 2 / Issue 4's shared formatting helper if landed first, or an equivalent local check if this issue lands before Issue 4): a stale `LocalBackend` session gets direct wording (e.g. asserting the directory no longer exists), a stale `CloudBackend` session gets softened wording acknowledging a cross-machine resume may be the explanation. The underlying `Path::exists()` computation itself does not differ by backend.
- [ ] When the shared helper returns `None` (no `template_source_dir` was ever recorded on the colliding session) or `Some(TemplateSourceStatus { exists: true, .. })` (the directory still resolves), neither collision path's message changes from its current, pre-this-issue text -- the new clause is additive and only appears when staleness is confirmed.
- [ ] The new header read added to the pre-check is best-effort with respect to the overall collision error: if the header read itself fails (e.g. corrupt state file), the pre-check still surfaces the existing `"already exists"` error (without the staleness clause) rather than crashing or producing a different error entirely -- collision detection must not become less reliable than it is today as a side effect of adding this diagnostic.
- [ ] **Repro test for tsukumogami/koto#189**: an integration/CLI test that (1) initializes a session with a `template_source_dir` pointing at a directory, (2) deletes that directory (simulating a reaped sandbox/removed worktree), (3) runs `koto init <same-name>` again, and (4) asserts the resulting error output now includes the staleness clause identifying the recorded `template_source_dir` as missing -- i.e. the original bug's exact repro no longer produces an undiagnosable generic `"already exists"` error.
- [ ] `cargo build` and `cargo test` pass; no existing test asserting the current pre-this-issue message text for either collision path is left broken -- update those tests' expected strings to include the new clause where staleness is part of the fixture, and leave them unmodified where it is not (per the `None`/`exists: true` AC above).

## Dependencies

Blocked by <<ISSUE:1>>

## Downstream Dependencies

None (leaf node)
