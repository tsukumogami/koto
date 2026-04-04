<!-- decision:start id="lineage-registration-discovery" status="assumed" -->
### Decision: Lineage Registration and Discovery

**Context**

koto needs a way to register parent-child relationships between workflows and surface them through the CLI. The exploration already decided on header-only lineage (parent_workflow: Option<String> in StateFileHeader), flat storage, and a parent.child naming convention as an ergonomic default. What remains is the concrete behavior: how init validates the parent reference, where exactly the metadata lives, how koto workflows exposes hierarchy, and whether naming is enforced.

The current handle_init() validates workflow names, checks for duplicates, creates session directories, and writes a header + two events. find_workflows_with_metadata() reads all headers through backend.list() and returns a flat JSON array of {name, created_at, template_hash}. Both LocalBackend and CloudBackend implement list() by reading headers, so parent info in the header is available without extra I/O.

**Assumptions**
- Cross-scope parent references (parent in a different repo's session directory) won't be needed. If they are, a qualified reference format would need to be added later.
- Session counts per repo remain small enough (tens, not thousands) that in-memory parent chain traversal for tree construction is acceptable.

**Chosen: Strict Validation, Header-Only, Rich Discovery, Convention-Only Naming**

**Parent validation:** `koto init --parent <name>` validates that the named parent workflow exists via `backend.exists(parent)`. If the parent doesn't exist, init fails with an error including the parent name and a suggestion to check `koto workflows`. This catches the most common failure mode -- typos in the parent name -- at the earliest possible point. If a parent was intentionally cleaned up before its children, the user can re-create it or init the child without --parent.

**Metadata storage:** parent_workflow goes in StateFileHeader only, with `#[serde(default)]` for backward compatibility. It is NOT duplicated in the WorkflowInitialized event payload. The header is written once and never modified, providing the same immutability guarantee as an event. Avoiding duplication keeps the event schema focused on its purpose (recording what happened during init) and prevents divergence between header and event values.

**koto workflows output:** Add a `parent_workflow` field (null when absent) to every WorkflowMetadata entry. Add two filter flags:
- `--roots`: show only workflows where parent_workflow is None
- `--children <name>`: show only workflows where parent_workflow equals the given name

No `--tree` flag. Agents can derive trees from the flat list + parent pointers. A `--tree` flag adds output format complexity (nested JSON) that doesn't match the flat model and would need its own output type.

**Naming:** Convention only. The parent.child naming pattern (e.g., `design.implement`) is documented and recommended but not enforced. `koto init --parent design custom-name` is valid. The parent_workflow header field is the authoritative link. This avoids coupling naming to metadata, preserves flexibility for cases where the convention doesn't fit (e.g., multiple children from different templates where descriptive names matter more than hierarchy in the name), and requires zero naming-validation code.

**tree_id:** Not included. It's derivable from parent chain traversal and adds a field that must be kept consistent across workflow lifecycle operations. If traversal performance becomes an issue, tree_id can be added later as a non-breaking header extension.

**Rationale**

Strict validation over warn-only or no-check: agent workflows benefit from failing fast. An agent that typos a parent name should get an immediate error, not silently create an orphaned child. The warn-only approach (Alternative 2) optimizes for a rare edge case (cleaned-up parent) at the cost of the common case (typo). No-check (Alternative 4) provides no safety net at all.

Header-only over header+event: the exploration already decided header-only lineage. Duplicating into the event adds code (modifying EventPayload, updating serialization, updating all event-creation call sites) for a data point that's already immutably recorded in the header. The event log's value is in capturing state transitions, not in echoing header fields.

Rich discovery flags over minimal or tree output: agents need to answer "what are the root workflows?" and "what children does X have?" frequently. Server-side filtering via --roots and --children is cleaner than requiring every agent to parse the full list and filter. A --tree flag (Alternative 4) introduces a second output format that agents must handle, while --roots/--children reuse the same flat array format with filtered results.

Convention-only naming over enforcement: enforcement (Alternative 3) creates a tight coupling between the name string and the parent metadata. If a workflow is renamed (future feature) or if the convention doesn't suit a use case, enforcement becomes a constraint rather than a convenience. The metadata field is the authoritative link; the naming convention is just ergonomics.

**Alternatives Considered**

- **Warn-Only Validation, Header+Event, Minimal Discovery**: Warn on missing parent instead of failing; duplicate parent in event payload; no filter flags. Rejected because warn-only misses the primary error case (typos), event duplication adds complexity without value, and no filter flags pushes work onto every agent.

- **Strict Validation, Header-Only, Enforced Naming**: Same as chosen except child names must start with `<parent>.`. Rejected because enforced naming couples the name string to metadata, reduces flexibility for edge cases, and requires additional validation logic that provides marginal benefit over convention.

- **No Validation, Header+Event, Tree Flag**: No parent existence check; duplicate in event; add --tree flag with nested JSON. Rejected because no validation catches zero errors, tree output adds format complexity, and tree_id adds a field that's derivable from existing data.

**Consequences**

What becomes easier: agents get immediate feedback on parent typos. Hierarchy queries (roots, children of X) are single CLI calls. Adding parent info to existing discovery paths requires only adding a field to three structs (SessionInfo, WorkflowMetadata, StateFileHeader).

What becomes harder: if a parent workflow is cleaned up before its children, those children can't use --parent to reference it (they'd need to be created without --parent, or the parent would need to be re-created first). This is an acceptable trade-off -- the cleanup-before-children pattern is unlikely in normal use and can be worked around.

What changes: koto workflows output gains a new field, breaking agents that do strict JSON schema validation. However, since koto workflows is consumed by agents that should tolerate extra fields, this is low-risk.
<!-- decision:end -->
