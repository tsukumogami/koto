<!-- decision:start id="skip-if-event-format" status="assumed" -->
### Decision: Synthetic Event Format for skip_if

**Context**

When a state auto-advances via skip_if, koto must write something to the event log so a resuming agent can reconstruct why it was passed. The event log is JSONL; the `Transitioned` event already carries a `condition_type` field (currently `"auto"` or `"evidence"`). The `EvidenceSubmitted` event records agent-submitted fields. No `AutoAdvanced` event type exists today.

The reporter explicitly requires a synthetic event -- not just a waypoint marker -- so a resuming agent knows *why* a state was passed ("if plan_context_injection auto-advanced because context.md existed, that fact should be in the log").

**Assumptions**

- Resuming agents read event history primarily via `koto query --events` or by inspecting the raw log, not just the live merged-evidence view.
- Backward compatibility with existing log readers is desirable but not a hard constraint.

**Chosen: Extend `Transitioned` with `condition_type: "skip_if"`**

Use the existing `Transitioned` event. Add `"skip_if"` as a new legal value for `condition_type`. Optionally add a `skip_if_matched` field carrying the condition key-value pairs that satisfied the predicate (e.g., `{"gates.context_file.exists": true}`). The field is optional and omitted when empty, keeping the wire format compact. `Transitioned` is already a state-changing event recognized by `derive_state_from_log` and all CLI output paths -- no new handling logic is needed.

**Rationale**

`Transitioned` is semantically correct: a state transition happened. The existing `condition_type` discriminator was designed for exactly this extensibility (it already distinguishes `"auto"` from `"evidence"`). Adding `"skip_if"` costs one new string value and one optional payload field, with zero changes to state-derivation or epoch-scoping logic. By contrast, a new `AutoAdvanced` event type requires new handling in `derive_state_from_log`, `derive_evidence`, and every CLI output path that enumerates event types. Adding `synthetic: bool` to `EvidenceSubmitted` is semantically wrong -- the event name says "submitted" but skip_if fired without agent submission.

**Alternatives Considered**

- **`EvidenceSubmitted` with `synthetic: bool` marker**: Backward-compatible and operationally cheap, but semantically wrong -- "evidence submitted" implies agent action that didn't happen. Also creates merge ambiguity when agent evidence and synthetic evidence carry the same field key. Rejected.
- **New `AutoAdvanced` event type**: Cleanest semantics, but requires changes in `derive_state_from_log`, `derive_evidence`, `merge_epoch_evidence`, and all CLI event-enumeration paths. The marginal clarity gain over extending `Transitioned.condition_type` doesn't justify the scope increase for v1. Can be revisited if the event log schema is refactored later.

**Consequences**

- `EventPayload::Transitioned` gains an optional `skip_if_matched: Option<BTreeMap<String, Value>>` field, serialized only when present.
- Existing log readers see `condition_type: "skip_if"` as an unknown value but don't break (forward-compat via serde defaults).
- CLI output for `koto query --events` surfaces skip_if transitions distinctly from unconditional auto-advances.
<!-- decision:end -->
