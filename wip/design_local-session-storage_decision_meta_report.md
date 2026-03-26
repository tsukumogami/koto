<!-- decision:start id="session-meta-json" status="assumed" -->
### Decision: session.meta.json keep or drop

**Context**

The local session storage design specifies that `create()` writes a `session.meta.json` file containing `schema_version`, `id`, and `created_at`. This file serves as the existence marker for `exists()` and the metadata source for `list()`. The pragmatic reviewer challenged whether this file carries its weight, given that the StateFileHeader in `koto-<name>.state.jsonl` already contains all three fields.

The codebase confirms the duplication. `StateFileHeader` (src/engine/types.rs) stores `schema_version`, `workflow` (equivalent to `id`), `template_hash`, and `created_at`. The existing `find_workflows_with_metadata` function (src/discover.rs) already reads StateFileHeaders from state files to populate workflow listings. Adding session.meta.json creates a second metadata source that must stay synchronized with the state file for no additional capability.

**Assumptions**

- There will be no "recreate session without recreating the workflow" operation. Session and workflow share a 1:1 lifecycle. If this changes, session-specific metadata can be added at that point.
- The state file is always present when a valid session exists. `create()` is immediately followed by state file creation in `handle_init`, with no observable intermediate state during normal operation.

**Chosen: Drop session.meta.json**

Remove session.meta.json from the design entirely. The state file (`koto-<name>.state.jsonl`) becomes the sole indicator of a valid session. Implementation changes:

- `create()` creates the session directory but does not write session.meta.json.
- `exists()` checks for the state file: `self.base_dir.join(id).join(format!("koto-{}.state.jsonl", id)).exists()`.
- `list()` scans subdirectories for state files and reads `StateFileHeader` from the first line of each, mirroring how `find_workflows_with_metadata` works today.
- `SessionInfo.created_at` comes from `StateFileHeader.created_at`.
- No orphan detection logic needed. A directory without a state file is either transient (mid-creation) or corrupt (same risk exists with session.meta.json).

**Rationale**

Every field in session.meta.json duplicates data already in StateFileHeader: `id` duplicates the directory name and `StateFileHeader.workflow`, `created_at` duplicates `StateFileHeader.created_at`, and `schema_version` duplicates `StateFileHeader.schema_version`. The file adds a write on create, a read on exists/list, a JSON parse step, and a sync invariant between two files -- all for zero additional capability.

The arguments for keeping it don't hold up under scrutiny. The "O(1) exists check" argument is negligible -- checking file existence vs reading one line are both single I/O operations. The "cross-backend consistency" argument is speculative -- each backend implements `exists()` and `list()` through the trait, so they're free to use whatever mechanism fits. The "separate session vs workflow timestamps" argument assumes a lifecycle separation that doesn't exist in the design (session ID = workflow name, 1:1 mapping). The "schema versioning" argument solves a problem that hasn't occurred and may never occur, while StateFileHeader already carries a schema_version field.

YAGNI applies cleanly here. If session-specific metadata is ever needed (distinct from workflow metadata), it can be added at that point with full knowledge of the actual requirements.

**Alternatives Considered**

- **Keep session.meta.json as designed**: writes `{"schema_version": 1, "id": "...", "created_at": "..."}` on create. Rejected because every field duplicates StateFileHeader data, adding maintenance cost and a sync invariant for no demonstrated benefit.
- **Lightweight marker file**: an empty `.koto-session` file as an existence marker without metadata. Avoids duplication but still adds an extra artifact per session. Rejected because the state file already serves as the existence marker -- there's no gap to fill.

**Consequences**

- `create()` becomes simpler: mkdir only, no file write.
- `exists()` depends on the state file existing, which means exists() returns false between `create()` and the state file write in `handle_init`. This is a sub-millisecond window during initialization only, not observable by external callers.
- `list()` pays the cost of reading the first line of each state file. This is the same approach `find_workflows_with_metadata` already uses, and the cost is trivial for realistic session counts.
- If session-specific metadata is ever needed, a metadata file can be introduced at that point with clear requirements rather than speculative fields.
<!-- decision:end -->
