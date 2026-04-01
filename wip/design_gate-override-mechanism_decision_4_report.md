# Decision 4: koto overrides CLI structure and derive_overrides scope

## Decision Question

How should the `koto overrides` CLI subcommand be structured, and what is the scope of `derive_overrides`?

## Context

### Existing Pattern: koto decisions

`DecisionsSubcommand` is a Clap enum with two variants:
- `Record { name: String, with_data: String }` — accepts `--with-data` as a single JSON blob
- `List { name: String }` — returns current-epoch decisions

`handle_decisions_record` validates the blob contains required string fields (`choice`, `rationale`) and optional array field (`alternatives_considered`). It appends a `DecisionRecorded` event without advancing.

`derive_decisions` in `persistence.rs` scopes to the current epoch: it finds the most recent state-changing event (`Transitioned`, `DirectedTransition`, `Rewound`) whose `to` matches the current state, then returns only `DecisionRecorded` events after that index. This means a rewind clears the visible decision set.

### PRD Requirements

- **R5**: CLI surface is `koto overrides record <name> --gate <gate_name> --rationale "reason" [--with-data '...']` — gate and rationale are explicit named flags, not packed into a JSON blob.
- **R5a**: Each call targets exactly one gate with one rationale.
- **R8**: `derive_overrides` is a cross-epoch query — returns all `GateOverrideRecorded` events across the full session. The acceptance criteria reinforces this: "Override events survive rewind and are visible in `koto overrides list`".
- **R12**: `--rationale` values are subject to the 1MB size limit.

## Option Analysis

### CLI Structure

**Option A (separate flags, PRD-specified):**
```
koto overrides record <name> --gate <gate_name> --rationale "reason" [--with-data '...']
```

Separate `--gate` and `--rationale` flags give Clap the ability to validate field presence at parse time rather than at runtime JSON introspection. The error messages are native CLI errors ("required argument --gate not provided") rather than custom JSON error payloads. This matches what the PRD examples show verbatim across R5, R5a, and all acceptance criteria examples.

The handler structure for `OverridesSubcommand::Record` would carry `{ name: String, gate: String, rationale: String, with_data: Option<String> }`. Gate name validation (gate exists in current state) and rationale validation (non-empty, ≤1MB) are applied to discrete fields rather than extracted from a blob.

**Option B (unified --with-data JSON blob):**
```
koto overrides record <name> --with-data '{"gate": "ci_check", "rationale": "...", "data": {...}}'
```

This mirrors the *implementation pattern* of `decisions record` but not the *interface*. The PRD defines a specific CLI interface. Packaging required fields into JSON pushes validation from Clap into the handler body, worsening the error experience (agent must receive a JSON error and parse it to understand what field was missing, rather than seeing standard CLI help). It also creates friction for the most common case — the no-`--with-data` override — which requires the agent to build a JSON object just to pass two strings.

The only advantage is code symmetry with the decisions handler, but that's surface symmetry at the cost of correctness relative to the PRD.

**Decision: Option A**

The PRD specifies this interface explicitly and the examples are consistent across six independent usage scenarios. Separate flags give better ergonomics and better error messages. The implementation diverges from `decisions record` at the Clap layer but mirrors it in event-appending logic.

### derive_overrides Scope

**Option A (two functions: current-epoch + cross-epoch):**

Two functions serve two callers with different needs:
- `derive_overrides_current_epoch(events)` — mirrors `derive_decisions` exactly, scoped to the current epoch via the same epoch-start-index logic. Used by the advance loop to determine which gates have active overrides during gate evaluation.
- `derive_overrides_all(events)` — scans all events for `GateOverrideRecorded` without epoch filtering. Used by `koto overrides list`.

This cleanly separates the two query purposes. The advance loop must not see overrides from previous epochs — an override for `ci_check` in a prior epoch should not affect gate evaluation after a rewind or transition away and back. `derive_decisions` establishes this precedent: decisions are epoch-scoped because they describe intent for the current state entry.

**Option B (single derive_overrides returns all epochs):**

A single function would work for `koto overrides list` but would force the advance loop to post-filter by epoch, duplicating the epoch-boundary logic inline. The PRD says overrides survive rewind for audit purposes (R8), but they must not be sticky across epochs for gate evaluation. If the advance loop receives a cross-epoch result and does its own filtering, that logic is untested and disconnected from the tested `derive_*` pattern.

Additionally, R8's cross-epoch requirement is specifically for the *query* use case. The PRD describes stickiness as within-epoch ("sticky within an epoch -- they persist in the event log until the state transitions"). These two requirements are not in conflict — they just target different callers.

**Decision: Option A (two functions)**

The naming should follow the established pattern:
- `derive_overrides` — current epoch, mirrors `derive_decisions`, used by advance loop
- `derive_overrides_all` — cross-epoch, used by `handle_overrides_list`

This keeps the advance loop calling a function with the same contract as `derive_decisions` and `derive_evidence`, while giving the CLI handler a separate function whose scope matches R8's intent.

## Output Format for koto overrides list

Mirroring `handle_decisions_list`, the output should be:
```json
{
  "state": "<current_state>",
  "overrides": {
    "count": N,
    "items": [
      {
        "state": "<state_when_recorded>",
        "gate": "<gate_name>",
        "rationale": "<rationale>",
        "override_applied": { ... },
        "actual_output": { ... },
        "timestamp": "<iso8601>"
      }
    ]
  }
}
```

The `state` field in each item reflects which state the workflow was in when the override was recorded (not necessarily the current state), supporting cross-epoch audit.

## Summary

- **CLI**: Option A — `--gate` and `--rationale` as separate named flags per PRD R5
- **Scope**: Option A — two functions: `derive_overrides` (current epoch, advance loop) and `derive_overrides_all` (cross-epoch, `koto overrides list`)
