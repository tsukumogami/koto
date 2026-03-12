# Explore Scope: unified-koto-next

## Core Question

Can `koto next` serve as the single command for all state evolution in koto — reading the
current directive, accepting required data, and triggering transitions — without growing the
CLI surface as new capabilities are added? And what does that interface actually look like?

## Context

koto is a workflow orchestration engine for AI coding agents. Today, state evolution uses
two separate commands: `koto next` (read current directive) and `koto transition` (advance
state). The cross-agent delegation design (issue #41) introduced a third: `koto delegate run`.
Issue #43 challenges this direction: since the state machine already knows what's valid at
any moment, should the CLI express that as a single adaptive `koto next` rather than separate
commands for each action?

koto has no users yet and no backward compatibility constraints. The goal is to establish
`koto next` as the sole state-evolution interface before downstream features (delegation,
evidence submission, approval gates) are implemented, so those designs build on top of this.

## In Scope

- Shape of the unified `koto next` interface (flags, stdin, data submission)
- How delegation fits into this model as a concrete test case
- Implicit vs. explicit transitions
- Error signaling when `koto next` is called in wrong state or with wrong data
- Extensibility: how future capabilities plug in without new subcommands

## Out of Scope

- Internal controller/engine implementation details (covered by delegation design)
- Config system and tag vocabulary (already decided upstream in delegation design)
- Non-state-evolution commands (`koto status`, `koto rewind`, `koto query`)

## Research Leads

1. **How do other tools unify multi-phase CLI interactions into a single command?**
   Tools like Terraform (`apply`), git (`merge --continue`), and interactive terminals do
   this. What patterns emerge and which ones work well for automation?

2. **What does the error model look like when one command serves multiple roles?**
   Distinguishing "nothing submitted yet" from "submitted but invalid" from "wrong state
   for this action" is harder with a single command. How do other CLIs handle this, and
   what does it mean for agent-parseable output?

3. **How should `koto next` accept state-dependent data without coupling its interface
   to the set of known action types?**
   Generic (`--data` / stdin) vs. per-action flags is the core tension. What are the
   real trade-offs for an agent that needs to read the output and know what to do next?

4. **Does implicit transition (koto advances state when gates are satisfied) change the
   contract for agents?**
   If the orchestrator no longer names the next state, how do branching decisions work?
   What does the agent need to do differently?

5. **How would delegation, evidence submission, and approval gates each fit into a
   unified `koto next`?**
   Using these three concrete future use cases as tests: can a generic data submission
   model accommodate all of them, or does each require something the model can't express?
