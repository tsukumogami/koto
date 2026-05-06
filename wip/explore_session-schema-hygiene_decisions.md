# Exploration Decisions: session-schema-hygiene

## Round 1

- **context_added emission is synchronous**: Emitted by `handle_context_add` immediately after `store.add()` returns. Deferred (batched at next `koto next`) loses causal ordering. Lazy reconstruction defeats the purpose. Synchronous requires `SessionBackend` access in the context CLI path, which is acceptable.

- **Timestamp precision is milliseconds (3-digit fractional second)**: `YYYY-MM-DDTHH:MM:SS.mmmZ`. Sub-millisecond precision doesn't add value for the stated problem (concurrent session disambiguation within one second). No external crate needed — `duration.subsec_millis()` suffices.

- **UUID variant is v4 (random)**: Stored as lowercase hyphenated string. UUID v7 (time-ordered) would be redundant since sessions already have `created_at`. Field name: `session_id`.

- **rationale is optional free-text String**: `Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`, no length limit. Structured rationale would constrain agents without benefit. Matches `GateOverrideRecorded.rationale` convention.

- **context_added event fields are `key`, `hash`, `size`**: Maps directly from `ctx/manifest.json` `KeyMeta` fields (`created_at` from KeyMeta is redundant with `Event.timestamp`). SHA-256 hex string for hash.

- **Adversarial lead does not block**: Demand is maintainer-driven pre-adoption hardening, not user-reported friction. "Demand not independently validated" vs. "demand validated as absent" — the former does not block the PRD.
