<!-- decision:start id="session-list-ordering" status="confirmed" -->
### Decision: Session List Ordering in the koto Dashboard

**Context**

The koto dashboard session list shows all local koto sessions in a flat view with tree nesting for parent-child hierarchies. The primary operator use case is monitoring a dozen parallel sessions — typically niwa workspaces running independent shirabe workflows — and quickly identifying which sessions need attention.

The current `--once` implementation sorts all session IDs alphabetically (`all_ids.sort()`). The PRD draft (R18) described "most recently active first" as the intended order. Neither matches how an operator actually reasons about session priority: when twelve sessions are running and two have failed, the failed sessions should appear at the top without requiring the operator to scan the full list.

The ordering applies to both the interactive TUI (`visible_rows()` iterates `tree.roots` in its stored order) and `--once` scripting output. Integration tests currently rely on alphabetical ordering because that's what `all_ids.sort()` produces.

**Assumptions**

- `mtime` on the state file is a reliable proxy for "most recently active" — the engine appends an event to the state file on every transition or gate evaluation, so mtime advances whenever the session does anything.
- The five status buckets (`failed`, `blocked`, `running`, `done`, `unknown`) from `classify_status()` in `dashboard.rs` map cleanly to a severity ordering. `failed` and `blocked` are the two states that require operator attention; `running` is healthy but worth monitoring; `done` and `unknown` are low-priority.
- `blocked` (non-terminal gate failure, session stuck waiting) shares urgency with `failed` in practice — a blocked session won't make progress without operator intervention.
- The ordering applies to root sessions only. Children are already sorted by `sort_priority()` inside `sorted_children()`.
- Any existing integration tests that assert on `--once` output order will need to be updated, but since there are currently no dashboard feature tests in `test/functional/`, this is a low-risk change.

**Chosen: Health severity first, recency as tiebreaker**

Sort root sessions by a two-key comparator: primary key is a severity bucket (lower number = shown first), secondary key is mtime descending (most recently active breaks ties within a bucket).

Severity bucket assignment:

| Bucket | Priority | Rationale |
|--------|----------|-----------|
| `failed` | 0 | Terminal failure, needs immediate attention |
| `blocked` | 1 | Non-terminal but stuck, needs operator action |
| `running` | 2 | Active and progressing, healthy |
| `unknown` | 3 | No state yet, may be initializing |
| `done` | 4 | Terminal success, lowest priority |

Within each bucket, sessions are ordered by mtime descending — most recently active first. This ensures that two failed sessions are ordered by which one failed more recently, giving the freshest failure top billing.

Both the TUI (`tree.roots` ordering, rebuilt on every `refresh()` call) and `--once` output (`all_ids` sort) adopt this comparator. The `--once` output change is the only externally visible behavior change.

**Rationale**

The operator's opening question — "which of my sessions needs attention right now?" — is best answered by putting actionable sessions at the top. A session that failed 40 minutes ago is more actionable than one that transitioned 5 seconds ago into a healthy running state. Recency-first ordering conflates "recent activity" with "urgency," forcing the operator to scan the entire list and mentally filter by status. Severity-first ordering answers the attention question directly.

Pure severity without a recency tiebreaker is technically correct but not deterministic for sessions in the same bucket. The mtime tiebreaker preserves the "most recently active" preference within a bucket, satisfies the determinism constraint (mtime is a stable value between refreshes), and produces no flickering in the TUI (mtime only changes when a session file changes, and the TUI refreshes the sort on every data poll).

Alphabetical ordering (the current `--once` implementation) serves tests and scripts that need a stable sort key, but it provides no value to the operator's attention task. Configurable ordering (Option 4) adds complexity for a feature that has no demonstrated need from the primary use case; if scripting consumers need alphabetical order, they can pipe `--once` output through `sort`.

The hybrid option (active sessions by severity, done sessions by recency) was considered but rejected because it introduces a non-obvious boundary: why is a `done` session sorted by time while a `running` session is sorted by severity? The pure severity-first model is simpler to understand and document.

**Alternatives Considered**

- **Most recently active first (PRD draft R18)**: Orders all sessions by mtime descending, regardless of health. Rejected because a healthy session that transitioned 2 seconds ago displaces a failed session that has been broken for 40 minutes. The operator must scan and mentally re-sort by status.

- **Alphabetical (current `--once` implementation)**: Stable, predictable, no ambiguity. Rejected for the TUI because alphabetical order has no relationship to operator urgency. Acceptable for scripting consumers who need a canonical order, but they can apply their own sort to `--once` output.

- **Hybrid: active sessions by severity, done sessions by recency**: Groups failed/blocked/running by severity, then lists done/unknown sessions by recency. Rejected because the boundary between the two groups is arbitrary and confusing; pure severity-first is simpler and covers the same use cases.

- **Configurable ordering with a default (`--sort=severity|recency`)**: Adds a `--sort` flag to `koto dashboard`. Rejected for now because: (a) there is one well-defined primary use case (operator monitoring), (b) the flag adds implementation complexity and skill documentation overhead, and (c) `--once` scripting consumers who need a different order can transform the output themselves. Can be added later if a concrete need emerges.

**Consequences**

- The TUI's root session list changes from alphabetical (current) to severity-first with recency tiebreaker. Operators see failed and blocked sessions at the top without scanning.
- `--once` output order changes from alphabetical to severity-first with recency tiebreaker. Any scripts that rely on the current alphabetical output order will need updating. Because there are no existing functional tests for `dashboard --once`, the test impact is low.
- `rebuild_roots()` in `dashboard_data.rs` currently does `roots.sort()` (alphabetical). This must be replaced with the severity-aware sort, which requires `CachedSession` data (is_terminal, is_blocked, current_state, mtime). The `SessionTree` already holds all of this, so no new data is needed.
- The `--once` path in `dashboard.rs` currently calls `all_ids.sort()`. This must be replaced with the same severity-aware sort.
- Both sort sites should use the same comparator function, extracted into a shared helper (e.g., `session_sort_key(session: &CachedSession) -> (u8, Reverse<SystemTime>)`).
<!-- decision:end -->
