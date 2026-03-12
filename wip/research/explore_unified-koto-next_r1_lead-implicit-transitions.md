# Lead: Does implicit transition change the contract for agents?

## Findings

### How `koto transition` Works Today

The current `koto transition <target>` (main.go:240-284) requires the agent to explicitly name the target state. The flow:

1. Agent calls `koto transition <target>`
2. Engine validates:
   - Current state is not terminal (engine.go:152-158)
   - Target state is in allowed `Transitions` list (engine.go:161-169)
   - All gates on current state pass with AND logic (engine.go:186-206)
3. If valid, engine commits atomically and returns `{"state": "<target>", "version": N}`

The agent must supply the target. The engine decides whether it's valid. This puts branching control with the agent.

### What Gates Do

Gates are per-state **exit conditions** (DESIGN-koto-template-format.md:313-323, engine.go:550-601). Three types:

- `field_not_empty`: Evidence field exists and is non-empty
- `field_equals`: Evidence field equals a specific value
- `command`: Shell command exits 0 (default 30s timeout)

All gates on a state use AND logic — all must pass before the state can be exited. Gates check the accumulated evidence map only (not variables). Evidence persists across rewind.

Example from the design doc (transitions: `[plan, escalate]`):

```yaml
assess:
  transitions: [plan, escalate]
  gates:
    task_defined:
      type: field_not_empty
      field: TASK
```

Before the agent can transition away from `assess`, the `TASK` field must exist in evidence.

### Branching Support

The engine explicitly supports branching: `MachineState.Transitions` is a `[]string` (types.go:47), and states can have multiple valid next states. Example from DESIGN-koto-template-format.md:

```yaml
assess:
  transitions: [plan, escalate]
```

The agent chooses which one to take by passing it to `koto transition`. No gates distinguish between the branches—gates are state-level, not transition-level. The agent's choice is uncontrolled by the engine.

Error handling (main.go:161-169) returns a `ValidTransitions` list when the agent names an invalid target, helping the agent recover:

```json
{
  "error": {
    "code": "invalid_transition",
    "valid_transitions": ["plan", "escalate"]
  }
}
```

### What `koto next` Does Today

The current `koto next` (main.go:286-317) is read-only. It loads the engine and controller, then returns a directive for the current state:

```go
ctrl, err := controller.New(eng, tmpl)
d, err := ctrl.Next()
```

The controller (controller.go:51-90) generates:
- For non-terminal states: `{"action": "execute", "state": "<current>", "directive": "..."}`
- For terminal states: `{"action": "done", "state": "<current>", "message": "..."}`

It does **not** check gates. It does **not** advance state. It only tells the agent what to do now.

### Implicit Transitions: What Would Change

If `koto next` became the unified command that both reads the directive AND auto-transitions when gates are satisfied:

1. **Agent no longer names the target.** The engine would need to:
   - Identify all valid next states from current state
   - Check which ones (if any) have all gates satisfied
   - Automatically advance to the unique valid target

2. **Branching creates a fundamental problem.** If `assess` can go to `[plan, escalate]` and both satisfy their gates, how does the engine pick? Options:
   - **Single valid next state only**: Enforce templates to have exactly one valid target per state at any time. Gates become the branch selector.
   - **Agent still names the choice**: `koto next --target escalate` when gates are satisfied. This is `transition` renamed, not unified.
   - **Template-level priority**: Define transition priority in the template. Deterministic but requires a new template feature.
   - **First-in-order**: Auto-select the first transition alphabetically. Feels arbitrary.

3. **Gates would have to distinguish branches.** Currently, gates are state-level (you can't branch on gates). To support implicit transitions with branching, you'd need:
   ```yaml
   assess:
     transitions:
       - target: plan
         gates: [basic_complexity]
       - target: escalate
         gates: [high_complexity]
   ```
   
   This is a **contract change**: templates move from state-level gates to transition-level gates. The engine's `GateDecl` type (types.go:54-60) has no `target` field.

### Current Contract for Agents

Today's agent contract is simple:
1. `koto init --template <path> --name <workflow-name>` → creates state file
2. `koto next` → read current state and get a directive
3. Execute the directive (write code, run tests, etc.)
4. `koto transition <target>` → move to the next state
5. Repeat from step 2

The agent chooses the target. Gates are exit requirements (read-only, agent-uncontrolled). The agent learns valid targets from the error message if it guesses wrong.

### What Implicit Transitions Would Require of Agents

Under implicit transitions (if the design chosen is "single valid next per state"):

1. `koto init --template <path> --name <workflow-name>` → creates state file
2. `koto next` → reads current state, checks if gates are satisfied, auto-transitions if yes, returns next directive
3. Execute the directive
4. Repeat from step 2

**Removed from agent responsibility:**
- Naming the target state
- Understanding what state names are valid (engine handles that)
- Handling branching decisions (template must have only one valid next at a time)

**Added agent uncertainty:**
- The agent calls `koto next` and might receive either a directive (action="execute", stay in same state) or a done message (action="done"). What changed? The gates. But the agent doesn't know which gates exist or their status. This is opaque.

If branching is preserved via gates (transition-level gates), the agent would need to know:
- Which gates exist on which transitions
- How to satisfy them (submit evidence)
- What the gate selection logic is

This is more complexity, not less.

### Gates and Evidence

The engine accepts evidence via `WithEvidence()` (engine.go:45-49), but the CLI has no `--evidence` flag (main.go:240-284 only parses positional target). The current agent pattern is **command gates** (DESIGN-koto-agent-integration.md:172):

> The first skill (quick-task) uses `command` gates that verify conditions independently (e.g., running tests, checking file existence) rather than relying on agent-supplied evidence fields. This is a cleaner pattern for agent workflows -- koto verifies instead of trusting the agent's claims.

So agents don't actively supply evidence today. Gates are self-verifying (command gates). The agent just executes the directive and calls `koto next` or `koto transition`.

To support implicit transitions with branching, the agent would need to supply evidence to disambiguate branches. This requires:
1. An `--evidence` flag on `koto next` or `koto transition`
2. Agent knowledge of which evidence keys affect the transition choice
3. Template design that uses transition-level gates (not state-level)

## Implications

### For a Unified `koto next`

The design goal is: *Can `koto next` serve as the single command for all state evolution — reading the directive, accepting evidence, and triggering transitions — without growing the CLI as capabilities are added?*

**The answer depends on template constraints:**

**Option A: Single valid next per state (most unified)**
- Template rule: from any state, at most one transition can have all gates satisfied at any point in time.
- `koto next` with no arguments: auto-transitions if gates pass, otherwise returns a directive and stays put.
- Gates are state-level, not transition-level.
- Agent contract: call `koto next` until action="done". No target naming, no evidence supply, no branching knowledge.
- CLI surface: `koto next [--state <file>] [--state-dir <dir>]` — unchanged. Evidence acceptance is implicit (not via flag).
- Limitation: Branching workflows must have mutually exclusive gate conditions. Example: `assess` → `[plan, escalate]` must use command gates that check different conditions (task_simple vs task_complex).

**Option B: Branching with explicit evidence supply**
- Template rule: transition-level gates. Each transition has its own gate set.
- `koto next --target <state> [--evidence key=value ...]`: reads gates for the named transition, checks them, advances if they pass.
- This is `transition` renamed with optional evidence flags.
- Agent contract: call `koto next --target <name>` after executing directive. Agent must know branch points and what evidence to supply.
- CLI surface: `koto next` grows `--target` and `--evidence` flags. Not unified — it's the same as `transition`.

**Option C: Implicit transitions with priority**
- Template rule: transitions on a state have an implicit priority order. When multiple are valid, take the first.
- `koto next` auto-transitions if any gates pass.
- Works for branching, but "first wins" feels arbitrary and fragile to template edits.
- Agent never controls the branch choice; template author does implicitly.

### For Agent Integration

The DESIGN-koto-agent-integration.md explicitly rejects command-gate-free workflows: the reference skill (quick-task) uses command gates that verify independently. Agents don't supply evidence.

If implicit transitions with branching require transition-level gates and agent-supplied evidence:
- The agent skill must document which evidence keys to supply when
- The agent must monitor gate status (or the directive must hint at it)
- The template designer must carefully order branches by specificity
- This is more complex than the current "gates are self-verifying" model

### For Template Design

Current template: gates are state-level exit requirements, independent of which branch is taken next.

Implicit transitions with branching: gates become branch selectors, requiring transition-level gates and deeper agent involvement.

The design doc example shows clear branching intent:
```yaml
assess:
  transitions: [plan, escalate]
```

If this must be deterministic in an implicit system without agent input, it requires one of:
1. Mutually exclusive gate conditions on the state (handles both branches)
2. A priority order (first valid branch wins)
3. Agent-supplied evidence to disambiguate

Option 1 works well. Options 2 and 3 add complexity.

## Surprises

1. **No branching in practice yet.** I found the `assess → [plan, escalate]` example in the design doc but no actual template using it. The hello-koto skill and integration tests use linear workflows. This means branching behavior is designed but unproven.

2. **Gates are state-level, not transition-level.** The gate system (GateDecl in types.go) has no target field. If implicit transitions with branching are desired, the gate model must change.

3. **Agents don't supply evidence today.** The CLI has no `--evidence` flag, and the agent integration design explicitly prefers command gates that verify independently. Implicit transitions requiring agent evidence would reverse this design choice.

4. **Version control and optimistic concurrency are asymmetric.** The engine persists a version counter (engine.go:430-467) and rejects writes if the disk version has changed. But `koto next` doesn't check this before deciding whether gates pass. If gates depend on file state (command gates), and the file changes between gate check and transition, the gate becomes stale. This is benign for command gates (they re-run at transition time), but would be problematic for field-based gates with agent evidence.

## Open Questions

1. **Is branching a real requirement?** The design doc shows it, but no shipped templates use it. If templates are always linear (one state → one next state), implicit transitions are straightforward.

2. **Should gates live on transitions or states?** Current design: states. Implicit transitions with branching would likely need: transitions. This is a breaking change to the gate evaluation logic and template format.

3. **Can agents supply evidence without a flag?** The current design prefers command gates. Could implicit transitions work by letting command gates run at `koto next` time (before the agent acts) to verify readiness? This would require gates to be re-evaluable mid-state, not just at transition time.

4. **What does the directive mean under implicit transitions?** Today: "execute this phase." Under implicit transitions, does a directive mean "the previous gates passed, now execute this"? Or does it mean "prepare for this"? The semantics shift.

5. **How does the agent know branching happened?** If `koto next` auto-transitions, does it return the new directive, the old directive, or both? Does the agent see the transition history?

## Summary

Implicit transitions would remove the agent's need to name target states, unifying `koto next` as the single advancement command. However, this requires either (a) linear workflows with single valid next states at any time, or (b) agents supplying evidence to disambiguate branches. The current gate system is state-level; supporting implicit branching would require transition-level gates and changes to the gate evaluation logic. The agent integration design explicitly prefers command gates (self-verifying, agent-uncontrolled); implicit transitions with branching would reverse this choice. Whether branching workflows are a real use case is unclear — the design shows examples, but shipped templates are linear.

