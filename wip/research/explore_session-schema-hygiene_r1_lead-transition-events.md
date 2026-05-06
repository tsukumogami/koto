# Lead: directed_transition and rewound event definitions

## Findings

Both variants are defined in `src/engine/types.rs` (lines 171-178):

```rust
DirectedTransition {
    from: String,
    to: String,
},
Rewound {
    from: String,
    to: String,
},
```

Both are minimal two-field structs. No rationale, no metadata.

The `EventPayload` enum uses `#[serde(untagged)]`, and the custom `Deserialize` implementation on `Event` dispatches by the `type` field string. Unknown fields in the payload value are ignored (no `deny_unknown_fields`), making optional additive fields backward compatible.

The established pattern for optional additive fields is `#[serde(default, skip_serializing_if = "Option::is_none")]`, used on `WorkflowInitialized.spawn_entry` and `EvidenceSubmitted.submitter_cwd`.

`GateOverrideRecorded` has a required `rationale: String` (not optional) — the closest precedent, showing free-text rationale is an accepted convention.

Five consumers read these events: `persistence.rs`, `cli/mod.rs`, `engine/batch.rs`, `engine/retry.rs`. None do field-level destructuring that would break on an added optional field.

## Implications

Adding `rationale: Option<String>` to both `DirectedTransition` and `Rewound` with `#[serde(default, skip_serializing_if = "Option::is_none")]` is non-breaking. Old readers ignore the field; new events with no rationale omit it; new events with a rationale include it.

## Surprises

The absence of `deny_unknown_fields` is intentional: prior additive fields confirm this is the designed extension point.

## Open Questions

None.

## Summary

`DirectedTransition` and `Rewound` are two-field structs (`from`, `to`) with no rationale field. Adding `Option<String> rationale` with `#[serde(default, skip_serializing_if = "Option::is_none")]` is non-breaking — no consumer does field-level destructuring. `GateOverrideRecorded.rationale: String` (required, free-text) is the nearest precedent for rationale conventions.
