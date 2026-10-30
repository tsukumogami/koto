---
schema: prd/v1
status: Accepted
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

Accepted

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
2. As a maintainer who accepts a design myself at my terminal, I want the record to name me by a
   handle and say which surface my decision came through, without koto holding an account for me,
   so a reader can tell my decision from an agent's.
3. As a coordinator that disagrees with a worker's acceptance, gate pass and merge, I want to
   reverse each from my own session and have every reader see my reversal, its decider, and that
   the original no longer stands.
4. As a maintainer working inside a coordinator's agent session, I want to reverse a worker's
   decision as myself, so the reversal names me rather than the coordinator.
5. As a coordinator, I want to see which questions a worker escalated, which it declined to
   escalate, and which judgment calls it settled itself, each with its decider, and to reverse any
   of them.
6. As a maintainer auditing gate passes, I want every pass to name its decider, defaulted ones
   marked as such, and a pass with no establishable decider refused.
7. As the person at the top of a three-level chain, I want to reverse a grandchild's decision
   after the middle session finished, and have the grandchild's live workflow stop at its next
   step, without my reads or writes changing anything in the sessions below me.
8. As one of two coordinators above the same worker, I want a reversal race to resolve the same
   way for every reader, with the losing reversal visibly refused.
9. As a person who moves a coordinating session to another workspace with `koto session
   import`, I want to keep reading and reversing the decisions below it from there.
10. As the author of a reporting tool, I want to count over a time window how many decisions of
    each kind were reversed, from the sessions koto keeps, with no host file to read.
11. As the maintainer of shirabe's coordinator and review panel, I want their decisions written to
    the shared record while `record/decisions`, `record/reversals` and the panel's verdict ledger
    keep returning what their readers get today.

## Requirements

### Terms

| Term | Meaning |
|---|---|
| deciding session | the koto session a decision's keys live in |
| agent session | one harness conversation; koto knows it by its anchor session, named in `KOTO_ANCHOR` |
| chain | the ordered sessions above a deciding session: first those reached by following koto-owned dispatched-by links upward from the deciding session, then its anchor (reached by its held-by link) and those reached upward from the anchor, skipping any session already listed |
| above | a session is above a decision when it, or its anchor, is on the decision's chain |
| established decider | a decider named with `--by`, or, with none named, a harness session identifier in `CLAUDE_CODE_SESSION_ID` or a readable anchor named in `KOTO_ANCHOR`; the koto session name alone never establishes one |
| status | each decision reads as exactly one of `in-effect`, `reversed`, `refused` or `unreadable`. A `reverse` reads `in-effect` when it wins R19's rule and `refused` when it loses; a `refused` decision always reads `refused`; any other decision reads `reversed` when a winning reversal names it, otherwise `in-effect` |
| decision keys | the keys holding decisions, under one koto-reserved key prefix the design names |

### Functional

**Decision kinds and subjects**

- **R1.** A decision has exactly one kind from this closed set, each with a closed set of
  outcomes:

  | Kind | Meaning | Outcomes |
  |---|---|---|
  | `accept` | accepting a document, a plan, one step of a plan, a test change, a staged draft, a request's intent, or a policy or configuration change | `accepted`, `rejected` |
  | `merge` | merging a pull request | `merged` |
  | `gate-pass` | letting a workflow past a gate it had not passed on its own: by override, by a directed transition past an unmet gate, or by an agent's `koto decider answer` of `pass` | `passed` |
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
  | `accept` of a document, plan, test change, or policy or configuration change | repository, path or paths, commit |
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

  A command given a kind, outcome or subject these tables don't allow fails with an error naming
  the field and records nothing.
- **R3.** Every decision carries an identifier unique within its session, fixed-width text whose
  byte order is the order the session recorded its decisions in; a UTC time to the millisecond;
  the workflow state its session was in when it was recorded, or empty for a session with no
  workflow state; its outcome; an optional reason; and an optional list of `basis` references
  (session name and identifier) naming earlier decisions it relies on. A decision is addressed
  from outside its session by the pair of session name and identifier. Every identifier
  comparison in this document is by bytes.

**Where a decision lives**

- **R4.** A decision is recorded as keys in the deciding session: the session whose agent or
  person decided, or, for a person's decision, the session that entered it. A `reverse` is
  recorded in the reverser's session when that session is on the original's chain, and otherwise
  in the reverser's anchor, so it always sits in a session on the chain. No decision, reversal, refusal or gate answer is written to
  or read from a file under `~/.koto/` outside the session directories.
- **R5.** Decision keys are write-once. Once a decision's keys exist, koto refuses every later
  write to or removal of them, by koto's own commands and by `koto context add` or `remove` from
  any process, with an error naming the key. Decision keys are written only by koto's decision
  commands; a template that assigns one fails to compile.
- **R6.** Under the cloud backend, decision keys push to the session's bucket prefix like any
  other key, and a second write of an existing decision key is refused there too. A session
  migrated by `koto session import` serves its decisions to readers and refuses new ones with
  `session_migrated`, as koto already does for its other writes.

**The decider**

- **R7.** Every decision names a decider of exactly one kind:
  - `session`: the koto session name; the harness session identifier when `CLAUDE_CODE_SESSION_ID`
    is set, else empty; the anchor session when `KOTO_ANCHOR` names a readable one, else empty;
    and the agent within it that decided, one of `main`, the identifier of an agent the main agent
    started, or `unknown`. koto takes the agent from `KOTO_AGENT_ID` (`observed`, set by koto's own
    harness hook from what the harness reports) or from `--agent` (`handed`), records `unknown`
    when neither gives one, and never fills in `main` on its own.
  - `person`: a handle the surface supplied, the surface (`terminal`, `forge`, `relay`), an
    optional link to the original, and the session that entered it.
  - `check`: the name of an automated check, such as koto's grader, and the session that ran it.
- **R8.** koto stores a person's handle as an opaque string. It has no user table, account,
  login, registration or credential for any decider, and never requires one to record or read a
  decision.
- **R9.** Each identity value carries a source label: `observed` when koto read it from its own
  environment or keys, `handed` when a caller supplied it. A person's handle is always `handed`.
  A `relay` decision is recorded as the person's only when the caller supplies a link that parses
  as an absolute URL with scheme `https`, a host, no user information and no query string;
  otherwise the decider is the entering session and the named person is kept as a handed note.
  koto stores links and never fetches them.
- **R10.** A decision records the run mode, `attended`, `unattended` or `unknown`: `observed`
  from `KOTO_RUN_MODE` when koto's harness hook sets it from what the harness reports, `handed`
  from `--run-mode`, `unknown` when neither gives one. The run mode is recorded separately from
  the decider.

**Authority and the chain**

- **R11.** Every decision records the authority its decider held, as these fields:
  - `kind`: `dispatch` for a session or check decider, `person` for a person decider;
  - `level` and `grants`: copied, labelled `handed`, from the authority the deciding agent
    session's anchor recorded when it opened; `null` and an empty list when it recorded none;
  - `dispatcher`: the session named by the deciding session's nearest dispatched-by link, or
    `none`;
  - `run_mode`: per R10.
- **R12.** Every decision records its chain as a snapshot taken when it was recorded: the ordered
  sessions in the order the Terms define, each with how koto wrote the link that reached it: `leg`
  (at a request-leg bind, from the request's requester), `init` (at `koto init --dispatched-by`),
  `link` (by `koto session link`), `anchor` (into an anchor when it opened, from its dispatcher),
  `held-by` (the hop from the deciding session to its anchor), or `import` (rewritten at an
  import). A chain whose
  next session can't be found is marked `incomplete`; missing links are never guessed and never
  count as a root.
- **R13.** koto reads no workspace-manager file and calls no workspace-manager command to build
  the decider, authority or chain. Values a workspace manager passes reach koto only through
  koto's own variables and links.

**Reading from above**

- **R14.** koto lists a session's decisions as JSON carrying a format version, each with its
  status (Terms), ordered by time, then identifier. A `--received` listing returns instead the
  decisions other sessions made about it: gate passes on its workflow, escalations addressed to it,
  and reversals of its decisions. Listings filter by kind, decider kind, subject field, time
  window, run mode, and whether the decider was defaulted; filters combine with AND.
- **R15.** koto lists the decisions of every session below a given session, through any number of
  finished sessions in between, for as long as retention keeps them.
- **R16.** Reading a session's decisions from another session changes no key and appends no event
  in the read session or in any session between them. The read may be recorded in the reader's own
  anchor.

**Reversal**

- **R17.** A `reverse` is accepted when its decider is:
  - the same decider as the original: the same deciding session for a session decider, the same
    check name run in the same session for a check, or any person entering through the same
    session for a person;
  - for an original decided by a session or a check, a session above the original, or a person
    (any handle) entering through such a session;
  - for an original decided by a person, a person (any handle) entering through a session above
    it. A session alone can't reverse a person's decision.

  A reversal records the chain path that gave it standing and its standing: `verified` when that
  path is complete. When no rule matches and the original's chain is `incomplete`, the reversal is
  accepted with standing `asserted` unless the original's deciding session is on the reverser's
  own chain (a session below the decider never asserts over it). In every other unmatched case
  koto refuses and records a `refused` decision in the would-be reverser's session.
- **R18.** koto refuses, and records as `refused` in the attempting session, a reversal of a
  `refused` or `reverse` decision, of a decision it can't find, and of a decision already
  `reversed` in what the attempting session can read.
- **R19.** When two or more reversals of one decision have standing, the winner is the one with
  the earliest time; equal times go to the reverser session whose name sorts first by bytes, then
  to the identifier that sorts first. Every reader applies this rule and reads every other reversal of the
  decision as `refused`. Right after `koto decisions reverse` writes a reversal, it reads the
  competing reversals it can reach; if it lost, it records a `refused` decision naming the winner
  in its own session and exits non-zero. Each later decision command in that session repeats the
  check for reversals it recorded. Nothing is written in the original decider's session.
- **R20.** A reversed decision reads as `reversed` in every listing, with the winning reversal's
  session and identifier. The original is never edited or removed. Restoring what was reversed
  takes a new decision of the original kind.
- **R21.** What reversing each kind means:
  - `accept` (`accepted`): the subject is no longer accepted. When a listing shows the reversal
    or the reversed decision, it computes, at read time, every decision whose `basis` names the
    reversed one among the lister's own session and the sessions R15 reaches below it, and shows
    them as affected. koto doesn't edit the document's status.
  - `accept` (`rejected`): the rejection no longer stands; the subject is neither accepted nor
    rejected until a new `accept` says so.
  - `merge`: the merge is no longer endorsed. The reversal carries a link to the compensating
    change, or records that none exists yet. koto doesn't change the forge.
  - `gate-pass`: the pass no longer stands, with R22's effect on a live workflow.
  - `escalate`: the question is withdrawn from its recipient. An answer the recipient already
    recorded stays on the record as given.
  - `decline-escalation`: the question counts as escalated to the reverser.
  - `judgment` and `verdict`: the call no longer stands. The reversal may carry the outcome that
    replaces it and, for a verdict that let a release through, a link to the compensating action,
    or record that none exists yet.

**A live decider and a reversal**

- **R22.** While the deciding session's workflow is not terminal, every `koto next` on it reads the
  reversals aimed at its decisions from the sessions on its chain. When one of its decisions reads
  `reversed`, `koto next` refuses to advance, leaves the state unchanged, and reports a blocking
  condition naming the reversal and its decider. The block clears when either:
  - `koto rewind` takes the workflow to the state the decision recorded (R3) or an earlier one;
    or
  - a new decision of the same kind names the reversal in its `basis`, recorded in a session on
    the deciding session's chain by a decider with standing over the reversal by R17's rules (the
    reverser, or a session or person above the reverser). For a `gate-pass` it must be for the same
    workflow, state and gate, and a pass whose standing is `asserted` doesn't clear a `verified`
    reversal. The deciding session can't clear its own block this way; its route is the rewind.

  A reversal that lost R19's race blocks nothing.
- **R23.** Reading reversals at `koto next` writes nothing in any session on the chain. Reads fail
  closed. A chain session is unreadable when its directory exists but a decision or reversal key in
  it fails to parse, its keys can't be read for a reason other than absence, or, under the cloud
  backend, its listing fails with an error other than not-found; then `koto next` blocks with a
  condition naming that session, and a listing shows every decision whose chain includes that
  session as `unreadable`, never `in-effect`, along with any decision in it whose key fails to
  parse. A session with no directory and no bucket object ends the chain as `incomplete` and
  blocks nothing; a migrated session resolves to the session it moved to.

**Every gate pass has a decider**

- **R24.** `koto overrides record`, `koto next --to` past a gate that hasn't passed, and `koto
  decider answer` with an answer of `pass` each record a `gate-pass` decision. A decider named with
  `--by` is used and labelled `handed`. With none named and an established decider (Terms), koto
  records the session decider per R7 and marks it `defaulted`. With no established decider, koto
  refuses the pass with an error naming the missing decider, leaves the workflow where it was,
  writes none of the records the command writes today, and records a `refused` decision whose
  decider is the koto session with empty harness and anchor fields, agent `unknown`, marked
  `unestablished`. When the command returns, or after the next koto command on that session
  following a crash between its writes, the command's existing records and its decision both exist
  or neither does.

**Existing records**

- **R25.** koto's existing records use this vocabulary:

  | Existing record | Becomes |
  |---|---|
  | `koto overrides record` | a `gate-pass` decision, written by the command |
  | `koto next --to` past an unmet gate | a `gate-pass` decision |
  | `koto decider answer` | `gate-pass` for `pass`, `judgment` for any other answer |
  | `koto decisions record <name> --with-data` | a `judgment` decision |
  | `koto disputes record --by agent\|person` | a `judgment` about the finding, its decider in R7's kinds |
  | The grader's consultations and checks | stay telemetry in the session log; decisions cite them as evidence |

  Each command keeps writing what it writes today, with the arguments it accepts today.
- **R26.** The decider ledger's records are kept in the deciding session, and `koto decider
  report` reads them from sessions. No koto code path writes or reads
  `~/.koto/_decider_ledger.jsonl`.
- **R27.** shirabe's decision points record through this vocabulary, in the session that ran the
  step:

  | shirabe step | Decision |
  |---|---|
  | `shirabe transition <doc> Accepted` run under a koto session | `accept` of the document |
  | the execute and coordinate merge step | `merge` |
  | the work-on review panel's pass or fail | `verdict`, decider the panel's session with each seat as the agent |
  | a seat dismissing its own finding | `judgment` |
  | the coordinator settling, escalating or holding a question | `judgment`, `escalate`, `decline-escalation` |
  | the coordinator recording a reversal row | `reverse` |

  `record/decisions`, `record/reversals` and the panel's verdict ledger keep every field they have
  today; each row they gain carries one added field citing the decision's session and identifier,
  and every existing reader of those keys passes its tests unchanged.
- **R28.** `koto decisions import <file>` reads JSON Lines, one entry per line with fields
  `source`, `source_id`, `time`, `kind`, `outcome`, `subject`, `decider`, `authority` and an
  optional `reason`, and records each as a decision in the importing session. An imported decision
  keeps its source `time`, gains the time it was imported, keeps its decider and authority labelled
  `handed`, and keeps its `source` and `source_id`. Any kind but `refused` may be imported; an
  imported `reverse` names its original by `source` and `source_id` and has standing `asserted`.
  The pair `source` and `source_id` identifies an entry: importing it again adds nothing. A line
  that fails to parse or breaks R1 or R2 stops the import with its line number, and the import then
  records nothing.

**Reports and evidence**

- **R29.** `koto decisions report` takes a UTC window that includes its start and excludes its
  end, and optionally `--below <session>` to limit it to one hierarchy; otherwise it reads every
  session in the store. Counting every kind except `reverse` and `refused`, it reports: the
  decisions made in the window and how many read `reversed` when the report runs, as a table of
  kind by decider kind; the reversals recorded in the window, by the reversed decision's kind and
  decider kind; each reversed decision with its reversal; the count of defaulted `gate-pass`
  decisions; and how many sessions it read. It reads no file outside the session directories.
- **R30.** A decision can carry pointers to what the harness records about the same moment (its
  session identifier, a tool-call identifier, an agent identifier) and to a koto session event. It
  stores the pointers and never copies or fetches what they point at.

**Tool independence**

- **R31.** Recording, reading, reversing and reporting work with koto alone, with no workspace
  manager installed. A workspace manager may set up koto's hooks and pass koto's variables through
  its configuration; nothing here makes it run koto, read or write workflow state, or fail without
  koto.

### Non-functional

- **R32.** Decisions are immutable once written. Two sessions each recording 100 decisions at once
  end with 200 decisions and no earlier decision changed; two processes recording 50 decisions each
  into one session end with 100 decisions under 100 distinct identifiers.
- **R33.** The existing test suites for the run journal, the feed, the grader report and the
  commands in R25 pass without edits, apart from adding a harness session identifier or a `--by`
  to tests that pass gates with neither (the one deliberate break, R24).
- **R34.** A decision's reason stays in the session and isn't copied into exported telemetry,
  matching the run journal's rule for override rationales.
- **R35.** Under the local backend, recording, listing and reversing make no network connection,
  and none depends on the workspace manager or harness being reachable; a missing value is
  recorded per R7, R9 and R12.
- **R36.** Measured as the median of five warm runs on the CI runner koto's workflows use: the
  reversal read R22 adds to `koto next` costs under 50 ms on the local backend at a chain depth of
  five with 1,000 decisions in each session, and touches only the sessions on the deciding
  session's own chain; over a store of 1,000 sessions in one hierarchy four levels deep, holding
  100 decisions each (100,000 in all), a listing below the root and the R29 report each finish in
  under five seconds.

## Acceptance Criteria

**Reading an unattended unit after it finished**

- [ ] A worker dispatched by a live coordinator through a request leg runs unattended with
  `CLAUDE_CODE_SESSION_ID` and `KOTO_ANCHOR` set and no acting agent reported. It accepts a
  document, passes a gate with `koto overrides record` and records a merge, then reaches a success
  terminal. With the coordinator's workflow still not terminal and the worker's session finished
  and kept by retention, listing from the coordinator's session the worker's decisions returns three, each with
  decider kind `session` naming the worker session and its harness identifier labelled `observed`,
  agent `unknown`, run mode `unattended`, authority `kind` `dispatch` with `dispatcher` the
  coordinator, and a chain whose first link is the coordinator labelled `leg`.
- [ ] In the same run, each of the three decisions exists as keys in the worker's session, and no
  file under `~/.koto/` outside `sessions/` is created or changed holding a decision, a reversal or
  a gate answer. With `~/.koto/_decider_ledger.jsonl` replaced by a directory before the run, every
  step succeeds, and `koto decider report` returns the gate answer from the worker's session; with
  the file instead holding a stale answer for another session, the report omits it.

**A person deciding**

- [ ] The same unit with a maintainer accepting the document at the terminal records an `accept`
  with decider kind `person`, the handle labelled `handed`, surface `terminal`, and the entering
  session; recorded with `--run-mode attended`, it lists run mode `attended` labelled `handed`;
  and the stored decision has no account, login or credential field.
- [ ] A relayed acceptance with link `https://forge.example/pr/1#review-2` is recorded as the
  person's with the link stored. The same relay with no link, with an `http` link, with
  `https://user:pw@forge.example/x`, and with `https://forge.example/x?token=1` is each recorded
  with the relaying session as decider and the person as a handed note.

**Reversing from above**

- [ ] From the coordinator's session, reversing the acceptance, the gate pass and the merge
  records three `reverse` decisions in the coordinator's session with decider kind `session` naming
  the coordinator and standing `verified`. Listing the worker's decisions then shows each original
  `reversed` with its reversal's session and identifier, and the worker's session keys and log are
  byte-identical to before the reversals.
- [ ] Reversing the merge with a revert pull request link stores the link; without one, it records
  that no compensating change exists.
- [ ] Reversing an acceptance that a later acceptance names in its `basis` lists the later one as
  affected, both on the reversal and on the reversed acceptance.
- [ ] Reversing an `accept` with outcome `rejected` lists the subject with no `in-effect` accept.
  Reversing a `verdict` with a replacement outcome `fail` and a rollback link stores both.
- [ ] A maintainer working in the coordinator's agent session reverses a worker's judgment with
  `--by person:<handle>`; the reversal names the person with the coordinator's session as entering
  session.

**Escalation and judgment**

- [ ] A worker records an `escalate` to its coordinator, a `decline-escalation` and a `judgment`.
  From the coordinator, listing returns all three with their deciders and the escalation's
  recipient, and the coordinator's `--received` listing returns the escalation. The coordinator
  reverses the judgment and the listing shows the reversal with the coordinator as decider.
- [ ] Reversing the escalation lists the question as withdrawn from the coordinator; reversing the
  decline lists the question as escalated to the reverser.

**Gate passes**

- [ ] `koto overrides record`, `koto next --to` past an unmet gate, and `koto decider answer` with
  `pass`, each run with no `--by` and `CLAUDE_CODE_SESSION_ID` set, each record a `gate-pass` whose
  decider is the session marked `defaulted`. Listing filtered on `defaulted` returns exactly those
  three; a pass recorded with `--by person:<handle>` isn't among them. `koto decider answer` with
  `fail` records a `judgment`.
- [ ] The same three commands run with `CLAUDE_CODE_SESSION_ID` and `KOTO_ANCHOR` unset and no
  `--by` are each refused with an error naming the missing decider; the workflow's state and
  existing records are unchanged, and a `refused` decision is listed whose decider is marked
  `unestablished` with agent `unknown`.
- [ ] For each of the three commands, a fault point compiled into koto's test builds stops the
  process after the command's existing record is written and before its decision key; the next
  `koto next` on that session finds both present.

**Three levels, a finished middle, a live grandchild**

- [ ] A dispatches B and B dispatches C, each through a request leg. C records a `judgment` and
  keeps running; B reaches a success terminal. From A, listing below A returns C's judgment, and
  reversing it records the reversal in A's session with standing `verified`. C's next `koto next`
  refuses to advance, leaves C's state unchanged and reports a blocking condition naming A's
  reversal.
- [ ] In that run, B's and C's session keys and logs are byte-identical before and after A's reads
  and A's reversal, and A's and B's are byte-identical before and after C's `koto next` read the
  reversal.
- [ ] `koto rewind` of C to the state the judgment recorded lets C advance. In a second run, a new
  `judgment` from C naming the reversal in its `basis` leaves C blocked, and a new `judgment`
  recorded by A naming the reversal clears the block.
- [ ] A person working in an agent session whose anchor, not whose koto session, is on C's chain
  reverses C's judgment: the reversal is recorded in that anchor, C's next `koto next` blocks on
  it, and listings from A, B and C all show the judgment `reversed`.
- [ ] With a gate pass in C reversed by A (`verified`), a new pass by C alone doesn't clear the
  block, a new pass recorded through a chain marked `incomplete` (`asserted`) doesn't either, and a
  new pass recorded by A does.
- [ ] With a reversal key in B holding malformed JSON while B still exists, C's `koto next` blocks
  naming B, and a listing of C's decisions shows the affected one `unreadable`. With B removed
  instead, C advances and its new decisions record their chain as `incomplete`.

**Racing reversals**

- [ ] With A above B above C, a fixture writes A's and B's reversals of the same decision of C's
  with known times, in both write orders. When the times differ, every listing from A, B and C names
  the earlier one as the winning reversal and the other `refused`; when the times are equal, every
  listing names the reversal from the session whose name sorts first. C's session keys and log are
  byte-identical to before the reversals, and a lost reversal doesn't block C.
- [ ] When `koto decisions reverse` finds it lost, it exits non-zero and its own session holds a
  `refused` decision naming the winner; no other session gains a key.

**Standing**

- [ ] A worker's attempt to reverse its coordinator's decision, with complete chains, is refused
  and recorded as `refused` in the worker's session.
- [ ] A coordinator's attempt to reverse a decision whose decider is a person is refused; a person
  entering through the coordinator's session reverses it.
- [ ] A reversal by a session not on the original's chain, where the original's chain is
  `incomplete`, is recorded with standing `asserted`.
- [ ] Reversing a `reverse`, a `refused`, an unknown identifier, and an already reversed decision
  are each refused and recorded.
- [ ] A worker reverses its own `judgment` and the reversal is accepted.

**Write-once and concurrency**

- [ ] `koto context add` and `koto context remove` against an existing decision key, from the
  deciding session and from another session, are refused with an error naming the key, and the key
  is unchanged. A template that assigns a decision key fails to compile.
- [ ] R32's two concurrency runs, each started from a shared barrier so the writers overlap, end
  with the stated counts and distinct identifiers.

**Cloud and import**

- [ ] With the owner's per-command cloud configuration, a recorded decision's key appears under the
  session's bucket prefix, and a second process's write of the same key is refused.
- [ ] With that configuration and one host, a coordinator above a finished worker is moved to
  another workspace with `koto session import`. From the imported session, listing below it returns
  the worker's decisions, and reversing one records the reversal in the imported session with
  standing `verified`; the worker's listing then shows it `reversed`. A decision recorded against
  the migrated source is refused with `session_migrated`.

**Listing**

- [ ] For a fixture of known decisions in one session, each filter in R14 (kind, decider kind,
  subject field, time window, run mode, defaulted) and one combination of two return exactly the
  matching decisions, oldest first with identifier breaking ties.
- [ ] A decision recorded with a harness tool-call identifier lists that identifier, and the
  stored decision holds no harness field beyond R30's three pointers.
- [ ] Listing output carries a format version, and the `--received` listing of a session returns a
  gate pass another session recorded on its workflow and a reversal of one of its decisions.
- [ ] A `verdict` recorded with decider kind `check` naming the grader and a pointer to its session
  check event lists that decider, the session that ran it, and the pointer.

**Existing records and shirabe**

- [ ] `koto overrides list`, `koto decisions list` and `koto disputes record --by agent|person`
  return what they return today, and each corresponding command also writes its decision per R25.
- [ ] Each shirabe step in R27's table, run in a fixture, writes its decision in the session that
  ran it, and `record/decisions`, `record/reversals` and the verdict ledger keep their existing
  fields, each new row naming the decision's session and identifier.
- [ ] Importing a JSON Lines file of three interim entries records three decisions with deciders and
  authority labelled `handed` and their source fields; importing it again adds nothing.

**Reporting**

- [ ] For a fixture of sessions spanning two months, the report for the second month counts the
  decisions made and reversed in a kind by decider-kind table matching the fixture, counts a
  second-month reversal of a first-month decision under the window's reversals, counts defaulted
  gate passes, and states how many sessions it read; `--below` a root counts only that hierarchy.
  A decision timed exactly at the window's start is counted and one timed exactly at its end is
  not, and the fixture's `reverse` and `refused` decisions appear in no decisions-made count.

**Field-level checks**

- [ ] Recording with an unknown kind, an outcome its kind doesn't allow, and a subject missing a
  field R2 requires each fails naming the field, and the session gains no key.
- [ ] Two decisions recorded in one run have distinct identifiers of equal length whose byte order
  matches recording order, and each lists the workflow state it was recorded in.
- [ ] With `KOTO_AGENT_ID=main` a decision lists agent `main` labelled `observed`; with
  `--agent sub-1` and no variable it lists `sub-1` labelled `handed`; with neither it lists
  `unknown`. With neither `KOTO_RUN_MODE` nor `--run-mode` the run mode lists `unknown`.
- [ ] With the deciding agent session's anchor opened with an authority of level `L2` and a grant
  `merge-pr`, a decision lists `level` `L2` and `grants` `[merge-pr]` labelled `handed`; with no
  authority recorded it lists `null` and an empty list.
- [ ] Sessions linked by `koto init --dispatched-by`, by `koto session link`, and through an anchor
  opened with a dispatcher list chain links labelled `init`, `link`, `anchor` and `held-by`; after
  the import scenario, the rewritten link lists `import`.
- [ ] A grader `check` reverses its own earlier verdict in the same session; a person entering
  through the session where another person decided reverses that decision.
- [ ] After a reversal loses R19's race without its command noticing (written by a fault point
  before the competing one appears), the losing session's next decision command records the
  `refused` decision.
- [ ] After an acceptance is reversed, a new `accept` of the same subject lists `in-effect` while
  the original still lists `reversed`. A reversed `judgment` whose reversal carries a replacement
  outcome lists that outcome on the reversal.
- [ ] Under the cloud backend, with a chain session's bucket listing made to fail with a permission
  error, C's `koto next` blocks naming it; with a chain session migrated by import, C reads
  reversals from the session it moved to.
- [ ] With a session that is not on C's chain made unreadable, C's `koto next` still advances.

**Independence and the rest**

- [ ] Every scenario above runs with no workspace manager installed. With a workspace-manager
  session file planted at `.niwa/sessions/<id>.json` naming a different parent, recorded chains
  are unchanged.
- [ ] With network access blocked and the local backend, recording, listing and reversing succeed.
- [ ] The suites R33 names pass with only the edits it allows.
- [ ] Exporting telemetry for a run whose decisions carry reasons produces no record containing
  reason text.
- [ ] A benchmark job on the CI runner measures R36's three timings over the stated fixtures and
  fails when a median exceeds its limit.

## Out of Scope

- **On whose behalf a decision was made.** Naming the person a session acted for is a later
  extension; the record leaves room for it.
- **Rules about who may decide.** Requiring a person at a point, excluding a document's author, and
  refusing a gate pass by anyone other than the deciders a team declares for that gate are policy
  layers that read this record. They wait on a way to declare deciders, which doesn't exist yet.
  This record says who decided and decides only who may reverse.
- **Undoing effects outside koto.** koto doesn't revert merges, reopen pull requests or edit
  documents when a decision is reversed.
- **Session retention, dispatch links and anchors.** They already shipped; this feature reads them
  and changes none of their rules. A person's explicit `koto session cleanup` still removes a
  session and its decisions.
- **Reading another host's sessions without an import.** Cross-host reads wait on the owner's
  instruction.
- **Workspace-manager changes.** The dispatch tree the workspace manager records stays its own; koto
  reads none of it.
- **Rebuilding shirabe's findings ledger or coordinator record.** R27 makes them write the record;
  their own structures stay.
- **Pruning decisions.** They go when their session goes.
- **Duplicating the harness's records.** Transcripts and tool permission decisions stay where the
  harness keeps them; R30 points at them.
- **Authenticating identities.** Identities are attribution, not authorization; the labels say how
  far each can be trusted.

## Known Limitations

- Overrule counts cover only sessions koto still keeps. A hierarchy whose sessions retention has
  removed no longer counts; a durable monthly reading is the reporting work's to keep.
- An `observed` session identifier can be forged by anything running as the operator. The record
  makes misattribution visible, not impossible.
- Until the harness reports which agent is acting, decisions record the agent as `unknown`.
- A reversal by a session whose chain to the decider passes through a session another host holds
  can't be judged from this host and records as `asserted`.
- R19 orders reversals by their recorded times; on one host the clock is shared, and across hosts a
  skewed clock can change which reversal wins, the same for every reader.

## Decisions and Trade-offs

- **Decisions live in the deciding session, not a host store.** The owner's ruling. Session
  retention removed the reason the first build gave for a host store, and the cloud backend carries
  the keys when a session moves.
- **A reversal lives in the reverser's session.** A session's keys have one writer. Writing a
  reversal into the decider's session would be a cross-session write, which the cloud sync doesn't
  support, a migrated source refuses, and another host can't reach.
- **Races are settled when read.** With no shared store and no lock between sessions, a write-time
  check can't see a reversal another session is writing at the same moment. Earliest time, then
  session name, then identifier gives every reader the same answer without either reverser writing
  into the other.
- **The first build's store-independent parts carry over.** The decision kinds, the decider and its
  labels, the standing rules, reversal as its own final decision, the blocked step after a reversed
  gate pass, and reads that fail closed. `verdict` gains `inconclusive` for a production verdict that
  is too early to call. The block now covers any reversed decision of a live workflow, not only a
  gate pass, because a worker that keeps going on a reversed call builds on it the same way.
- **Default the gate decider to the observed session.** The owner's settled rule: every pass names a
  decider, a defaulted one is marked and countable, and a pass is refused only when no decider can be
  established. The koto session name alone doesn't establish one, since a script with no harness
  identity can tick any session.
- **The chain comes from koto's own links.** The workspace manager's session map holds a harness id
  that is deleted at reap and sits outside koto; koto's dispatched-by links survive an import and are
  written by koto.
- **A reversed party can't clear its own block.** Letting the decider record a new decision that
  names the reversal would let it overrule the session above it. It rewinds, or the reverser or
  someone above records the replacement.
- **Gate declarations wait.** Refusing a pass by anyone other than a gate's declared deciders needs
  declarations, which a later governance feature adds; none of this feature's criteria depend on it.
- **Counts come from a scan of kept sessions.** The record has no host-wide index; a reporting tool
  that wants a longer memory keeps its own reading.
- **Reversal is final.** Reversing a reversal would make "in effect" depend on the parity of a chain;
  a new decision restores.
