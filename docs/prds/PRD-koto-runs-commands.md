---
schema: prd/v1
status: Draft
upstream: docs/briefs/BRIEF-koto-runs-commands.md
problem: |
  koto can already run a shell command when a workflow enters a state, but
  nothing around that command works: the output is captured and thrown away,
  a non-zero exit changes nothing, the command runs in whatever directory
  `koto next` was typed in, and no rule says which commands qualify.
  Template authors respond by leaving the mechanical work in prose for an
  agent to carry out, which costs turns and drifts from what the repository
  actually needs. Two defects underneath the same execution path already
  reach gate authors today, before any action is declared.
goals: |
  An author moves a mechanical step out of the prose and into the state, and
  the workflow gets more reliable for it. The command's answer reaches the
  states that come after it, a failing command stops the run and hands the
  agent its own output alongside the written fallback, a session ticked from
  the wrong tree refuses and says which tree it belongs to, and the rule
  deciding which commands qualify is something an author looks up rather
  than re-derives.
---

# PRD: koto runs the mechanical commands

## Status

Draft

## Problem Statement

A koto template state can declare a `default_action` — a shell command the
engine runs itself when a workflow enters that state. It shipped in March 2026,
it works in the current binary, and no markdown template in koto or in the
shirabe plugin declares one. The workflows that drive koto hardest instead
spell the mechanics out in prose for the agent to copy: "then run `git
rev-parse --abbrev-ref HEAD` to get the current branch." No design, issue, or
pull request records a decision to skip the capability. It was dropped, not
declined.

It stayed unused because an author who reaches for it finds that the command
runs and nothing else does.

**A command's answer has nowhere to go.** `run_shell_command`
(`src/action.rs:26-107`) captures what the command prints, and the advance loop
then discards it: `src/engine/advance.rs:290-292` matches
`ActionResult::Executed { .. }` and keeps none of its fields. Of the seven
`NextResponse` variants, only `ActionRequiresConfirmation` carries an
`action_output` at all (`src/cli/next_types.rs:104-112`). Nor is there a
mechanism a later state could read from: `{{VAR}}` substitution is built once
at init from the `WorkflowInitialized` event (`src/template/substitute.rs:57-73`)
with no post-init ingestion point, gate evidence resets on every transition, and
the context store is written by agents and read by gates but is never visible to
substitution. So any step whose whole value is its output — a branch name, an
issue number, the count that decides what happens next — can't move into a
state, because the state that follows has no way to read what the previous one
learned. The steps most worth automating are exactly the ones the surface can't
carry.

**A failure changes nothing.** A non-zero exit never stops the advance loop.
Where a state declares gates, a gate may catch the problem, but a command gate's
evidence keeps only `exit_code` and `error` (`src/gate/mod.rs:206-230`) and never
the command's stdout — so the agent learns the state is blocked, not why. Where a
state declares an action and no gates, there is no detection at all: the gate
block is guarded on a non-empty gate list, the command exits non-zero, and the
workflow advances as though it worked. `requires_confirmation` does not fill the
gap; it fires unconditionally after execution, on success and failure alike
(`src/engine/advance.rs:297-311`). That leaves an author with no fallback to
design around — no way to say "run this, and if it doesn't work, here's how to do
it by hand" — which is the first thing anyone wants before trusting an engine
with a step. The shape of that risk isn't hypothetical: twelve child workflows
once got dispatched against a branch nobody had created, because the error that
would have stopped them was filtered away.

**The command runs wherever `koto next` was typed.** `handle_next`
(`src/cli/mod.rs:3082`) reads `std::env::current_dir()` fresh on every tick and
hands it unchecked to both the gate evaluator and the action executor
(`src/cli/mod.rs:3979-3983`). Nothing in the persisted session record names a
directory: neither `StateFileHeader` nor `MachineState` has such a field, and the
one directory-shaped field that exists, `template_source_dir`
(`src/engine/types.rs:260`), is the template file's own parent and is used only
for resolving child template paths. Open a second terminal in a second checkout,
tick the same session, and the branch command reports on a tree with a different
branch checked out. This isn't specific to actions — the gates shipping today run
against the same unbound directory — so it predates the question and would
outlive an answer that only covered actions.

Underneath all three sit two defects in code that gates and actions share.
`run_shell_command` reads the child's pipes only after `wait_timeout` returns
(`src/action.rs:60-84`), so a command that emits more than the operating system's
pipe buffer blocks on its own write, the wait expires, and every byte of output
is discarded and reported as a timeout — not truncated, lost, and misattributed.
Separately, `migrate_if_needed` (`src/session/local.rs:657-720`) prints uncapped
warnings to stderr on every session-touching invocation, once per name collision,
forever (koto issue #193). The two compound: a nested `koto` invocation inside a
gate or an action can fill the pipe with koto's own warnings and trigger the
deadlock against koto itself. Gate authors are exposed to both today, before any
action exists, which is why fixing them once fixes both surfaces.

There's a fourth gap, and it's the one that doesn't show up in a stack trace. An
author deciding whether to move a step also has to decide whether the step is one
the engine should be running at all. Some commands are safe to hand over and some
are permanently not, the difference isn't obvious from the command itself, and
what the project does say is both scattered and wrong: a reversibility rule
appears in `docs/designs/current/DESIGN-shirabe-work-on-template.md:541-543` and
in `docs/designs/current/DESIGN-default-action-execution.md:444-446`, and the
mechanism both rely on — `requires_confirmation` — doesn't do what its name says.
Meanwhile the entire authoring surface for `default_action` is one row in a format
table (`docs/template-format.md:142`) and a single Rust integration test
(`tests/integration_test.rs:3846-3924`), so every author reaching for it
re-derives the judgment alone and reaches a different answer.

The result is that the mechanical work stays with the agent. Turns go on `git
rev-parse` and `gh pr list`; prose instructions grow to cover shell the engine
could run; and each place the prose drifts from what the repository actually
needs becomes a failure nobody sees until much later.

## Goals

A template author moves a mechanical step out of the prose and into the state,
and the workflow gets more reliable for it. The command's answer is available to
the states that come after it, so a step can be automated because of its output
rather than in spite of it. A command that fails stops the workflow where it
stands and hands the agent the command's own output alongside the instructions
for doing the step by hand, so a broken step becomes a handoff instead of a
silent advance. And a session ticked from a tree it doesn't belong to refuses and
says which tree it does belong to.

The author also stops re-deriving the judgment call. Where the boundary sits
between a command the engine should run and one the agent should keep is
something an author looks up rather than something each author invents, and the
reasoning behind it is recorded alongside it. Two authors arriving at the same
question arrive at the same answer.

For the agent running the workflow, the change is what it spends turns on. The
bookkeeping steps stop arriving as instructions to carry out and start arriving
as facts already established, and what's left in the prose is the work that
actually needed an agent. For the developer watching the session, the change is
that the workflow either does the mechanical step or says clearly that it
couldn't — and never, quietly, does it somewhere else.

## User Stories

### Story 1: A template author moves a mechanical step into the state

As a maintainer of a koto-backed workflow, I want to declare a command on a state
and name where its answer lands, so that a later state's prose can read that name
and the instruction telling an agent to run shell comes out of the template
entirely. When I run the workflow, the state advances with the value already in
hand, and the state that needed it never had to ask.

This is the story the whole feature exists for, and it fails today because the
output has nowhere to land.

### Story 2: An agent hits a failing action and falls back to prose

As a coding agent driving a workflow, I want a state whose command fails — a tool
that isn't installed, a tree that isn't clean, a branch that isn't there — to stop
the session and give me the exit status, the command's own error text, and the
state's written fallback instructions, so that I can do the step by hand and
continue in the same turn. The developer watching sees the failure named and
handled once, rather than seeing a run that looks healthy until it isn't.

This is the path that has to exist before an author will move anything into a
state at all.

### Story 3: A developer ticks a session from the wrong tree

As a developer keeping two checkouts of the same repository — a worktree for the
current branch, the main clone for everything else — I want `koto next` in the
second terminal to refuse and name the directory the session is bound to, so that
nothing runs against the wrong tree and I can either move or deliberately rebind.

This story is reachable without any of the others: today's gates already run
unanchored, so the exposure exists whether or not a single action is ever
declared.

### Story 4: A template author decides an outward-facing command doesn't qualify

As a template author wanting to put `gh pr create` in a state so the engine opens
the pull request, I want published guidance that gives me the test — does the risk
live in a bad success, or only in a bad failure? — so that I can see `gh pr
create` fail it, leave the command with the agent, and move on without spending an
afternoon re-deriving the boundary. A later author reading the same guidance stops
at the same place.

This story produces no automation, and it's the one that keeps the others from
being applied where they shouldn't be.

## Requirements

Requirements state the user-visible obligation. Several have more than one
credible mechanism, and this PRD deliberately leaves the choice to the design
that follows it — see Decisions and Trade-offs.

### Functional: output routing

**R1.** A state that declares a `default_action` can also declare a name for the
command's output. The name is written on the state, not inferred from the command.
A state that declares no name behaves exactly as it does today.

**R2.** A value delivered under an R1 name is readable from the instruction text
of states entered after the command ran, in the same session. This holds when the
engine auto-advances through several states within a single `koto next` call — the
producing state and the state that reads the value are frequently not the same
tick, and the same-tick case is the one output routing exists for.

**R3.** When a command's output cannot be delivered under its declared name —
because it is empty, exceeds the size bound of R23, or contains characters the
delivery mechanism cannot carry — the outcome is defined, documented, and visible
to whoever is running the workflow. Output is never silently dropped. Whether the
problem surfaces at template compile time, as a workflow stop at run time, or as a
recorded skip is a design choice; that it surfaces is not.

**R4.** Re-entering a state that delivers a value replaces the previous value
rather than accumulating alongside it, and the published behavior states what
`koto rewind` does to a delivered value when a session is rewound past the state
that produced it.

### Functional: the failure path

**R5.** A `default_action` that exits non-zero stops the workflow at the state
that ran it. The session does not transition, and no subsequent state's action
runs.

**R6.** R5 holds for a state that declares an action and no gates. That case
detects nothing today and is the one an author is most likely to write first.

**R7.** The response the agent receives on an R5 stop carries the failing
command's own output: its exit status, its standard output, and its standard
error. This applies whether the failure was detected by R6 or by a gate the state
declares.

**R8.** The same response carries the state's written fallback instructions — the
prose telling the agent how to perform the step by hand. A template must have a
place to write that prose distinct from the state's normal directive, which today
is a single static string per state with no per-outcome variant.

**R9.** A stop caused by a failing action is distinguishable, by the agent
reading the response, from a stop caused by a failing gate or by any other blocked
condition, and the response names the state and the command that failed.

### Functional: execution anchoring

**R10.** A session records, at creation, the directory it was created in.

**R11.** `koto next` invoked from a directory that does not satisfy the session's
recorded anchor refuses: no action executes, no gate evaluates, and no transition
occurs. The refusal names the directory the session is bound to. Whether "satisfy"
means equality with the anchor or containment beneath it is a design question; the
refusal and the named directory are not.

**R12.** A developer whose checkout genuinely moved can rebind a session to a new
directory through one deliberate, explicit command, and the rebind is recorded on
the session. koto has no such verb today: `SessionCommand` (`src/cli/mod.rs:376`)
offers `Start`, `Dir`, `List`, `Cleanup`, `Resolve`, and `Update`, none of which
sets an execution directory.

**R13.** A session created before anchoring exists keeps working. On its first
tick after the upgrade it adopts the directory it is ticked from, tells the
developer once that it has done so, and is anchored from then on. It must not
refuse, and it must not adopt silently.

**R14.** A session whose recorded anchor does not resolve on the machine it is
being ticked from is refused with a message saying so, and pointing at the rebind
of R12. This case is distinguishable from being ticked from the wrong tree on the
same machine. The session record travels intact between machines already — the
cloud backend round-trips the whole state file verbatim
(`src/session/cloud.rs:712-744`) — so what changes across machines is that the
recorded path may name nothing.

**R15.** koto's published description of anchoring states what it guarantees and
what it does not. It guarantees the directory a workflow starts in. It does not
bound what a command reaches once running, because an authorized command can name
absolute paths or change directory regardless. No koto document, error message,
skill, or release note describes anchoring as containment, sandboxing, isolation,
or a guard on what a command can touch.

### Functional: the two shared-path defects

**R16.** A command whose output exceeds the operating system's pipe buffer runs
to completion and its output reaches the caller. This holds for gate commands and
for `default_action` commands alike, because both go through
`run_shell_command`. Gates ship today and are exposed now, so this requirement is
not contingent on any template declaring an action.

**R17.** Output above whatever size bound koto imposes is truncated at a stated
boundary and marked as truncated, rather than lost or misreported. The existing
64KB cap (`src/cli/mod.rs:61`) applies only to `default_action` output after the
fact and never to gates; the bound after this work covers both.

**R18.** koto's own diagnostic output is bounded per session rather than per
invocation: a repeated condition is reported at most once for a given session, not
on every invocation indefinitely. Beyond the noise this removes, the bound matters
because koto's warnings are one of the ways a nested `koto` call fills a pipe and
triggers R16's failure against koto itself.

### Functional: authoring guidance

**R19.** koto publishes the rule that decides which commands a template state may
run, stated as the question an author asks: **does the command's risk live in a
bad success, or only in a bad failure?** Risk in a bad failure is answered by
R5 through R9. Risk in a bad success — where a successful run is itself the
unrecallable, externally visible event — is answered by nothing on any roadmap,
because no signal arriving afterward can un-fire it, so those commands stay with
the agent permanently. The published rule carries worked examples on both sides
and the reasoning behind the boundary, so an author applies it rather than
re-deriving it.

**R20.** `default_action` has authoring documentation sufficient to write one
correctly without reading koto's source or test suite: what the field accepts, how
the command is invoked, what directory it runs in, what happens to its output,
what happens when it fails, and how it interacts with the state's gates.

**R21.** The `koto-author` and `koto-user` skills describe every behavior this
PRD introduces, and the existing drift in `koto-author`'s dispatch table is
corrected in the same change.

### Non-functional

**R22.** Every change here is additive for existing content. A template that
declares no `default_action` compiles and runs unchanged, and an existing session
behaves as before. The single intentional behavior change for existing sessions is
R13's one-time anchor adoption, and it is a notice rather than a failure.

**R23.** Everything an action's output makes visible — to a later state, to a
`koto next` response, or to the event log — is bounded in size, and the bound is
stated in the authoring documentation rather than discovered by hitting it.

**R24.** A failing action's stop is reported in the `koto next` call that ran the
command. An agent does not need a second call to learn why the workflow stopped or
what to do instead.

## Acceptance Criteria

### Output routing

- [ ] A template state declares a command and a name for its output, compiles, and
      runs (R1).
- [ ] A state entered after that command, in the same session, renders the value in
      its instruction text (R2).
- [ ] The same holds when the two states are reached within one `koto next` call
      via auto-advance (R2).
- [ ] A state that declares a command and no output name produces byte-identical
      behavior to the current release (R1, R22).
- [ ] Output that cannot be delivered under its name produces a documented,
      observable outcome; no test case results in the value being dropped with no
      signal anywhere (R3).
- [ ] Re-entering the producing state replaces the value rather than appending to
      it (R4).
- [ ] The documentation states what a rewind past the producing state does to a
      delivered value, and the behavior matches (R4).

### The failure path

- [ ] A state with a `default_action` that exits non-zero and no gates does not
      transition (R5, R6).
- [ ] A state with a `default_action` that exits non-zero and one or more gates
      does not transition (R5).
- [ ] The response from the stopping `koto next` call contains the command's exit
      status, its stdout, and its stderr (R7, R24).
- [ ] The response contains the state's written fallback prose, distinct from the
      state's normal directive (R8).
- [ ] A template can declare that fallback prose, and a template that declares none
      still compiles (R8, R22).
- [ ] An agent reading the response can tell an action failure from a gate failure
      without inspecting the command string, and the response names the state and
      the command (R9).
- [ ] A `default_action` that exits zero produces the same transition behavior as
      the current release (R22).

### Execution anchoring

- [ ] A session created by `koto init` has a recorded execution directory (R10).
- [ ] `koto next` run from a directory that does not satisfy the anchor performs no
      action, evaluates no gate, and makes no transition (R11).
- [ ] That refusal message names the directory the session is bound to (R11).
- [ ] A single documented command rebinds the session to a new directory, after
      which `koto next` succeeds there and the rebind is recorded (R12).
- [ ] A session file written by the previous release ticks successfully on first
      use, emits exactly one notice that it has adopted a directory, and refuses
      from a different directory afterward (R13, R22).
- [ ] A session whose anchor names a path that does not exist produces a refusal
      that says the anchor is unresolvable and names the rebind command, textually
      different from the wrong-tree refusal (R14).
- [ ] A search of koto's documentation, error strings, skills, and release notes
      finds no claim that anchoring contains, sandboxes, isolates, or restricts what
      a command can reach (R15).
- [ ] The anchoring documentation states plainly that a command can leave the
      anchored directory by absolute path or by changing directory (R15).

### Shared-path defects

- [ ] A gate command emitting several megabytes completes, is evaluated on its real
      exit status, and is not reported as a timeout (R16).
- [ ] A `default_action` emitting several megabytes completes and its output is
      delivered (R16).
- [ ] Output above the stated bound arrives truncated and explicitly marked as
      truncated, for both gates and actions (R17).
- [ ] A repeated diagnostic condition produces at most one message for a given
      session across many invocations (R18).
- [ ] A gate or action that invokes `koto` as a subprocess under the condition that
      previously produced repeated warnings completes without a deadlock or a false
      timeout (R16, R18).

### Authoring guidance

- [ ] Published guidance states the bad-success versus bad-failure question as the
      rule, with worked examples on both sides (R19).
- [ ] The guidance classifies `gh pr create` as permanently agent-run and explains
      why no future koto capability changes that (R19).
- [ ] The guidance classifies at least one command as engine-runnable and explains
      why the failure path of R5 through R9 is what makes it so (R19).
- [ ] A template author can write a working `default_action`, including its output
      name and its fallback prose, using only the published documentation (R20).
- [ ] The documentation states what directory the command runs in and what happens
      to its output on success and on failure (R20, R23).
- [ ] `koto-author` and `koto-user` describe the output name, the failure response,
      anchoring, and the rebind command (R21).
- [ ] `koto-author`'s dispatch table matches the shipped CLI surface (R21).
- [ ] Skill evals pass for both skills after the change (R21).

## Out of Scope

Carried from the upstream brief's scope boundary, which this PRD does not reopen.

- **Rewriting the shirabe plugin's templates to use any of this.** It depends on
  this work and belongs in that project, afterward. Nothing about that rewrite is
  blocked on analysis; it's blocked on the engine.
- **The hardcoded commands in shirabe skills that aren't koto-backed.**
  `default_action` doesn't exist outside a koto template, so no engine change
  reaches them.
- **CI monitoring as a typed koto integration.**
  `docs/designs/current/DESIGN-default-action-execution.md` names that as the right
  long-term home for the work, and it's a separate feature with its own shape.
- **Template integrity of any kind.** Decided, not deferred: rely on the pinning
  agent harnesses already provide. No expected-hash argument on `koto init`, no
  manifest of template digests, no koto-side trust store, no extension of release
  checksums to templates. The space was mapped and then closed on purpose.
- **Bounding what the event log records.** Command strings, gate-override payloads,
  and init-time variables go into the log unbounded and unredacted. That's a real
  problem about what gets written down, and it isn't this feature's problem. It has
  no issue yet, and filing one is a follow-up this feature owes — see Known
  Limitations.
- **Renaming `requires_confirmation`.** The flag executes the command and only then
  asks, which makes its name wrong. The authoring rule of R19 makes the flag
  unnecessary rather than dangerous, so the rename is tidying and its blast radius
  has never been counted.
- **Containing what an authorized command can reach.** R15 requires koto to say so
  plainly; nothing here builds it. Every mechanism that would close that gap either
  collapses into documentation nothing enforces or breaks koto's single-binary,
  no-sudo, four-platform distribution.
- **Routing engine-run commands through the agent's permission layer, or confirming
  each one before it runs.** Loading a workflow is itself the grant: invoking a
  koto-backed workflow authorizes the commands that workflow bakes in,
  deliberately. Relocating consent from per-command prompting to the decision to run
  the workflow is what lets koto carry mechanical work at all, and a
  preview-and-approve step would reproduce the prose-plus-gate pattern this feature
  exists to replace.
- **Retiring the retry-clearing instructions in shirabe's workflows.** That waits on
  a decision in that project about its own current design for retry clearing. No
  engine change retires it.
- **Deciding which specific shirabe commands convert.** R19 publishes the rule; the
  per-command classification of another project's templates belongs to that
  project's own rewrite.

## Decisions and Trade-offs

### D1. A write to koto's own context store is not a disqualifying side effect

Closes the first framing question the brief left open. The store lives at
`~/.koto/sessions/<id>/ctx/`, is local to the machine unless the operator has
opted into cloud sync, and is not visible to anyone outside it. The engine
already writes to it autonomously today, without any agent involvement, during
batch finalization (`src/cli/mod.rs:4470-4480`). Under R19's test, a
context-store write creates no unrecallable externally visible event on success;
the worst a bad run leaves behind is a stale local value, which is repairable.
So it does not push a command onto the permanently-agent-run side.

The alternative — treating any durable write as disqualifying — was rejected
because it would exclude most of what the feature exists to automate while
protecting against nothing anyone can name. One asymmetry is real and is recorded
under Known Limitations: `koto rewind` repoints the state pointer and does not
unwind context writes, so a rewind does not undo one.

### D2. Anchoring ships as an additive change, not a breaking one

Closes the second framing question. Pre-existing sessions have no recorded tree,
so R11's refusal has nothing to compare against. Three behaviors were available:
refuse until someone binds them, adopt silently, or adopt with a one-time notice.
R13 chooses the last.

Refusing would break every in-flight session on upgrade to enforce a guarantee
that never existed for them, which is a large cost for a population whose actual
exposure is unchanged from the day before. Adopting silently gets the migration
for free but hides a state change from the person who most needs to see it —
particularly since the adopted directory is whatever tree happened to be current,
which may not be the right one. Adopting with a notice keeps every existing
session working, makes the new binding visible at the moment it is created, and
leaves R12's rebind as the repair. The residual risk is recorded under Known
Limitations.

### D3. Pushing to an already-open pull request stays with the agent, on unproven evidence

Closes the third framing question, and closes it honestly rather than
confidently. The case rested on one checkable claim: that GitHub's `synchronize`
event — new commits pushed to an already-open pull request — notifies subscribers
meaningfully more quietly than `opened` or `ready_for_review`. GitHub's published
notification documentation does not settle it. There is no push or synchronize
entry in the documented notification-reason taxonomy, and the one directional
signal available (`review_requested` fires when review is requested, not on plain
pushes) supports an inference, not a citation.

So the classification stays where the burden of proof puts it: under R19, a
command whose classification depends on an unverified claim about external
visibility stays with the agent until the claim is checked. R19 requires the
published guidance to say that, so the next author facing the same question
inherits the rule rather than the guess. Nothing in this PRD's requirements
depends on the answer — converting anyone's templates is out of scope — so this
resolves the question without blocking any work.

### D4. Requirements state the obligation; the design picks the mechanism

Four approaches to output routing were costed during exploration and none was
settled: naming the capture on the state and folding it into instruction
substitution, populating an output field on every response variant, merging output
into the same state's gate evidence, and writing to the context store. They differ
sharply in cost and in reach — one of them can't cross a state boundary at all,
and the cheapest-looking one carries a same-tick staleness trap that would make it
fail on exactly the auto-advance case R2 names. Anchoring likewise has three
unresolved questions: whether the anchor requires an explicit flag, how the check
compares directories, and where containment and spawn failures surface.

Rather than pre-decide these, R1 through R4 and R10 through R14 are written
against user-visible behavior, and the design that follows this PRD chooses. The
trade-off accepted is that these requirements are somewhat less concrete than they
could be; the alternative was a PRD that silently made architecture decisions
where four costed options exist.

### D5. Gates stay the arbiter of success; no `on_failure:` field

The failure path is built by making detection work where it doesn't (R6) and by
carrying output and fallback prose into the response (R7, R8) — not by adding a
per-state failure handler to the template schema and not by redefining what a
non-zero exit means throughout the engine. Gates were always the intended arbiter
in koto's model, and a general redefinition of exit-code semantics would contradict
that model for every state that already declares gates. This is carried forward as
settled so the downstream design does not reopen it.

### D6. The two shared-path defects are requirements of this feature, not separate work

Both could be argued out of scope as pre-existing bugs. They stay in because both
reach gates that ship in the current release, both go through the same
`run_shell_command` this feature makes load-bearing, and the deadlock in
particular converts a routine large-output command into silent total data loss
misreported as a timeout — which is the precise failure mode R5 through R9 exist to
eliminate. Shipping the failure path on top of an execution primitive that loses
output would deliver a guarantee that doesn't hold.

### D7. The fallback prose needs its own place in the template

R8 requires the failure response to carry instructions distinct from the state's
normal directive, which implies a template field that doesn't exist today — a state
has one static directive with no per-outcome variant. The alternative, reusing the
directive, was rejected because the directive is what the agent is told to do when
the state works, and Story 2's whole point is that the agent needs different
instructions when it doesn't. Naming and shape are the design's call; that the two
are separate is not.

## Known Limitations

- **Anchoring is not containment.** R11 guarantees the directory a workflow starts
  in. An authorized command can still name absolute paths or change directory, and
  nothing in this feature stops it. R15 exists to make sure no koto document ever
  implies otherwise, because a promise of containment that isn't one is worse than
  no promise.
- **A pre-existing session first ticked from the wrong tree anchors to the wrong
  tree.** That is the accepted cost of D2. It is bounded — the exposure is exactly
  what existed before the upgrade — the notice makes it visible immediately, and
  R12's rebind repairs it.
- **A rewind does not unwind what a command already did.** `koto rewind` repoints
  the state pointer and does not undo a context-store write, a delivered value, or
  anything the command changed on disk. R4 requires the behavior to be documented,
  not changed.
- **The event log records command strings unbounded and unredacted.** A secret
  interpolated into a command's arguments is written down verbatim even when the
  command prints nothing. Bounding the log is out of scope by the brief's boundary,
  and the exposure is zero today only because no template declares an action —
  which is the condition this feature is designed to end. This feature owes a
  follow-up issue on the log's content bounds, and that issue should be filed
  before the first template declares an action.
- **The push-to-open-pull-request classification is unresolved by evidence,** not
  by argument. D3 records the default and the reason; a later author who checks the
  claim can move it.
- **There is no existing test coverage to extend for the output defect.** No test
  in koto exercises output above the pipe buffer or the truncation path, so R16 and
  R17 arrive with new tests rather than modified ones.
