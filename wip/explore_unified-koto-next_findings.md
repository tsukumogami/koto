# Exploration Findings: unified-koto-next

## Core Question

Can `koto next` serve as the single command for all state evolution in koto — reading the
current directive, accepting required data, and triggering transitions — without growing the
CLI surface as new capabilities are added? And what does that interface actually look like?

## Round 1

### Key Insights

- **koto next is a controller loop protocol, not a user-facing CLI** *(unified-cli-patterns)*
  The closest precedents are orchestration frameworks (Temporal, Kubernetes controllers), not
  user-facing CLIs. Agents call `koto next` in a loop the same way a controller calls Reconcile.

- **The `expects` field resolves the generic-vs-typed tension** *(data-input-model)*
  `koto next` (read) returns an `expects` schema describing what the current state accepts.
  The CLI interface stays constant (`koto next --submit <file>`); the schema varies by state.
  This is HATEOAS applied to CLI design — the same mechanism AI agents use for tool discovery.

- **The JSON output needs an explicit `op` field** *(error-model)*
  When `koto next` doubles as read and write, the response must declare which operation happened
  and what the outcome was. The gRPC error taxonomy maps directly: `precondition_failed`,
  `invalid_input`, `gate_blocked`. Agents branch on error code, not parsed text.

- **The engine already supports evidence storage — the gap is entirely at the CLI layer** *(future-use-cases)*
  `engine.WithEvidence()` exists. Gate evaluation exists. The unified model is a CLI design
  decision, not an engine architecture change.

- **Branching is designed but treated as unproven — CORRECTED by user** *(implicit-transitions)*
  The codebase has no branching templates yet, but branching and looping are essential, not
  edge cases. `/explore` itself demonstrates the requirement: loop through discover-converge
  rounds until user confirms crystallize. This is a two-way branch at every converge phase.

### Tensions

- **Implicit transitions and branching don't fit together without transition-level gates**
  Gates are currently state-level. Implicit auto-advancement with branching requires either
  mutually exclusive gate conditions or a new transition-level gate model. The evidence-based
  model (agent satisfies conditions that make exactly one branch valid) resolves this without
  adding new concepts — but requires transition-level gates.

- **`expects` schema depth** — simple type hint vs. full JSON Schema fragment. Not resolved.

### Gaps

- Full JSON schema for `koto next` output end-to-end not produced.
- Approval gate model only sketched (external write vs. koto-managed integration).

### User Focus (Round 1 corrections and clarifications)

1. **`koto next` is NOT idempotent.** It advances state when gates clear. CI gates, time-based
   gates, and other external checks mean re-calling is intentional. A separate read-only subcommand
   provides debugging/visibility for current state and unsatisfied blockers, but it is NOT part of
   the agent workflow loop.

2. **Branching and looping are essential.** `/explore` is the concrete example: loop until user
   decides to crystallize. Two-way branches at every converge phase. Must be supported.

3. **koto owns subprocess invocation for known integrations.** CI checks, delegate CLIs — koto
   bakes in knowledge of how to invoke these. The agent only runs processes when koto has no
   built-in integration. This means `koto delegate run` as a standalone command may not be needed:
   delegation is what `koto next` does when it encounters a delegation-tagged state and the agent
   calls it. One call — koto detects config, invokes delegate, captures response, returns next
   directive.

---

## Accumulated Understanding

### What we know

**The core model:**

`koto next` is the only state-evolution command. When called:
1. koto checks all gates on the current state (running subprocess gates like CI checks, evaluating
   evidence fields, etc.)
2. If gates clear → koto auto-advances to the next valid state and returns the new directive
3. If gates don't clear → koto returns the current directive and waits

There is no separate `koto transition` in this model. The agent's only loop is:
```
while true:
  directive = koto next [--submit <file>]
  if directive.action == "done": break
  execute directive
  if directive.expects: construct submission, call koto next --submit <file>
  else: call koto next (gates will clear when conditions are met)
```

**Branching via evidence:**

The agent controls which branch to take by satisfying conditions, not by naming a target. For
a two-way branch (`[discover, crystallize]`), the template defines mutually exclusive gate
conditions on each transition. The agent submits evidence (`{"user_decision": "crystallize"}`).
`koto next` checks transition-level gates, only one branch clears, auto-advances. The agent
never says "go to X" — it creates the conditions that make X the only valid path.

This requires **transition-level gates** (not the current state-level model). The gate model
must be extended.

**koto owns integrations:**

koto runs subprocess checks (CI, delegate CLIs, external APIs) as part of gate evaluation or
as part of `koto next` state actions. The agent's subprocess invocation is the fallback for
unknowns. This principle means:
- Delegation: koto detects delegation-tagged state + config, invokes delegate CLI as part of
  processing `koto next`, captures response, returns next directive. Agent calls `koto next` once.
- CI checks: koto queries CI as a gate condition. Agent doesn't manage this.
- Future: GitHub approvals, Slack approvals — koto-managed, not agent-managed.

**Read-only subcommand:**

A separate non-workflow command (e.g., `koto status`) provides visibility into current state and
unsatisfied gate blockers. Used for debugging, not part of the agent workflow loop.

### What's still open

1. **Transition-level gates design.** The gate model must change. What does transition-level gate
   syntax look like in templates? How does `GateDecl` change?

2. **What happens when `koto next` invokes a delegate.** Does the agent see the delegate response
   in the directive? Does koto store it as evidence? The agent may need to act on the delegate's
   output (e.g., incorporate findings into the next step).

3. **`expects` schema depth.** Type hint vs. full JSON Schema fragment.

4. **Approval gates.** External-initiated approvals — what is the koto-managed integration? Is
   it a webhook? A polling gate that checks an external API?
