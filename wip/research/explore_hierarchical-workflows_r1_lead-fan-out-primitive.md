# Lead: What's the right primitive for fan-out?

## Findings

### Current Architecture Summary

The advance loop (`src/engine/advance.rs`) processes states in a sequential chain. At each iteration it checks seven stopping conditions in order: signal, chain limit, terminal, integration, action execution, gates, and transition resolution. The loop signature is:

```rust
pub fn advance_until_stop<F, G, I, A>(
    current_state: &str,
    template: &CompiledTemplate,
    evidence: &BTreeMap<String, serde_json::Value>,
    all_events: &[Event],
    append_event: &mut F,
    evaluate_gates: &G,
    invoke_integration: &I,
    execute_action: &A,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
```

**StopReason enum** (`src/engine/advance.rs:52-85`): Terminal, GateBlocked, EvidenceRequired, Integration, IntegrationUnavailable, CycleDetected, ChainLimitReached, ActionRequiresConfirmation, SignalReceived, UnresolvableTransition.

**Gate types** (`src/gate.rs`): Three types today -- `command` (shell exit code), `context-exists` (key presence in context store), `context-matches` (regex on context value). Each gate produces a `StructuredGateResult` with an `outcome` (Passed/Failed/TimedOut/Error) and a typed `output` JSON value. The gate evaluator runs all gates without short-circuiting.

**Gate struct** (`src/template/types.rs:86-109`): Fields are `gate_type`, `command`, `timeout`, `key`, `pattern`, `override_default`. The struct is flat -- all gate types share the same field bag, with unused fields defaulting to empty.

**Template state** (`src/template/types.rs:46-63`): Contains `directive`, `details`, `transitions`, `terminal`, `gates`, `accepts`, `integration`, `default_action`. No `children` or `spawn` concept exists today.

**Actions** (`src/action.rs`): Only `run_shell_command` -- a simple shell command executor with process-group isolation and timeout. Actions are not a typed dispatch system; `ActionDecl` in `types.rs` has `command`, `working_dir`, `requires_confirmation`, and `polling`.

**NextResponse** (`src/cli/next_types.rs`): Six variants mapping to action strings: `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `done`, `confirm`. These are the JSON shapes agents receive from `koto next`.

**Event types** (`src/engine/types.rs`): WorkflowInitialized, Transitioned, EvidenceSubmitted, IntegrationInvoked, DirectedTransition, Rewound, WorkflowCancelled, DefaultActionExecuted, DecisionRecorded, GateEvaluated, GateOverrideRecorded. No parent/child relationship events exist.

**Session model**: `koto init` creates a workflow in a session directory. Sessions are flat -- a session has one state file, one compiled template, and context entries. No parent-child linking exists in the session backend.

---

### Approach A: Gate-based (`children-complete`)

**Concept**: Add a new gate type `children-complete` (or `child-status`) that checks whether child workflows have reached specific states. The parent template declares these gates on states where it needs to wait for children. Fan-out itself is implicit in the directive -- the agent reads the directive, spawns children externally, then the gate blocks until children reach the expected state.

**How it fits existing code**:
- Gate evaluation already handles multiple types via a match arm in `evaluate_gates()` (`src/gate.rs:64-81`). Adding a new arm is straightforward.
- The `Gate` struct's flat field bag could accommodate a `children` field (list of child workflow names/patterns) and an `expected_state` field, though the struct is getting crowded.
- Gate output schema (`gate_type_schema` in `src/template/types.rs:176-184`) would need a new entry.
- The `built_in_default` function needs a new arm.
- The compiler (`src/template/compile.rs`) would need validation for the new gate type.

**Advance loop changes**: Minimal. The advance loop doesn't know about gate types -- it just calls `evaluate_gates` and checks outcomes. A new gate type is invisible to the loop itself. The gate evaluator would need access to the session backend to query child workflow state, which means `evaluate_gates` needs an additional parameter (or the gate closure already has access through its closure environment in `src/cli/next.rs`).

**Template schema impact**: Low. Only the `Gate` struct gains new optional fields. No new top-level state fields.

**Convergence**: Natural. Gates already block advancement. A `children-complete` gate would query child state files (via session backend), check that each child has reached its expected state, and return Passed/Failed. The advance loop blocks as it does for any other failed gate. The agent can use `koto overrides record` to bypass if needed.

**Fan-out**: Not modeled by this approach. The directive tells the agent to spawn children, but koto doesn't know how many children or what templates they use until the gate evaluates. This is a problem -- the gate needs to know which children to check, but who declares that? Options:
  1. The gate itself lists expected child workflow names (static, known at template-author time)
  2. The agent registers children via evidence/context, and the gate reads from context store
  3. A separate mechanism (like `koto init --parent`) registers the relationship, and the gate queries all children of the current workflow

Option 3 is cleanest -- it separates the "register relationship" step from the "check status" step. The gate just checks "all children of this workflow are in state X."

**Strengths**: Minimal code changes. Consistent with existing patterns. Gate output feeds into transition routing via `gates.*` when clauses, enabling different paths based on child outcomes.

**Weaknesses**: Fan-out is invisible to koto -- it only sees the convergence point. No template-level declaration of what children should exist. The directive carries all the spawn instructions as prose, which is fragile.

---

### Approach B: Action-based (`spawn`)

**Concept**: Add a new action type (beyond shell commands) that `koto next` returns to tell the agent to spawn child workflows. The agent executes `koto init` for each child, then calls `koto next` again. The action declares which templates to use and how many children.

**How it fits existing code**:
- `ActionDecl` (`src/template/types.rs:115-124`) is currently command-specific: `command`, `working_dir`, `requires_confirmation`, `polling`. A spawn action would need different fields: `template`, `count`, `naming_pattern`.
- The advance loop's action handling (`advance.rs:269-296`) calls `execute_action` and expects `ActionResult` (Executed/Skipped/RequiresConfirmation). A spawn action doesn't fit this model because koto doesn't launch agents -- the *agent* needs to receive instructions and act on them.
- `NextResponse` would need a new variant (e.g., `SpawnRequired`) or the action output would need to carry spawn instructions.

**Advance loop changes**: Significant. Either:
  1. Add a new `StopReason::SpawnRequired` variant and corresponding `NextResponse::SpawnRequired`, which breaks the current six-response contract.
  2. Overload `ActionRequiresConfirmation` to carry spawn metadata, which is semantically misleading.

**Template schema impact**: Medium-high. `ActionDecl` would need to become an enum or gain optional spawn-specific fields. The compiler would need new validation paths.

**Convergence**: Separate from spawn. After spawning, the agent still needs gates (or something) to wait for children. So this approach actually requires *both* a spawn action *and* a convergence gate -- two new primitives instead of one.

**Strengths**: Makes fan-out explicit in the template. koto knows what children should be created.

**Weaknesses**: Requires two new primitives (spawn + wait). `ActionDecl` is tightly coupled to shell commands. Adding a new action type is a bigger refactor than adding a gate type. The agent still does the actual spawning, so the "action" is really just an instruction -- it doesn't execute anything. This doesn't match the existing action model where koto runs the command and reports the result.

---

### Approach C: State-level declaration (`children` block)

**Concept**: Add a `children` field to `TemplateState` that declares child workflows to spawn and a `wait_for` condition. The advance loop treats a state with `children` as a special case: it returns spawn instructions, then on re-entry checks child status.

**How it fits existing code**:
- `TemplateState` (`src/template/types.rs:46-63`) would gain a new field: `children: Option<ChildrenDecl>` with sub-fields for template, count, and completion condition.
- The advance loop would need a new step between action execution and gate evaluation (or replacing one of them).
- The `NextResponse` would need a new variant or the existing variants would need to carry child-related metadata.

**Advance loop changes**: Most significant of the three. A new step in the loop (or a new StopReason) would be needed. The loop currently has a clean seven-step pipeline. Adding a children-aware step that behaves differently on first vs. subsequent visits (spawn vs. wait) introduces statefulness *within* a single state, which the current model doesn't have.

**Template schema impact**: Highest. New struct types (ChildrenDecl, ChildSpec), new compiler validation, new serialization.

**Convergence**: Built into the declaration. The `children` block would specify both what to spawn and when to consider them complete. This is the most complete model but also the most complex.

**Strengths**: Template authors declare intent fully. Fan-out and convergence are a single concept. Static analysis (template compilation) can validate child references.

**Weaknesses**: Largest implementation surface. Introduces a "stateful state" concept (first visit = spawn, subsequent visits = check) that doesn't exist today. The advance loop's clean pipeline gets a special case that's fundamentally different from gates, actions, and integrations. Risk of the `children` block becoming a mini-language for describing child orchestration.

---

### Cross-cutting concern: How does koto learn about child workflows?

All three approaches need a mechanism for registering parent-child relationships. Options:

1. **`koto init --parent <workflow>`**: The agent passes the parent workflow name when creating children. koto records the relationship in both state files (parent gets a `ChildSpawned` event, child header gets a `parent` field). This is the cleanest because it's a single CLI flag on an existing command.

2. **Evidence/context-based**: The agent writes child workflow names to the parent's context store or evidence. Gates/children-blocks query this. Indirect and fragile.

3. **Convention-based**: Child workflows follow a naming pattern (e.g., `parent.child-1`). koto infers relationships from names. Simple but inflexible.

Option 1 is strongly preferred regardless of which approach is chosen for fan-out.

---

### Comparison Matrix

| Criterion | A: Gate | B: Action | C: State-level |
|-----------|---------|-----------|----------------|
| Code changes to advance loop | None | Medium | High |
| New primitives needed | 1 (gate type) | 2 (action + gate) | 1 (children block) |
| Template schema impact | Low | Medium | High |
| Fan-out visibility to koto | None (agent-driven) | Explicit | Explicit |
| Convergence model | Natural (gate blocks) | Needs separate gate | Built-in |
| Fits existing patterns | Yes | Partially | No (new concept) |
| Static validation possible | Limited | Partial | Full |
| Agent complexity | Medium | High (two steps) | Low |

## Implications

**Approach A (gate-based) is the strongest fit for the current codebase.** It requires the fewest changes, follows established patterns, and the convergence model falls out naturally from existing gate mechanics. The main gap -- fan-out visibility -- can be addressed incrementally: start with gate-based convergence, add `koto init --parent` for relationship registration, and later add template-level declarations if the directive-as-prose approach proves too fragile.

The gate-based approach also preserves a key design principle: koto doesn't launch agents. Adding a `spawn` action or `children` block creates pressure to make koto more orchestration-aware, moving toward a model where koto needs to understand agent lifecycle. The gate approach keeps koto in its lane -- it checks conditions and reports status. The agent remains responsible for all external actions.

The `children-complete` gate type would need access to the session backend, which is a new dependency for gate evaluation. Currently gates get a `working_dir` and optional `context_store`. The gate closure in `src/cli/next.rs` already has access to the backend through its environment, so wiring this through is straightforward -- the new gate type just needs the session backend passed through (or the gate closure captures it).

If the project later wants explicit fan-out declarations in templates, approach C can be layered on top as a higher-level abstraction that compiles down to gates + directives. This keeps the engine simple while allowing template authors to express intent more declaratively.

## Surprises

1. **Actions are not a dispatch system.** The `ActionDecl` struct is tightly coupled to shell commands. There's no `action_type` discriminator -- every action is a shell command with `run_shell_command`. This makes approach B harder than it initially appears, because "add a new action type" means refactoring `ActionDecl` from a struct into an enum, touching the compiler, the advance loop, and all six `NextResponse` serialization paths.

2. **The gate evaluator runs independently of the advance loop.** Gates are evaluated by a standalone function that takes a map of `Gate` definitions and returns results. The advance loop just calls this function and checks outcomes. This clean separation makes adding a new gate type nearly trivial from the loop's perspective -- the loop doesn't even need to change.

3. **Gate output already feeds into transition routing.** The `gates.*` evidence namespace (`GATES_EVIDENCE_NAMESPACE`) means a `children-complete` gate's output could drive conditional transitions. For example, a parent could route differently based on whether all children passed vs. some failed, using `when: { "gates.children_check.all_passed": true }`. This is a powerful capability that falls out for free from approach A.

4. **The context store is already a natural place for child relationship data.** Context gates (`context-exists`, `context-matches`) check the session's context store. A `children-complete` gate could use the context store to find registered children (written there by `koto init --parent`), avoiding the need for any new storage mechanism.

## Open Questions

1. **Should `koto init --parent` write to the parent's state file, the child's state file, or both?** Writing to both enables bidirectional querying but introduces a write to the parent's log from a child's init, which complicates the single-writer model.

2. **How should the `children-complete` gate identify which children to check?** By explicit list (fragile, requires template author to know child names), by naming pattern (convention-dependent), or by querying all workflows with a parent pointer to the current workflow (most flexible)?

3. **What completion condition should the gate check?** Terminal state only? Specific state name? Any state matching a pattern? The simplest is "child workflow has reached a terminal state" but real workflows may need "child is in state X with evidence Y."

4. **Should gate output include per-child status?** A simple boolean "all children done" is easy but loses information. A map of `{child_name: {state, evidence}}` is richer but produces unbounded output for large fan-outs.

5. **How does the override mechanism work for children-complete gates?** The existing `koto overrides record` system lets agents bypass blocked gates. For children-complete, the override default would need to represent "pretend all children are done" -- what's the right shape for that?

## Summary

A new `children-complete` gate type is the best fit for the current codebase because it requires no advance-loop changes, follows the established pattern of adding gate types (a single match arm in the evaluator), and convergence falls out naturally from the existing gate-blocks-advancement model. The main trade-off is that fan-out itself stays invisible to koto (the directive tells the agent what to spawn, but the template doesn't declare it structurally), which can be addressed incrementally by layering declarative children blocks on top later. The biggest open question is how the gate identifies which children to check -- explicit listing, naming convention, or querying all workflows with a parent pointer -- since this choice determines whether `koto init --parent` alone is enough or whether additional registration steps are needed.
