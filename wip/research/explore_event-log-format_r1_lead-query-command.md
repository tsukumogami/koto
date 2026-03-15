# Research: koto query command scope

## Summary

`koto query` should NOT be added in issue #46. The Go implementation's `koto query`
was intentionally excluded from the Rust migration (DESIGN-migrate-koto-go-to-rust.md
marks it as "Excluded | #48 or later"). No current agent skill or integration test
requires query functionality. Adding it in #46 would be premature — the output shape
depends on decisions not yet made in #47 and #48.

## What Go's koto query returned

Go's `cmdQuery` called `engine.Snapshot()` which returned a `State` struct:

```go
type State struct {
    SchemaVersion int
    Workflow      WorkflowInfo  // name, version
    CurrentState  string
    Variables     map[string]string
    Evidence      map[string]string  // global accumulated evidence (all states)
    History       []HistoryEntry     // from, to, timestamp, type, evidence per entry
}
```

Key point: Go's evidence model was **global** — all submitted evidence accumulated in
one map across all states. The new model is **per-state via epoch boundary** — evidence
is scoped to the current state's last epoch. These are architecturally different, so
`koto query` can't just replay into the old shape.

## Does log replay in #46 naturally produce this?

Partially, with differences:
- **Current state**: yes — derived from last `transitioned`/`directed_transition`/`rewound`
  event's `to` field
- **Variables**: yes — from `workflow_initialized` event payload
- **Evidence**: different — new model is epoch-scoped, not global; the shape would
  change depending on `accepts` schema decisions in #47
- **History**: yes, and richer — event log has typed events with seq numbers, which is
  more detailed than Go's flat HistoryEntry

A `koto log` command in #46 could return the raw event log (just all events), which
is the canonical source of truth. But a `koto query`-style snapshot depends on
interpretation of evidence and expects-schema, which isn't defined until #47/#48.

## Agent/user need evidence

The shipped hello-koto plugin skill uses only: `koto init`, `koto next`,
`koto transition` (being replaced by `koto next --to` in #48), and `koto workflows`.
Integration tests inspect state via `koto next` output and direct file reads. No
test or skill calls `koto query`.

The DESIGN-migrate-koto-go-to-rust.md design doc explicitly marks `koto query` as
"Excluded" — intentional, not an oversight.

The JSONL state file is directly readable — agents can `cat koto-*.state.jsonl` or
pipe it through `jq`. This suffices for debugging. A command wrapper adds value only
when the format is complex enough to warrant abstraction.

## Recommendation: Defer to post-#49

**Do NOT add `koto query` or `koto log` in #46.**

Rationale:
1. **No blocking need**: Current agent contract (koto next + koto workflows) covers all
   agent workflows. No feature in #47, #48, or #49 depends on query.
2. **Output shape unstable**: The `expects` field (from #47 template format and #48 CLI
   contract) is part of what a full query response would include. Defining query output
   before those are accepted forces coordination overhead.
3. **Direct file access is sufficient**: JSONL is human- and machine-readable. `jq` on
   the state file gives the same information without a dedicated command.
4. **Scope isolation**: #46 should focus on event taxonomy and replay semantics. Adding
   a user-facing inspection command is a UI decision that belongs after the architectural
   decisions (#46-#49) are settled.

If a post-#49 issue adds `koto log` (raw event dump) or `koto inspect` (snapshot view),
the event log replay logic from #46 provides the foundation.
