# Exploration Findings: session-schema-hygiene

## Core Question

What schema fields must be added to koto's JSONL session event log before external adoption occurs, with precise enough type, contract, and behavioral specifications to prevent scope creep and ensure all additions land together?

## Round 1

### Key Insights

- **Header is not truly immutable** (lead: header-structure): `StateFileHeader` has six fields, no UUID, and is written at init — but `relocate()` rewrites it during session rename. The PRD must explicitly state that `session_id` survives relocation unchanged, since the code doesn't enforce this today.

- **Timestamp change is a single-function fix** (lead: timestamp-format): `now_iso8601()` uses `.as_secs()`. Millisecond precision requires adding `.subsec_millis()` to the same call. One function change; all callers inherit it. No new crate needed.

- **Context manifest metadata maps 1:1 to event fields** (lead: context-sidecar): `KeyMeta { created_at, size, hash }` per key is exactly what a `context_added` event needs to carry (minus `created_at`, which `Event.timestamp` already covers). The emission point is `ContextStore::add()` in the CLI handler.

- **Adding optional fields to transitions is non-breaking** (lead: transition-events): `EventPayload` has no `deny_unknown_fields`. The established pattern is `#[serde(default, skip_serializing_if = "Option::is_none")]`. `GateOverrideRecorded.rationale: String` is the precedent for free-text rationale.

- **Demand is maintainer-driven, not user-reported** (lead: adversarial-demand): No independent external demand found. The necessity is technical: append-only log, immutable header writes, mutable sidecar with no log counterpart. Not a user feature request — a pre-adoption hardening requirement.

### Tensions

- **context_added ordering strategy**: Three options exist (synchronous during `koto context add`, deferred to next `koto next`, lazy reader reconstruction). Synchronous is the only approach that preserves strict causal ordering but requires `SessionBackend` access in the context add CLI path. Resolved in auto mode: synchronous.

### Gaps

None — all four additions are fully specified enough to write a PRD.

### Decisions

- **Synchronous `context_added` emission**: Emitted by `handle_context_add` immediately after `store.add()` returns. Reason: deferred batching loses causal ordering relative to transitions; lazy reconstruction defeats the purpose of a log event.
- **Millisecond timestamp precision**: 3-digit fractional seconds in RFC 3339 format. Reason: sufficient for the stated problem (concurrent session ambiguity within one second); no external crate needed; backward compatible.
- **UUID v4 for session_id**: Random UUID stored as lowercase hyphenated string. Reason: simpler than v7 (session already has `created_at` for ordering); avoids time-coordination complexity; `uuid` crate v1.x with `v4` feature.
- **Optional free-text `rationale` on directed_transition and rewound**: `Option<String>`, omitted when absent, no length limit. Reason: matches `GateOverrideRecorded.rationale` convention; structured rationale constrains agents without benefit.

### User Focus

Auto mode — no user narrowing input. Findings are sufficient; proceeding to crystallize.

## Accumulated Understanding

koto's JSONL event log needs four schema additions before external consumers adopt the format. All four are confirmed missing from the codebase. All four have clear, implementable specifications grounded in the current code:

1. **`session_id`** (UUID v4 string) on `StateFileHeader` — required, generated at init, copied unchanged during relocate
2. **Millisecond precision** on all `timestamp` fields — RFC 3339 with 3-digit fractional second, one function to change
3. **`context_added` event** with `key`, `hash`, `size` — emitted synchronously by `koto context add`
4. **`rationale: Option<String>`** on `DirectedTransition` and `Rewound` — additive, non-breaking

The technical non-back-fillable argument is sound for each: session UUIDs after cleanup are unresolvable, sub-second values are not inferrable from whole-second records, context additions without log entries are permanently invisible to readers, and rationale cannot be attributed after the fact.

## Decision: Crystallize
