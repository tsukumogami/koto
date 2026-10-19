---
schema: prd/v1
status: In Progress
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
absorbed:
  - docs/briefs/BRIEF-decision-record.md
---

# PRD: The decision record

## Status

In Progress

Absorbed [BRIEF-decision-record](docs/briefs/BRIEF-decision-record.md); carried in Absorbed Brief.

## Absorbed Brief

Dispatched agents, and the agents they start, accept documents and plan
steps, merge, get past gates, decide whether to escalate, and settle judgment
calls while nobody watches. Often the session that wrote something also
accepts it, which is legitimate when it holds the authority. What's missing
is any way to tell afterwards who made each decision (a session and the agent
within it, a person, or an automated check), what authority they held, or how
to overrule it so the rest of the toolkit honours the overrule. The partial
records koto and the shirabe plugin keep each answer "who decided"
differently, and a worker's decisions usually disappear with its session.

The outcome this feature exists for: the person or agent that dispatched a
session opens the record after the session is gone and reads every decision
it made or received, with its kind, decider and authority. When they
disagree, they overrule it with a decision of their own, the original visibly
no longer stands, and work built on decisions reads one record instead of
each keeping its own.

The journeys that frame it, carried as the User Stories below: a coordinator
audits an unattended run that accepted, passed a gate and merged; a
maintainer decides at an acceptance point and is named without an account;
the coordinator overrules all three decisions; a worker escalates one
question, keeps another and settles a judgment call; an anonymous gate pass
is turned away; a reporting tool counts overrules by kind.

The boundary, carried as the Requirements and Out of Scope below: in are the
decision kinds and what reversing each means, the two-level decider and
people without accounts, the authority and dispatch chain, a record that
outlives its session, bringing the existing records into agreement, and
refusing an unattributable gate pass. Out are on whose behalf a decision was
made, rules about who may decide, record-health instruments, undoing effects
outside koto, cross-machine durability and copying the harness's records.

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
   itself, each with its decider, and to overrule any of them, so that
   choices that leave no trace in the workflow today become visible and
   correctable.
5. As a maintainer at the root of a dispatch chain, I want to overrule a
   decision made anywhere below me from my own terminal, so that I am never
   dependent on the coordinator to correct a worker.
6. As a worker session, I want koto to refuse my attempt to overrule a
   decision made by the coordinator above me, and record the attempt, so
   that authority can't silently flow upward.
7. As a maintainer running unattended work, I want every gate pass to carry
   a decider and a pass whose decider can't be established to be refused, so
   that an audit never meets a pass nobody owns.
8. As the author of a tool that reports on decisions, I want to count, over a
   time window, how many decisions of each kind were overruled and list them,
   telling a session's decisions from a person's or an automated check's, so
   that the overrule rate can be read without special-casing any writer.
9. As a skill author whose skill already records decisions its own way, I
   want my skill to write to the shared record, or render its existing view
   from it, without breaking the readers that depend on its current output.

## Requirements

### Functional

**Decision kinds and subjects**

- **R1.** A decision has exactly one kind from this closed set, each with a
  closed set of outcomes:

  | Kind | Meaning | Outcomes |
  |---|---|---|
  | `accept` | accepting a document, a plan, one step of a plan, a test change, or a request's intent | `accepted`, `rejected` |
  | `merge` | merging a pull request | `merged` |
  | `gate-pass` | letting a workflow past a gate it had not passed on its own: by override, by a directed transition past an unmet gate, or by an agent's answer that lets a check pass | `passed` |
  | `escalate` | taking a question to someone above | `escalated` |
  | `decline-escalation` | deciding a question won't be taken to anyone above | `kept` |
  | `judgment` | settling a judgment call, including disputing a finding | `settled` |
  | `verdict` | a pass or fail judgment on evidence, such as a review verdict or a production verdict | `pass`, `fail` |
  | `reverse` | overruling an earlier decision | `reversed` |
  | `refused` | a decision attempt koto refused | `refused` |

- **R2.** Every decision names its subject in a form a reader can resolve
  after the deciding session is gone:

  | Kind | Subject |
  |---|---|
  | `accept` of a document or plan | repository, path, commit |
  | `accept` of a plan step | repository, plan path, step identifier, commit |
  | `accept` of a test change | repository, path or paths, commit |
  | `accept` of a request's intent | where the intent lives (a koto request identifier, a repository path, or a forge issue), plus an optional link to the ticket or alert it was drafted from |
  | `merge` | repository, pull request number, merged head commit |
  | `gate-pass` | workflow (koto session name), state, gate |
  | `escalate` | a question identifier and text, and the recipient (a session or a person) |
  | `decline-escalation` | a question identifier and text |
  | `judgment` | what was judged, as free text plus an optional pointer |
  | `verdict` | what was judged, at least one pointer to the evidence judged, and, when the verdict is on a change or a release, the repository and the commit judged |
  | `reverse` | the identifier of the decision it overrules |
  | `refused` | the attempted kind and subject, and the reason for refusal |

  A verdict's evidence pointer is required because the verdict is only as
  good as what it judged: `show` returns it for a reviewer to follow, and the
  review and release work that consumes verdicts checks it against the
  commit judged.

- **R3.** Every decision carries:
  - a stable identifier, unique across the record;
  - a UTC time to the second;
  - its outcome, from R1;
  - an optional reason, as free text;
  - an optional list of `basis` identifiers, naming earlier decisions it
    relies on (for example, a later plan step's acceptance naming an earlier
    step's).

**The decider**

- **R4.** Every decision names a decider of exactly one of three kinds:
  - `session`: a session identifier, plus the agent within it that decided,
    either `main` or the identifier of an agent the main agent started. When
    the agent can't be established, the agent is recorded as `unknown`,
    never assumed to be `main`.
  - `person`: a handle supplied by the surface the decision came through, the
    surface (one of `terminal`, `forge`, `relay`), an optional link to the
    original (such as a forge review), and the session that entered the
    decision.
  - `check`: the name of an automated check, such as koto's grader, and the
    session that ran it.

  Two deciders are the same decider when they are the same session (whatever
  agent within it), the same person handle, or the same check run by the same
  session.
- **R5.** koto stores a person's handle as an opaque string. koto has no user
  table, account field, login, registration or credential for any decider,
  and never requires one to record or read a decision.
- **R6.** Each identity in a decision (the session, the agent, the person,
  each link in the dispatch chain) carries one source label: `observed` when
  koto read the value itself and nobody upstream merely claimed it, or
  `handed` when a caller supplied it. A value read from the workspace
  manager's records keeps the provenance those records give it: today the
  workspace manager records a session's parent as its dispatcher supplied it,
  so koto labels that parent `handed`. When a handed value and an observed
  value exist for the same identity and differ, both are recorded.
- **R7.** A person's handle is always `handed`: koto can't tell a person at a
  terminal from an agent running the same command. A `person` decider is
  recorded with surface `terminal` or `forge` as the caller states. With
  surface `relay` (a session passing on what a person said elsewhere), the
  decider is recorded as `person` only when the caller supplies a link to a
  forge review or comment, which koto accepts as an absolute `https` URL
  with no credentials or query string.
  Without one, the decider is the entering session and the reported person
  is kept as a handed note, which `show` returns so an auditor can follow up
  with the person the relayer named. koto stores links and never fetches
  them.
- **R8.** A decision records the run mode (`attended`, `unattended` or
  `unknown`): `observed` when the agent harness reports whether its session
  is attended, otherwise as handed by the caller, and `unknown` when neither
  gives it,
  separately from who the decider is, so an agent's decision in an
  unattended run is never read as a person's confirmation. R19 filters on
  it.

**Authority**

- **R9.** Every decision records the authority its decider held, with these
  fields:
  - the authority kind: `dispatch` for a `session` or `check` decider,
    `person` for a `person` decider;
  - the dispatch identifier the deciding session was launched under, when
    there is one;
  - a content digest of the instructions handed to the deciding session at
    dispatch, when available;
  - the run mode;
  - the immediate dispatcher (a session or a person), labelled per R6;
  - on `reverse` decisions only, the standing R12 assigned: `verified` or
    `asserted`.

  For a `person` decider the session fields are the entering session's, and
  for a `check` decider the session's that ran the check. A later authority
  setting adds another authority kind without changing what `dispatch` and
  `person` records mean.
- **R10.** Every decision records its dispatch chain: an ordered list of
  links from the deciding session (for a `person` decider the entering
  session, for a `check` the session that ran it) up to the root. A link is
  the root only when its source says so: the workspace manager's lineage
  records no dispatcher for that session, or the caller hands the link as a
  person or as a session with no dispatcher. Each link is labelled per R6. A
  chain whose next link no source supplies is marked `incomplete`; missing
  lineage never counts as a root, and missing links are never guessed.
- **R11.** The workspace manager doesn't read or write workflow state, and
  it doesn't run koto, and this feature doesn't change it. koto obtains each
  link of the chain from the records the workspace manager already keeps
  about the sessions it launched (today the dispatch identifier and the
  parent), or from the dispatching session handing it over. koto defines a
  versioned lineage format through which the workspace manager can later
  supply a fuller chain (an affirmed root, a copied chain that survives an
  ancestor's removal, and entries for sessions started for an incoming
  event), and reads it when present. Where a launch supplies no lineage, the
  link is recorded as absent and the chain as `incomplete`.

**Overruling**

- **R12.** Only the original decision's chain decides who may reverse it.
  A `reverse` decision is accepted when its decider is:
  - the same decider as the original (R4);
  - for an original decided by a `session` or a `check`, a session that
    appears in the original's chain, or the person at that chain's root;
  - for an original decided by a `person`, the person at the root of the
    original's chain. A session can't reverse a person's decision.

  Matching is on recorded values. The reversal's standing is `verified`
  when the match rests on an `observed` value and `asserted` when it rests on
  `handed` values only; handed values never raise standing, and a match
  through an `incomplete` chain is never `verified`. When nothing matches and
  the original's
  chain is `incomplete`, koto records the reversal with authority
  `asserted`. When nothing matches and the original's chain is complete,
  koto refuses the reversal and records a `refused` decision.
- **R13.** koto refuses, and records as `refused`, a reversal of a `refused`
  decision, of a `reverse` decision, of a decision that is already reversed,
  or of an identifier that isn't on the record. When two reversals of the
  same decision race, exactly one is recorded as `reverse` and the other is
  refused as already reversed.
- **R14.** Once reversed, a decision reads as `reversed` in every query and
  view, with its reversal's identifier. The original is never edited or
  removed. To restore what was reversed, a decider records a new decision of
  the original kind.
- **R15.** What a reversal means, per kind:
  - `accept` with outcome `rejected`: the rejection no longer stands, and
    the subject is neither accepted nor rejected until a new `accept` says
    so.
  - `accept`: the subject is no longer accepted. The reversal's result, and
    showing the reversed decision, list every decision whose `basis` names it
    directly as affected; following those lists gives the transitive set.
    koto doesn't edit the document's status.
  - `merge`: the merge is no longer endorsed. The reversal carries a pointer
    to the compensating change (a revert pull request), or records that none
    exists yet. koto doesn't change the forge.
  - `gate-pass`: the pass no longer stands. If the workflow that passed the
    gate still exists, every later `koto next` on it, at whatever state the
    workflow has reached, refuses to advance and reports the reversal as a
    blocking condition. koto doesn't move the workflow back itself. A
    reversal blocks on the one pass it names, even when the gate was passed
    several times. The block clears when the workflow is rewound to or before the gated state,
    or when the reverser, or a decider R12 would let reverse the reverser,
    records a new `gate-pass` for that gate. A new `gate-pass` clears a
    reversal whose standing is `verified` only when its own R12 match over
    the reverser is `verified`; an `asserted` match clears only an
    `asserted` reversal.
  - `escalate`: the question is withdrawn from its recipient.
  - `decline-escalation`: the question counts as escalated to the reverser.
  - `judgment` and `verdict`: the call or verdict no longer stands. The
    reversal may carry the outcome that replaces it and, for a verdict that
    let a release through, a pointer to the compensating action (a
    rollback), or a note that none exists yet.

**Every gate pass has a decider**

- **R16.** Each way of getting past a gate by choice (`koto overrides
  record`, `koto next --to` past a gate that hasn't passed, and `koto decider
  answer` with an answer that lets the check pass) records a `gate-pass`
  decision with a decider. A decider the caller names, labelled `handed`,
  is used as given. When the caller names none, koto uses the session it
  observes and marks the decider `defaulted` in addition to its R6 label.
  When neither exists, koto refuses the pass, names the missing decider in
  its error, leaves the workflow where it was, writes none of the records the
  command writes today, and records a `refused` decision. Otherwise the
  command's existing records and its decision are written together or not
  at all.
- **R16a.** Where a team declares who may decide at a gate, a pass whose
  decider is none of the declared deciders is refused and recorded as
  `refused`, whether the decider was named or defaulted. This rule takes
  effect when such declarations exist; until then no gate has declared
  deciders and R16 alone applies. A person passing a gate from inside an
  agent's session names themselves with `--by person:<handle>`; otherwise the
  pass is recorded as the session's, marked `defaulted`.

**Durability, access and reading**

- **R17.** A decision stays readable after the session that made it is
  removed, after a successful end and after the workspace it ran in is
  reclaimed. No session cleanup or workspace reclaim removes decisions.
  Decisions are kept until explicitly pruned, and pruning isn't part of this
  feature.
- **R18.** Any session on the same machine can record decisions, including
  ones whose subject is another session's workflow, and can read every
  decision on the record.
- **R19.** koto offers commands with machine-readable output to record a
  decision, to show one by identifier, and to list decisions filtered by any
  combination of:
  - the deciding session, including or excluding the decisions it received
    (decisions whose subject is its workflow, that escalate to it, or that
    reverse its decisions);
  - the subject;
  - the kind;
  - the decider kind;
  - a time window;
  - the run mode;
  - the dispatch identifier or instructions digest the decider acted under,
    so a coordinator can find every decision made under one set of
    instructions when those instructions turn out to be wrong;
  - whether the decider was `defaulted`.

  Every result says whether the decision is in effect or reversed. Results
  are ordered by time, oldest first, with the identifier breaking ties. A
  record command given a kind, outcome or subject that R1 and R2 don't allow
  fails with an error naming the field and records nothing.
- **R20.** koto reports, for a time window in UTC that includes its start
  and excludes its end, counting every kind except `reverse` and `refused`
  as decisions:
  - the decisions made in the window and how many of them have been reversed
    as of the report, as a table of kind by decider kind;
  - the reversals recorded in the window, as a table of the reversed
    decision's kind by its decider kind;
  - each reversed decision, listed with its reversal;
  - for `gate-pass` decisions, how many had a `defaulted` decider.
- **R21.** A decision can carry evidence pointers:
  - to records the agent harness keeps about the same moment (the harness's
    session identifier, a tool-call identifier, and the agent identifier
    when supplied);
  - to a koto run-journal record, by session and sequence number, such as
    the grader's check record a decision relied on.

  The record stores the pointers only and never copies what they point at.

**Bringing existing records together**

- **R22.** The decider vocabulary in R4 is the only one koto uses for who
  made a decision. The existing records map onto it as follows:

  | Existing record | Becomes | Decider mapping |
  |---|---|---|
  | `koto overrides record` and the run journal's override records | a `gate-pass` decision, written by the existing command | the overriding session or person |
  | `koto decisions record` | a `judgment` decision, written by the existing command | the recording session or person |
  | `koto decider answer` | `gate-pass` when the answer lets the check pass, `judgment` otherwise | the answering session and agent |
  | The grader's consultation and check records in the run journal | stay telemetry; decisions cite them as evidence | `check` when a decision names the grader |
  | Disputed findings (the pending change's `--by agent\|person`) | a `judgment` decision about the finding | the R4 vocabulary replaces `agent\|person` |
  | The shirabe findings ledger | keeps its per-round records; a panel pass writes a `verdict`, a dismissal a `judgment` | `reviewer` and `agent` writers become `session` with the agent named; `check` and `decider` writers become `check` |
  | The shirabe coordinator's decision table and reversals | a view rendered from the record; settled entries are `judgment` or the question's kind, escalations `escalate`, reversals `reverse` linked by identifier | "the coordinator" becomes the coordinator's `session`; "a person" becomes `person` with its surface |
  | Document acceptance through the plugin's status transition, and merges through its merge step | `accept` and `merge` decisions | the transitioning or merging session, or the person |

  Each existing koto command keeps writing what it writes today and also
  writes the decision. The coordinator's table is a view; the findings
  ledger keeps its own records and also writes the decision. The caller
  chooses the kind: `verdict` for a pass or fail on named evidence,
  `judgment` for anything else settled, and koto validates only the subject
  each kind requires.

  koto delivers the record and the koto rows of this table. The shirabe
  rows are delivered in that plugin against this vocabulary, and they belong
  to this feature even though they ship from another repository.
- **R23.** In koto, the grader keeps the name "decider" in its commands,
  template fields and configuration. The record's field for who decided is
  named so it doesn't collide, and a grader appears in it as a decider of
  kind `check`.
- **R24.** koto offers a command to import decisions recorded before the
  record existed, such as those a coordinator kept in its own record. An
  imported decision keeps its recorded decider and authority, labelled
  `handed`, with a pointer to where it was recorded. The pointer (source
  location plus the entry's identifier there) is the entry's identity:
  importing the same source entry twice adds it once. An imported decision
  gets a new identifier, keeps the time the source recorded, and has a chain
  marked `incomplete` unless the source records one. Which source formats
  are read first is the design's choice; the coordinator's decision table is
  one of them.

### Non-functional

- **R25.** Decisions are append-only and immutable once written. Concurrent
  writers on the same machine never lose an entry and never corrupt one.
- **R26.** Readers of koto's existing events, run journal and feed keep
  working unchanged. Every existing command keeps working with the arguments
  it accepts today, with one deliberate exception: R16's refusal of a gate
  pass whose decider can't be established. Tests that run the three
  gate-passing commands with no harness session identifier set must now set
  one or name a decider.
- **R27.** A decision's reason text follows the run journal's privacy rule
  for override rationales: it stays in the local record and isn't copied
  into exported telemetry.
- **R28.** Recording or reading a decision makes no network call and doesn't
  depend on the workspace manager or the agent harness being reachable. A
  missing harness or lineage value is recorded per R4, R6 and R10 rather
  than failing the decision.
- **R29.** On a record holding 100,000 decisions spread over twelve months,
  listing one month's decisions and producing the R20 report each complete
  in under five seconds on the project's CI runner.

## Acceptance Criteria

**Reading an unattended run**

- [ ] An unattended session accepts a document, gets past a gate with `koto
  overrides record`, and records a merge. After the session reaches a
  successful end and its session directory is gone, listing by that session
  returns three decisions, each with its kind, subject per R2, decider kind
  `session` with an agent value, authority per R9, chain per R10, identifier,
  UTC time and outcome.
- [ ] The same listing still returns all three after the workspace that ran
  the session has been reclaimed.
- [ ] Two decisions recorded in one run have different identifiers, and a
  decision recorded with a reason returns that reason.
- [ ] A `verdict` on a change records and lists the repository and commit
  judged; recording a verdict on a change without a commit is refused with an
  error naming the field.
- [ ] An `accept` of a request's intent recorded with a forge issue as its
  location and a link to its source ticket lists both.
- [ ] An `accept` recorded with outcome `rejected` lists with that outcome.
  Recording a decision with a kind or outcome outside R1, or a `verdict`
  with no evidence pointer, is refused with an error naming the field.
- [ ] A decision recorded from a session whose acting agent can't be
  established lists the agent as `unknown`.

**People as deciders**

- [ ] The same unit with a maintainer accepting the document at the terminal
  records an `accept` with decider kind `person`, the maintainer's handle,
  surface `terminal`, and the entering session.
- [ ] An acceptance relayed by a coordinator with a forge review link is
  recorded with decider kind `person`, the link stored, and the handle
  labelled `handed`. The same relay without a link is recorded with the
  coordinator's session as decider and the person kept as a handed note.
- [ ] koto's stored decision format has no account, login or credential
  field, and recording a person decision succeeds with no prior registration
  step.

**Identity labels and chains**

- [ ] A decision recorded from a session whose harness session identifier is
  set has its session labelled `observed`. A decision whose caller hands a
  different session identifier keeps both values.
- [ ] A decision recorded in a session the workspace manager launched by
  direct dispatch, and one by detached dispatch, each carry the dispatch
  identifier and parent from the workspace manager's records, labelled
  `handed`, with the chain marked `incomplete`. A session started for an
  incoming event, or outside the workspace manager, with nothing handed,
  records a chain marked `incomplete`.
- [ ] With a fixture session entry in the versioned lineage format carrying
  a copied chain and `parent: none`, a decision's chain reaches that root,
  is not marked `incomplete`, and each link carries the label the entry gives
  it.
- [ ] A decision recorded with a handed `--dispatched-by none` closes the
  chain at a handed root.
- [ ] Dispatching a session with koto absent from the dispatcher's path
  succeeds, showing the workspace manager runs no koto command.
- [ ] A decision recorded with run mode `unattended` returns decider kind
  `session` and run mode `unattended`, and a decision recorded with no run
  mode returns `unknown`. Listing filtered by run mode `unattended` returns
  the first and not the second.
- [ ] A decision recorded where the harness reports its session as attended,
  with no run mode handed, records run mode `attended` labelled `observed`.
- [ ] Listing filtered by a dispatch identifier, and by an instructions
  digest, each returns exactly the decisions recorded under it from a
  fixture of known decisions.
- [ ] A `verdict` recorded with decider kind `check` naming the grader, and
  an evidence pointer to a run-journal check record, lists with that decider,
  the session that ran the check, and the pointer.

**Overruling**

- [ ] The dispatching coordinator reverses the acceptance, the gate pass and
  the merge from its own session. Each reversal has decider kind `session`
  naming the coordinator, its own authority, and the original's identifier.
  Each original then lists as `reversed`.
- [ ] Reversing a `verdict` that let a release through, with a rollback
  link, stores the link; without one, it records that no compensating action
  exists yet.
- [ ] Reversing the merge with a revert pull request link stores the link.
  Reversing a merge without one records that no compensating change exists.
- [ ] Reversing an acceptance that a later acceptance names in its `basis`
  returns the later acceptance as affected, both in the reversal's result
  and when showing the reversed acceptance.
- [ ] After the gate pass is reversed while its workflow still exists and
  has moved one state past the gate, `koto next` on that workflow refuses to
  advance, leaves the state unchanged, and reports the reversal as a blocking
  condition. After `koto rewind` to the gated state, the block is gone. In a
  second run, a new `gate-pass` recorded by the reverser clears it instead.
  In a third run, a `verified` reversal stays blocking after a new
  `gate-pass` whose match over the reverser is only `asserted`.
- [ ] A worker reverses its own `judgment`, and the reversal is accepted
  with authority `verified`.
- [ ] A coordinator's attempt to reverse a decision whose decider is a
  `person` is refused, and the person at the chain's root reversing it is
  accepted.
- [ ] Reversing an identifier that isn't on the record is refused.
- [ ] Two sessions reversing the same decision at the same time produce
  exactly one `reverse` and one `refused`.
- [ ] A maintainer whose handle is the root of the chain reverses a worker's
  judgment call from the terminal, and the reversal is accepted.
- [ ] A worker's attempt to reverse a decision made by its coordinator, with
  both chains complete, is refused and listed as a `refused` decision.
- [ ] A reversal by a session with no match when the original's chain is
  `incomplete` is recorded with authority `asserted`.
- [ ] Attempts to reverse a `reverse` decision, a `refused` decision and an
  already reversed decision are each refused and recorded.
- [ ] After an acceptance is reversed, a new `accept` of the same subject
  lists as in effect while the original still lists as `reversed`.

**Escalation and judgment**

- [ ] A worker records an `escalate` to its coordinator, a
  `decline-escalation` and a `judgment`. Listing returns all three with
  their deciders and the escalation's recipient.
- [ ] The coordinator reverses the judgment, and the reversal is listed with
  its decider.
- [ ] Reversing the escalation lists the question as withdrawn from the
  coordinator. Reversing the decline lists the question as escalated to the
  reverser.

**Gate passes**

- [ ] `koto overrides record`, `koto next --to` past an unmet gate, and `koto
  decider answer` with a passing answer, each run with no decider named, each
  record a `gate-pass` whose decider is the observed session marked
  `defaulted`. Listing filtered on `defaulted` returns exactly those three,
  and a pass recorded with `--by person:<handle>` is not among them.
- [ ] The same three commands run with the harness session identifier unset
  and no decider named are each refused. The error names the missing
  decider, the workflow's state is unchanged, and a `refused` decision is
  listed.

**Querying and reporting**

- [ ] Listing filtered by subject, by kind, by decider kind and by a time
  window each returns exactly the matching decisions from a fixture of known
  decisions.
- [ ] Listing by a coordinator's session, including received decisions,
  returns the escalations addressed to it and the reversals of its
  decisions. Excluding them returns only the decisions it made.
- [ ] A session records a `judgment` about another session's workflow, and
  a third session lists it.
- [ ] For a fixture spanning two months, the report for the second month
  counts the decisions made in it and how many are reversed, in a kind by
  decider-kind table whose cells match the fixture. It also counts a
  reversal recorded in the second month of a first-month decision under the
  reversals recorded in the window, in the cell of that decision's kind and
  decider kind. Its count of `gate-pass` decisions with a `defaulted`
  decider matches the fixture.
- [ ] A decision recorded with a harness tool-call identifier returns that
  identifier and contains no copied harness content.

**Existing records**

- [ ] `koto overrides record` writes a `gate-pass` decision with a session
  decider, and `koto overrides list` still returns the override.
- [ ] `koto decisions record` writes a `judgment` decision with a session
  decider, and `koto decisions list` still returns the choice.
- [ ] `koto decider answer` writes a `gate-pass` for a passing answer and a
  `judgment` for a failing one.
- [ ] Recording a disputed finding writes a `judgment` whose decider uses the
  R4 kinds and no `agent|person` value.
- [ ] A findings-ledger panel pass writes a `verdict` and a dismissal writes
  a `judgment`, with the writer mapped as R22 states.
- [ ] The coordinator's decision table rendered from the record, for a
  fixture of settled, escalated and reversed entries, matches the table the
  coordinator produces today for the same entries, except that reversals link
  by identifier.
- [ ] A document status transition and a merge through the plugin each write
  an `accept` and a `merge` decision.
- [ ] Importing a coordinator's recorded decisions keeps each decider and
  authority labelled `handed` with a source pointer, and importing the same
  entries again adds nothing.

**Non-functional**

- [ ] Two sessions recording 100 decisions each at the same time produce 200
  complete entries, and no existing entry changes.
- [ ] The existing test suites for the run journal, the feed and the grader
  report pass unchanged. The existing functional tests for `koto overrides
  record`, `koto decisions record`, `koto next --to` and `koto decider
  answer` pass without changes to their arguments, with at most a harness
  session identifier added to their environment.
- [ ] Exporting telemetry for a run whose decisions carry reasons produces no
  record containing any reason text.
- [ ] With networking disabled, the harness session identifier unset and no
  lineage available, recording a decision that names its decider succeeds,
  and the missing values carry the labels R4, R6 and R10 require.
- [ ] On a generated record of 100,000 decisions over twelve months, a
  one-month listing and a one-month report each finish in under five seconds
  on the CI runner.

## Out of Scope

- **On whose behalf a decision was made.** Recording the person a session was
  acting for at the top of its chain is a later extension built on this
  record.
- **Rules about who may decide.** Requiring a person at a point, and the
  settings that narrow authority, are policy layers that read this record
  later. Excluding the author from a decision is one such setting, chosen per
  decision point: off at the solo end, where an authoring session may accept
  its own work when its authority covers it, and on at the strict end, where
  "non-author" means a decider in a different session. Later separation-of-duties
  work enforces it by reading this record's decider and subject. This record
  says who decided and under what authority; R12's matching only decides who
  may overrule.
- **Record-health instruments.** Detecting a workspace that stopped
  recording, or auditing whether escalations were warranted, read this record
  later.
- **Undoing effects outside koto.** koto doesn't revert merges, reopen pull
  requests or edit documents when a decision is overruled.
- **Cross-machine durability.** The record lives in koto's local store on one
  machine.
- **Changes to the workspace manager.** Publishing the versioned lineage
  format, copying a session's chain into its children's records, and
  recording sessions started for an incoming event belong to planned work on
  the recorded dispatch tree. This feature reads them once they exist and
  works without them.
- **Pruning and compaction.** R17 keeps decisions until pruned; a pruning
  command is later work.
- **Duplicating the harness's records.** Tool permission decisions and
  transcripts stay where the harness keeps them; R21 only points at them.
- **Authenticating identities.** Identities are attribution, not
  authorization: anything running as the operator can write the record, and
  R6's labels say how far each identity can be trusted.

## Known Limitations

- R16a, the refusal at gates with declared deciders, has no acceptance
  criterion here because nothing declares deciders yet. It gets one when
  declarations exist.
- An `observed` session identifier can still be forged by anything running
  as the operator. The record makes misattribution visible (R6), not
  impossible.
- The harness's session identifier changes when a conversation is cleared, so
  one logical session can appear as several deciders. Design decides whether
  koto relates them.
- Telling a session's main agent from an agent it started needs the harness
  to say which agent is acting. Until it does, many decisions will record the
  agent as `unknown` (R4).
- The record grows without bound until a pruning command exists. R29 sets
  the size it must handle.

## Decisions and Trade-offs

- **One record outside any single session.** Alternatives: keep decisions in
  each session's log and export at the end, carry them only on request legs,
  or treat the coordinator's GitHub record as authoritative. Sessions are
  removed when they succeed, root sessions have no request leg, and a GitHub
  record leaves koto unable to answer offline. The shape of the store is left
  to design.
- **Default the decider to the observed session instead of requiring an
  argument.** Requiring an explicit decider on every gate-passing command
  would break about fifty call sites across koto's plugin and the shirabe
  plugin and add an argument that records nothing koto can't observe.
  Defaulting keeps every pass attributed and marks it `defaulted`, and the
  refusal still fires where no decider can be established. The project
  owner settled this; see the settled items below.
- **Reversal is a new decision, never an edit.** Letting the latest record
  win is what the findings ledger and several koto paths do today, and it
  erases who overturned what. Prior art from approval systems and delegation
  chains points the same way.
- **A reversal is final.** Reversing a reversal would make "in effect" depend
  on the parity of a chain. Restoring by recording a new decision keeps every
  state readable from one link.
- **The original decider may reverse its own decision.** A session that
  notices its own mistake shouldn't have to escalate to undo it, and the
  reversal is still named.
- **Record-only reversal outside koto, enforced reversal inside a live
  workflow.** koto can't undo a merge or a status change, but it can stop a
  live workflow from building on a gate pass that was overruled. That makes
  the gate reversal more than a note.
- **A relayed person decision needs a forge link.** A name typed by a relayer
  is the relayer's claim. Only a link a reader can check earns the person
  decider; without one the relayer owns the decision.
- **Keep the grader's name.** Renaming koto's "decider" command and fields
  would touch templates, configuration, the CLI, the ledger and the run
  journal. Naming the record's field differently costs nothing.
- **Grader verdicts stay telemetry.** The grader's pass never advances a
  workflow on its own, so its consultations are evidence a decision cites,
  not decisions.
- **Owned by koto, written by its consumers.** koto owns the record and its
  vocabulary; the shirabe plugin and other workflows write to it where they
  already decide, and render their own views from it.

### Items the project owner settled

The project owner settled these on 2026-10-19; each was open in an earlier
draft of this PRD.

- **Gate passes with no named decider.** Decided: keep R16's default to the
  observed session, marked `defaulted`, with `defaulted` as a filter (R19)
  and a report column (R20), and add R16a's refusal at gates with declared
  deciders. The acceptance scenario the planning above this PRD gave, a pass
  naming no decider being refused, is restated as: every gate pass records
  its decider, and a pass whose decider can't be established is refused.
- **Who may accept their own work.** Decided: excluding the author is a
  setting per decision point, off at the solo end and on at the strict end,
  enforced by later separation-of-duties work (see Out of Scope). This record
  doesn't encode it.
- **Further consumers.** Decided: review verdicts record the repository and
  commit judged, and intent acceptance records where the intent lives and an
  optional link to its source (R2). Release-time verdicts, rollbacks and
  reverting a change to how strict a decision point is need nothing new: a
  release gate pass outside a koto workflow is a `verdict` on the release's
  repository and commit, and its reversal carries the rollback (R15).
