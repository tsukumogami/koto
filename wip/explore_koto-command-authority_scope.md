# Explore Scope: koto-command-authority

## Visibility

Public

## Core Question

koto executes shell commands on an agent's behalf, and the author has ruled that
this is deliberate: loading a skill that drives koto is itself the broad grant,
so invoking a koto-backed workflow authorizes every command that workflow bakes
in. Consent lives at workflow invocation, not per command. That ruling settles
an objection — and relocates the whole safety question. If the template is where
authority is granted, then the template is the security boundary, and nothing in
koto or shirabe currently treats it as one. This exploration asks what that
boundary has to look like, what it lets through today, and how much more of the
existing workflows become convertible now that the permission argument no longer
withholds anything.

## Context

This is a follow-on to the `koto-runs-commands` exploration, whose artifacts live
on the `docs/koto-runs-commands` branches of `tsukumogami/koto` (the `/scope`
handoff) and `tsukumogami/shirabe` (findings, decisions, crystallize scoring, and
22 lead files). Read the Author Ruling section at the top of shirabe's
`wip/explore_koto-runs-commands_findings.md` before anything else.

What that exploration established, and this one does not revisit:

- `default_action` is implemented, shipped, and verified working; shirabe uses it
  zero times; nobody ever decided against it.
- The three-path model works today with no koto changes — an action, a gate that
  independently verifies the outcome, and a transition keyed on the gate's exit
  code. Verified running.
- Three engine gaps: action output is captured and discarded, a failing action
  changes nothing, and commands run in whatever directory `koto next` was invoked
  from with no binding to the session's tree.
- Two defects underneath: `run_shell_command` deadlocks above 64KB because it
  waits before draining pipes, and koto emits ~106KB of migration warnings per
  session-touching command.
- `context_assignments` is silently discarded while shirabe declares it 28 times.

What the ruling changed, and why this exploration exists:

- The permission-bypass finding is struck as an objection. Both conversion maps
  applied it to the writes-remote bucket, so their yields are floors computed
  under a premise that no longer holds.
- The carve-out for remote mutations now rests only on irreversibility and on
  failure diagnosis being blind — both fixable.
- `requires_confirmation` is the only in-band checkpoint left, and it fires after
  the command has already run.
- The surviving hazard is that action output is persisted to an event log
  committed to feature branches.
- Execution anchoring becomes the primary remaining guard rather than one of
  several.

## In Scope

- What the template has to become if it is the security boundary: provenance,
  review practice, integrity, and what koto's compiled-template cache does with
  trust.
- Re-mapping shirabe's writes-remote commands as conversion candidates under the
  amended principle.
- `requires_confirmation`: what it should mean, and what a correct
  confirm-before-execute would look like.
- The event-log exposure: what actually lands there, where it goes, and what
  bounds it.
- What execution anchoring must guarantee now that it carries more weight.

## Out of Scope

- Re-litigating the ruling. It is settled input.
- The two defects, the output-routing design, the failure plumbing, and the
  anchoring design as already scoped — they are in the `/scope` handoff on
  `docs/koto-runs-commands` and this exploration does not redo them.
- shirabe's eighteen non-koto skills.
- The retry-clearing question, which is blocked behind a shirabe design decision.

## Research Leads

1. **If the template is the security boundary, what does koto do with templates
   today — and what would it have to do?**
   Where templates come from, how they are compiled and cached, whether anything
   verifies integrity or provenance, what `--from-stdin` and inline definitions
   allow, and what a review or pinning practice would need. This is the lead most
   likely to outgrow the exploration.

2. **What can a template actually do to the machine it runs on?**
   The concrete blast radius: what a command inherits, what the variable
   allowlist does and does not stop, whether anything bounds destructive or
   network-reaching commands, and what the worst realistic careless template
   looks like as distinct from a hostile one.

3. **Which writes-remote commands become conversion candidates under the amended
   principle, and what does each still need?**
   Re-map the bucket both prior maps set aside — pushes, PR creation and edits,
   the finalization cascade — against irreversibility and diagnosability rather
   than authorization.

4. **What should `requires_confirmation` mean, and what would confirm-before-
   execute cost?**
   It fires after execution today, and it is now the only in-band checkpoint.
   What the design intended, what the engine would need to pause and resume
   around a pending action, and whether a confirmation that halts is even the
   right shape.

5. **What does the event log actually expose, and what bounds it?**
   What is written, where those files travel, whether anything redacts or
   truncates before commit, and what a template author can do about it.

6. **What must execution anchoring guarantee now that it is the primary guard?**
   The prior exploration scoped anchoring against accidental misdirection. Under
   the ruling it is also the main thing standing between a broad grant and an
   unintended target. Whether the design already scoped is sufficient for that
   heavier role.
