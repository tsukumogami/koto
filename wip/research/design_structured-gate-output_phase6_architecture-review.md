# Architecture Review: DESIGN-structured-gate-output

## Scope

Review of `docs/designs/DESIGN-structured-gate-output.md` against the current
codebase state. Focused on structural fit, feasibility of the evidence map type
change, edge cases in `resolve_value`, alternative approaches, and serialization
requirements.

## Question 1: Is the architecture clear enough to implement?

**Verdict: Yes, with one gap to close.**

The design identifies the three layers that change (gate evaluation, advance
loop, CLI response) and specifies the data flow between them. The phased
implementation approach (Phase 1: types, Phase 2: resolver, Phase 3: CLI) is
a valid compilation-order that keeps the repo buildable after each phase.

**Gap: the `Transition.when` type stays as `BTreeMap<String, Value>`.** The
design says `resolve_transition` changes its evidence parameter from
`BTreeMap<String, Value>` to `&serde_json::Value`, but the `when` field on
`Transition` (`src/template/types.rs:70`) remains a
`BTreeMap<String, serde_json::Value>`. The condition matching loop at
`advance.rs:411-413` iterates `conditions.iter()` where `conditions` is the
`when` BTreeMap, and compares each `(field, expected)` against the evidence.
With the proposed `resolve_value` approach, `field` becomes a dot-path key
and `expected` stays as `&Value`. This works -- no change needed to
`Transition.when`. But the design should state this explicitly so implementers
don't try to change the template type too.

## Question 2: Evidence map change from BTreeMap to Value -- caller impact

**Feasibility: moderate. Roughly 15-20 call sites, but mechanically updatable.**

Current callers of `resolve_transition` that pass `&BTreeMap<String, Value>`:

| Location | Nature |
|----------|--------|
| `advance.rs:319` | The only production call site |
| `advance.rs:568-707` | ~13 test call sites in `resolve_transition` unit tests |

Current callers of `advance_until_stop` that construct `BTreeMap<String, Value>` evidence:

| Location | Nature |
|----------|--------|
| `advance.rs:166` | Function signature |
| `advance.rs:184` | `current_evidence` clone |
| `advance.rs:343` | Fresh `BTreeMap::new()` for auto-advanced states |
| `cli/mod.rs` | Upstream caller (constructs evidence from `merge_epoch_evidence`) |
| `advance.rs:453` `merge_epoch_evidence` | Returns `BTreeMap<String, Value>` |
| All `advance_until_stop` tests | Construct evidence BTreeMaps |

The design says to wrap with `serde_json::to_value()` at each call site. That's
correct for `resolve_transition` callers. But for `advance_until_stop`, the
design proposes that evidence merging happens *inside* the advance loop (gate
output merged with agent evidence). This means:

- `advance_until_stop` can keep its `&BTreeMap<String, Value>` parameter --
  the conversion to `Value::Object` happens internally before calling
  `resolve_transition`.
- Only `resolve_transition` itself and its direct callers (the one production
  call + ~13 tests) need the type change.

**This is feasible.** The test updates are mechanical: wrap
`BTreeMap::new()` in `serde_json::Value::Object(Map::new())` or use
`serde_json::to_value(&evidence).unwrap()`. No test logic changes.

**Advisory: `merge_epoch_evidence` return type.** Currently returns
`BTreeMap<String, Value>`. The design doesn't propose changing this, and it
shouldn't -- it produces flat agent evidence. The merge with gate output
happens in the advance loop, which is the right place. Just noting this is
correctly scoped.

## Question 3: Edge cases in resolve_value

The proposed implementation:

```rust
fn resolve_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}
```

**Edge case analysis:**

| Input | Behavior | Correct? |
|-------|----------|----------|
| Empty string `""` | `split('.')` yields one empty segment `""`, `current.get("")` returns `None` for any object | Yes -- correctly fails to match |
| Single segment `"mode"` | One iteration, equivalent to `root.get("mode")` | Yes -- backward compatible |
| Trailing dot `"gates."` | Last segment is `""`, returns `None` | Yes -- no false match |
| Leading dot `".gates"` | First segment is `""`, returns `None` immediately | Yes -- no false match |
| Double dot `"gates..ci"` | Middle segment is `""`, returns `None` | Yes -- no false match |
| Numeric index `"gates.0"` | `Value::get("0")` works on objects (key "0"), returns `None` on arrays | **Partial.** Works for object keys. Does NOT support array indexing. |
| Key containing dots `"a.b"` | Treated as path `a` -> `b`, not literal key | **Ambiguity.** See below. |

**Blocking concern: dot-in-key ambiguity.** If an agent submits evidence with
a key containing a literal dot (e.g., `"file.name"`), the resolver would try
to traverse it as a path. Today's `BTreeMap::get("file.name")` does an exact
key lookup. After this change, `resolve_value(evidence, "file.name")` would
look for `evidence["file"]["name"]` instead.

However, looking at the codebase: `when` clause keys come from template YAML,
and evidence keys come from agent `--with-data` submissions. Both are
currently flat strings. The design introduces dotted keys (`gates.ci.exit_code`)
specifically for gate output. Agent evidence keys are unlikely to contain dots
today, but the design should document that dot-separated keys in `when`
clauses are always treated as paths, and agent evidence keys should not
contain dots.

**Advisory, not blocking**: this is a documentation gap, not a structural flaw.
The resolver behavior is deterministic and consistent. If a future need arises
for literal dot keys, an escape mechanism can be added.

**Array indexing**: The design doesn't claim to support arrays, and gate output
schemas are all flat objects. Not a gap for Feature 1.

## Question 4: Simpler alternatives overlooked?

**Considered: flatten gate data into dot-separated BTreeMap keys.**

The design considered and rejected this (lines 179-183). The rejection reasoning
is sound: flattening loses structure, and the key ambiguity it creates (is
`"gates.ci.exit_code"` a flat key or a path?) is worse than the ambiguity from
the chosen approach, because with flattening both flat and dotted keys coexist
in the same map with no way to distinguish them.

**Alternative not considered: keep BTreeMap, add a separate gate_evidence parameter.**

Instead of changing `resolve_transition`'s evidence type from `BTreeMap` to
`Value`, keep the `BTreeMap` parameter for agent evidence and add a second
parameter `gate_evidence: &BTreeMap<String, Value>` where gate names map to
their structured output. The resolver checks prefixed keys (`gates.X.Y`)
against the gate evidence map using a specialized lookup, and unprefixed keys
against the agent evidence map using the existing `get()`.

Pros: no type change on `resolve_transition`, no caller updates, no ambiguity
between flat and nested keys. Cons: two parameters instead of one, and the
resolver needs to parse the `gates.` prefix to decide which map to query.
This is arguably *more* parallel-pattern-like (two evidence sources with
different lookup logic).

**Assessment: the design's chosen approach is cleaner.** One evidence value,
one lookup path, one matching algorithm. The type change is mechanical and
contained. The alternative would avoid the type change but introduce a
branching concern in the resolver that would compound as more namespaces are
added.

## Question 5: Does StructuredGateResult need Serialize/Deserialize?

**Yes, it needs `Serialize`. It does not need `Deserialize`.**

Current data flow for gate results:

1. `evaluate_gates` returns `BTreeMap<String, GateResult>` (no serialization)
2. `StopReason::GateBlocked(BTreeMap<String, GateResult>)` carries results to
   the CLI layer (no serialization of `GateResult` itself)
3. `blocking_conditions_from_gates` (`next_types.rs:397`) converts
   `GateResult` variants to `BlockingCondition` structs (which do derive
   `Serialize`)
4. `GateResult` is never serialized directly -- only destructured into
   `BlockingCondition` fields

With the proposed change:

- `StructuredGateResult.output` is a `serde_json::Value`, which is already
  serializable.
- The `output` field will be included in `BlockingCondition` (per R4a), so
  `BlockingCondition` needs a new `output: serde_json::Value` field. This
  field serializes via serde_json, not via a derive on `StructuredGateResult`.
- `GateOutcome` needs to be convertible to a string for the `status` field
  of `BlockingCondition` (same pattern as today's match on `GateResult`
  variants).

**For event logging**: The design mentions R11 (event ordering) but doesn't
propose a new event type for gate output. Gate results flow through
`StopReason`, not through `EventPayload`. If a future feature adds a
`GateEvaluated` event payload, `StructuredGateResult` would need `Serialize`
at that point. For Feature 1, it doesn't.

**Recommendation**: Derive `Serialize` on both `GateOutcome` and
`StructuredGateResult` proactively. Cost is zero (one derive line), and it
avoids a follow-up change when event logging is added. Do not derive
`Deserialize` -- gate results are never read from external input.

## Structural assessment

### Fits well

- **No action dispatch bypass.** Gate evaluation stays behind the
  `evaluate_gates` function boundary. The advance loop calls it through the
  closure parameter `G`, preserving the existing injection pattern.
- **No parallel pattern.** The design replaces `GateResult` with
  `StructuredGateResult` rather than adding a second result type alongside it.
  Clean substitution.
- **Dependency direction preserved.** `gate.rs` has no new imports. The
  advance loop imports from `gate.rs` (same direction as today). CLI imports
  from both (same direction as today).
- **Template type untouched.** `Transition.when` keeps its
  `BTreeMap<String, Value>` type. The dot-path traversal is a resolver
  implementation detail, not a template schema change.

### Potential concerns

**Advisory: `Value::Object` vs `BTreeMap` in advance_until_stop signature.**
The design says the evidence map changes from `BTreeMap` to `Value::Object`,
but the cleanest implementation keeps `advance_until_stop`'s parameter as
`BTreeMap` and only converts to `Value` internally for the
`resolve_transition` call. The design's summary at line 239 could be read as
changing the public API of `advance_until_stop`, which would be a much larger
change. Phase 2 implementation should clarify that the conversion is internal.

**Advisory: `BlockingCondition` output field.** The design says each blocking
condition includes "the gate's structured output field" but doesn't show the
updated `BlockingCondition` struct. The current struct has four fields: `name`,
`condition_type`, `status`, `agent_actionable`. Adding `output: Value` is
straightforward, but the design should specify it explicitly to avoid
implementers guessing the field name or type.

## Summary of findings

| # | Finding | Severity |
|---|---------|----------|
| 1 | `resolve_value` handles edge cases correctly (empty path, trailing dots, leading dots). Dot-in-key ambiguity is a documentation gap, not a structural flaw. | Advisory |
| 2 | Evidence type change is contained: ~1 production call site + ~13 tests for `resolve_transition`. `advance_until_stop` signature can stay as `BTreeMap`. | Not a concern |
| 3 | Derive `Serialize` on `GateOutcome` and `StructuredGateResult` proactively for future event logging. Skip `Deserialize`. | Advisory |
| 4 | Design should explicitly state that `Transition.when` type does not change. | Advisory |
| 5 | Design should show the updated `BlockingCondition` struct with the `output` field. | Advisory |
| 6 | No structural violations detected. Type replacement is clean, dependency direction is preserved, no parallel patterns introduced. | Pass |
