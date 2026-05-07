# Lead: Event Type Name Canonicalization

## Findings

### Wire format overview

Every event line in a JSONL state file has the shape:

```json
{"seq":1,"timestamp":"2026-01-01T00:00:00.000Z","type":"<event_type>","payload":{...}}
```

The `type` field is a free string set by `EventPayload::type_name()` in `src/engine/types.rs`. All 15 current type strings are snake_case, matching Rust enum variant names lowercased with underscores. Serialization is hand-rolled (custom `Serialize` impl on `Event`), not derived from the enum, so the mapping is explicit and stable.

The header line (first line, no `seq`) is a flat JSON object with its own schema.

### Event type inventory

| Wire type string | Rust variant | Notes |
|---|---|---|
| `workflow_initialized` | `WorkflowInitialized` | Clear. Marks session birth. |
| `transitioned` | `Transitioned` | Slightly ambiguous — could mean many things. No noun context. |
| `evidence_submitted` | `EvidenceSubmitted` | Clear to koto users; may require explanation for new consumers. |
| `directed_transition` | `DirectedTransition` | Clear. Explicit manual override by an agent. |
| `integration_invoked` | `IntegrationInvoked` | Ambiguous: "integration" has no public definition. |
| `rewound` | `Rewound` | Clear intent (rollback), but past-tense verb is slightly unusual. |
| `context_added` | `ContextAdded` | Mostly clear; "context" is a koto-specific term. |
| `workflow_cancelled` | `WorkflowCancelled` | Clear. |
| `default_action_executed` | `DefaultActionExecuted` | Ambiguous: "default action" is an internal koto concept. |
| `decision_recorded` | `DecisionRecorded` | Clear. |
| `gate_evaluated` | `GateEvaluated` | Clear to those familiar with gate-based workflows; needs docs. |
| `gate_override_recorded` | `GateOverrideRecorded` | Paired with `gate_evaluated`, clear once gate concept is explained. |
| `scheduler_ran` | `SchedulerRan` | Internal-feeling. "scheduler" is a koto-internal subsystem. |
| `batch_finalized` | `BatchFinalized` | Reasonably clear if "batch" is explained. |
| `child_completed` | `ChildCompleted` | Clear in context of parent/child workflow hierarchies. |

### Field-level clarity analysis

**`transitioned` payload:**
- `from: Option<String>` — null on first transition (from nothing). A consumer needs to know `null` means "initial state". The field name is clear.
- `to: String` — clear.
- `condition_type: String` — only two observed values in code: `"auto"` and `"gate"`. A third value `"skip_if"` is produced by the `skip_if` evaluation path in `advance.rs` (line 506). These three values form an undocumented enum. A consumer has no way to know the complete set or meaning without reading source.
- `skip_if_matched: Option<Object>` — omitted when null; key-value pairs from matched `skip_if` conditions. The field name leaks the template concept `skip_if`. A consumer needs to understand koto template YAML to interpret it.

**`workflow_initialized` payload:**
- `template_path: String` — refers to a compiled cache path, not the source template path. The integration test at line 1027 shows the wire value is `/cache/abc.json`, not the human-authored template file. A consumer has no way to know this path is ephemeral/cache-relative.
- `variables: HashMap<String,String>` — clear.
- `spawn_entry: Option<SpawnEntrySnapshot>` — present only for batch-spawned children. Field name is opaque; "spawn entry" is a batch scheduler concept. Sub-fields (`template`, `vars`, `waits_on`) are mostly clear, but `vars` uses a different name than the outer `variables`.

**`evidence_submitted` payload:**
- `state: String` — the state machine state name; clear.
- `fields: HashMap<String,serde_json::Value>` — the submitted key-value pairs. The field name `fields` is generic; a consumer won't know these are arbitrary agent-provided key-value data without docs.
- `submitter_cwd: Option<PathBuf>` — internal: used as a path resolver fallback for batch child spawning. A dashboard developer would not know why this field exists or why it matters. The name is internal-feeling.

**`integration_invoked` payload:**
- `state`, `integration: String`, `output: serde_json::Value` — the `integration` field holds a name like `"github"` but "integration" as a concept has no public definition in the wire format documentation. A consumer can't tell whether this is a plugin, a webhook, or something else.

**`default_action_executed` payload:**
- `command: String`, `exit_code: i32`, `stdout: String`, `stderr: String` — all clear, conventional names. The event type name itself (`default_action_executed`) is the problem: "default action" is a koto-internal concept (a shell command run automatically on state entry). A consumer would not know what "default" signifies here.

**`gate_evaluated` payload:**
- `gate: String` — the gate identifier (e.g., `"ci-passes"`). Clear.
- `output: serde_json::Value` — the raw gate evaluator output. The schema of this object is gate-type-specific (command gates emit `exit_code`/`error`; context gates emit different fields). A consumer has no way to know the output shape varies by gate type.
- `outcome: String` — observed values `"passed"` and `"failed"` in tests, but the complete enum is undocumented.
- `timestamp: String` — duplicates the outer `Event.timestamp`. The code comment notes this is intentional ("so downstream consumers reading just the payload don't need to pair it with the outer envelope") but a consumer would not know why there are two timestamps.

**`gate_override_recorded` payload:**
- `override_applied: serde_json::Value` — the value that was substituted as if the gate had produced it. The name `override_applied` is accurate but slightly jargony.
- `actual_output: serde_json::Value` — the real gate output that was overridden. Clear.
- Both `override_applied` and `actual_output` have gate-type-dependent schemas (same problem as `gate_evaluated.output`).

**`scheduler_ran` payload:**
- `tick_summary: SchedulerTickSummary` — nested object with `spawned_count`, `errored_count`, `skipped_count`, `reclassified`. All counts are named consistently. `reclassified: bool` means a child's classification changed during this tick — not self-evident without source context.
- `timestamp: String` — again duplicated from outer envelope, intentionally.

**`batch_finalized` payload:**
- `view: serde_json::Value` — a frozen snapshot of the `children-complete` gate output. The field name `view` is opaque; a consumer would not know this is a gate output snapshot.
- `superseded_by: Option<SupersededByRef>` — present only when a later event invalidated this finalization. The sub-object has `seq`, `type`, and `timestamp`. This is a log-level annotation rather than a business event field; a consumer would not understand when or why this appears without reading design docs.

**`child_completed` payload:**
- `child_name: String` — the full composed session id (e.g., `"parent.task-1"`).
- `task_name: String` — the short piece after the dot prefix. The distinction between `child_name` and `task_name` is subtle; a consumer needs to know that composed session names are dot-delimited.
- `outcome: TerminalOutcome` — serialized as snake_case: `"success"`, `"failure"`, `"skipped"`. Clear and well-documented in source.
- `final_state: String` — the terminal state name. Clear.

**`context_added` payload:**
- `key: String`, `hash: String`, `size: u64` — all clear. The `hash` value is a SHA-256 hex digest but there's no field-level documentation of the algorithm on the wire.

**`rewound` and `directed_transition` payloads:**
- Both use `from`, `to`, and optional `rationale`. All clear and conventional. `directed_transition` additionally requires `from` to be non-null (unlike `transitioned`), which is correct but undocumented.

### Structural issues

1. **Timestamp duplication**: `gate_evaluated`, `gate_override_recorded`, `scheduler_ran`, and `batch_finalized` all embed a `timestamp` field inside their payloads. This duplicates the outer `Event.timestamp`. The rationale (payload-self-contained reads) is not visible on the wire. A consumer parsing events will encounter two timestamps and not know which is authoritative.

2. **`condition_type` is an undocumented enum**: The three string values (`"auto"`, `"gate"`, `"skip_if"`) are not enumerated anywhere in the public surface. Consumers must guess or read source.

3. **`template_path` in `workflow_initialized`**: The path stored is the compiled cache path, not the source template. This is confusing for consumers who expect a path they can open or display.

4. **`vars` vs `variables` naming inconsistency**: The outer `workflow_initialized` payload uses `variables` for its var map; the nested `spawn_entry.vars` uses `vars`. Two names for the same concept at different nesting levels.

5. **`view` in `batch_finalized`**: The field name `view` is opaque — it could be any object. The actual content is a frozen gate output snapshot, which is a specific, structured thing.

6. **Internal-use-only fields on the wire**: `submitter_cwd` on `evidence_submitted` is a path resolver fallback for the batch scheduler. It has no meaning to a dashboard or relay consumer. It appears on every evidence event (when non-null) but serves only internal engine purposes.

## Implications

The contract needs to address the following decisions:

1. **Enumerate `condition_type` values**: The contract must define `"auto"`, `"gate"`, and `"skip_if"` as the complete set of valid values for `transitioned.condition_type`, with meaning for each.

2. **Rename or alias `view` in `batch_finalized`**: `batch_view` or `children_summary` would be less ambiguous than bare `view`.

3. **Clarify `template_path` semantics**: Document that this is the compiled cache path, not the source path. Consider whether the source path should be a separate field.

4. **Harmonize `vars` / `variables`**: The contract should use one name. The outer `variables` is more explicit; `spawn_entry.vars` should either be renamed or the inconsistency documented.

5. **Classify `submitter_cwd` as internal**: The contract should mark this field as an internal hint not intended for consumer interpretation, or omit it from the public schema.

6. **Explain `outcome` enum values**: For `gate_evaluated`, the valid outcomes (`"passed"`, `"failed"`) need to be enumerated. Same for `TerminalOutcome` on `child_completed` (though this is already snake_case and well-named).

7. **Decide on `default_action_executed` naming**: The name leaks an internal concept. Options: rename to `shell_command_executed` or `action_executed`, or document what "default action" means in a koto state machine context.

8. **Address `scheduler_ran` naming**: This is the most internally-named event. Options: rename to `batch_tick_recorded` or `batch_progress_recorded` to better describe what consumers see.

9. **Document timestamp duplication policy**: Clarify that when a payload carries its own `timestamp`, it is authoritative for that event's logical time (and matches the outer timestamp).

10. **Define gate output schemas**: The `output` and `override_applied`/`actual_output` fields in gate events are gate-type-specific. The contract needs either a discriminant on these objects or a reference to gate type documentation.

## Surprises

1. **`condition_type: "skip_if"` is a third undocumented value**: The code and tests use only `"auto"` and `"gate"` explicitly, but `advance.rs` line 506 emits `"skip_if"` as a third `condition_type` value when a `skip_if` condition triggers a transition. This third value is not tested via the event round-trip tests and could surprise consumers who assume the field is binary.

2. **`from` is nullable on `transitioned` but required on `directed_transition` and `rewound`**: This asymmetry (same field name, different nullability across event types) is not obvious. On `transitioned`, `from: null` means the initial transition from no state. On `directed_transition` and `rewound`, `from` is always present. A consumer treating these events uniformly would need to handle this difference.

3. **`batch_finalized.superseded_by` is a computed projection, not a native event field**: The source code comment explicitly states it is "written as `None` at append time" and higher-level code "may compute and attach this projection when rendering stale events." This means the field on the wire is always absent at write time and only appears in rendered/projected views. A consumer reading raw JSONL will never see `superseded_by` populated; only a rendering layer adds it. This is a significant contract ambiguity — the field exists in the schema but is never actually written to the log.

4. **No event marks workflow completion**: There is no `workflow_completed` or `workflow_succeeded` event type. Terminal state arrival is inferred by consumers via `transitioned.to` matching a state with `terminal: true` in the template — which requires template knowledge to interpret. A pure event-log consumer cannot detect workflow completion without the template.

5. **`scheduler_ran` is deliberately sparse**: Non-trivial ticks only (at least one of `spawned_count`, `errored_count`, `skipped_count` non-zero, or `reclassified: true`). Pure no-op ticks are suppressed. A consumer counting scheduler ticks from the log will see a lower number than actual tick invocations.

6. **`hash` in `context_added` has no algorithm identifier**: The field stores a SHA-256 hex digest but the algorithm is not carried on the wire. If the algorithm changes, consumers have no way to detect the change.

## Open Questions

1. **Should `submitter_cwd` be removed from the public contract entirely?** It serves only as a path resolver hint for the batch scheduler. A public consumer (dashboard, relay) has no use for it. If it stays, it needs a clear "internal hint, do not rely on" annotation in the contract.

2. **Should `condition_type` be modeled as an enum with a discriminant, or remain a free string?** An enum allows exhaustive documentation but constrains future extension. The current three values (`"auto"`, `"gate"`, `"skip_if"`) suggest it will not grow arbitrarily, which favors enumerating them.

3. **Is `batch_finalized.superseded_by` in scope for the F2 public contract?** Since it is never written to the raw JSONL log (only projected by rendering code), it may belong in a rendering/projection layer spec rather than the event log contract.

4. **Should `default_action_executed` be renamed?** This requires a wire-breaking change (old logs carry the old type string). If yes, F2 must define a migration or aliasing strategy.

5. **Should the contract include a `workflow_completed` synthetic event type, or document the terminal-state detection pattern?** Consumers without template access cannot detect completion today.

6. **What is the complete set of `gate_evaluated.outcome` values?** The tests show `"passed"` and `"failed"`. Are there others (e.g., `"skipped"`, `"error"`, `"overridden"`)? The contract needs to enumerate them.

7. **Should the nested `timestamp` fields in `gate_evaluated`, `gate_override_recorded`, `scheduler_ran`, and `batch_finalized` be considered part of the public contract or implementation detail?** If part of the contract, the policy on when payload timestamp and outer timestamp diverge (if ever) needs to be stated.

## Summary

The 15 event type strings are all readable snake_case and largely self-documenting at the type level, but several payloads contain fields that leak internal koto concepts (`condition_type` values, `default_action_executed`, `scheduler_ran`, `submitter_cwd`, `view`) or have structural ambiguities (timestamp duplication, `from`-nullability asymmetry, `vars`/`variables` inconsistency) that a dashboard developer would trip over without source access. The biggest contract gap is `condition_type`: it has an undocumented third value (`"skip_if"`) that appears in production logs but is not covered by event round-trip tests, and its full value set is never enumerated. The largest open question is whether `batch_finalized.superseded_by` belongs in the event log contract at all, since it is documented as a computed projection that is never actually written to the raw JSONL file.
