# Phase 3 Research: Expects Derivation

## Questions Investigated

1. How does `accepts` map to `expects.fields`? Is it a direct 1:1 mapping of FieldSchema to the output format?
2. How do transition `when` conditions map to `expects.options`? Each transition with a `when` becomes an option entry?
3. What about transitions WITHOUT `when` conditions -- do they appear in `options`?
4. What about states that have `accepts` but all transitions are unconditional (no `when`)? Is `expects.options` omitted or empty?
5. What about states with no `accepts` -- can they still have `expects` (e.g., for `--to` directed transitions)?
6. How does `--to` interact with `expects`? Does the output still show options when `--to` is used?
7. What's the `event_type` field -- is it always `"evidence_submitted"`?

## Findings

### Q1: accepts -> expects.fields mapping

The mapping is nearly 1:1, but with a field name translation. The template type `FieldSchema` (`src/template/types.rs:56-64`) has:
- `field_type: String` (enum, string, number, boolean)
- `required: bool`
- `values: Vec<String>` (for enums)
- `description: String`

The strategic design's `expects.fields` output (DESIGN-unified-koto-next.md, lines 323-325) shows:
```json
"decision": { "type": "enum", "values": ["proceed", "escalate"], "required": true }
```

The key difference: `FieldSchema.field_type` in the Rust type becomes `"type"` in the JSON output. The `description` field is present on `FieldSchema` but not shown in any of the strategic design's output examples. The derivation logic should:
- Rename `field_type` to `type` in the output (serde `#[serde(rename)]` or custom serialization)
- Include `values` only for enum fields (already `skip_serializing_if = "Vec::is_empty"` in types.rs)
- Include `description` if non-empty (already `skip_serializing_if = "String::is_empty"`)
- Carry `required` as-is

This is a straightforward structural mapping, not a computed derivation.

### Q2: Transition when -> expects.options mapping

Yes, each `Transition` with a `when` condition becomes one entry in `expects.options`. The strategic design (lines 327-330) shows:
```json
"options": [
  { "target": "deploy", "when": { "decision": "proceed" } },
  { "target": "escalate_review", "when": { "decision": "escalate" } }
]
```

This maps directly to the `Transition` struct (`src/template/types.rs:48-52`):
```rust
pub struct Transition {
    pub target: String,
    pub when: Option<BTreeMap<String, serde_json::Value>>,
}
```

For transitions with `when: Some(map)`, the option entry is `{ "target": t.target, "when": t.when }`. The `Transition` struct already has the right shape -- it could almost be serialized directly as an option entry if filtered to only include conditional transitions.

### Q3: Transitions WITHOUT when conditions in options

The strategic design's examples only show conditional transitions in `options`. Unconditional transitions (those with `when: None`) should NOT appear in `expects.options`.

Rationale from the data flow (DESIGN-unified-koto-next.md, lines 460-463):
- If accepts block exists: evaluate `when` conditions against evidence. If none match, stop and wait for evidence.
- If no accepts and gates pass: auto-advance (unconditional).

An unconditional transition on a state with `accepts` would mean "also advance automatically regardless of evidence," which contradicts the evidence routing model. The template validation in `types.rs` does not currently forbid mixing conditional and unconditional transitions on the same state, but such a mix would be semantically odd -- the unconditional path would always win during auto-advancement before evidence is ever submitted.

For the `expects` output: only transitions with `when` conditions appear in `options`. Unconditional transitions are handled by the auto-advancement engine and don't surface in the agent-facing contract.

### Q4: accepts with all unconditional transitions

This is a valid template configuration. The test `accepts_with_unconditional_transitions` (`types.rs:773-789`) confirms the validator accepts it:
```rust
state.accepts = Some(accepts);
// No when condition -- unconditional transition is fine.
t.validate().unwrap();
```

In this case, `expects` should include `fields` (so the agent knows what evidence to submit) but `options` would be empty or omitted. The agent submits evidence via `--with-data` but the state auto-advances regardless of what values are submitted. The evidence is recorded in the log for audit purposes but doesn't drive routing.

This is the "data collection without branching" pattern: the state accepts structured data but has a single deterministic path forward.

The strategic design doesn't show an explicit example of this case, but the derivation rule follows naturally:
- `expects.fields` = derived from `accepts` (always present when accepts exists)
- `expects.options` = derived from transitions with `when` (empty/omitted when none have when)

### Q5: States with no accepts -- can they have expects?

No. The strategic design shows `"expects": null` for states without evidence requirements:
- Gate-blocked state (line 343): `"expects": null`
- Terminal state (line 372): `"expects": null`

The data flow confirms this (line 463): "if no accepts and gates pass: append transitioned event -> fsync -> continue." States without `accepts` auto-advance or block on gates; they never wait for evidence submission.

For `--to` directed transitions: the `--to` flag is an input modifier, not an output field. A state doesn't need `expects` to support `--to`. Directed transitions bypass the evidence routing system entirely -- they're a direct jump validated against the transition target list, not against `when` conditions. The data flow (lines 446-449) shows `--to` as a separate branch that "returns immediately" after appending the event.

### Q6: --to interaction with expects

The `--to` flag and `expects` operate in different phases:

1. **Without `--to`** (read-only call): `koto next` returns output including `expects` so the agent knows what to submit.
2. **With `--to`** (write call): `koto next --to <target>` appends a `directed_transition` event and returns immediately (DESIGN-unified-koto-next.md, lines 446-449).

The question is what the output looks like after a `--to` transition. The `--to` flag triggers state advancement, so the output reflects the NEW state after the directed transition (and any auto-advancement chain that follows). The `expects` in the response describes the new current state, not the state that was just left.

Key insight: `--to` doesn't suppress `expects`. It changes the state, and the response for the new state may or may not have `expects` depending on that state's template definition.

### Q7: event_type field

Every `expects` example in the strategic design shows `"event_type": "evidence_submitted"`. This is currently the only event type an agent can produce (the others -- `transitioned`, `integration_invoked`, `rewound` -- are system events).

The event taxonomy (DESIGN-unified-koto-next.md, lines 213-220) shows that `evidence_submitted` is triggered by `koto next --with-data` and `directed_transition` is triggered by `koto next --to`. Since `--to` bypasses the expects system entirely, `event_type` in expects is always `"evidence_submitted"`.

However, including it in the output is still valuable as a self-describing contract: it tells the agent what type of event to construct, even though there's currently only one option. If future event types are added (e.g., agent-initiated cancellation), the field provides extensibility without schema changes.

For implementation: `event_type` is a constant `"evidence_submitted"` in the expects output. It's not derived from template data -- it's baked into the derivation logic.

## Implications for Design

### Derivation is simple assembly, not computation

The `expects` object is assembled from template data, not computed from complex rules:
1. Check if state has `accepts`. If not, `expects = null`.
2. If yes, `expects.event_type = "evidence_submitted"` (constant).
3. `expects.fields` = map `accepts` entries, renaming `field_type` to `type`.
4. `expects.options` = filter transitions to those with `when`, serialize directly.

This means the dispatcher doesn't need complex logic -- it's structural mapping. The `ExpectsSchema` type mentioned in the CLI output contract design (line 132) can be built with a simple constructor that takes `&TemplateState`.

### The mixed conditional/unconditional case needs a policy decision

The validator allows states with `accepts` and unconditional transitions. The design should decide whether `expects.options` is:
- Omitted entirely (no `options` key)
- Present but empty array (`"options": []`)
- Present with unconditional transitions listed as `{ "target": "...", "when": null }`

The strategic design examples only show the fully conditional case. The cleanest choice is to omit `options` when there are no conditional transitions, matching the `skip_serializing_if` pattern used throughout the codebase.

### --to output describes the destination state

After a `--to` directed transition (and any subsequent auto-advancement), the output's `expects` field describes whatever state the engine stops at, not the origin. This means the dispatcher always operates on the current state after all mutations are applied.

### event_type is a constant for now

No need to derive it. Hard-code `"evidence_submitted"` and leave extensibility for the future.

## Surprises

1. **The tactical CLI output contract design (DESIGN-koto-cli-output-contract.md) is incomplete.** It ends at line 141 with the Decision Outcome section but has no Solution Architecture, no detailed `expects` derivation rules, and no response type definitions. The `ExpectsSchema` type is mentioned by name (line 132) but not defined. This means the expects derivation rules will need to be specified as part of completing that design.

2. **Mixed conditional/unconditional transitions are valid.** The validator allows a state with `accepts` to have transitions without `when` conditions (`types.rs:773-789` test). The strategic design doesn't address this case in its output examples. It's a gap that needs a design decision.

3. **The pairwise mutual exclusivity check was improved beyond what the strategic design specified.** The strategic design (line 301-308) says the compiler can only verify single-field conditions. But the implementation in `types.rs:250-285` handles multi-field conditions correctly, finding exclusivity when any shared field disagrees. The strategic design's limitation note is outdated relative to the implementation.

4. **`description` on FieldSchema has no output example.** The Rust type has it, the YAML source format supports it, but none of the strategic design's JSON output examples include it. It should propagate to `expects.fields` entries when present.

## Summary

The `expects` derivation is structural assembly, not computation: `accepts` maps 1:1 to `expects.fields` (with `field_type` renamed to `type`), conditional transitions map to `expects.options`, and `event_type` is a constant `"evidence_submitted"`. States without `accepts` produce `expects: null`. The `--to` flag doesn't suppress `expects` -- it changes state, and the response describes wherever the engine stops. Two gaps need resolution: the incomplete CLI output contract design (which should specify these rules formally) and the policy for `expects.options` when a state has `accepts` but only unconditional transitions.
