---
schema: prd/v1
status: Draft
problem: |
  Delegated sessions accept documents, pass gates, merge, escalate or decline to, and
  settle judgment calls with nobody watching. Nobody above can later read who made a
  decision and on what authority, or reverse it so koto honours the reversal. koto's
  partial records each attribute differently or not at all, and its decider ledger keeps
  gate answers in a host file because sessions used to vanish. Session retention now
  keeps a finished session readable while its dispatch hierarchy is alive, so the record
  can live in the session that decided.
goals: |
  Every delegated decision is kept as write-once keys in the session that made it,
  naming its decider and the authority it acted under. Any session or person above it
  in the dispatch chain reads it after the deciding session finished and reverses it
  from its own session, with every reader agreeing on what is still in effect, and a
  live workflow stops before building on a reversed decision. Nothing about decisions is
  written to or read from a file under koto on the host.
upstream: docs/briefs/BRIEF-decision-record.md
motivating_context: |
  A first build (koto#394, koto#396) kept decisions in a host store,
  `~/.koto/decisions/`, because koto removed a session when its workflow succeeded. The
  owner closed both unmerged and asked for decisions to live in the koto session itself,
  read and reversed from above through session retention, which has since shipped
  (koto#410 and the work it indexes). The first build's store-independent parts carry
  over as requirements here.
---

# PRD: The decision record

## Status

Draft

## Problem Statement

When a person hands work to an agent, that agent and the sessions it dispatches make
decisions without the person watching. They accept a design, a plan or one step of a plan,
merge a pull request, let a workflow past a gate, decide whether a question is worth taking to
whoever dispatched them, and settle judgment calls. Often the session that wrote something also
accepts it, which is legitimate when it holds the authority to.

For each of those decisions, nothing records who made it (a session, an agent inside it, a
person, or an automated check), what entitled them to, or how the session above can overrule it
in a way a later reader can see and koto's own workflows respect.

The records that exist disagree. `koto overrides record`, `koto decisions record` and `koto next
--to` past a gate keep a choice with no actor. `koto decider answer` implies the agent.
`koto disputes record --by agent|person` names a writer category and nothing more. The decider
ledger keeps gate answers in a host file, `~/.koto/_decider_ledger.jsonl`, outside any session.
shirabe's findings ledger and its coordinator's decision table each keep their own idea of who
decided. Those are parallel records forming where one shared record was meant to be.

A first build of the shared record solved attribution and reversal, then put every decision in a
host-level store because the deciding session would be gone by the time anyone read it. The
owner rejected that store as a workaround for koto's session lifetime and asked for decisions to
live in the koto session. koto now keeps a finished session readable while any session in its
dispatch hierarchy is alive, writes koto-owned dispatch links in each dispatched session, and
leaves a read session untouched when another session reads it. The record can now be built on
that footing.

## Goals

- The session or person above a delegated session reads every decision it made or received, from
  the deciding session's own keys, after it finished: kind, subject, decider, authority, chain and
  time.
- Anyone above the decider in the dispatch chain reverses a decision from its own session. The
  reversal is its own decision, and every reader agrees the original no longer stands.
- A live workflow that relied on a reversed decision stops at its next step.
- A person deciding is named without koto holding an account for them, and getting past a gate is
  never anonymous.
- koto's existing partial records write to this one vocabulary, and the decider ledger stops being
  a host file.
- The record works with koto alone; niwa may set it up and is never required.

## User Stories

1. As a coordinator that dispatched an unattended worker, I want to read, after the worker
   finished, each decision it made (accepting a design, getting past a gate, merging) with the
   worker session as decider, the agent within it, and the authority it held, so I can audit the
   run without having watched it.
2. As a maintainer who accepts a design myself, I want the record to name me as the decider by a
   handle and say which surface my decision came through, without koto holding an account for me,
   so a reader can tell my decision from an agent's.
3. As a coordinator that disagrees with a worker's acceptance, gate pass and merge, I want to
   reverse each from my own session and have every reader see my reversal, its decider, and that
   the original no longer stands.
4. As a coordinator, I want to see which questions a worker escalated, which it declined to
   escalate, and which judgment calls it settled itself, each with its decider, and to reverse any
   of them.
5. As a maintainer auditing gate passes, I want every pass to name its decider, defaulted ones
   marked as such, and a pass with no establishable decider refused.
6. As the person at the top of a three-level chain, I want to reverse a grandchild's decision
   after the middle session finished, and have the grandchild's live workflow stop at its next
   step, without my reads or writes changing anything in the sessions below me.
7. As one of two coordinators above the same worker, I want a reversal race to resolve the same
   way for every reader, with the losing reversal visibly refused.
8. As a person who moves a coordinating session to another workspace with `koto session
   import`, I want to keep reading and reversing the decisions below it from there.
9. As the author of a reporting tool, I want to count over a time window how many decisions of
   each kind were reversed, from the sessions koto keeps, with no host file to read.
10. As a skill author whose skill already records decisions its own way, I want my skill to write
    to the shared record without breaking the readers of my current output.

## Requirements

### Functional

**Decision kinds and subjects**

- **R1.** A decision has exactly one kind from this closed set, each with a closed set of
  outcomes:

  | Kind | Meaning | Outcomes |
  |---|---|---|
  | `accept` | accepting a document, a plan, one step of a plan, a test change, a staged draft, a request's intent, or a policy or configuration change | `accepted`, `rejected` |
  | `merge` | merging a pull request | `merged` |
  | `gate-pass` | letting a workflow past a gate it had not passed on its own: by override, by a directed transition past an unmet gate, or by an agent's answer that lets a check pass | `passed` |
  | `escalate` | taking a question to someone above | `escalated` |
  | `decline-escalation` | deciding a question won't be taken above | `kept` |
  | `judgment` | settling a judgment call, including disputing or dismissing a finding | `settled` |
  | `verdict` | judging evidence, such as a review verdict or a production verdict | `pass`, `fail`, `inconclusive` |
  | `reverse` | overruling an earlier decision | `reversed` |
  | `refused` | a decision attempt koto refused | `refused` |

- **R2.** Every decision names its subject in a form a reader can resolve after the deciding
  session is gone:

  | Kind | Subject |
  |---|---|
  | `accept` of a document, plan or test change | repository, path or paths, commit |
  | `accept` of a plan step | repository, plan path, step identifier, commit |
  | `accept` of an intent or staged draft | where it lives (a koto request identifier, a repository path, or a forge issue), plus an optional link to the ticket or alert it came from |
  | `merge` | repository, pull request number, merged head commit |
  | `gate-pass` | workflow (koto session name), state, gate |
  | `escalate` | a question identifier and text, and the recipient (a session or a person) |
  | `decline-escalation` | a question identifier and text |
  | `judgment` | what was judged, as free text plus an optional pointer |
  | `verdict` | what was judged, at least one evidence pointer, and, for a change or a release, the repository and commit judged |
  | `reverse` | the session name and identifier of the decision it overrules |
  | `refused` | the attempted kind and subject, and the reason |

- **R3.** Every decision carries an identifier unique within its session, a UTC time to the
  second, its outcome, an optional reason, and an optional list of `basis` references (session
  name and identifier) naming earlier decisions it relies on. A decision is addressed from outside
  its session by the pair of session name and identifier.

**Where a decision lives**

- **R4.** A decision is recorded as keys in the session that made it: the session whose agent or
  person decided, or, for a person's decision, the session that entered it. A `reverse` is
  recorded in the reverser's session. No decision, reversal, refusal or gate answer is written to
  or read from a file under koto on the host outside the sessions.
- **R5.** Decision keys are write-once. Once a decision's keys exist, koto refuses every later
  write to them, by koto's own commands and by `koto context add` or `remove` from any process,
  and the refusal names the key. Decision keys are written only by koto's decision commands, never
  by a template's context assignment.
- **R6.** Decision keys sync and import like any other key under the cloud backend. A session
  migrated by `koto session import` serves its decisions to readers and refuses new ones, as koto
  already does for its other reads and writes.

**The decider**

- **R7.** Every decision names a decider of exactly one kind:
  - `session`: the koto session name, the harness session identifier when koto observes one, the
    agent session's anchor when there is one, and the agent within it that decided: `main`, the
    identifier of an agent the main agent started, or `unknown` when it can't be established,
    never assumed to be `main`.
  - `person`: a handle the surface supplied, the surface (`terminal`, `forge`, `relay`), an
    optional link to the original, and the session that entered it.
  - `check`: the name of an automated check, such as koto's grader, and the session that ran it.
- **R8.** koto stores a person's handle as an opaque string. It has no user table, account,
  login, registration or credential for any decider, and never requires one to record or read a
  decision.
- **R9.** Each identity value carries a source label, `observed` when koto read it itself or
  `handed` when a caller supplied it. A person's handle is always `handed`. A `relay` decision is
  recorded as the person's only when the caller supplies an absolute `https` link to a forge
  review or comment with no credentials or query string; otherwise the decider is the entering
  session and the named person is kept as a handed note. koto stores links and never fetches
  them.
- **R10.** A decision records the run mode (`attended`, `unattended`, `unknown`), observed when
  the harness reports it and handed otherwise, separately from the decider.

**Authority and the chain**

- **R11.** Every decision records the authority its decider held: the authority kind (`dispatch`
  for a session or check, `person` for a person), the authority level and grants recorded for the
  deciding session's agent session when there are any, the immediate dispatcher, and the run mode.
- **R12.** Every decision records its dispatch chain as a snapshot at decision time: the ordered
  sessions from the deciding session up to a root, read from the koto-owned dispatched-by links in
  each session's own keys. A link is labelled by how koto came to write it. A chain whose next
  session is not readable is marked `incomplete`; missing links are never guessed and never count
  as a root.
- **R13.** koto reads no workspace-manager file and calls no workspace-manager command to build
  the decider, authority or chain. Where a workspace manager passes values to a session, they reach
  koto only through koto's own variables and links.

**Reading from above**

- **R14.** koto lists a session's decisions, and the decisions it received (gate passes on its
  workflow, escalations addressed to it, reversals of its decisions), with each decision's in-effect
  status. Filters: kind, decider kind, subject, time window, run mode, and whether the decider was
  defaulted. Results are ordered by time, then identifier, and are machine-readable.
- **R15.** koto lists the decisions of every session below a given session, through any number of
  finished sessions in between, for as long as retention keeps them.
- **R16.** Reading a session's decisions from another session changes no key and appends no event
  in the read session, and the read writes nothing into any session between them.

**Reversal**

- **R17.** Only the original decision's chain decides who may reverse it. A `reverse` is accepted
  when its decider is:
  - the same decider as the original;
  - for an original decided by a session or a check, a session that appears above the deciding
    session in the original's chain, or a person entering through such a session;
  - for an original decided by a person, a person entering through the same session or a session
    above it. A session alone can't reverse a person's decision.

  The reversal records the chain path that gave it standing and its standing, `verified` when every
  link on that path is koto-written and the chain is complete, `asserted` otherwise. When nothing
  matches and the original's chain is `incomplete`, koto records the reversal as `asserted`. When
  nothing matches and the chain is complete, koto refuses and records a `refused` decision in the
  would-be reverser's session.
- **R18.** koto refuses, and records as `refused` in the attempting session, a reversal of a
  `refused` or `reverse` decision, of a decision it can't find, and of a decision the attempting
  session can already read as reversed.
- **R19.** When two or more reversals of one decision exist, every reader settles on the same one
  by a fixed rule, and reads every other as refused. The losing reverser's own process, when it
  learns it lost, records its refusal in its own session; no reader writes the refusal anywhere
  else. Nothing is written in the original decider's session.
- **R20.** A reversed decision reads as `reversed` in every listing, with the reversal's session
  and identifier. The original is never edited or removed. Restoring what was reversed takes a new
  decision of the original kind.
- **R21.** What reversing each kind means:
  - `accept` (`accepted`): the subject is no longer accepted. The reversal and the reversed
    decision both list every decision whose `basis` names it, in sessions the reader can reach.
    koto doesn't edit the document's status.
  - `accept` (`rejected`): the rejection no longer stands; the subject is neither accepted nor
    rejected until a new `accept` says so.
  - `merge`: the merge is no longer endorsed. The reversal carries a link to the compensating
    change, or records that none exists yet. koto doesn't change the forge.
  - `gate-pass`: the pass no longer stands (R22 says what that does to a live workflow).
  - `escalate`: the question is withdrawn from its recipient.
  - `decline-escalation`: the question counts as escalated to the reverser.
  - `judgment` and `verdict`: the call no longer stands. The reversal may carry the outcome that
    replaces it and, for a verdict that let a release through, a link to the compensating action,
    or record that none exists yet.

**A live decider and a reversal**

- **R22.** While the deciding session's workflow is live, every `koto next` on it reads the
  reversals aimed at its decisions from the sessions up its chain. When one of its decisions is
  reversed, `koto next` refuses to advance, leaves the state unchanged, and reports a blocking
  condition naming the reversal and its decider. The block clears only when the workflow is rewound
  to or before the state the decision was made in, or a new decision on the same subject names the
  reversal in its `basis`. For a `gate-pass`, that new pass must come from the reverser or a decider
  with standing over the reverser, and a pass whose standing is `asserted` doesn't clear a
  `verified` reversal.
- **R23.** Reading reversals at `koto next` writes nothing in any session above the decider.
  Reads fail closed: when a session on the chain exists but its reversals can't be read, `koto
  next` blocks with a condition naming that session instead of assuming nothing was reversed, and
  a listing reports a decision whose keys can't be parsed as unreadable, never as in effect. A
  session that no longer exists ends the chain as `incomplete` and blocks nothing.

**Every gate pass has a decider**

- **R24.** `koto overrides record`, `koto next --to` past a gate that hasn't passed, and `koto
  decider answer` with an answer that lets the check pass each record a `gate-pass` decision. A
  decider the caller names is used and labelled `handed`. When the caller names none, koto uses the
  session it observes and marks the decider `defaulted`. When neither exists, koto refuses the pass,
  names the missing decider, leaves the workflow where it was, writes none of the records the
  command writes today, and records a `refused` decision. Otherwise the command's existing records
  and its decision are written together or not at all.
- **R25.** Where a team declares who may decide at a gate, a pass whose decider is none of them is
  refused and recorded as `refused`. Until such declarations exist, R24 alone applies.

**Existing records**

- **R26.** koto's existing records use this vocabulary:

  | Existing record | Becomes |
  |---|---|
  | `koto overrides record` | a `gate-pass` decision, written by the command |
  | `koto next --to` past an unmet gate | a `gate-pass` decision |
  | `koto decider answer` | `gate-pass` when the answer lets the check pass, `judgment` otherwise |
  | `koto decisions record <name> --with-data` | a `judgment` decision |
  | `koto disputes record --by agent\|person` | a `judgment` about the finding, its decider in R7's kinds |
  | The grader's consultations and checks | stay telemetry in the session log; decisions cite them as evidence |

  Each command keeps writing what it writes today, with the arguments it accepts today.
- **R27.** The decider ledger's gate answers are recorded in the deciding session, and `koto
  decider report` reads them from sessions. koto stops writing the ledger file and stops reading it.
- **R28.** shirabe's decision points record through this vocabulary: a document's acceptance
  through its status transition writes an `accept`, its merge step a `merge`, a review panel's
  pass or fail a `verdict` attributed to the reviewing seats, a finding's dismissal a `judgment`,
  and its coordinator's escalations, settlements and reversals the matching kinds. Its own views of
  those decisions render from the record.
- **R29.** koto imports decisions recorded before the record existed, such as a coordinator's
  interim entries, into the importing session. An imported decision keeps its recorded decider and
  authority labelled `handed` and a pointer to its source entry, and importing the same entry
  twice adds it once.

**Reports and evidence**

- **R30.** koto reports, for a UTC time window that includes its start and excludes its end,
  counting every kind except `reverse` and `refused`: the decisions made in the window and how many
  are reversed as of the report, as a table of kind by decider kind; the reversals recorded in the
  window, by the reversed decision's kind and decider kind; each reversed decision with its
  reversal; and the count of `gate-pass` decisions with a defaulted decider. It reads them from the
  sessions koto keeps, says how many sessions it read, and reads no host file.
- **R31.** A decision can carry pointers to what the harness records about the same moment (its
  session identifier, a tool-call identifier, an agent identifier) and to a koto session event. It
  stores the pointers and never copies what they point at.

**Tool independence**

- **R32.** Recording, reading, reversing and reporting work with koto alone, with no workspace
  manager installed. A workspace manager may set up koto's hooks and pass koto's variables through
  its configuration; nothing here makes it run koto, read or write workflow state, or fail without
  koto.

### Non-functional

- **R33.** Decisions are immutable once written. Concurrent writers in different sessions never
  lose or corrupt each other's decisions, and two writers in one session never produce two
  decisions under one identifier.
- **R34.** Readers of koto's existing events, run journal and feed keep working unchanged, and
  every existing command keeps the arguments it accepts today. The one deliberate exception is
  R24's refusal of a gate pass whose decider can't be established; tests that pass gates with no
  harness session identifier set must set one or name a decider.
- **R35.** A decision's reason stays in the session and isn't copied into exported telemetry,
  matching the run journal's rule for override rationales.
- **R36.** Under the local backend, recording or reading a decision makes no network call.
  Nothing in it depends on the workspace manager or the harness being reachable; a missing value is
  recorded per R7, R9 and R12 rather than failing the decision.
- **R37.** On the local backend, the reversal read R22 adds to `koto next` costs under 50 ms at a
  chain depth of five with 1,000 decisions in each session, and touches only the sessions on the
  deciding session's own chain.
- **R38.** On a store of 1,000 retained sessions holding 100,000 decisions, a one-month listing of
  every session below a root and the R30 report each finish in under five seconds on the project's
  CI runner.

## Acceptance Criteria

**Reading an unattended unit after it finished**

- [ ] A worker dispatched by a live coordinator runs unattended, accepts a document, passes a gate
  with `koto overrides record` and records a merge, then reaches a success terminal. From the
  coordinator's session, listing the worker's decisions returns three, each with its kind, subject
  per R2, decider kind `session` naming the worker and an agent value, authority per R11, chain per
  R12, identifier, time and outcome.
- [ ] In the same run, every one of the three decisions exists as keys in the worker's session,
  and a scan of `~/.koto/` outside `sessions/` finds no file created or changed by the run that
  holds a decision, a reversal or a gate answer; `~/.koto/_decider_ledger.jsonl` is neither
  created nor modified, and deleting it before the listing changes nothing in its output.
- [ ] `koto decider report` over that run returns the gate answer from the worker's session with
  the ledger file absent.

**A person deciding**

- [ ] The same unit with a maintainer accepting the document at the terminal records an `accept`
  with decider kind `person`, the maintainer's handle labelled `handed`, surface `terminal`, and
  the entering session, and koto's stored decision has no account, login or credential field.
- [ ] A relayed acceptance with a forge review link is recorded as the person's with the link
  stored; the same relay without a link is recorded with the relaying session as decider and the
  person as a handed note.

**Reversing from above**

- [ ] From the coordinator's session, reversing the acceptance, the gate pass and the merge
  records three `reverse` decisions in the coordinator's session, each with decider kind `session`
  naming the coordinator. Listing the worker's decisions then shows each original as `reversed` with
  its reversal's session and identifier, and the worker's session keys and log are byte-identical
  to before the reversals.
- [ ] Reversing the merge with a revert pull request link stores the link; without one, it records
  that no compensating change exists.
- [ ] Reversing an acceptance that a later acceptance names in its `basis` lists the later one as
  affected, both on the reversal and on the reversed acceptance.

**Escalation and judgment**

- [ ] A worker records an `escalate` to its coordinator, a `decline-escalation` and a `judgment`.
  From the coordinator, listing returns all three with their deciders and the escalation's
  recipient. The coordinator reverses the judgment and the listing shows the reversal named with the
  coordinator as decider.
- [ ] Reversing the escalation lists the question as withdrawn from the coordinator; reversing the
  decline lists the question as escalated to the reverser.

**Gate passes**

- [ ] `koto overrides record`, `koto next --to` past an unmet gate, and `koto decider answer` with a
  passing answer, each run with no decider named and a harness session identifier set, each record a
  `gate-pass` whose decider is the observed session marked `defaulted`. Listing filtered on
  `defaulted` returns exactly those three; a pass recorded with `--by person:<handle>` isn't among
  them.
- [ ] The same three commands run with the harness session identifier unset and no decider named
  are each refused with an error naming the missing decider; the workflow's state and existing
  records are unchanged and a `refused` decision is listed in the session.

**Three levels, a finished middle, a live grandchild**

- [ ] A dispatches B and B dispatches C, each binding through koto's dispatch link. C records a
  `judgment` and keeps running; B reaches a success terminal. From A, listing below A returns C's
  judgment, and reversing it records the reversal in A's session. C's next `koto next` refuses to
  advance, leaves C's state unchanged and reports a blocking condition naming A's reversal.
- [ ] In that run, B's and C's session keys and logs are byte-identical before and after A's reads
  and A's reversal, and A's and B's are byte-identical before and after C's `koto next` read the
  reversal.
- [ ] C rewound to the state where the judgment was made advances again; in a second run, a new
  `judgment` from C naming the reversal in its `basis` clears the block instead.
- [ ] With a gate pass in C reversed by A, a new pass by C alone doesn't clear the block; a new
  pass recorded by A does.
- [ ] With B's reversal keys made unreadable (a key holding malformed JSON) while B still exists,
  C's `koto next` blocks naming B; with B removed instead, C advances and its decisions list their
  chain as `incomplete`.

**Racing reversals**

- [ ] With A above B above C, A and B each reverse the same decision of C's, started at once. Each
  reversal is recorded in its reverser's own session. Listing from A, from B and from C each names
  the same reversal as in effect and the other as refused, and C's session keys and log are
  byte-identical to before the reversals.
- [ ] After learning it lost, the losing reverser's next decision command records a `refused`
  decision in its own session, and no other session gains a key.

**Standing**

- [ ] A worker's attempt to reverse its coordinator's decision, with complete chains, is refused
  and recorded as `refused` in the worker's session.
- [ ] A coordinator's attempt to reverse a decision whose decider is a person is refused; a person
  entering through the coordinator's session reverses it.
- [ ] A reversal through a chain whose middle session was removed is recorded with standing
  `asserted`; one through a complete koto-written chain is `verified`.
- [ ] Reversing a `reverse`, a `refused`, an unknown identifier, and an already reversed decision
  are each refused and recorded.
- [ ] A worker reverses its own `judgment` and the reversal is accepted.

**Write-once**

- [ ] `koto context add` and `koto context remove` against an existing decision key, from the
  deciding session and from another session, are refused with an error naming the key, and the key
  is unchanged. A template that assigns a decision key fails to compile.

**Import**

- [ ] With the owner's per-command cloud configuration and one host, a coordinator above a finished
  worker is moved to another workspace with `koto session import`. From the imported session,
  listing below it returns the worker's decisions, and reversing one records the reversal in the
  imported session; the worker's listing then shows it `reversed`. A decision recorded against the
  migrated source is refused as `session_migrated`.

**Existing records and shirabe**

- [ ] `koto overrides list`, `koto decisions list` and `koto disputes record --by agent|person`
  return what they return today, and each corresponding command also writes its decision per R26.
- [ ] A shirabe status transition to Accepted writes an `accept`, a shirabe merge step writes a
  `merge`, and a review panel's verdict writes a `verdict` naming the reviewing seats, each in the
  session that ran the step.
- [ ] Importing a coordinator's interim entries keeps each decider and authority labelled `handed`
  with a source pointer, and importing them again adds nothing.

**Reporting**

- [ ] For a fixture of sessions spanning two months, the report for the second month counts the
  decisions made and reversed in a kind by decider-kind table matching the fixture, counts a
  second-month reversal of a first-month decision under the window's reversals, counts defaulted
  gate passes, and states how many sessions it read, with no host file present.

**Independence and the rest**

- [ ] Every scenario above that involves no workspace manager passes with no workspace manager
  installed, and a session dispatched by one with koto absent from its path dispatches normally.
- [ ] The existing suites for the run journal, the feed and the grader report pass unchanged, and
  the functional tests for the three gate-passing commands pass with at most a harness session
  identifier added to their environment.
- [ ] Exporting telemetry for a run whose decisions carry reasons produces no record containing
  reason text.
- [ ] The R37 and R38 timings hold on the CI runner.

## Out of Scope

- **On whose behalf a decision was made.** Naming the person a session acted for is a later
  extension; the record leaves room for it.
- **Rules about who may decide.** Requiring a person at a point, excluding a document's author, and
  the declarations behind R25 are policy layers that read this record. This record says who decided
  and decides only who may reverse.
- **Undoing effects outside koto.** koto doesn't revert merges, reopen pull requests or edit
  documents when a decision is reversed.
- **Session retention, dispatch links and anchors.** They already shipped; this feature reads them
  and changes none of their rules. A person's explicit `koto session cleanup` still removes a
  session and its decisions.
- **Reading another host's sessions without an import.** Cross-host reads wait on the owner's
  instruction.
- **Workspace-manager changes.** The dispatch tree the workspace manager records stays its own; koto
  reads none of it.
- **Rebuilding shirabe's findings ledger or coordinator record.** R28 makes them write the record;
  their own structures stay.
- **Pruning decisions.** They go when their session goes.
- **Duplicating the harness's records.** Transcripts and tool permission decisions stay where the
  harness keeps them; R31 points at them.
- **Authenticating identities.** Identities are attribution, not authorization; the labels say how
  far each can be trusted.

## Known Limitations

- Overrule counts cover only sessions koto still keeps. A hierarchy whose sessions retention has
  removed no longer counts; a durable monthly reading is the reporting work's to keep.
- An `observed` session identifier can be forged by anything running as the operator. The record
  makes misattribution visible, not impossible.
- Until the harness says which agent is acting, many decisions record the agent as `unknown`.
- A reversal by a session whose chain to the decider passes through a session another host holds
  can't be judged from this host and records as `asserted`.
- R25 has no acceptance criterion here because nothing declares deciders yet.

## Decisions and Trade-offs

- **Decisions live in the deciding session, not a host store.** The owner's ruling. Session
  retention removed the reason the first build gave for a host store, and the cloud backend carries
  the keys when a session moves.
- **A reversal lives in the reverser's session.** A session's keys have one writer. Writing a
  reversal into the decider's session would be a cross-session write, which the cloud sync doesn't
  support, a migrated source refuses, and another host can't reach.
- **Races are settled when read.** With no shared store and no lock between sessions, a write-time
  check can't see a reversal another session is writing at the same moment. A fixed read rule gives
  every reader the same answer without either reverser writing into the other.
- **The first build's store-independent parts carry over.** The decision kinds, the decider and its
  labels, the standing rules, reversal as its own final decision, the blocked step after a reversed
  gate pass, and reads that fail closed. `verdict` gains `inconclusive` for a production verdict that
  is too early to call. The block now covers any reversed decision of a live workflow, not only a
  gate pass, because a worker that keeps going on a reversed call builds on it the same way.
- **Default the gate decider to the observed session.** The owner's settled rule: every pass names a
  decider, a defaulted one is marked and countable, and a pass is refused only when no decider can be
  established.
- **The chain comes from koto's own links.** The workspace manager's session map is a harness id
  deleted at reap and outside koto; koto's dispatched-by links survive an import and are written by
  koto.
- **Counts come from a scan of kept sessions.** The record has no host-wide index; a reporting tool
  that wants a longer memory keeps its own reading.
- **Reversal is final.** Reversing a reversal would make "in effect" depend on the parity of a chain;
  a new decision restores.
