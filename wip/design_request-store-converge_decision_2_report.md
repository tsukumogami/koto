# Decision 2: Result-envelope type and exact field set

Executed INLINE.

## Question
Typed minimal envelope vs free-form JSON blob; and the precise fields/types.

## Options
- **2A — Typed minimal envelope.** A fixed typed core (`status`, `summary`)
  plus an optional structured `payload`. `status` is a typed enum reusing the
  existing `TerminalOutcome` (`Success`/`Failure`/`Skipped`, snake_case wire
  form). `summary` is a bounded human-readable string. `payload` is optional
  `serde_json::Value`.
- **2B — Free-form JSON blob.** The producer writes whatever JSON it likes;
  the parent reads an opaque object.

## Chosen: 2A — typed minimal envelope

The envelope is:

```rust
pub struct WorkflowResult {
    pub status: TerminalOutcome,        // reuse existing enum, snake_case wire form
    pub summary: String,                // bounded human-readable end-of-work statement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,  // optional structured detail
}
```

- `status` reuses `TerminalOutcome` verbatim — koto already classifies this on
  completion and serializes it snake_case, so the result's status is the same
  value `ChildCompleted` already carries. No new enum.
- `summary` is a single bounded string. It is the legible end-of-work artifact
  a parent reads (PRD R8). When the terminal state's `accepts` block declares a
  conventionally-named summary field, that value is used; otherwise a default
  derived from the final state name.
- `payload` is `Option<serde_json::Value>`, omitted from the wire when `None`
  via `skip_serializing_if`, matching koto's additive-field idiom. It carries
  the terminal `EvidenceSubmitted.fields` so a parent that wants structured
  detail can read it, while a parent that only wants outcome+summary ignores it.

The summary is bound-checked so the result, when its has_result flag and the
small inline projection ride near the index, never threatens the 4096-byte
index line bound (see Decision 3 — only a flag goes in the index, but bounding
the summary keeps the dereferenced directive payload sane).

## Rejected: 2B — free-form blob
Forces every converging parent to know each child's private shape, defeating
the uniform read PRD R2 / R6 / AC2 require (one accessor, no per-child special
casing). koto's own idiom is typed (`TerminalOutcome` was chosen over a
stringly value precisely so consumers match exhaustively and the wire format
stays stable). The optional `payload` already preserves the producer freedom
2B offered, without giving up the typed common path.

## Confidence: high. Directly extends an existing typed enum; matches PRD D2.
