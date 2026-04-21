<!-- decision:start id="skip-if-transition-target" status="assumed" -->
### Decision: Transition Target When skip_if Fires

**Context**

When skip_if conditions are satisfied, the engine must select a transition target without agent input. States can have unconditional transitions (no `when` clause), conditional transitions (with `when` clauses), or a mix. The template-schema research identified two viable approaches: require an unconditional fallback (simple, compile-time enforceable), or re-run transition resolution using skip_if condition values as synthetic evidence (selects from conditional transitions).

The motivating case for conditional-transition routing is `plan_validation`: it has two conditional transitions (`verdict: proceed → setup_plan_backed`, `verdict: skip → validation_exit`). When skip_if fires with `verdict: proceed`, the intent is to take the `proceed` path.

**Assumptions**

- Compile-time validation can enforce transition-target rules for skip_if states.
- Template authors understand that skip_if + unconditional-only works differently from skip_if + conditional transitions.

**Chosen: Re-run transition resolution with skip_if condition as synthetic evidence**

When skip_if fires, inject the skip_if condition key-value pairs as synthetic evidence into the merged evidence map, then call `resolve_transition()` normally. This selects the correct conditional transition without special-casing. For states with an unconditional fallback, the fallback is selected as usual.

Compile-time validation requires: if skip_if exists and all transitions are conditional, the skip_if condition values must match exactly one transition's `when` clause. Ambiguity (multiple matches) is a compile error. No-match (skip_if fires but no transition matches) is also a compile error.

**Rationale**

This approach reuses `resolve_transition()` without modification. The skip_if condition dict is already structured as `BTreeMap<String, Value>` -- the same type as evidence. Injecting it into the merged evidence map is a one-liner. By contrast, "require unconditional fallback" would force authors to add dummy unconditional transitions to states that semantically have conditional routing (`plan_validation`), making templates less clear. The synthetic-evidence approach is also internally consistent: skip_if effectively declares "I know what evidence would be submitted; here it is."

The compile-time ambiguity check prevents the main risk (multiple transitions matching a skip_if). Template authors get a clear error rather than silent routing surprises.

**Alternatives Considered**

- **Require unconditional fallback (compile-time enforced)**: Simpler rule -- every skip_if state must have exactly one unconditional transition. But it forces template authors to add dummy unconditionals to states with meaningful conditional routing. Rejected because it degrades template clarity for the primary use case (`plan_validation`).
- **Explicit target field in skip_if** (`skip_if: {target: foo, condition: {...}}`): Unambiguous, but requires a different YAML structure (not a flat conditions dict). Rejected because it breaks the reuse of the `when`-clause syntax and adds parsing complexity.

**Consequences**

- skip_if condition values are injected as synthetic evidence before `resolve_transition()` is called.
- `resolve_transition()` is called unchanged; the skip_if path and the normal path share the same resolution logic.
- Template compile step adds: for each skip_if state, verify the condition values match exactly one transition (or there's an unconditional fallback).
- The synthetic evidence from skip_if is NOT written to the event log as `EvidenceSubmitted` -- only the `Transitioned` event with `condition_type: "skip_if"` and `skip_if_matched` metadata is written.
<!-- decision:end -->
