# Pragmatic review — request-store-converge Issue 1 (walking skeleton), commit 8f1aa9a

Focus: over-engineering, dead code, scope creep, YAGNI/KISS. No code edited.

## Summary

The skeleton is mostly right-sized. The `WorkflowResult` envelope, the additive
`ChildCompleted.result`, the standalone `synthesize_workflow_result`, and the
converge inline read are all proportionate to Issue 1 and each is exercised by a
test. One finding is a genuine dead branch (the short-name result-map fallback),
and one is a build-ahead variant (`RequestStoreResult`) whose justification
depends on how strictly Issue 1 is scoped.

## Findings

### 1. Short-name result-map fallback is dead — the `.or_else` branch can never match. ADVISORY (leaning blocking-if-not-justified)

`src/cli/batch.rs:2435-2443`:

```rust
for entry in &mut entries {
    let composed = format!("{}.{}", parent_name, entry.name);
    if let Some(r) = result_by_child
        .get(&entry.name)
        .or_else(|| result_by_child.get(&composed))
    {
        entry.result = Some(r.clone());
    }
}
```

`result_by_child` is keyed by `child_name` from `ChildCompleted`, which is always
the composed `<parent>.<task>` form (set in `append_child_completed_to_parent`,
mod.rs:2225, and in the legacy non-composed case the raw id). Both entry builders
set `entry.name` to the same composed/session-id form:

- hook path `build_entries_from_tasks` (batch.rs:2557): `name: composed` =
  `format!("{}.{}", parent_name, task.name)`
- no-hook path `build_entries_from_disk` (batch.rs:2739): `name: session_id`,
  which is `info.id` (the composed id for batch children, raw for legacy).

So `entry.name` already equals the map key, and the first lookup
`result_by_child.get(&entry.name)` is the one that hits. The fallback computes
`composed = format!("{}.{}", parent_name, entry.name)` = `parent.parent.task`
(double-prefixed) — a key that no producer ever writes. The branch is unreachable.

The doc comment claims "the no-hook fallback path may name a cleaned-up child by
its short task name, so try the composed form too." That premise is false in this
commit: `build_entries_from_disk` names by `session_id`, not by short name. The
only converge test (batch.rs:4423) uses the hook path and matches on the first
lookup; the `.or_else` is never exercised.

Fix: drop the `.or_else(...)` branch and the `composed` local, inline to
`result_by_child.get(&entry.name)`. If a real short-name path is expected in a
later issue, add it then with a test that actually drives it — right now it is
speculative dead code with a misleading comment.

### 2. `RequestStoreResult` variant is constructed only in deserialization + one round-trip test. ADVISORY

`src/engine/types.rs:644` (variant), `:1046` (deserialize), `:2646` (test).

No production path ever appends a `RequestStoreResult` event. Issue 1 carries the
result on the parent's `ChildCompleted.result` (Decision 3) and the converge read
uses that copy exclusively (batch.rs:2316, never opens a child log). The child-log
durable record is explicitly Issue 2-5 work.

This is build-ahead: a fully-wired deserialize arm + payload struct + round-trip
test for an event nothing emits yet. It is inert (deserialize-only, additive
namespace) and small, so not blocking. But the commit message frames "the child
log is the durable record" as if shipped — it is not in Issue 1. If the team wants
a strict walking skeleton, this variant belongs in the issue that first emits it.
If keeping it now, the round-trip test is the right minimum and the framing in the
doc comment should say "reserved; emitted starting Issue N."

### 3. Standalone `synthesize_workflow_result` — single caller, but justified. NOT A FINDING

`src/cli/mod.rs:2139`, called once from `append_child_completed_to_parent`
(mod.rs:2222). Normally a single-caller helper is inline-bait, but here it is
directly unit-testable in isolation (the summary-field convention + final-state
fallback is the substantive logic), it has its own named concept
(evidence → result auto-promotion), and Issue 2-5 explicitly thicken it. Keeping
it standalone is the right call. No change.

### 4. `WorkflowResult` struct + reuse of `TerminalOutcome` for `status`. NOT A FINDING

`src/engine/types.rs:698`. Three fields, reuses the existing `TerminalOutcome`
instead of inventing a new status enum, `payload` is `skip_serializing_if` per the
repo's additive idiom. Right-sized; no speculative fields.

### 5. `ChildCompletedAppend` enum and append-failure deferral. NOT A FINDING (pre-existing)

`src/cli/mod.rs:2097` predates this commit (Issue #134). The three-way return with
`AppendFailed` → defer-cleanup is load-bearing (loses terminal-child visibility
otherwise), not over-engineering. Out of scope for this review anyway.

## Scope check

No scope creep observed. The diff stays within the skeleton: envelope type,
additive event field, synthesis fn, converge inline read, 6 tests. No drive-by
refactors, no new utility modules, no docstring churn elsewhere. The index flag,
durable child-log record, gate predicate, and docs are correctly absent (Issues
2-5).

## Recommendation

Address finding 1 before merge (delete the unreachable `.or_else` fallback and its
misleading comment, or add a test that proves a short-name path exists). Finding 2
is a judgment call on skeleton strictness — flag to the maintainer; it is inert if
kept. Findings 3-5 are clean.
