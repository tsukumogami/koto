---
schema: prd/v1
status: Draft
problem: |
  Agents a person dispatches, and the agents those start, make real decisions
  unattended: they accept documents and plan steps, merge, get past gates,
  escalate questions or decide not to, and settle judgment calls. Afterwards
  nobody can tell which session or agent made a decision, what authority it
  held, or overrule it on a record the rest of the toolkit honours. koto and
  the shirabe plugin each keep partial records that disagree about who
  decided, and a koto session deletes itself when it succeeds, taking its
  decisions with it.
goals: |
  Every decision a dispatched session makes or receives is on one record that
  outlives the session, naming its kind, its decider (a session and the agent
  within it, a person named without a user account, or an automated check)
  and the authority the decider held. Anyone above the decider in the
  dispatch chain can overrule it with a decision of their own, after which
  the original reads as no longer in effect everywhere the record is read.
  The partial records that exist today write to it or read from it instead
  of keeping their own answer.
upstream: docs/briefs/BRIEF-decision-record.md
---

# PRD: The decision record

## Status

Draft

## Problem Statement

When a person hands work to an agent, that agent and the agents it starts go
on to make decisions without the person watching. They accept a design, a
plan or one step of a plan, merge a pull request, let work past a gate, decide
whether a question is worth taking to whoever dispatched them, and settle
judgment calls. Often the session that wrote something also accepts it, which
is legitimate when it was given the authority to.

Three things are missing for every one of those decisions:

- **Who made it.** A session runs a main agent and possibly several agents
  under it, and the session itself was dispatched by a person or by an agent
  in another session. Nothing tells these apart, and where a person decided,
  nothing names them.
- **What entitled them to.** Authority narrows as it is handed down: no agent
  holds more than its session's main agent, and no session holds more than
  whoever dispatched it. Nothing records the authority a decision was made
  under.
- **How to overrule it.** The session or person above sees results, not the
  decisions behind them, and has no way to overturn one that the rest of the
  toolkit will respect or that a later reader can see.

The records that do exist make this worse rather than better:

- koto's run journal records when the automated grader was consulted and when
  a gate check was overridden, but not who overrode it.
- `koto decider answer` records an agent's answer when the grader gives none,
  with the agent implied rather than named.
- `koto decisions record` and `koto overrides record` keep a choice or an
  override with a rationale and no actor.
- A pending change adds disputed findings tagged "agent" or "person", which
  nothing reads.
- The shirabe plugin's findings ledger names its writers as reviewer, check,
  decider or agent, with no person and no wall-clock time.
- The shirabe coordinator keeps a decision table in a GitHub issue with a
  free-text "decided by" column. It is the only record that keeps a reversed
  decision next to its reversal.

The word "decider" means koto's automated grader in some of these and a
person or coordinator in others. And because a koto session is removed when
it reaches a successful end, a worker's decisions are usually gone by the
time anyone above it wants to question them.

## Goals

- The person or agent that dispatched a session can, after the session is
  gone, read every decision it made or received: the kind, the decider, the
  authority the decider held, when, and what it was about.
- Anyone above a decider in the dispatch chain can overrule a decision. The
  overrule is itself a decision with its own decider and authority, and the
  original reads as no longer in effect to every reader.
- A person's decision is attributable to that person without koto holding a
  user account, and it is always clear which surface it came through.
- Getting past a gate is never anonymous.
- The existing partial records stop disagreeing: each either writes to the
  record or is a view over it.
- Work built on decisions (crediting an accepted plan step, counting how
  often each kind of decision is overruled) reads one record instead of
  keeping its own.

## User Stories

1. As a coordinator that dispatched an unattended session, I want to read,
   after the session and its workspace are gone, each decision it made
   (accepting a design, getting past a gate, merging) with the session and
   agent that made it and the authority it held, so that I can audit the run
   without having watched it.
2. As a maintainer who accepts a design myself, from my terminal or by
   approving a review on the forge, I want the record to name me as the
   decider and say which surface my decision came through, without koto
   holding an account for me, so that a reader can tell my decision from an
   agent's.
3. As a coordinator that disagrees with a worker's acceptance, gate pass and
   merge, I want to overrule each one and have the record show my overrule,
   who I am and what authority I hold, and that the originals no longer
   stand, so that nothing downstream keeps treating them as settled.
4. As a coordinator, I want to see which questions a worker escalated to me,
   which it decided not to escalate, and which judgment calls it settled
   itself, each with its decider, and to overrule a judgment call, so that
   choices that leave no trace in the workflow today become visible and
   correctable.
5. As a maintainer running unattended work, I want koto to refuse a gate pass
   when no decider can be established, so that an audit never meets a pass
   nobody owns.
6. As the author of a tool that reports on decisions, I want to count, over a
   time window, how many decisions of each kind were overruled and list them,
   telling a session's decisions from a person's or an automated check's, so
   that the overrule rate can be read without special-casing any writer.
7. As a skill author whose skill already records decisions its own way, I
   want my skill to write to the shared record, or render its existing view
   from it, without breaking the readers that depend on its current output.

## Requirements

### Functional

**Decision kinds**

- **R1.** A decision has exactly one kind from this closed set:
  - `accept`: accepting a document, a plan, one step of a plan, a test
    change, or a request's intent.
  - `merge`: merging a pull request.
  - `gate-pass`: letting a workflow past a gate it had not passed on its own
    (by override, by a directed transition past an unmet gate, or by an
    agent's answer that lets a check pass).
  - `escalate`: taking a question to someone above.
  - `decline-escalation`: deciding a question will not be taken to anyone
    above.
  - `judgment`: settling a judgment call, including disputing a finding.
  - `verdict`: a pass or fail judgment on evidence, such as a review verdict
    or a production verdict.
  - `reverse`: overruling an earlier decision.
  - `refused`: a decision attempt koto refused, kept so the attempt is
    visible.
- **R2.** Every decision names its subject in a form a reader can resolve
  after the deciding session is gone:
  - for `accept`, the repository, path and commit of what was accepted;
  - for `merge`, the repository, pull request number and merged head;
  - for `gate-pass`, the workflow, state and gate;
  - for `escalate` and `decline-escalation`, the question and, for
    `escalate`, who it was taken to;
  - for `judgment` and `verdict`, what was judged, and for `verdict` at least
    one pointer to the evidence judged;
  - for `reverse`, the decision it overrules;
  - for `refused`, the attempted kind and subject.
- **R3.** Every decision carries a UTC time, a stable identifier that is
  unique across the record, its outcome (for example accepted, passed,
  failed, settled-as), and, where the decider gave one, a reason.

**The decider**

- **R4.** Every decision names a decider of one of three kinds:
  - `session`: a session, plus the agent within it that decided (its main
    agent, or a named agent it started). When the agent can't be
    established, the record says so rather than assuming the main agent.
  - `person`: a person, named by a handle the surface supplies, with the
    surface it came through (terminal, forge review or comment, relayed
    message) and, where the surface has one, a link to the original. A
    person decider also records the session that entered the decision.
  - `check`: an automated check, named, such as koto's grader.
- **R5.** koto never creates, stores or requires a user account for any
  decider. A person is identified only by the handle and surface in R4.
- **R6.** Each identity in a decision (the session, the agent, the person,
  each link in the dispatch chain) is labelled with how koto obtained it:
  `observed` when koto read it itself, or `handed` when a caller supplied it.
  When a handed value contradicts an observed one, both are kept.
- **R7.** A decision that re-enters someone else's decision (an agent typing
  a person's approval, a coordinator relaying an answer) is the decision of
  whoever entered it, with a pointer to the original. It is attributed to the
  original decider only when the pointer resolves to something that names
  them, such as a forge review.
- **R8.** A decision records its decider's confidence and the run mode
  (attended or unattended) separately from who the decider is, so an agent
  confident under an unattended run is never read as a person's
  confirmation.

**Authority**

- **R9.** Every decision records the authority its decider held: until a
  separate authority setting exists, that is what the decider was dispatched
  with (the dispatch it was launched under, a reference to the instructions
  and decisions handed down to it, and its run mode). The form leaves room
  for a later basis, such as a declared policy, without changing what earlier
  decisions mean.
- **R10.** Every decision records the dispatch chain above its decider, from
  the deciding session up to the person or parentless session at the root, as
  far as koto can establish it. Each link carries the R6 label. A chain koto
  could not complete is recorded as incomplete, never filled in by
  assumption.
- **R11.** The workspace manager doesn't read or write workflow state.
  The dispatch chain therefore reaches the record from what the workspace
  manager already keeps about the sessions it launched, or from the
  dispatching session handing it over, and never by the workspace manager
  calling koto. It has to work in every way the workspace manager launches a
  session; where a launch path supplies no lineage, the link is recorded as
  absent.

**Overruling**

- **R12.** Anyone above a decider in its recorded dispatch chain, or the
  person at the root of that chain, can record a `reverse` decision against
  any decision except a `refused` one or another `reverse`. koto refuses a
  reversal whose recorded chain shows the reverser is not above the decider
  (for example a worker reversing its coordinator). When either chain is
  incomplete, koto records the reversal and labels its authority as asserted
  rather than verified.
- **R13.** Once reversed, a decision reads as no longer in effect in every
  query and view, and the reversal is shown next to it. The original is
  never edited or removed.
- **R14.** A reversal is final. To restore what was reversed, a decider
  records a new decision of the original kind.
- **R15.** What a reversal means, per kind:
  - `accept`: the subject is no longer accepted. Decisions that recorded the
    reversed acceptance as their basis are listed with it, so a consumer can
    stop crediting work built on it. koto doesn't edit the document's
    status; whoever acts on the reversal does.
  - `merge`: the merge is no longer endorsed. The reversal records the
    compensating change (a revert pull request) when one exists, and says
    none exists yet otherwise. koto doesn't touch the forge.
  - `gate-pass`: the pass no longer stands. If the workflow that passed the
    gate is still live, its next `koto next` reports the reversal, and the
    workflow can't advance further until the reversal is acknowledged.
  - `escalate`: the question is withdrawn from whoever it was taken to.
  - `decline-escalation`: the question is treated as escalated, to the
    reverser.
  - `judgment` and `verdict`: the call or verdict no longer stands. The
    reversal may carry the outcome that replaces it.

**Refusing anonymous gate passes**

- **R16.** Every way of getting past a gate by choice (`koto overrides
  record`, `koto next --to` past a gate that has not passed, and `koto
  decider answer` with an answer that lets the check pass) records a
  `gate-pass` decision with a decider. When the caller names no decider,
  koto uses the session it observes and labels the decider as observed and
  defaulted. When no decider can be established at all, koto refuses the
  pass, names what is missing, and records a `refused` decision where it can.

**Durability and reading**

- **R17.** A decision stays readable after the session that made it is
  removed, including after a successful end and after the workspace it ran
  in is reclaimed. Removing or cleaning up a session never removes its
  decisions.
- **R18.** Any session on the same machine can record decisions, including
  ones whose subject belongs to another session, and read every decision on
  the record.
- **R19.** The record can be queried by deciding session (returning the
  decisions it made and the decisions about its work or addressed to it), by
  subject, by kind, by decider kind and by time window, and each result says
  whether the decision is still in effect.
- **R20.** koto reports, for a time window, the number of decisions of each
  kind, how many of them were overruled, and each overruled decision with its
  reversal, broken down by decider kind.
- **R21.** A decision may carry pointers to records the agent harness keeps
  about the same moment (for example the harness's session and tool-call
  identifiers). The record never copies those records.

**Bringing existing records together**

- **R22.** The decider vocabulary in R4 is the only one koto uses for who
  made a decision. The existing records map onto it as follows:

  | Existing record | Becomes | Decider mapping |
  |---|---|---|
  | `koto overrides record` and the run journal's override records | `gate-pass` decision, written by the existing command | the overriding session or person (no actor today) |
  | `koto decisions record` | `judgment` decision, written by the existing command | the recording session or person (no actor today) |
  | `koto decider answer` | `gate-pass` when the answer lets the check pass, `judgment` otherwise | the answering session and agent (implied today) |
  | The grader's consultation and check records in the run journal | stay telemetry; decisions cite them as evidence | `check` when a decision names the grader |
  | Disputed findings (the pending change's `--by agent\|person`) | `judgment` decision about the finding | the R4 vocabulary replaces `agent\|person` |
  | The shirabe findings ledger | keeps its per-round records; a panel pass writes a `verdict`, a dismissal a `judgment` | `reviewer` and `agent` writers become `session` with the agent named; `check` and `decider` writers become `check` |
  | The shirabe coordinator's decision table and reversals | a view rendered from the record; settled entries are `judgment` or the question's kind, escalations `escalate`, reversals `reverse` linked by identifier | "the coordinator" becomes the coordinator's `session`; "a person" becomes `person` with its surface |
  | Document acceptance through the plugin's status transition, and merges through its merge step | `accept` and `merge` decisions | the transitioning or merging session, or the person |

- **R23.** In koto, the grader keeps the name "decider" in its commands,
  template fields and configuration. The record's field for who decided is
  named so that it doesn't collide with that name, and a grader appears in
  it as a decider of kind `check`.
- **R24.** Decisions recorded before the record existed, such as those a
  coordinator kept in its own record, can be imported. An imported decision
  keeps its recorded decider and authority, labelled as handed, with a
  pointer to where it was originally recorded.

### Non-functional

- **R25.** Decisions are append-only and immutable once written. Concurrent
  writers on the same machine never lose or interleave entries.
- **R26.** Readers of koto's existing events, run journal and feed keep
  working unchanged. New fields are optional, and no existing command needs
  a new argument to keep working.
- **R27.** A decision's reason text follows the run journal's privacy rule
  for override rationales: it stays in the local record and is not copied
  into exported telemetry.
- **R28.** Recording a decision makes no network call and doesn't depend on
  the workspace manager or the agent harness being reachable; a missing
  harness or lineage value degrades to the R6 and R10 labels instead of
  failing the decision.
- **R29.** Querying a month of decisions on a machine holding a year of them
  answers in under five seconds.

## Acceptance Criteria

- [ ] An unattended session accepts a document, gets past a gate by override
  and records a merge. After the session reaches its successful end and is
  removed, a query by that session returns all three decisions, each with
  kind, subject, decider (session and agent), authority and time.
- [ ] The same unit, with a maintainer accepting the document from their
  terminal, records the acceptance with a `person` decider showing the
  maintainer's handle and the surface `terminal`, the entering session, and
  no user account anywhere in koto's state.
- [ ] A person's acceptance through a forge review records the review's link,
  and a relayed acceptance with no resolvable link is attributed to the
  session that entered it, with the person recorded only as handed.
- [ ] The dispatching coordinator reverses the acceptance, the gate pass and
  the merge from its own session. Each reversal names the coordinator's
  session as decider, carries its authority, and links the original. Each
  original reads as no longer in effect, and the merge's reversal records the
  revert pull request, or that none exists yet.
- [ ] A reversal of the gate pass while the passing workflow is still live
  causes that workflow's next `koto next` to report the reversal and refuse
  to advance until it is acknowledged.
- [ ] A worker records one `escalate`, one `decline-escalation` and one
  `judgment`. A query returns all three with their deciders. The coordinator
  reverses the judgment, and the reversal is named with its decider.
- [ ] A worker's attempt to reverse a decision made by the coordinator that
  dispatched it is refused, and the refusal is recorded.
- [ ] A reversal recorded when the reverser's chain is incomplete is accepted
  and labelled asserted.
- [ ] `koto overrides record`, `koto next --to` past an unmet gate, and `koto
  decider answer` with a passing answer each record a `gate-pass` decision
  whose decider is the observed session when none was named.
- [ ] A gate pass from a process with no observable session and no named
  decider is refused, the error names the missing decider, and the workflow
  stays at the gate.
- [ ] Each identity in a recorded decision carries an `observed` or `handed`
  label, and a decision whose handed session differs from the observed one
  keeps both.
- [ ] A decision recorded in a session launched by the workspace manager
  carries a dispatch chain whose links are labelled, and a session launched
  with no lineage available records the chain as incomplete rather than
  guessing.
- [ ] The overrule report for a window lists, per kind and per decider kind,
  total and overruled counts, and each overruled decision with its reversal;
  a reversal recorded inside the window appears in that window's count.
- [ ] Every existing record in the R22 table, exercised once, produces the
  decision kind and decider the table names, and the coordinator's decision
  table rendered from the record shows the same rows and reversal links as
  before.
- [ ] A decision recorded by an agent confident in an unattended run is
  returned with decider kind `session` and run mode unattended, never as a
  person's confirmation.
- [ ] Decisions imported from a coordinator's own record keep their decider
  and authority, labelled handed, with a pointer to the source.
- [ ] Two sessions recording decisions concurrently produce two complete,
  separate entries, and neither changes an existing entry.
- [ ] The existing test suites for the run journal, the feed and the grader
  report pass unchanged, and every existing command still works without new
  arguments.
- [ ] A decision's reason does not appear in exported telemetry.
- [ ] With the harness's session identifier unset and no lineage available, a
  decision that names its decider is still recorded, with the missing values
  labelled.
- [ ] A month's query over a record holding a year of synthetic decisions
  returns in under five seconds.
- [ ] No koto state, command output or document added by this feature names
  a user account.

## Out of Scope

- **On whose behalf a decision was made.** Recording the person a session was
  acting for at the top of its chain is a later extension built on this
  record.
- **Rules about who may decide.** Requiring a person at a point, forbidding a
  session from accepting its own work, and the settings that narrow
  authority are policy layers that read this record later. This record says
  who decided and under what authority; it doesn't decide who may.
- **Record-health instruments.** Detecting a workspace that stopped
  recording, or auditing whether escalations were warranted, read this record
  later.
- **Undoing effects outside koto.** koto doesn't revert merges, reopen pull
  requests or edit documents when a decision is overruled.
- **Cross-machine durability.** The record lives in koto's local store on one
  machine.
- **Duplicating the harness's records.** Tool permission decisions and
  transcripts stay where the harness keeps them; R21 only points at them.
- **Authenticating identities.** Identities are attribution, not
  authorization: anything running as the operator can write the record, and
  R6's labels say how far each identity can be trusted.

## Known Limitations

- An `observed` session identifier is still forgeable by anything running as
  the operator. The record makes misattribution visible (R6), not
  impossible.
- The harness's session identifier changes when a conversation is cleared, so
  one logical session can appear as several deciders. Design decides whether
  koto relates them.
- Telling a session's main agent from an agent it started needs the harness
  to say which agent is acting. Until it does, many decisions will record
  the agent as not established (R4).
- Some planning above this PRD expects the decider at acceptance and merge
  to be someone other than the author. This PRD follows the framing that an
  authoring session may accept its own work when its authority covers it,
  and leaves that expectation to the policy layer named in Out of Scope; the
  two still need reconciling with the project owner.
- Further consumers are candidates but not yet confirmed: review verdicts
  bound to a pull request head, release-time verdicts and rollbacks beyond
  the production verdict, accepting a ticket's intent, and reverting a change
  to how strict a decision point is. The kinds in R1 cover them, but none is
  a requirement until the owner decides.

## Decisions and Trade-offs

- **One record outside any single session.** Alternatives: keep decisions
  in each session's log and export at the end, carry them only on request
  legs, or treat the coordinator's GitHub record as authoritative. Sessions
  are removed when they succeed and root sessions have no request leg, and a
  GitHub record leaves koto unable to answer offline or in public repos. The
  shape of the store is left to design.
- **Default the decider to the observed session instead of requiring an
  argument.** Requiring an explicit decider on every gate-passing command
  would break about fifty existing call sites across koto's plugin and the
  shirabe plugin and add a ritual argument that records nothing koto can't
  observe. Defaulting keeps every pass attributed, and the refusal still
  fires where no decider can be established. The cost is that "named a
  decider" includes "koto named it for you", which R16 labels.
- **Reversal is a new decision, never an edit.** The alternative, letting the
  latest record win, is what the findings ledger and several koto paths do
  today, and it erases who overturned what. Prior art from approval systems
  and delegation chains points the same way.
- **A reversal is final.** Reversing a reversal would make "in effect" depend
  on the parity of a chain. Restoring by recording a new decision keeps every
  state readable from one link.
- **Record-only reversal outside koto, enforced reversal inside a live
  workflow.** koto can't undo a merge or a status change, but it can stop a
  live workflow from building further on a gate pass that was overruled.
  That makes the gate reversal more than a note.
- **Keep the grader's name.** Renaming koto's "decider" command and fields
  would touch templates, configuration, the CLI, the ledger and the run
  journal. Naming the record's field differently costs nothing.
- **Grader verdicts stay telemetry.** The grader's pass never advances a
  workflow on its own, so its consultations are evidence a decision cites,
  not decisions.
- **Owned by koto, written by its consumers.** koto owns the record and its
  vocabulary; the shirabe plugin and other workflows write to it at the
  points where they already decide, and render their own views from it.
