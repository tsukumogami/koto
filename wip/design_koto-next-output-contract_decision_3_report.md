# Decision: Visit Count Computation and Propagation

**Question:** How should visit counts be computed and propagated for conditional details inclusion?

**Chosen:** (a) Count during existing log replay via `derive_visit_counts`

**Confidence:** High

## Context

The `koto next` output contract needs visit tracking to determine whether a state is being entered for the first time (details included) or a repeat visit (details omitted unless `--full`). The JSONL event log already records all state-entry events, and PRD R9 prohibits new state files or schema changes.

## Alternatives Evaluated

### (a) derive_visit_counts -- HashMap counting (chosen)

Add a `derive_visit_counts(events: &[Event]) -> HashMap<String, usize>` function to `src/engine/persistence.rs`. It scans all events once, incrementing a counter for each state name that appears as a `to` field in `Transitioned`, `DirectedTransition`, or `Rewound` events. The CLI handler calls this alongside `derive_state_from_log` and passes `visit_counts[&current_state]` to the response serialization layer.

**Why chosen:**
- Follows the established `derive_*` pattern exactly (pure function, `&[Event]` input, no I/O)
- Returns counts, not just presence, which supports future features (e.g., loop detection, retry budgets) at zero extra cost
- Single forward scan over events already loaded in memory -- negligible overhead for tens-to-hundreds of events
- No new state files or schema changes (satisfies PRD R9)

### (b) Boolean scan -- HashSet of visited states (rejected)

Same approach but returns `HashSet<String>` instead of `HashMap<String, usize>`.

**Why rejected:** Saves one word of storage per state name (counter vs. boolean) but loses count information. The implementation complexity is identical -- both iterate the same events and match the same variants. Choosing a HashSet over a HashMap is a strictly less capable design for negligible simplification.

### (c) Persist visit count alongside events (rejected)

Add a running `visit_count` field to derived state or a separate tracking file.

**Why rejected:** Directly violates PRD R9 ("no new state files or schema changes"). The event log already contains all the information needed to derive counts.

## Implementation Sketch

```rust
// src/engine/persistence.rs
pub fn derive_visit_counts(events: &[Event]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for event in events {
        let target = match &event.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if let Some(state_name) = target {
            *counts.entry(state_name.clone()).or_insert(0) += 1;
        }
    }
    counts
}
```

The CLI handler (`handle_next`) calls `derive_visit_counts(&events)` after `read_events`, then uses `counts.get(&current_state).copied().unwrap_or(0)` to determine first-visit status. A count of 1 means first visit; greater than 1 means repeat. The `--full` flag bypasses this check entirely at the serialization layer.

## Assumptions

- Typical workflows have tens to low hundreds of events, so a full scan adds negligible latency.
- The event log is already fully loaded into memory by `read_events` before any `derive_*` function runs.
- "First visit" means the state has been entered exactly once (count == 1), not zero times (the current entry is already logged before `koto next` reads it, or will be checked after the transition event is appended).
- `--full` bypass happens at the response construction layer, not inside `derive_visit_counts`.
