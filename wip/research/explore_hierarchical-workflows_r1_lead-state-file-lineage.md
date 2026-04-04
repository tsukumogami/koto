# Lead: How should koto init --parent change the state file and event log?

## Findings

### Current State File Structure

State files are JSONL, stored at `~/.koto/sessions/<repo-id>/<workflow-name>/koto-<workflow-name>.state.jsonl`. The first line is a `StateFileHeader` (defined in `src/engine/types.rs`):

```json
{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123","created_at":"2026-03-15T14:30:00Z"}
```

Header fields today: `schema_version` (u32), `workflow` (String), `template_hash` (String), `created_at` (String). No optional fields, no parent reference.

After the header, `koto init` writes two events:
1. `workflow_initialized` (seq 1) with `template_path` and `variables`
2. `transitioned` (seq 2) with `from: null`, `to: <initial_state>`, `condition_type: "auto"`

### Session Storage Layout

Sessions live in `~/.koto/sessions/<repo-id>/` where repo-id is a hash of the canonicalized working directory (`src/session/local.rs`). Each session is a subdirectory named by the workflow ID:

```
~/.koto/sessions/<repo-id>/
  alpha/
    koto-alpha.state.jsonl
  beta/
    koto-beta.state.jsonl
```

Discovery (`src/session/local.rs`, `list()`) iterates directories under `base_dir`, checks each for a matching state file (`koto-<dir-name>.state.jsonl`), reads the header, and returns `SessionInfo { id, created_at, template_hash }`.

The `koto workflows` command (`src/cli/mod.rs:678`) calls `find_workflows_with_metadata()` in `src/discover.rs`, which delegates to `backend.list()` and maps the results to `WorkflowMetadata { name, created_at, template_hash }`.

### Option A: Header-Only (`parent_workflow: Option<String>`)

**Change**: Add `parent_workflow: Option<String>` to `StateFileHeader`.

**Pros**:
- Minimal change: one field added to the header struct. Serde handles `Option` with `#[serde(skip_serializing_if = "Option::is_none")]` for backward compatibility.
- Header is read by `read_header()` in persistence.rs, already used by `list()` -- parent info is available during discovery without reading the full event log.
- The child's lineage is durable: the header is the first line, written once, never mutated.
- `koto workflows` can easily add a `parent` field to its JSON output by threading the new field through `WorkflowMetadata`.
- Querying "children of X" requires scanning all workflow headers, which `list()` already does.

**Cons**:
- The parent doesn't know it has children. Finding children requires scanning all headers and filtering by `parent_workflow == Some("parent-name")`.
- No event on either side captures the moment the child was spawned. The relationship exists only in static metadata.

**Implementation complexity**: Low. Changes to `StateFileHeader`, `WorkflowMetadata`, `SessionInfo`, `handle_init` (accept `--parent` flag), and `koto workflows` output.

**Backward compatibility**: `schema_version` remains 1 since `Option<String>` with `#[serde(default)]` deserializes from old files without the field as `None`.

### Option B: Dual Event (Header + Parent Event)

**Change**: Child header gets `parent_workflow: Option<String>` (same as A), and additionally a new `ChildWorkflowSpawned` event is appended to the parent's state file.

**Proposed event variant**:
```rust
ChildWorkflowSpawned {
    child_workflow: String,
    child_template_path: String,
    state: String, // parent's current state when child was spawned
}
```

**Pros**:
- Bidirectional: parent knows about its children, child knows its parent.
- The parent's event log records when and in which state the child was spawned -- useful for replay/audit.
- Finding children of a parent is a log scan of one file (the parent), not all workflows.

**Cons**:
- `koto init --parent <name>` now writes to two state files atomically: the child's new file and the parent's existing file. If the child write succeeds but the parent append fails, the relationship is inconsistent.
- The parent's event log grows with events that aren't state transitions -- this is fine architecturally (evidence and decisions already do this), but it introduces a cross-session write dependency.
- The `ChildWorkflowSpawned` event needs a new `EventPayload` variant, a new deserialization branch in `Event::deserialize`, and a new type-name string. Moderate code churn in types.rs.
- If the parent is cancelled or cleaned up, the child's header still references it, creating a dangling parent reference. The parent's child-spawn events are lost.

**Implementation complexity**: Medium. All of Option A's changes plus: new `EventPayload` variant, serialization/deserialization, cross-session write in `handle_init`, and the atomicity question.

### Option C: Directory Nesting

**Change**: Child state files are stored in a subdirectory under the parent's session directory.

```
~/.koto/sessions/<repo-id>/
  parent-wf/
    koto-parent-wf.state.jsonl
    child-wf/
      koto-child-wf.state.jsonl
```

**Pros**:
- Lineage is visible in the filesystem. Cleanup of a parent can cascade to children.
- Finding children is a directory listing.

**Cons**:
- Breaks the current flat discovery model. `LocalBackend::list()` iterates top-level directories only. Nested discovery requires recursive traversal, which changes the performance characteristics and the contract of `list()`.
- The `SessionBackend` trait's `exists()`, `session_dir()`, `create()` all assume a flat namespace (`base_dir.join(id)`). Nesting means the session path depends on the full lineage chain, not just the session ID. This is a deep API change.
- Cloud backend (`CloudBackend`) mirrors the local structure. Nesting would need to propagate through S3 key prefixes.
- Workflow names must now be unique only within their parent scope, not globally. This changes the semantics of `koto init <name>` and `koto next <name>`.
- Multi-level nesting (grandchildren) compounds the path resolution problem.
- Breaks the naming convention: `state_file_name()` and `workflow_state_path()` in discover.rs assume flat layout.

**Implementation complexity**: High. Requires reworking `SessionBackend` trait, `LocalBackend`, `CloudBackend`, discovery, and all CLI commands that take a workflow name.

### Option D: Combination (Header + Parent Event + Flat Directory)

**Change**: Child header gets `parent_workflow: Option<String>`, parent gets a `ChildWorkflowSpawned` event, but all state files stay in the flat directory structure.

This is Option B with an explicit decision to keep flat storage.

**Pros**:
- All benefits of B (bidirectional, auditable) without the discovery/API disruption of C.
- `koto workflows` can present a tree view by reading headers (parent field) and optionally enriching with child-spawn events from parent logs.
- Flat storage keeps the SessionBackend trait stable.

**Cons**:
- Same atomicity concern as B (writing to two files).
- Same dangling-parent concern as B.
- Slightly more complex than A, but the parent event provides real value for audit and querying.

**Implementation complexity**: Medium (same as B).

## Implications

1. **Option A is the minimum viable lineage.** It's simple, backward-compatible, and handles the most common query pattern ("what is this workflow's parent?"). If the parent-side query ("list my children") can tolerate scanning all headers -- and it can, since `list()` already does this -- then Option A is sufficient for the initial implementation.

2. **Option B/D adds value only when the parent needs to know about children without scanning all workflows.** This matters if the number of workflows grows large, or if the spawning moment (which parent state?) needs to be recorded. For the agent use case described (parent queries child state to inform transitions), the parent agent already knows the child name because it spawned it. The parent event is mainly useful for crash recovery or different-agent-resumes-parent scenarios.

3. **Option C is a non-starter.** The disruption to the session backend API, discovery, naming, and both backends makes it disproportionately expensive relative to the problem it solves. Flat storage with metadata-based lineage achieves the same logical relationships without filesystem coupling.

4. **The `schema_version` field can stay at 1.** Adding an optional field to the header with `#[serde(default)]` is backward-compatible. Old koto binaries will ignore the unknown field; new binaries will read old files as `parent_workflow: None`.

5. **The `koto workflows` output should include `parent` in its JSON.** This lets agents discover the workflow tree with a single command. Adding a `--tree` flag to render hierarchically is a nice-to-have but not required initially.

6. **`WorkflowInitialized` event is another candidate for the parent reference** instead of the header. Advantage: it's an event, fits the append-only log model. Disadvantage: you'd need to read event seq 1 during discovery (currently only the header is read). The header is the right place because it's already read during listing.

## Surprises

1. **Sessions live in `~/.koto/sessions/<repo-id>/`, not in the working directory.** The CLAUDE.md says state files live as `koto-<name>.state.json` in the working directory, but the actual implementation uses a hashed repo-id directory under `~/.koto/`. This means `koto workflows` doesn't glob the working directory -- it reads from the centralized session store. The state file naming convention (`.state.jsonl`, not `.state.json`) also differs from the docs. This doesn't change the lineage analysis, but it means directory nesting (Option C) would be nesting inside `~/.koto/sessions/`, not the working directory.

2. **The `SessionBackend` trait has no concept of relationships between sessions.** Every method takes a single `id: &str`. There's no `list_children()` or `parent_of()`. Any relationship must be derived from data inside the state files (header or events). This favors the header approach since the header is already extracted during `list()`.

3. **The cloud backend mirrors local structure.** Adding lineage must work for both backends. Header-based lineage (Option A) requires no backend trait changes. Event-based lineage (Option B/D) requires the cross-session write to work through the trait, which it already can -- `append_event` takes any session ID.

## Open Questions

1. **Should `koto init --parent` validate that the parent workflow exists?** The parent might be in a different repo-id scope (different working directory). Should cross-repo parentage be supported, or is it always same-scope?

2. **What happens when a parent workflow is cleaned up but children still reference it?** Should cleanup be blocked, cascade to children, or just leave dangling references?

3. **Should `koto workflows` output change to include parent/children information by default, or only with a flag like `--tree`?** Adding `parent` to the JSON output is low-risk, but changing the output shape could break existing consumers.

4. **Does the parent agent need `koto` to manage child lifecycle (spawn, monitor, cleanup), or does the agent handle that directly via `koto init` / `koto next` / `koto session cleanup` calls?** This affects whether koto needs new commands like `koto children <parent>` or if existing commands plus the header field are sufficient.

5. **Should `parent_workflow` go in the header, the `WorkflowInitialized` event payload, or both?** Putting it only in the header keeps discovery fast. Putting it in the event payload keeps the log self-describing. Both is redundant but practical.

## Summary

Option A (adding `parent_workflow: Option<String>` to `StateFileHeader`) is the clear starting point -- it requires minimal code changes (header struct, init flag, workflows output), is fully backward-compatible without bumping schema_version, and satisfies the primary query patterns since `list()` already reads all headers. Option B's parent-side `ChildWorkflowSpawned` event adds audit value but introduces cross-session write atomicity concerns that should be deferred until crash-recovery requirements are concrete. The biggest open question is whether `koto init --parent` should validate that the parent exists and whether cross-repo-scope parentage needs to be supported, since the session backend scopes sessions by a hashed repo-id.
