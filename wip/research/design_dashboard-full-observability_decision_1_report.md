<!-- decision:start id="dashboard-session-discovery-scope" status="assumed" -->
### Decision: Dashboard Session Discovery Scope

**Context**

`LocalBackend::new(working_dir)` derives a SHA-256 repo-id hash from the canonicalized working directory and roots all session I/O at `~/.koto/sessions/<repo-id>/`. Every command — engine commands (init, next, rewind) and the dashboard alike — goes through a single `build_backend()` function in `src/cli/mod.rs`, which calls `build_local_backend()`, which calls `LocalBackend::new(&std::env::current_dir())`. This means `koto dashboard` run from `/workspace-a` sees only workspace-a sessions; sessions in other repos are invisible.

PRD R18 requires the dashboard to discover all sessions on the local machine regardless of cwd. The F5 milestone will extend scope to S3-backed cloud storage. Any solution must work with the existing `SessionBackend` trait (which exposes `list()`, `session_dir()`, `cleanup()`) and must not break the state file format or session naming convention.

The `CloudBackend` also uses repo-id today — once as the local `LocalBackend` sub-directory and again as the S3 object prefix. So cloud is also per-repo scoped, not globally scoped as the problem statement implied.

**Assumptions**

- Session name collisions across repos are rare enough that abandoning repo-id scoping does not cause practical data loss. If two repos use the same user-defined session name (e.g., both use `task_issue-1`), they will occupy the same directory after migration. A migration helper must detect and refuse to merge collisions.
- Existing sessions stored under `~/.koto/sessions/<repo-id>/` are migrated by an automated, collision-safe helper that runs on first startup under the new layout. If migration is skipped, existing sessions become invisible to the new layout.
- The F5 cloud backend will be designed from the start with a global S3 prefix model (no repo-id in the S3 key path), consistent with this decision.

**Chosen: Remove repo-id scoping entirely (Option 4)**

Change `LocalBackend::new()` to always set `base_dir = ~/.koto/sessions/` with no repo-id segment. All sessions on the machine share one flat namespace at `~/.koto/sessions/<session-name>/`. Both engine commands and the dashboard use the same backend, the same directory, and the same `list()` implementation without changes.

Add a one-time migration helper: on the first run where the old per-repo layout is detected (`~/.koto/sessions/<16-hex-chars>/` subdirectories), move each session directory up one level. Sessions with naming collisions across repos are flagged and left in place under their old repo-id path with a warning printed to stderr.

`CloudBackend` is updated to drop the repo-id S3 prefix at F5 time, consistent with this decision.

**Rationale**

Options 1 (scope parameter) and 2a (global constructor with per-repo storage) both require `list()` to handle a two-level directory tree for the global case (`~/.koto/sessions/<repo-id>/<session>/`) versus the per-repo case (`~/.koto/sessions/<repo-id>/<session>/` as seen locally). The topology difference means the global backend cannot reuse `list()` as-is and must walk one extra directory level, returning compound paths or silently flattening names. This inconsistency is a latent bug surface.

Option 3 (config-driven scope) adds a user-facing config key for something that should be structural and automatic. The dashboard should always show all sessions; a config toggle inverts that by making users opt in. It also does not resolve the topology problem.

Option 4 eliminates the topology problem entirely. `list()` already reads all subdirs of `base_dir`; setting `base_dir = ~/.koto/sessions/` requires no change to `list()`, `session_dir()`, `cleanup()`, or `dashboard_data.rs`. The only code change is in `LocalBackend::new()` (remove the `repo_id` join), `CloudBackend::new()` (align at F5 time), and `build_local_backend()` (no longer needs to call `current_dir()`). A migration helper covers existing data.

Repo-id scoping was always an extra isolation layer, not the primary key. User-defined session names already provide the practical uniqueness guarantee within any given repo. The `KOTO_SESSIONS_BASE` environment variable used in tests shows the code already treats the base directory as a pluggable parameter.

**Alternatives Considered**

- **Scope parameter on LocalBackend** (`scope: Option<RepoId>`): Rejected because the two scopes have incompatible directory topologies (`~/.koto/sessions/` contains repo-id subdirs, not session subdirs), requiring `list()` to recurse differently depending on scope. This adds hidden complexity to the implementation contract.
- **Dashboard-specific global constructor** (`LocalBackend::global()`): Rejected in sub-variant 2a for the same topology reason as Option 1. Sub-variant 2b (flat storage + both engine and dashboard use global) is structurally identical to Option 4 but avoids naming it a removal; Option 4 is the cleaner framing.
- **Config-driven scope** (`session.scope = "global" | "repo"`): Rejected because it makes a structural concern user-configurable, adds a config key that the dashboard would have to override anyway, and still doesn't resolve the topology issue if the storage stays per-repo.

**Consequences**

What changes:
- `LocalBackend::new()` no longer calls `repo_id()` or appends a hash segment to `base_dir`. The constructor becomes simpler.
- `build_local_backend()` no longer needs `std::env::current_dir()` for the local backend path (though it still needs it for the cloud backend prefix until F5).
- A migration helper is added (run once, on startup, when old layout detected).
- `CloudBackend` will drop the repo-id S3 prefix at F5, but this is deferred.

What becomes easier:
- The dashboard sees all sessions on the machine without configuration.
- `list()` and all `SessionBackend` trait methods are unchanged.
- Testing: `KOTO_SESSIONS_BASE` already works for test isolation without repo-id.

What becomes harder:
- Session name collisions across repos are now user-visible (instead of silently isolated). Users who run the same session name in multiple repos will need to use distinct names. This is consistent with how cloud backends typically work.
- Migration adds a one-time startup check; the migration code must be maintained and eventually removed.
<!-- decision:end -->
