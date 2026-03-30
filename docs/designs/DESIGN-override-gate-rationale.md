---
status: Proposed
upstream: docs/prds/PRD-override-gate-rationale.md
problem: |
  Gate overrides in koto's advance loop are implicit and untracked. The engine
  needs a new event type, a CLI flag that threads rationale into the advance
  path, and a cross-epoch query function -- all without breaking existing
  evidence submission or template schemas.
decision: |
  Placeholder -- will be filled after decision execution phases.
rationale: |
  Placeholder -- will be filled after decision execution phases.
---

# DESIGN: Override gate rationale

## Status

Proposed

## Context and problem statement

The koto engine's advance loop (`advance_until_stop` in `src/engine/advance.rs`)
handles gate failures through a fallback path: if gates fail and the state has
an `accepts` block, the engine returns `EvidenceRequired` instead of
`GateBlocked`, letting the agent submit evidence to resolve a conditional
transition. This implicit override works but produces no audit trail.

The technical challenge has four parts:

1. **New event type in the persistence layer.** `GateOverrideRecorded` must be
   added to `EventPayload` in `src/engine/types.rs`. The JSONL format uses
   untagged serialization with a `type` discriminant string, so the new variant
   needs a `type_name()` mapping and compatible deserialization. The event must
   carry gate failure context (from `BTreeMap<String, GateResult>` already
   available in the advance loop) and the rationale string.

2. **CLI flag threading.** A new `--override-rationale <string>` flag on
   `koto next` must reach the advance loop. Today, evidence flows through
   `--with-data` -> `validate_evidence` -> `EvidenceSubmitted` event ->
   `derive_evidence` -> `advance_until_stop`. The override flag needs a
   parallel path that doesn't go through evidence validation (it's not evidence)
   but reaches the point where gate failure is detected.

3. **Advance loop changes.** When `--override-rationale` is present and gates
   fail, the engine should bypass the gate-blocked stop reason, emit
   `GateOverrideRecorded`, and continue advancing. Today, gate-only states
   (no `accepts` block) return `GateBlocked` with no override path. The
   `--override-rationale` flag must work on these states too.

4. **Cross-epoch query function.** Existing `derive_*` functions in
   `src/engine/persistence.rs` are epoch-scoped (they filter events after the
   most recent state-changing event). `derive_overrides` needs to scan the full
   event log, which is a different pattern. Plus a `koto overrides list` CLI
   command to expose it.

## Decision drivers

- **Audit completeness**: override events must be self-contained for future
  visualization consumers (no cross-event correlation needed)
- **Backward compatibility**: existing `--with-data '{"status": "override"}'`
  patterns must keep working as plain evidence submission
- **Engine universality**: every gate-blocked state must be overridable,
  regardless of whether the template has an `accepts` block
- **Minimal surface area**: one flag (`--override-rationale`), one new event
  type, one new query function -- no additional template schema changes
- **Event ordering**: when combined with `--with-data`, evidence and override
  events must have deterministic sequence ordering
- **Forward compatibility**: the event shape should support future redo/replay
  without schema migration
