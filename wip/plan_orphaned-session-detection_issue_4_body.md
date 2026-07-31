---
complexity: testable
complexity_rationale: New JSON output surface on two commands driven by shared backend-conditional wording logic (local vs. cloud) plus a doc update -- low structural complexity, but the branching wording behavior and the conditional-key wire shape both need direct test coverage to avoid silent drift between the two call sites.
---

## Goal

Wire `handle_status` and `handle_list` to surface `template_source_status` as a conditional `stale_template_source_dir` JSON key (only when the directory is missing), through a shared backend-aware wording helper that softens language for cloud-synced sessions instead of asserting deletion, and document the new field in `docs/guides/cli-usage.md`.

## Context

`<<ISSUE:1>>` introduces `TemplateSourceStatus { path, exists, machine_id }` and `check_template_source_dir` in `src/engine/template_source_status.rs`, and `<<ISSUE:3>>` populates a new `template_source_status: Option<TemplateSourceStatus>` field on `SessionInfo`, filled in by `LocalBackend::list()` and left `None` on `CloudBackend`'s remote-only placeholder rows. Neither of those issues changes any CLI output -- this issue is where the computed fact actually reaches the two commands operators use to notice a torn-down session (`tsukumogami/koto#189`): `koto status` and `koto session list`.

`handle_status` (`src/cli/mod.rs`, currently defined starting at line 4387) already reads the session's full header via `backend.read_events(name)` and builds its response with `serde_json::json!({...})`, then conditionally adds a top-level `batch` key (when `derive_batch_view` returns `Some`) and a `superseded_branches` key (when non-empty) -- confirmed by reading the function directly. This issue adds a third conditional key, `stale_template_source_dir`, following that same present-only-when-relevant pattern rather than an always-present-but-often-null field.

`handle_list` (`src/cli/session.rs`, currently at line ~504) is presently a two-line function: it calls `backend.list()` and serializes the resulting `Vec<SessionInfo>` directly via `serde_json::to_string_pretty`. Because `SessionInfo` derives `Serialize`, once `<<ISSUE:3>>` lands, the raw `template_source_status` field (path/exists/machine_id) will already appear in this output with no code change here -- but the design also calls for a shared wording-formatting helper "consulted by both" `handle_status` and `handle_list` (Implementation Approach, Phase 4), so this issue's job on the list side is to apply that same backend-aware note text to each stale row, not merely let the raw struct fall through unformatted.

The design's wire-shape example for `koto status` is:
```json
{
  "stale_template_source_dir": {
    "path": "/home/user/repo-that-was-deleted",
    "machine_id": "host-a",
    "note": "template source directory not found (if this session was synced from another machine, this may be expected)"
  }
}
```
where the `note` text branches on whether the session's backend is cloud-backed (Decision 2).

**Correction from plan review (Category B/C/D):** an earlier version of this issue independently added the `Backend::is_cloud()` accessor here (reasoning that it didn't exist yet on the `Backend` enum, only on the concrete `CloudBackend` struct at `cloud.rs:559-561`). That created a design-vs-plan contradiction (the design's own text at the time said no new accessor was needed) and, separately, a missing-dependency risk (Issue 5 also needs this accessor but doesn't depend on this issue). Per the corrected design and <<ISSUE:1>>, `Backend::is_cloud()` is now added once, in <<ISSUE:1>>, as foundational shared infrastructure -- this issue only *consumes* it (`Blocked by <<ISSUE:1>>`, already declared below), it does not define it.

This issue also carries the design's docs-coverage requirement: the design's Context section references `docs/guides/cloud-sync-setup.md` as the shipped feature that motivates Decision 2's backend-aware wording, which triggers the plan's docs-coverage-emit rule. Reading `docs/guides/cli-usage.md` directly shows there is no dedicated `### status` section documenting `koto status`'s JSON shape at all -- only a single bullet under "Batch surface on existing commands" (`koto status <parent>` -- read-only view..., around line 901). By contrast, `koto session list`'s JSON shape is fully documented under its own `#### session list` heading (around line 308-333) with a worked example. The documentation acceptance criterion below is written against this actual, uneven doc structure rather than assuming a symmetric "status" section exists to extend.

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

## Acceptance Criteria

- [ ] A shared wording-formatting helper function (e.g. `fn format_stale_template_source_note(is_cloud: bool) -> &'static str` or equivalent, placed in `src/cli/mod.rs` or a small shared location reachable from both `src/cli/mod.rs` and `src/cli/session.rs`) returns:
  - [ ] Softened wording acknowledging possible cross-machine resume when `is_cloud` is `true` (e.g. "template source directory not found (if this session was synced from another machine, this may be expected)"), matching the design's Decision 2 example text.
  - [ ] More direct wording (e.g. asserting the directory no longer exists, without the cross-machine hedge) when `is_cloud` is `false`.
  - [ ] Both `handle_status` and `handle_list` call this single helper rather than each hand-rolling their own note text, so the two surfaces cannot drift out of sync with each other.
- [ ] `handle_status` (`src/cli/mod.rs`, `fn handle_status`, currently ~line 4387) calls `check_template_source_dir` on the header it already has in scope (from its existing `backend.read_events(name)` call) and, only when the result is `Some(status)` with `status.exists == false`, adds a top-level conditional `stale_template_source_dir` key to the response, following the exact same present-only-when-relevant pattern already used for `batch` and `superseded_branches` in this same function (confirmed present in current source: both are added via `response["batch"] = ...` / `response["superseded_branches"] = ...` inside `if`/`if !....is_empty()` guards, never as an always-present nullable field). The key's value is an object with `path` (string), `machine_id` (string or null), and `note` (string, from the shared wording helper gated on `backend.is_cloud()`).
- [ ] When `check_template_source_dir` returns `None` (no `template_source_dir` recorded in the header) or `Some(status)` with `status.exists == true`, `handle_status`'s response contains no `stale_template_source_dir` key at all (not `null`).
- [ ] `handle_list` (`src/cli/session.rs`, `pub fn handle_list`, currently ~line 504) surfaces `template_source_status` for each session row in its JSON output. Since `SessionInfo` derives `Serialize` and will carry the field directly after `<<ISSUE:3>>` lands, verify (and if needed, adjust) that the raw `path`/`exists`/`machine_id` values are present per row for sessions where `template_source_status` is `Some`, and that the same shared wording helper's note text is attached alongside them for rows where `exists == false` (gated on the list's own backend via `backend.is_cloud()`, since the whole `koto session list` invocation runs against a single backend instance).
- [ ] Rows where `template_source_status` is `None`, or `Some` with `exists == true`, carry no note text (only the raw field, or an absent/null field per whatever shape `<<ISSUE:3>>` establishes -- this issue does not change that shape, only adds wording where relevant).
- [ ] `docs/guides/cli-usage.md` is updated to document the new behavior:
  - [ ] The `#### session list` section (around line 308-333) -- which already shows a full JSON example with `id`, `created_at`, `template_hash` -- is updated to mention the new `template_source_status` field (or the additional stale-note text), consistent with the existing worked-example style in that section.
  - [ ] The `koto status <parent>` bullet under "Batch surface on existing commands" (around line 901) -- the only existing prose in this file that mentions `koto status`'s output at all -- is extended (or a new short subsection is added) to describe the conditional `stale_template_source_dir` key, since no dedicated `### status` section with a JSON example exists today to extend instead.
  - [ ] The doc text notes the backend-aware wording distinction (local sessions get direct wording; cloud-synced sessions get softened wording per `docs/guides/cloud-sync-setup.md`'s cross-machine resume workflow) so operators reading the CLI guide understand why the same underlying condition can read differently across sessions.
- [ ] `cargo build` and `cargo test` pass.
- [ ] New or extended test(s) confirm: `handle_status`'s response omits `stale_template_source_dir` entirely for a session with no recorded `template_source_dir` and for one whose recorded directory exists; includes it with local (direct) wording for a `LocalBackend` session with a missing directory; includes it with cloud (softened) wording for a `CloudBackend` session with a missing directory; and `handle_list`'s output surfaces the same distinction per row.

## Dependencies

Blocked by <<ISSUE:1>>, <<ISSUE:3>>

## Downstream Dependencies

None (leaf node)
