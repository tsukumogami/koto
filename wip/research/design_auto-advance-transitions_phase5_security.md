# Security Review: auto-advance-transitions

## Dimension Analysis

### External Artifact Handling
**Applies:** Marginally — no new exposure beyond the existing template trust boundary.

`skip_if` conditions are authored inside template YAML frontmatter. That's the same input surface as the existing `when` clauses on transitions, `gates` declarations, and `accepts` field schemas. No new external sources are introduced: the engine parses template YAML at `koto template compile` time and evaluates conditions against in-memory `serde_json::Value` at advance time. The advance loop performs dot-path equality checks — it does not execute the `skip_if` values as code or pass them to any subprocess.

The only processing risk is malformed YAML. `serde_yaml_ng` already handles all frontmatter parsing; `skip_if: BTreeMap<String, serde_json::Value>` adds no new parser code paths beyond what `when` clauses already exercise. Compile-time validation catches structural errors (skip_if on terminal states, skip_if with no transitions, ambiguous routing) before any state file is written.

No mitigations needed beyond what already exists.

---

### Permission Scope
**Applies:** No.

`skip_if` evaluation is pure in-memory work. The feature adds no file reads beyond the template file that already has to be read for the workflow to function, no network calls, and no new process spawns. The `has_gates_routing` extension that scans `skip_if` keys for `gates.*` references is a string scan over a `BTreeMap` that was already in memory.

The gate commands that produce the output consumed by `gates.*` skip_if conditions are evaluated by the existing gate runner, under the same permission scope as before. `skip_if` does not cause gates to run that would not otherwise run — gates are still evaluated as part of step 6 of the advance loop before `skip_if` is checked.

No permission escalation is possible.

---

### Supply Chain or Dependency Trust
**Applies:** No.

`serde_json::Value` and `serde_yaml_ng` are already in the dependency tree. The design adds no new crates. The `conditions_satisfied()` helper is new Rust code in the existing codebase; it implements equality comparison over types already handled by `serde_json`. There is no new transitive dependency surface.

Template files are the same artifact class that already controls the entire workflow graph — gate commands, transition routing, integration names, action commands. Trusting a template file's `skip_if` block requires no more trust than the rest of the template. Template authors already have the ability to make states auto-transition via unconditional transitions; `skip_if` makes that condition-dependent, which is strictly less powerful than the status quo for a malicious template author.

No supply chain concerns.

---

### Data Exposure
**Applies:** Partially — a minor documentation note is warranted.

The `skip_if_matched` field on `EventPayload::Transitioned` writes the matched condition map to the JSONL state file. The values that appear in this map come from three sources:

1. **Template constants** — literal values in `skip_if` (e.g., `true`, `"some_string"`). No sensitivity; these are authored by the template.
2. **Template variables** — values supplied by the user at `koto init --with NAME=value`. These are already written verbatim to `WorkflowInitialized.variables`, so `skip_if_matched` adds no new exposure for this category.
3. **Gate output** — the structured output of a gate at evaluation time. Gate output is already written verbatim to `GateEvaluated.output` events, so `skip_if_matched` again adds no new exposure.

The state file already contains all three data categories before this feature. The new field is a projection of data that already exists in the log. The state file lives in the user's home directory (`~/.koto/` or a session-relative path) with filesystem permissions controlled by the OS. No new data categories are introduced and no data leaves the local machine.

**Note worth documenting:** Template authors who store sensitive values in `skip_if` predicates (e.g., a specific token value rather than a boolean existence check) will see those values reflected in the `skip_if_matched` field. This is not a new attack surface relative to the existing `when` clause evidence pattern, but it could surprise authors who treat skip_if as an opaque check rather than a recorded condition. Adding a brief note to the template authoring documentation that `skip_if_matched` records the exact condition values is appropriate.

---

### Injection and Condition Manipulation
**Applies:** Marginally — trust boundary is clear, no new attack surface within it.

`skip_if` conditions are template-authored constants compared against in-memory evidence via `serde_json::Value` equality. There is no string interpolation, no shell expansion, and no code evaluation. An attacker who can write arbitrary `skip_if` values can also write arbitrary gate commands, transition routing, and action commands — so `skip_if` manipulation provides no capability uplift beyond what already exists for a compromised template.

Within the existing trust boundary (authentic template + authentic evidence), the concern becomes whether a legitimate template can be crafted to cause unintended transitions at runtime. Three mitigations already address this:

1. **Compile-time validation** catches `skip_if` on terminal states (E-SKIP-TERMINAL), `skip_if` with no transitions (E-SKIP-NO-TRANSITIONS), and ambiguous routing (E-SKIP-AMBIGUOUS) before the template is usable.
2. **The visited-set** prevents a `skip_if` chain from revisiting a state it has already auto-transitioned through in the same invocation, which blocks cycles.
3. **`MAX_CHAIN_LENGTH = 100`** caps consecutive skip_if transitions, preventing denial of service via a linear chain of 1000 auto-advancing states.

One subtle consideration: E-SKIP-AMBIGUOUS validation uses synthetic evidence to detect whether `skip_if` conditions could lead to ambiguous routing. If the condition evaluator and the compile-time validator diverge (e.g., future changes to `resolve_transition()` add new matching logic), the compile-time guarantee may no longer fully hold at runtime. This is not a present gap but is worth noting in the implementation plan as an invariant to preserve.

---

### Chaining and Loop Safety
**Applies:** Yes — addressed by existing mechanisms; no new gaps.

A template could declare many consecutive states all with `skip_if` conditions that are always true. The advance loop handles this via two guards applied to skip_if transitions on the same code paths as all other transitions:

- **`MAX_CHAIN_LENGTH = 100`**: If `skip_if` fires 100 times in a single `advance_until_stop()` call, the loop returns `StopReason::ChainLimitReached`. The workflow is not corrupted; the next `koto next` invocation starts from the state where the chain halted.
- **Visited-set cycle detection**: The visited set tracks states transitioned through during the current invocation. If `skip_if` on state B would advance to state A (which was already visited this invocation), `StopReason::CycleDetected` fires before the transition is persisted.

The chain limit and cycle detection existed before this feature and are not relaxed by it. The only scenario not caught is a very long *acyclic* linear chain (e.g., 99 auto-advancing states followed by a user-driven state). This is by design: the chain limit is a defense-in-depth guard against template bugs, not a policy limit on template depth. A template author who intentionally designs 99 auto-advancing states gets what they asked for; one who creates them accidentally will see `ChainLimitReached` as a signal to investigate.

No new loop safety concerns.

---

## Recommended Outcome

**OPTION 2 - Document considerations:**

No design changes are needed. The two considerations worth capturing are:

1. **skip_if_matched records exact condition values.** Template authors should know that values appearing in `skip_if` predicates are written verbatim to the state file under `skip_if_matched`. This matters if a template uses a specific value (rather than a boolean) as the predicate — that value is preserved in the event log. Add a sentence to the koto-author skill documentation noting this behavior.

2. **Compile-time / runtime evaluator divergence is an invariant to preserve.** The compile-time E-SKIP-AMBIGUOUS validation uses the same condition evaluator as the runtime. Future changes to `resolve_transition()` must ensure the compile-time path stays aligned. A comment in the compile validation code noting this dependency is sufficient.

Neither consideration is a blocker. Both are low-severity observations about expected system behavior rather than vulnerabilities.

---

## Summary

The `skip_if` design introduces no new attack surface, permission escalation, or external input sources. Condition evaluation is pure in-memory equality comparison over data already present in the evidence map; the existing chain limit and cycle detection bound execution; and compile-time validation prevents the template errors most likely to cause unintended auto-advancement. The two considerations identified — that matched condition values are recorded in the event log, and that the compile-time evaluator should stay aligned with the runtime evaluator — are documentation notes rather than design defects.
