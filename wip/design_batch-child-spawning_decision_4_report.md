<!-- decision:start id="child-template-path-resolution" status="assumed" -->
### Decision: Child template path resolution for the batch scheduler

**Context**

Today, `koto init --template <path>` resolves the template path relative to the process cwd at invocation time (`src/cli/mod.rs` line 1074 calls `compile_cached(Path::new(template), false)`). Only the *compiled* cache path under `~/.cache/koto/<hash>.json` is persisted into the `WorkflowInitialized` event; the original source path is not retained in the state file or header. For interactive init, this is fine -- the agent that runs `koto init` is standing in the directory where the template lives.

The batch scheduler introduces a separation in time and place. A parent workflow submits a task list via `koto next parent --with-data @file.json`, where each entry carries a `template: <path>` field. The scheduler materializes those entries into child workflows possibly during a later `koto next parent` invocation, from a different working directory, on a different day, and (under cloud sync) on a different machine. The child itself then runs under `koto next child` from yet another cwd. Three cwds, one path.

The child-template path must resolve identically regardless of which cwd the scheduler happens to be in, survive a resume, and -- ideally -- survive a machine hop under cloud sync without forcing the author to hardcode absolute filesystem paths.

**Assumptions**

- `std::env::current_dir()` in `handle_next` (line 1312) is already captured but is not currently threaded into template resolution. Using it for batch submissions is an additive change, not a refactor of existing behavior.
- Cloud-synced workflows sync the session directory (state file, context store) but do **not** sync the template cache at `~/.cache/koto/`. Templates are either re-read from source on machine B or must be retrievable by path.
- Agents running shirabe-style work-on-plan batches typically have a stable repo layout where coordinator template and child template live in the same directory or a small known subtree (e.g., `plugins/shirabe-skills/skills/work-on/koto/`).
- Existing `.koto/config.toml` and `~/.koto/config.toml` provide a natural extension point for configuration surface. Adding a new config key is cheap; adding a new env var is cheaper.
- Users will not accept hardcoded absolute paths in template files checked into a repo.
- The batch scheduler can be given access to `machine_state.template_path` (the parent's cached template), and the header can be extended with a new optional field without breaking the v1 schema (existing fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`).

**Chosen: Parent's template source directory as the base, with submitter-cwd fallback captured in the evidence event**

When the scheduler materializes a child-spawn task entry, it resolves the `template` field using this search order:

1. **Absolute paths pass through unchanged.** If `template` starts with `/`, use it as-is. This remains the escape hatch for unusual deployments.
2. **Relative paths resolve against the parent's template source directory.** When `koto init --template parent.md` runs, extend `handle_init` to canonicalize the source path via `std::fs::canonicalize(Path::new(template))` and store the resulting absolute directory in a new optional header field `template_source_dir: Option<String>`. The batch scheduler joins relative `template` values against this directory. For a parent initialized as `koto init coord --template /home/user/repo/workflows/coord.md`, a task entry `{"template": "impl.md"}` resolves to `/home/user/repo/workflows/impl.md`, and `{"template": "../shared/review.md"}` resolves to `/home/user/repo/shared/review.md`.
3. **Submitter cwd as diagnostic fallback.** Extend the `EvidenceSubmitted` event payload with an optional `submitter_cwd: Option<String>` field captured from the existing `std::env::current_dir()` call in `handle_next` (line 1312). If step 2 fails to locate the template file (ENOENT on the joined path), the scheduler tries `submitter_cwd.join(template)` before erroring out. This handles the edge case where the parent was initialized from a different cwd than the one where the batch was submitted (e.g., `koto init` in repo root, `koto next --with-data @batch.json` from `workflows/`).
4. **On failure, error with all attempted paths.** If neither resolution finds the file, emit a structured error listing both the parent-template-dir attempt and the submitter-cwd attempt so the agent can correct the task list.

To make this work, three additive changes are required:

- `StateFileHeader` gains `template_source_dir: Option<String>` (skip_serializing_if = "Option::is_none"), populated during `handle_init` from `std::fs::canonicalize(Path::new(template)).parent()`.
- `EventPayload::EvidenceSubmitted` gains an optional `submitter_cwd: Option<String>` populated when the batch scheduler recognizes the evidence as a task-list submission (the payload size limit already bounds risk).
- The batch scheduler's child-spawn materialization reads both fields and applies the resolution order above.

**Rationale**

This choice optimizes for the common case that dominates shirabe-style workflows: a parent template and its child templates live in the same repo, usually the same directory. Under that assumption the entire resolution collapses to "join relative paths against the directory containing the parent template file," which is (a) portable across machines as long as the repo layout is stable, (b) resume-safe because the base directory is persisted in the header at init time and never re-read from the environment, and (c) ergonomic because authors write `"template": "impl.md"` next to their `coord.md` and it just works.

The submitter-cwd fallback catches the one realistic failure mode this model misses: the parent template lives in `/repo/workflows/coord.md` but the author runs `koto next --with-data @batch.json` from `/repo/` with a task list that says `"template": "workflows/impl.md"`. Without the fallback, the parent-template-dir lookup would try `/repo/workflows/workflows/impl.md` and fail. With the fallback, the second attempt against submitter cwd finds `/repo/workflows/impl.md`. The cost is storing one extra path string per evidence event, and the scheduler has to try two paths before erroring.

Cloud sync works because both `template_source_dir` and `submitter_cwd` are filesystem paths, but **they point to content in the repo**, not to koto's local cache. On machine B, as long as the same repo is checked out at the same absolute path (which is the assumption for any cloud-synced workflow that also edits code), both paths resolve identically. This is the same portability constraint koto already places on users today -- the current init behavior is also non-portable across machines with different layouts. No new portability restriction is introduced; what changes is that we get portability for free in the common case (same repo, same path on both machines) instead of getting it nowhere.

Against the constraint list:

| Constraint | Met? | How |
|------------|------|-----|
| Works for single-invocation batches | Yes | parent_template_dir is always set at init time |
| Works for resumed batches from a different cwd | Yes | resolution reads persisted state, never live cwd |
| No hardcoded absolute paths required | Yes | relative paths work in the common case |
| Composes with cloud sync (same path both machines) | Yes | repo path is stable across machines under cloud sync |
| Compatible with existing `handle_init` path resolution | Yes | existing behavior is preserved; a new field is populated alongside |

**Alternatives Considered**

- **Option 1: Absolute paths only.** Simple, bulletproof for resume, but brittle and hostile to cloud sync across machines with different home directories or repo paths. Forces every task entry to know filesystem layout. Rejected because it moves the portability burden from koto onto the author, where it is impossible to discharge correctly for a workflow that must run on multiple machines.
- **Option 2: Submitter cwd only, captured at submission time.** Simpler to implement than the chosen option and correct for the single-invocation common case. Rejected as the primary mechanism because the whole point of the batch scheduler is to defer materialization -- if the scheduler runs later from a different cwd, submitter cwd is the *right* base but only because it happens to coincide with parent-template-dir in the common case. Elevating it to the primary mechanism gives up the portability win when a subsequent `koto next` invocation submits *additional* batches from a different cwd, and mixing two bases silently. Kept as a fallback, not the primary path.
- **Option 3: Relative to parent's session dir.** Forces every batch to bundle templates under `<parent session>/templates/`. Breaks the common case where multiple parents share the same child template file, and would require agents to `cp` templates into place before submitting. Rejected on ergonomics.
- **Option 4: `KOTO_TEMPLATE_ROOT` env var or config field.** Portable if set consistently, and the config infrastructure already exists. Rejected as the primary mechanism because it adds a configuration burden to every agent that runs a batch (and to CI, to cloud machines, to every cwd). A single workspace-level config key is plausible as a future extension and compatible with the chosen design -- if `template_source_dir` is unset (e.g., for a parent workflow that itself originated from a template without a local file), fall back to `$KOTO_TEMPLATE_ROOT`. This is a future extension, not a day-one requirement.
- **Option 6: Named lookup via registry config.** Biggest change, most portable, but requires installing a registry, naming every template, maintaining the registry across machines, and shifting mental models from "file path" to "named recipe" (tsuku-style). Rejected as disproportionate to the problem: the batch scheduler needs to spawn a handful of templates per workflow, not curate a shared library across dozens of teams. If such a library becomes a real need, a named-lookup layer can be added later without invalidating path-based resolution.

**Consequences**

Easier:
- Shirabe work-on-plan batches work end-to-end with `"template": "impl.md"` next to `coord.md`, with no new config.
- Resumed batches and multi-machine cloud sync keep working as long as the repo path is stable, which is already a koto assumption.
- Migration from the current CLI is zero-change for existing `koto init --template <path>` callers: the new header field is populated silently and existing workflows that don't touch the batch scheduler never read it. Old state files missing `template_source_dir` deserialize fine via `#[serde(default)]`.

Harder:
- `handle_init` grows a `canonicalize()` call that may fail on non-existent parent directories (mitigated: we already require the template file to exist, so the parent directory exists by construction; canonicalize should succeed or the init already failed earlier).
- The batch scheduler must implement two-pass resolution (parent-template-dir then submitter-cwd) and emit a structured error with both attempts on failure. This is a small amount of new logic but needs test coverage for both the hit and the fallback paths.
- The event log schema gains one optional field on `EvidenceSubmitted` (`submitter_cwd`), adding a small amount of noise to state files for non-batch evidence submissions. Can be scoped to only batch submissions if noise becomes a concern.
- A parent workflow whose header predates this change (migrated from an older koto version) will have `template_source_dir: None`. The batch scheduler must degrade gracefully -- fall back to submitter_cwd only, or error with a clear "this workflow was initialized before batch support existed, re-init to use batches" message.

**Migration note**

This change is **backward compatible** for all non-batch uses of `koto init --template`. The behavior of resolving the template path at invocation time remains unchanged; only an additional header field is populated. Existing state files continue to load without the new field via `#[serde(default)]`. No user-visible change in `koto init`, `koto next`, `koto rewind`, or `koto query` for workflows that don't use the batch scheduler. Users who eventually want to add `KOTO_TEMPLATE_ROOT` as a third fallback can do so in a future change without touching this resolution order.

**Concrete example: shirabe work-on-plan batch**

Repo layout:
```
/home/user/repo/
├── plugins/shirabe-skills/skills/work-on-plan/koto/
│   ├── coord.md          # parent workflow template
│   ├── impl-issue.md     # child: implement one issue
│   └── review-pr.md      # child: review the resulting PR
└── src/
```

Day 1, agent runs from `/home/user/repo/`:
```bash
koto init work-on-plan-42 --template plugins/shirabe-skills/skills/work-on-plan/koto/coord.md
```

`handle_init` resolves `plugins/...coord.md` against cwd `/home/user/repo`, canonicalizes to `/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/coord.md`, and stores:
```json
{"schema_version":1,"workflow":"work-on-plan-42","template_source_dir":"/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto",...}
```

Day 1, parent submits a batch from `/home/user/repo/` (cwd captured in evidence):
```bash
koto next work-on-plan-42 --with-data '@batch.json'
# batch.json contains:
# {"children": [
#   {"template": "impl-issue.md", "name": "issue-101"},
#   {"template": "impl-issue.md", "name": "issue-102"},
#   {"template": "review-pr.md",  "name": "review"}
# ]}
```

The `EvidenceSubmitted` event records `submitter_cwd: "/home/user/repo"`.

Day 2, scheduler materializes children (possibly invoked from `/tmp` or `/home/user/repo/src/`):
- Reads `template_source_dir` from the header: `/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto`.
- Joins `impl-issue.md` -> `/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/impl-issue.md`. Found. Spawn child.
- Same for `review-pr.md`. Found. Spawn child.

Day 3, on machine B (cloud-synced, same repo at same path): identical resolution, works identically.

Alternate path: batch submitted from `/home/user/repo/` but with entries `{"template": "plugins/shirabe-skills/skills/work-on-plan/koto/impl-issue.md"}`. Parent-template-dir attempt yields `/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/plugins/shirabe-skills/skills/work-on-plan/koto/impl-issue.md` (ENOENT). Fallback tries `submitter_cwd.join(...)` -> `/home/user/repo/plugins/shirabe-skills/skills/work-on-plan/koto/impl-issue.md`. Found. Spawn succeeds, scheduler emits a warning that the task entry could be simplified.
<!-- decision:end -->

---

## Decision Result Summary

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Parent's template source directory as primary base, submitter cwd as fallback"
  confidence: "medium"
  rationale: >-
    Optimizes for the dominant shirabe-style layout where parent and child templates
    share a directory, gives portability under cloud sync for free when repo paths
    are stable (already a koto assumption), and degrades gracefully via a submitter-cwd
    fallback captured in the evidence event. Additive schema changes only; fully
    backward compatible with existing `koto init --template` behavior.
  assumptions:
    - "Shirabe-style batches live in one repo directory; parent and children colocated"
    - "Cloud-synced workflows run on machines with identical repo checkout paths"
    - "handle_next's existing std::env::current_dir() capture can be threaded into EvidenceSubmitted"
    - "StateFileHeader can gain an optional template_source_dir field without schema version bump"
    - "std::fs::canonicalize on the template source succeeds during init (file already exists)"
  rejected:
    - name: "Absolute paths only"
      reason: "Moves portability burden to author; impossible for multi-machine workflows"
    - name: "Submitter cwd as primary base"
      reason: "Coincides with parent-template-dir only by accident; breaks on delayed submissions from other cwds"
    - name: "Parent session dir bundling"
      reason: "Breaks the common case where multiple batches share one template file"
    - name: "KOTO_TEMPLATE_ROOT env/config"
      reason: "Adds config burden to every cwd and every machine; viable as future fallback, not primary"
    - name: "Named template registry"
      reason: "Disproportionate to the problem; tsuku-style recipe layer is a different scale"
  migration_note: >-
    Backward compatible. Existing koto init --template behavior is unchanged.
    New optional header field template_source_dir is populated silently; old state
    files deserialize via #[serde(default)]. Workflows that don't use the batch
    scheduler never observe the new field. Old parent workflows without the field
    degrade to submitter_cwd-only resolution with a warning.
  report_file: "wip/design_batch-child-spawning_decision_4_report.md"
```
