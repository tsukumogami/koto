# Lead: What isolation model prevents cross-hierarchy pollution?

## Findings

### Current Architecture

Sessions are stored under `~/.koto/sessions/<repo-id>/<session-name>/`, where `repo-id` is a 16-character hex hash of the canonicalized working directory (`src/session/local.rs:378`). Each session directory contains:
- `koto-<name>.state.jsonl` -- the append-only event log
- `ctx/` -- context store with manifest and content files

The `LocalBackend::list()` method (`src/session/local.rs:74-121`) iterates top-level directories under the base dir, checks for a matching state file, and reads the header. There is no filtering, grouping, or ownership metadata. The `StateFileHeader` (`src/engine/types.rs:8-21`) contains only `schema_version`, `workflow`, `template_hash`, and `created_at` -- no parent reference or tree identifier.

`find_workflows_with_metadata()` (`src/discover.rs:80-93`) delegates directly to `backend.list()` and maps results to `WorkflowMetadata` structs containing `name`, `created_at`, and `template_hash`. The `koto workflows` command (`src/cli/mod.rs:678-691`) serializes this list as JSON with no filtering options.

### Approach A: Metadata-based filtering

**Mechanism:** Add `parent: Option<String>` and `tree_id: String` fields to `StateFileHeader`. All state files remain flat as peer directories under `~/.koto/sessions/<repo-id>/`. `koto workflows` gains `--roots` (filter to workflows where `parent` is `None`) and `--tree <id>` or `--children <parent>` flags.

**3-level nesting:** Works naturally. Grandparent -> parent -> child each store their own `parent` field. Querying a tree requires scanning all sessions and filtering by `tree_id`, or walking `parent` chains.

**Parent cancellation/completion:** No structural impact. Child state files persist independently. The parent's terminal state doesn't affect children unless the parent agent explicitly cancels them.

**Discoverability:** All workflows remain visible in `koto workflows` (no flags). Debugging is straightforward -- you can see the full flat list and filter with `--tree` or `--roots`. Shell tools like `jq` can post-filter.

**Name collisions:** Two hierarchies can have a child named "validate" because session names are already globally unique within a repo-id scope. If two different parents both want a child called "validate", the second `koto init` would fail with "session already exists." This is a real problem -- you'd need naming conventions like `parent.child` or a namespace prefix.

**Cross-directory parents:** Not an issue. All sessions for a repo-id are stored in the same base directory regardless of which subdirectory `koto init` was called from (the working dir is only used for repo-id computation, not per-session).

**Impact on existing code:**
- `StateFileHeader` gains two optional fields (backward-compatible with schema_version bump)
- `SessionInfo` and `WorkflowMetadata` gain corresponding fields
- `list()` stays unchanged; filtering happens in `find_workflows_with_metadata()` or CLI layer
- `koto init` gains `--parent` flag; sets parent and inherits/generates tree_id
- Existing workflows (no parent, no tree_id) are treated as roots with an implicit single-node tree

### Approach B: Directory-based isolation

**Mechanism:** Child state files are stored in subdirectories: `~/.koto/sessions/<repo-id>/<parent>/<child>/koto-<child>.state.jsonl`. Discovery becomes recursive. Three levels: `<repo-id>/grandparent/parent/child/`.

**3-level nesting:** Works but discovery complexity increases. `LocalBackend::list()` currently only reads one level of directories. Making it recursive changes behavior for everyone, or you need a separate recursive list method.

**Parent cancellation/completion:** Cleanup of a parent directory would cascade-delete all children. This is either a feature (clean hierarchy teardown) or a hazard (accidental data loss).

**Discoverability:** Filesystem hierarchy mirrors logical hierarchy, making it intuitive to browse with `ls` or `find`. But `koto workflows` would need to display paths or indented trees instead of flat names.

**Name collisions:** Naturally resolved. Two parents can each have a child named "validate" because they live in different directories: `parent-a/validate/` vs `parent-b/validate/`.

**Cross-directory parents:** Not applicable; hierarchy is structural.

**Impact on existing code:**
- `LocalBackend` needs recursive directory walking in `list()`
- `session_dir()` must accept hierarchical paths, breaking the current `base_dir.join(id)` pattern
- `exists()`, `cleanup()`, `create()` all need path hierarchy awareness
- `CloudBackend` (S3) needs corresponding prefix changes for `s3_list_sessions()`
- `validate_session_id()` currently rejects `/` in names, which would need to change for hierarchical paths -- or use a different encoding
- Session IDs become ambiguous: is "parent/child" one ID or a hierarchy? This bleeds path semantics into the session model

### Approach C: Session-tree IDs

**Mechanism:** Each hierarchy gets a UUID-based tree ID generated at root creation. `koto init --parent` causes the child to inherit the parent's tree_id. State files stay flat. `koto workflows --tree <id>` filters by tree. No parent field needed for filtering, though one could still be stored for lineage queries.

**3-level nesting:** Works. All members of a tree share the same tree_id regardless of depth.

**Parent cancellation/completion:** Same as Approach A -- no structural coupling.

**Discoverability:** Requires knowing the tree ID to filter, which isn't human-friendly. You'd need `koto workflows --roots` first to find tree IDs, then `--tree <id>` to drill in. Or you always show tree_id in output so agents can parse it.

**Name collisions:** Same problem as Approach A. Session names are globally unique within repo-id scope. Two trees wanting a child named "validate" collide.

**Impact on existing code:**
- Similar to Approach A but without the parent field
- Loses lineage information (parent-child relationships within the tree) unless you also add parent
- Essentially Approach A minus the parent field, plus a generated tree_id instead of deriving grouping from parent chains

## Implications

**Approach A (metadata-based filtering) is the strongest fit.** It preserves backward compatibility, keeps the flat session model that both LocalBackend and CloudBackend rely on, and adds just two fields to the header. The name collision problem is real but solvable through naming conventions (`koto init --parent sprint-7 sprint-7.validate` generates the child name from a dot-separated hierarchy).

**Approach B (directory-based) has cascading impact.** Every `SessionBackend` method assumes a flat `base_dir/id/` layout. Making this hierarchical touches `create`, `exists`, `cleanup`, `list`, `session_dir`, `append_header`, `append_event`, `read_events`, and `read_header`. The CloudBackend's S3 prefix scheme would also need rethinking. The payoff -- natural filesystem isolation and free name scoping -- is attractive but the migration cost is high.

**Approach C (tree IDs alone) is a strict subset of Approach A** and doesn't add enough to justify its own category. If you want tree-scoped queries, add `tree_id` to Approach A. If you don't need them, parent chains in Approach A already let you derive tree membership by walking to the root.

The recommended path: **Approach A with a naming convention for children**. A parent named `sprint-7` spawns children like `sprint-7.validate`, `sprint-7.build`. The dot convention is already allowed by `validate_workflow_name()` (dots are valid characters per `src/discover.rs:47`). This gives natural namespacing without directory restructuring, and `koto workflows --children sprint-7` can filter by prefix or by the `parent` metadata field.

Whether to also include a `tree_id` depends on query patterns. If the parent agent needs to enumerate all descendants (not just direct children), a shared tree_id makes that a single filter instead of a recursive parent-chain walk. For 3 levels of nesting, either approach works fine. Tree IDs become more valuable at deeper nesting or when you need O(1) "is this workflow in my hierarchy?" checks.

## Surprises

1. **Sessions are already scoped per repo-id, not per working directory.** The `repo_id()` function (`src/session/local.rs:378-383`) hashes the canonicalized working directory, so all `koto` commands from the same project share the same session pool. This means hierarchical workflows in different projects are already naturally isolated -- the pollution problem is strictly within a single project.

2. **The CloudBackend merges local and S3 sessions in `list()`.** Any hierarchy metadata scheme needs to work across both backends. The S3 list (`s3_list_sessions`) currently returns just session IDs without metadata, so filtering by parent/tree would require downloading headers from S3 -- or storing hierarchy metadata in the S3 key prefix, which pushes toward Approach B's directory model for the cloud case.

3. **`validate_session_id()` (distinct from `validate_workflow_name()`) is used for backend operations.** It lives in `src/session/validate.rs` and may have different rules. Any naming convention that embeds hierarchy (like dots) needs to pass both validators.

## Open Questions

1. **Should the parent field reference a session name or a session + repo-id?** If we ever support cross-project hierarchies (a parent in project A spawning a child in project B), the parent identifier needs to be fully qualified. For now, same-project is likely sufficient.

2. **What happens to `koto workflows` default output?** Should it show all workflows (current behavior) or default to `--roots` once hierarchies exist? Changing defaults would break existing agents. Keeping the flat list means agents see unrelated workflows, which is the pollution the lead is investigating.

3. **Should `koto cleanup` cascade to children?** If a parent is cleaned up, should its children be automatically removed? This is a policy decision that affects both safety and convenience.

4. **How does the CloudBackend's S3 list handle hierarchy metadata?** Currently `s3_list_sessions()` returns IDs from S3 key prefixes without reading headers. Filtering by parent/tree on the S3 side would require either (a) encoding hierarchy in the S3 key prefix, or (b) downloading headers for all remote sessions during list operations.

5. **Does `validate_session_id()` in `src/session/validate.rs` allow dots?** If the dot-based naming convention (`parent.child`) is adopted, both validators must accept dots. `validate_workflow_name()` already does, but the session validator may differ.

## Summary

The current session model is completely flat with no hierarchy concept -- `StateFileHeader` has no parent or tree fields, `LocalBackend::list()` does a single-level directory scan, and `koto workflows` dumps all sessions unfiltered. Metadata-based filtering (adding `parent` and optionally `tree_id` to the header, plus `--roots`/`--children` flags) is the strongest approach because it preserves the flat storage model that both Local and Cloud backends depend on, while a dot-based child naming convention (`parent.child`) solves name collision without restructuring directories. The biggest open question is how the CloudBackend's S3 listing -- which currently returns bare session IDs without metadata -- would support parent/tree filtering without downloading every remote header.
