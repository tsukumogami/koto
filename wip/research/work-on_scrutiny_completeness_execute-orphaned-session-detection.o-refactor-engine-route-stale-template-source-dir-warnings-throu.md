# Scrutiny: Completeness

Issue 2 -- refactor(engine): route stale-template-source-dir warnings through the shared module.
Commit reviewed: a8944a3.

## Acceptance criteria vs. implementation

1. Per-tick probe (`~line 875-876`) calls `check_template_source_path` instead of
   `.map(|p| p.exists())` -- **met**. `template_source_status` is computed via
   `check_template_source_path(template_source_dir.as_deref())`; `template_source_dir_exists`
   is derived from `.map(|s| s.exists)`, preserving the `Option<bool>` type and value.
2. `spawn_ready_task`/`spawn_skip_marker_task`/`canonical_paths_tried`/
   `resolve_template_path_with_base_status` signatures unchanged -- **met**, verified via
   `git diff` (none of the four function signatures appear in the diff) and direct read of
   their current definitions (lines 1532-1544, 1594-1606, 1838-1843 in batch.rs;
   `resolve_template_path_with_base_status` lives in path_resolution.rs, which is untouched).
3. `emit_template_source_dir_warnings` builds from `Option<TemplateSourceStatus>` instead of
   bare `base_exists: Option<bool>` + its own `current_machine_id()` call -- **met**. New
   signature takes `Option<&TemplateSourceStatus>`; warning is built from `status.path`,
   `status.exists`, `status.machine_id.clone()`.
4. `path_resolution.rs` not modified -- **met**, confirmed via `git diff --name-only` showing
   only `src/cli/batch.rs` (plus an unrelated pre-existing Cargo.lock drift left uncommitted).
5. Wire format byte-for-byte unchanged -- **met**. `SchedulerWarning::StaleTemplateSourceDir`'s
   struct definition in `scheduler_warning.rs` is untouched; `path` is still built via
   `to_string_lossy().into_owned()`, `machine_id` is still `Option<String>`, `falling_back_to`
   still a `PathBuf`.
6. `path_resolution.rs`, `scheduler_warning.rs`, `batch.rs` existing test suites pass
   unmodified -- **met**, ran each explicitly (`cargo test --lib path_resolution`,
   `cargo test --lib scheduler_warning`) plus the full `cargo test` suite with zero failures
   and no test file diffs.
7. `cargo build`, `cargo test`, `cargo fmt --check`, `cargo clippy --lib -- -D warnings` all
   pass -- **met**, ran all four, all clean.

## Evidence verifiability

All claims are directly checkable from the commit diff (`git show a8944a3`) and command output
captured in this session. No claim depends on unverifiable assertions.

## Verdict

blocking_count: 0
advisory_count: 0

Every acceptance criterion has a corresponding, verifiable implementation change or verification
step. Nothing is missing.
