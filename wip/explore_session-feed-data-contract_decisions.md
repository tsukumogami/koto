# Exploration Decisions: session-feed-data-contract

## Round 1

- Crystallize to Design Doc (auto mode): all five decision questions have evidence-backed
  answers; further rounds would not change outcomes. The design doc is the right artifact
  because requirements are given (the issue specifies the contract scope), the technical
  approach questions are open, and architectural decisions were made during exploration
  that must be on record.
- Markdown spec as primary artifact form: JSON Schema cannot encode behavioral guarantees
  (ordering, atomicity); fits koto's existing doc convention; right for a 2-5 person audience.
- Three-tier event classification: flat spec would force each of three consumers to
  independently re-derive audience classifications with no shared guidance.
- schema_version activation as the in-band version signal: already in every log at line 1;
  activation requires only a constant + guard. Separate spec-only versioning (no in-band
  signal) provides no programmatic compatibility check.
- Unknown event type handling: both koto implementation (add Unknown catch-all) and contract
  consumer requirement (MUST skip unknown types) are needed; these are complementary.
- Raw-log-only scope: `batch_finalized.superseded_by` is never written to raw JSONL
  (it's a rendering-layer projection); the contract covers only what appears in the file.
