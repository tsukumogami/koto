---
complexity: testable
complexity_rationale: Touches a shared struct plus two distinct backend implementations with different population rules, and must preserve a specific `Option` idiom (not the sibling `String::new()` sentinel idiom) whose correctness downstream code depends on -- worth explicit test coverage, though the change itself has no new I/O or branching logic beyond a single conditional per backend.
---

## Goal

Add an additive `template_source_status: Option<TemplateSourceStatus>` field to `SessionInfo`, populated by `LocalBackend::list()` from the header it already holds in memory and left `None` by `CloudBackend::list()`'s remote-only placeholder rows.

## Context

koto's state-file header already records `template_source_dir`, and `<<ISSUE:1>>` introduces the shared `TemplateSourceStatus` type and `check_template_source_dir(header: &StateFileHeader) -> Option<TemplateSourceStatus>` helper in `src/engine/template_source_status.rs`. Today, `SessionInfo` (`src/session/mod.rs`) discards `template_source_dir` after reading each session's header: `LocalBackend::list()` (`src/session/local.rs:115-123`) builds a `SessionInfo` from `created_at`, `template_hash`, and `parent_workflow` only, and `CloudBackend::list()` (`src/session/cloud.rs:677-699`) delegates to `self.local.list()` for locally-known sessions and separately constructs placeholder rows for sessions that exist only in S3 (`created_at: String::new(), template_hash: String::new(), parent_workflow: None`, lines 688-694) since no header is available to extract real values from.

This issue wires the shared helper into both `list()` implementations so `koto session list` (a later issue) can surface staleness without any new I/O. The design is explicit that `SessionInfo`'s new field must use `Option` semantics matching the existing `parent_workflow: Option<String>` field -- not the `String::new()` sentinel idiom `created_at`/`template_hash` use -- because `CloudBackend`'s placeholder rows have no header to check at all; getting this idiom wrong would make every remote-only placeholder row misfire the staleness check regardless of any wording layered on top downstream. The design also explicitly accepts, rather than resolves, an ambiguity this leaves behind: a `None` on `CloudBackend` rows can mean either "no `template_source_dir` was ever recorded" or "no header available to check" -- this issue's job is to document that limitation via a doc comment, not to resolve it.

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

## Acceptance Criteria

- [ ] `SessionInfo` in `src/session/mod.rs` gains a new public field:
  ```rust
  pub template_source_status: Option<TemplateSourceStatus>,
  ```
  added alongside the existing `id: String`, `created_at: String`, `template_hash: String`, and `parent_workflow: Option<String>` fields. It uses the same `Option` idiom as `parent_workflow` (bare `Option<T>`, no sentinel), not the `String::new()`-as-"unknown" convention `created_at`/`template_hash` use.
- [ ] The new field carries a doc comment stating that `None` is ambiguous between "no `template_source_dir` was ever recorded in the header" and "no header is available to check" (the latter applies specifically to `CloudBackend`'s remote-only placeholder rows), and that this ambiguity is a known, accepted limitation per the design's Consequences/Mitigations section, not a bug.
- [ ] `SessionInfo` continues to derive/implement whatever it already does (currently `#[derive(serde::Serialize)]`) with the new field included in serialization; no existing derive or trait impl is removed.
- [ ] `LocalBackend::list()` (`src/session/local.rs`, the `Ok(header) => { results.push(SessionInfo { ... }) }` branch around lines 115-123) populates `template_source_status` by calling `check_template_source_dir(&header)` on the header already read into memory for that iteration -- no new file reads, no new `Path::exists()` calls beyond what the helper itself performs.
- [ ] `CloudBackend::list()` (`src/session/cloud.rs:677-699`):
  - [ ] The remote-only placeholder-row construction (the `local_sessions.push(SessionInfo { id: remote_id, created_at: String::new(), template_hash: String::new(), parent_workflow: None })` block, lines 688-694) explicitly sets `template_source_status: None`, since no header exists for these rows to check.
  - [ ] Rows produced via the existing `self.local.list()` delegation (line 678) require no additional change in this method -- they already carry a correctly-populated `template_source_status` from `LocalBackend::list()`'s change above, satisfying "populate normally for rows already synced locally."
- [ ] `cargo build` and `cargo test` pass with no modification to any existing test's expected values (this is a purely additive field; no existing `SessionInfo` construction site elsewhere in the repo needs updating beyond adding `template_source_status` to satisfy the struct literal, e.g. any test fixtures that construct `SessionInfo` directly).
- [ ] New or extended unit test(s) confirming: a `LocalBackend::list()` session with a `template_source_dir` recorded and existing on disk reports `Some(TemplateSourceStatus { exists: true, .. })`; one with a recorded-but-missing directory reports `Some(TemplateSourceStatus { exists: false, .. })`; one with no `template_source_dir` in its header reports `None`; and a `CloudBackend::list()` remote-only placeholder row reports `None` for `template_source_status` regardless of what any real session with that ID might have recorded.

## Dependencies

Blocked by <<ISSUE:1>> (needs `TemplateSourceStatus` and `check_template_source_dir` to exist in `src/engine/template_source_status.rs` before this issue can call them from `src/session/local.rs`).

## Downstream Dependencies

- <<ISSUE:4>> (`feat(cli): surface stale template_source_dir on koto status and koto session list`) needs `SessionInfo.template_source_status` populated correctly by `LocalBackend::list()`, with the `Option` semantics preserved exactly as specified here, so `handle_list` (`src/cli/session.rs`) can read the field straight off each row and project it into JSON output with no new I/O of its own.
