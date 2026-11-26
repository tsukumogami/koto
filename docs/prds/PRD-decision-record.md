---
schema: prd/v1
status: In Progress
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
motivating_context: |
  A first build (koto#394, koto#396) kept decisions in a host store,
  `~/.koto/decisions/`, because koto removed a session when its workflow succeeded. The
  owner closed both unmerged and asked for decisions to live in the koto session itself,
  read and reversed from above through session retention, which has since shipped
  (koto#410 and the work it indexes). The first build's store-independent parts carry
  over as requirements here.
absorbed:
  - docs/briefs/BRIEF-decision-record.md
---

# PRD: The decision record

## Status

In Progress

Absorbed [BRIEF-decision-record](docs/briefs/BRIEF-decision-record.md); carried in Absorbed Brief.

## Absorbed Brief

Delegated sessions decide unattended: they accept documents, pass gates, merge, take questions
above them or decide not to, and settle judgment calls. Afterwards nobody above can read who made
each decision and on what authority, or reverse one so that koto honours the reversal, and the
partial records koto and shirabe keep each name the decider their own way, one of them in a host
file. A first build put a shared record in a host store; the owner rejected that as a workaround
for koto's session lifetime and asked for decisions to live in the koto session, which session
retention now makes possible.

The outcome this feature exists for: a coordinator, or the person at the top of a chain, opens any
session below it and reads every decision that session made or received, from the deciding
session's own keys, after it finished and while the reader's session is alive. When they
disagree, they reverse it from their own session, every reader sees the original no longer in
effect, and a live workflow that built on a reversed decision stops at its next step. A person
deciding is named without an account, and no gate is passed without a named decider.

The journeys that frame it, carried as the User Stories below: a coordinator audits an unattended
unit after it finished; a maintainer decides at the acceptance point; the coordinator reverses what
it disagrees with; a worker's escalations, declines and judgment calls become visible; a gate pass
nobody named is defaulted or refused; a person reverses two levels down after the middle session
finished, and two reversers race; the session above moves to another workspace by import and keeps
reading and reversing.

The boundary, carried as the Requirements and Out of Scope below: in are the decision kinds and
what reversing each means, the two-level decider and people without accounts, the authority and
chain read from koto's own links, write-once keys in the deciding session, reversal in the
reverser's session with a deterministic race rule, the blocked next step, the gate decider rule,
retiring the decider ledger as a host file, and bringing koto's partial records onto one
vocabulary. Out are on whose behalf a decision was made, who-may-decide policy, undoing effects
outside koto, changes to retention itself, cross-host reads without an import, any coupling of
koto and niwa, rebuilding shirabe's own records, and pruning.

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

A second consumer depends on the record too. A coordinator is replaced every few hours through
`koto init <successor> --continues <predecessor> --supersede`, and the workers it dispatched outlive
it. shirabe's coordinator makes every standing rule it follows cite the decision it came from, so a
reader can tell a person's ruling from a coordinator's own prose, and so the person can reverse the
ruling later. That needs three things the first draft of this record left open. A successor has to
stand where its predecessor stood. Otherwise a ruling recorded at the root of a run can't be
reversed by anyone once the run's first coordinator is replaced: the fenced session refuses writes,
and the successor isn't on the ruling's chain. A session has to read one decision held in another
session, including one above it, with every field the consumer computes from. And the harness id a
decision records has to match the driver id koto hands a gate, so a gate can check who is ticking.

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
- A session that takes over another through koto's supersede fence reverses with exactly the
  standing its predecessor had, read from the fence and never from the successor's own links.
- Any session on the host reads one decision by session and identifier, with every recorded field,
  its status and the reversal that won, and reads a session's chain as koto walks it.

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
12. As a coordinator that took over a predecessor's run through koto's supersede fence, I want to
    reverse a decision made below my predecessor, or by it, with the standing it had and no more,
    so the workers it dispatched still answer to the run after the handover.
13. As the person at the root of a run whose coordinator has been replaced, I want to reverse,
    through the run's newest member, a ruling I gave an earlier member, so a standing rule never
    becomes irreversible at the first handover.
14. As a manager citing a ruling its director recorded, I want to read that decision by session
    and identifier from my own session, with its decider, its entering session, its chain, its
    status and the reversal that won, though the director is above me.
15. As the maintainer of a coordinator's provenance reader, I want koto to give me a session's chain
    as it walks it, supersede hops included, so my reader keeps no chain walk of its own.

## Requirements

### Terms

| Term | Meaning |
|---|---|
| deciding session | the koto session a decision's keys live in |
| agent session | one harness conversation; koto knows it by its anchor session, named in `KOTO_ANCHOR` |
| chain | the ordered sessions above a deciding session: first those reached by following koto-owned dispatched-by links upward from the deciding session, then its anchor (reached by its held-by link) and those reached upward from the anchor, skipping any session already listed |
| above | a session is above a decision when it, or its anchor, is on the decision's chain, or when it is a later member of the run of a session on that chain, reached through supersede markers koto confirms (R39) |
| established decider | a decider named with `--by`, or, with none named, an id-shaped harness session identifier in `CLAUDE_CODE_SESSION_ID` (R47) or a readable anchor named in `KOTO_ANCHOR`; the koto session name alone never establishes one |
| run | a session and the successors that `koto init <successor> --continues <session> --supersede` fenced it for, one after another; each is a member of the run |
| supersede marker | the write-once marker koto writes beside a fenced session, naming its successor's session name and id, the fence's actor, the fence's time and the highest decision identifier the fence saw (R38) |
| hop | one step of a walk from a fenced session to its successor through the supersede marker; a hop is confirmed when the successor's own session id equals the one the marker names |
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
  | `judgment` | settling a judgment call, including disputing or dismissing a finding, and a ruling that binds a session until it is reversed | `settled` |
  | `verdict` | judging evidence, such as a review verdict or a production verdict | `pass`, `fail`, `inconclusive` |
  | `reverse` | overruling an earlier decision | `reversed` |
  | `refused` | a decision attempt koto refused | `refused` |
  | `answer` | answering a question an `escalate` raised (R50), or its raiser closing it (R77) | `answered`, `settled`, `withdrawn` |
  | `readdress` | passing a question an `escalate` raised up to a new recipient (R55) | `passed` |

- **R2.** Every decision names its subject in a form a reader can resolve after the deciding
  session is gone:

  | Kind | Subject |
  |---|---|
  | `accept` of a document, plan, test change, or policy or configuration change | repository, path or paths, commit |
  | `accept` of a plan step | repository, plan path, step identifier, commit |
  | `accept` of an intent or staged draft | where it lives (a koto request identifier, a repository path, or a forge issue), plus an optional link to the ticket or alert it came from |
  | `merge` | repository, pull request number, merged head commit |
  | `gate-pass` | workflow (koto session name), state, gate |
  | `escalate` | a question identifier and text, and the recipient: `session:<name>`, `anchor:<name>`, `person:<handle>`, or, on an entry written before R49, an untyped value; optionally a `packet` (R69), a `label` and a `need` (R78) |
  | `decline-escalation` | a question identifier and text |
  | `judgment` | what was judged, as free text plus an optional pointer |
  | `verdict` | what was judged, at least one evidence pointer, and, for a change or a release, the repository and commit judged |
  | `reverse` | the session name and identifier of the decision it overrules |
  | `refused` | the attempted kind and subject, and the reason |
  | `answer` | the asking session's name, the question's identifier in that session, and the answer text, which a `withdrawn` answer may leave out |
  | `readdress` | the asking session's name, the question's identifier in that session, and the new recipient, typed as R49 types an `escalate`'s; when the recording session has a dispatcher, that dispatcher's name (R75) |

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
    is set and id-shaped (R47), else empty; the anchor session when `KOTO_ANCHOR` names a readable one, else empty;
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
    reversal. The deciding session, deciding as a session, can't clear its own block this way; its
    route is the rewind. The next case is the one exception, and it turns on who decides, not
    where; or
  - the reversal was entered by a person through the deciding session itself (R17's same-decider
    rule for a person), and a person entering through the deciding session records a new decision
    of the same kind naming the reversal in its `basis`. A person changing their mind through a
    session overrules no session above it, so the block clears. The deciding session alone still
    can't clear it. The replacement is recorded in the deciding session (R4), which isn't on its
    own chain, so for this case `koto next` also lists the aimed area of the deciding session
    itself, one more listing in a session it already reads, and counts a replacement there only
    when a person decider entered it. That listing fails closed as R23 says for chain sessions:
    when the deciding session's own aimed area can't be read, `koto next` blocks naming it.

  A reversal that lost R19's race blocks nothing. A reversal of a fenced session's decision blocks
  no member of its run (R42).
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

The requirements from R37 on were added for the second consumer. They come last in each list, R37
to R47 here and R48 under Non-functional, and their numbers start after R36 so every earlier number
keeps its meaning. One of them changes an earlier
requirement on purpose: R47 narrows R7's harness identifier and the established decider in Terms
to an id-shaped value, and both say so.

**Succession through the supersede fence**

- **R37.** A fenced session records no decision. Every decision command addressed to a session
  whose log holds a `session_superseded` fence (recording, reversing, importing, and the
  gate-passing commands of R24) refuses with `session_superseded`, naming the successor, and
  writes nothing. On one host, a fence and a decision racing on one session never land in the
  wrong order: either the decision is written before the fence, or the command refuses. Under the
  cloud backend the fence can be pushed from another host, and a decision can be written by a
  process that hasn't seen it yet, so the marker records the highest decision identifier the fence
  saw (R38). Every reader treats a decision in a fenced session with a higher identifier as
  written after the fence: it reads `refused`, gives no standing and blocks nothing. The fenced
  session keeps serving its decisions to readers, as a migrated session does (R6). The cutoff
  assumes a session's decision identifiers only increase, which holds while one host writes the
  session, since each identifier is the highest the writer holds plus one. Its two limits are in
  Known Limitations.
- **R38.** When koto fences a session, it writes a supersede marker beside it, the way it marks a
  migrated session. The marker is write-once. It names the successor's session name and id, the
  fence's actor, the fence's time and the highest decision identifier the session held when the
  fence was written (R37), and a reader gets it in one read with no log scan, under
  both backends. Only the fencing command writes it, and koto refuses every other write and every
  removal of it, as R5 does for decision keys. A fencing command that stops between the fence and
  the marker writes the missing marker when it's run again; until the marker exists, the fence
  confirms no hop (R40).
- **R39.** Standing and reads follow a run forward. When koto judges standing (R17), picks where a
  reversal is recorded (R4), reads reversals at `koto next` (R22), or lists below a session (R15),
  it follows each session on a chain forward through its supersede marker to the run's later
  members, the way it follows a migration marker, and confirms each hop. A later member reached
  through confirmed hops stands in the fenced session's position:
  - it is above whatever the fenced session was above;
  - it counts as the fenced session for R17's same-decider rule, so it may reverse the fenced
    session's own session decisions;
  - a person entering through it counts as entering through the fenced session;
  - a reversal recorded in it counts as recorded in a session on the original's chain, so a live
    decider below the fenced session finds it at `koto next`.

  The position is the fenced session's and no more. koto continues upward from the fenced
  session's links as each decision's chain froze them, never from the successor's own links, so a
  successor gains no standing above or beside that position. A decision's chain snapshot (R12) is
  never rewritten; hops are applied when it is read. A walk follows at most 64 hops through one
  run, and treats the next one as unconfirmed.
- **R40.** A hop koto can't confirm gives no standing. When a reversal would have standing only
  through such a hop (a fenced session with no marker, a marker naming a session id other than the
  successor's own, or a marker that can't be read), koto refuses it with `reversal_refused` reason
  `supersede_unconfirmed` and records a `refused` decision in the would-be reverser's session. An
  unconfirmed hop never yields `asserted` standing, even where R17 would otherwise assert. For
  `koto next` and listings, a marker that exists but fails to parse makes its session unreadable
  (R23); a fenced session with no marker just ends the forward walk there.
- **R41.** A reversal that got its standing through one or more hops records each hop in its
  standing path: the fenced session, its successor, the fence's actor and the fence's time. A path
  whose hops koto confirmed and whose links are complete gives standing `verified`, so the
  reversal blocks and clears like any other verified one (R22). A listing filters reversals by a
  fenced session their path names, so a reader can list every reversal one fence enabled.
- **R42.** A reversal of a fenced session's decision blocks no member of its run. R22's block is
  on the deciding session's own workflow, and a fenced session has none left to advance. A later
  member reads the decision's status instead (R44).
- **R43.** Succession doesn't change who may reverse a person's decision. A later member, as a
  session, can't reverse a person's decision entered through any member of its run. A person
  entering through the later member can, and the reversal records the hop (R41).

**Reading one decision and the chain**

- **R44.** koto shows one decision, addressed by session name and identifier, to any session on the
  host, whether or not the reader is above it. Reading needs no standing; standing governs
  reversing only. The answer carries:
  - every field the decision recorded: kind, outcome, subject, time, state, the decider with each
    of R7's fields and its source labels (a person decider's entering session included), the
    authority, the chain snapshot with each link's kind and its `incomplete` mark, basis, evidence
    and reason;
  - its status (Terms);
  - when it reads `reversed`, the winning reversal's session, identifier, time, decider, reason,
    standing and standing path, hops included;
  - the dependents R21 computes.

  R16 holds for it: the read changes nothing in any session. A decision held in a session koto
  can't read shows as `unreadable`, never `in-effect`. The basis, evidence and reason text is shown
  as recorded, so whatever a decider writes there is readable by every session on the host. Under
  the cloud backend the read reaches only sessions this host holds a copy of, as koto's other
  reads do; a session left only in the bucket has to be pulled first.
- **R45.** koto reads a session's chain as it would walk it now, without recording anything: each
  session in R12's order with the link kind that reached it, each supersede hop followed forward
  from a session on it with whether koto confirmed it and the marker's successor, actor and time,
  and the `incomplete` mark. The same read reports the session's own supersede marker when it has
  one. It writes nothing, fails closed as R23 does, and uses the same walk standing uses, so a
  consumer computing where a session stands needs no walk of its own.
- **R46.** Every decision command names the session it acts on as an argument: recording, listing,
  showing and reversing. A decision is recorded in the named session, which is its deciding
  session (R4), and a session decider names that session (R7). koto never takes the session to
  record in from the environment.

**The harness id and the tick's driver**

- **R47.** koto records a session decider's harness identifier as `observed` only when
  `CLAUDE_CODE_SESSION_ID` is id-shaped by the rule koto's run journal applies to a tick's driver
  (1 to 128 characters, each a letter, a digit, `.`, `_`, `:` or `-`), and records it empty
  otherwise; only an id-shaped value establishes a decider (Terms). When
  koto hands its gates the tick's driver id (#600), that value and the harness identifier a
  decision records in the same process are the same string, so a gate comparing a tick's driver
  with a decision's decider compares like with like. The record builds no driver variable and
  reads none.

  This narrows R7 and the established decider in Terms, which as first written took any value of
  `CLAUDE_CODE_SESSION_ID`. The narrowing is deliberate: a value that isn't id-shaped can never
  equal a tick's driver, so a decider established by it couldn't be checked by any gate.

**Questions and answers**

- **R49.** The recipient of an `escalate` is typed: `session:<name>` or `anchor:<name>`, with a
  name koto accepts for a session, or `person:<handle>`, with a handle R8 accepts. A recipient with
  one of these prefixes and a name or handle koto doesn't accept fails with an error naming
  `subject.recipient` and records nothing. `koto decisions show` and `list --format v1` return the
  recipient beside the stored subject as separate values (`kind` and `handle`, or `kind` and
  `name`), so a caller selects a person's questions by comparing fields. A recipient with none of
  the prefixes, such as one written before this requirement, still reads, with the value it was
  stored with and marked untyped, and is never a person's question.
- **R50.** An `answer` answers one `escalate`. Its target is the question's identity, the asking
  session's id with the question's identifier, never the session's name alone, so a later session
  that reuses the name is not answered by it. koto refuses an answer whose question doesn't exist,
  isn't an `escalate`, has an untyped recipient or doesn't read `in-effect` (a reversed question
  takes no answer), an `answered` answer given in the asking session itself, a person's answer that
  fails R62 to R67, and an entry R76 refuses; each refusal exits 2 and writes nothing. An
  answer is never reversed: a `reverse` aimed at one is refused and recorded as `refused` (R18).
  `koto decisions record` doesn't take the `answer` kind; `koto decisions answer` records it.
- **R51.** An answer is recorded in the session it is given in, beside the reversals that session
  aims at other sessions, and never in the asker's session: answering writes no file in the asking
  session.
- **R52.** `koto decisions open --person <handle>` lists every question in this host's store whose
  recipient is that person and that is open. It takes no session argument and makes no chain test:
  a question is listed whether or not its session has a parent, an anchor or any link. Each entry
  carries the question's identity, the asking session's name, the question identifier and text,
  the typed recipient and the time it was raised.
- **R53.** Open is computed when a read runs: a question in effect is open while no closing entry
  that counts (R72) targets its identity. No file records a question as closed other than the
  closing entry itself, and an entry that doesn't count closes nothing.
- **R54.** While a session's run holds a question whose entry in effect (R74) is an `answered`
  answer and no decision in the run cites that answer in its `basis`, every `koto next` on any
  member of the run carries a `questions` array reporting it: the question's identity, its session
  and identifier, the state `answered`, and the answer with its session, identifier, text, decider,
  run mode and time, and for a person's answer the reading R68 gives it. The report follows the
  entry in effect: only that entry is reported, and when a later entry takes effect the question is
  reported again with it, even if the run cited the earlier one. A question the run closed itself
  with a `settled` or `withdrawn` entry is not reported, and neither is an entry that doesn't
  count. The report repeats on every call, including once the workflow is terminal, and stops once
  a decision in the run cites the entry in effect. A citation counts only when the decision it names is an answer to
  the question and was recorded no later than the citing decision, so an answer from a later
  session that reuses the cited name and identifier is still reported. Reading it writes nothing, takes no lock, makes no network
  connection and never blocks or changes the exit code; a session with nothing to report carries
  no `questions` key, so its response is byte-identical to one from a binary that predates it.
- **R55.** A `readdress` passes one question up to a new recipient. Its target is the question's
  identity, as an answer's is (R50), and its subject carries the new recipient as `session:<name>`,
  `anchor:<name>` or `person:<handle>`. The question keeps its identity: the `escalate` is
  unchanged and no second one is written. koto refuses a pass-up whose question doesn't exist,
  isn't an `escalate`, has an untyped recipient, doesn't read `in-effect` or is closed (R53); whose
  new recipient has no type prefix, has a prefix with a name or handle koto doesn't accept, is the
  recipient the question waits on now, or breaks R75's destination rule; and whose decider is a
  person. Each refusal exits 2 and writes nothing. A pass-up is never reversed: a `reverse` aimed
  at one is refused and recorded as `refused` (R18). `koto decisions record` doesn't take the
  `readdress` kind; `koto decisions readdress` records it.
- **R56.** A pass-up is recorded in the session that passed the question up, under the same
  aimed area as its answers and reversals (R51), and in no other session. It is written in the
  asking session only when the asking session passed the question up itself.
- **R57.** A pass-up is accepted only from the asking session, matched by the session id the
  question carries, or from the session or anchor the question waits on when the pass-up is
  recorded, or from a session that continues one of those through a confirmed succession; any
  other recording session is refused and nothing is written. R75 says which recipients a pass-up
  may name.
- **R58.** A question's current addressee and its addressee history are computed when a read runs,
  and no file stores them. The history starts with the recipient the question was raised with and
  adds one entry for each pass-up of the question that R57 would accept against the history built
  so far and that keeps R75's destination rule, by causality rather than by clock: it repeatedly
  adds the first pass-up not yet added, in order of time, then recording session name, then
  identifier, that R57 accepts against the last entry, that keeps R75 and that names a recipient
  other than the last entry, until none is accepted. A pass-up whose recorded time sorts before the one it
  follows is still added. The current addressee is the history's last entry.
- **R59.** Every entry of an open read carries `recipient`, the recipient the question was raised
  with; `addressee`, the current addressee; and `addressees`, the whole history in order, each
  typed as R49 prints a recipient.
- **R60.** `koto decisions open --at <session-or-anchor>` lists every open question in this host's
  store that was raised at the position, waits on it now, or was raised in a session below it,
  each exactly once. The position is the named session or anchor and the sessions it continues
  through a confirmed succession. A session is directly below the dispatcher `koto session origin`
  reports for it and below the anchor holding it, both when it has both; "below" follows those
  links through any number of
  steps. Each entry says in `relation` whether the position raised the question, is its current
  addressee, both, or neither and the question comes from below. The read takes exactly one of
  `--person` and `--at`, refuses an `--at` that names no session or anchor, writes nothing and
  makes no network connection.
- **R61.** `koto decisions open --person <handle>` follows the current addressee: it lists a
  question while the question waits on that person, whoever the question was raised to, and stops
  once it is passed on to someone else. A pass-up reports nothing on the asking session's
  `koto next`; R54's report of an answer applies to a question that was passed up as to any other.

**A person's answer**

- **R62.** A person's answer (`--by person:<handle>`) is recorded only with evidence that the
  person was asked: `--prompt <harness>/<call>/<position>`, naming a harness session, a call in
  that session's record of its question prompts, and the 1-based position of the question within
  that call. Without `--prompt`, a person's answer is refused with a message saying it needs the
  question-prompt evidence. A session's answer (`--by session`) takes no `--prompt` and is refused
  with one. A question can take a person's answer only when its `escalate` carries a `packet`
  (R69) and its content is stored.
- **R63.** koto finds the record from the harness session alone: the named harness session must
  be the one running the command, as `CLAUDE_CODE_SESSION_ID` reports it (R47), and, when koto has
  recorded a harness session driving the answering koto session (at its creation or on a later
  command), the same one. Exactly one regular file named `<harness>.jsonl`, not itself a symbolic
  link, must sit one directory below `.claude/projects/` in the effective user's home as the
  account database gives it, never `$HOME`, and its canonical path must lie under the root's
  canonical path (a symbolic link to `.claude` itself is followed). `CLAUDE_CONFIG_DIR` isn't read.
  The harness session id is an unauthenticated value from the environment: any process running as
  the user can set it. The driver binding catches a mismatch where a driver is recorded; it narrows
  nothing against a deliberate forger, who picks `--in`, can use a session created without the
  harness variable (which records no driver) and can delete the sidecar that holds it.
- **R64.** The call must be the harness's question tool, `AskUserQuestion`; the position must
  name one of its questions; and that question's `header` must be the question's prompt header,
  `#` followed by the first 11 hexadecimal digits of the SHA-256 of `<asking session id>/<question
  id>`, compared byte for byte. `koto decisions show` and `list --format v1` return the header as
  `prompt_header` on every `escalate` that carries a `packet`.
- **R65.** The question text and the option labels the call showed equal the stored content's
  `question` and `options[].label`, byte for byte and in order: no trimming, no case folding and no
  Unicode normalization. An option dropped, added or moved fails.
- **R66.** The call's result must be the person's reply: not an error, with a permission decision
  of `accept` whose source is exactly one the captured records show for a reply the user gave
  (`user_temporary`), and an answer for the question's text. The call and its result must both be
  lines of the harness session's own conversation, not a side conversation's (`sidechain`), and
  the call must have been made no earlier than the question was raised (`time`). The answer's text
  is the result's answer for that question, whatever `--text` the caller passed. Only lines that
  contain the call id are parsed. A record whose call or result lacks a field koto reads, holds two
  questions with the same text, holds the call or its result on more than one line, puts the result
  before the call, has a line holding the call id that isn't valid JSON, or has a line over 16 MiB
  or more than 1 GiB in all fails with the `shape` check, so a change in the harness's record
  format refuses answers instead of misreading them.
- **R67.** One position of one call verifies one answer on a host. Before writing, an answer takes
  a marker under the koto root named for the harness session, call and position, locked while it
  writes and freed if the write fails; a second answer citing the same position, from any
  answering session on the host, is refused while the marker is locked or a stored answer on the
  host cites the position, so of two answers racing on one position exactly one is recorded. A
  marker left by a process that stopped before publishing, with no stored answer citing it, is
  taken over.
- **R68.** A verified answer stores the pointer and a mark, `verified`, labelled `observed`; no
  caller input produces it. The answering host keeps a note of each answer it verified in the
  answering session's directory, outside its context, which no sync, pull or import carries,
  written before the answer is published. Each note line is sealed: a SHA-256 over a host identity
  koto keeps under its root (created once, published whole, never synced or imported, and not used
  when its group or others may write it), the session directory's name and the SHA-256 of the
  stored answer. A read
  shows the answer as `verified` only when the note in the directory it read the answer from holds
  that answer's sealed line. Anywhere else (a copy brought by a pull or an import, on this host or
  another, a session directory copied by hand to another koto root, a line written for another
  body) a read shows the mark as `remote` and the reading "a remote session's answer". `show`,
  `list --format v1` and the `questions` on `koto next` all give that reading; `koto context get`
  prints the stored body, whose mark is not the reading. Reading never opens the harness record, so
  a record removed later doesn't change the reading. `koto session import` carries the decision
  areas and the content area, every body the source recorded itself byte for byte, and the copy
  reads a carried body as its own.
- **R69.** An `escalate` may carry `packet=sha256:<64 lowercase hex digits>`, the hash of the
  question's content. `koto decisions content write <session> <id> --file <path>` stores that
  content once under the asking session's `koto/packets/<id>`: at most 16 KiB, refused when its
  hash isn't the `packet`, when the question carries none, or when content is already stored. No
  context command writes or removes a key under `koto/packets/`.
- **R70.** `koto decisions content read <session id>/<question id>` prints the stored content
  byte for byte, from any session or directory.
- **R71.** Every refusal of a person's answer exits 2 with code `prompt_unverified`, field
  `prompt`, and a message naming the pointer and the check that failed (`session`, `record`,
  `call`, `tool`, `position`, `identity`, `question-text`, `options`, `result`, `sidechain`,
  `time`, `shape`, `call-position-reused` or `content`). No text from the record appears in koto's
  output.

**Which entries count and which is in effect**

- **R72.** A closing entry is an `answer` of any outcome aimed at a question's identity. Each read
  judges whether it counts, against the question's addressee history (R58) as it stands when the
  read runs, so an entry that counted can stop counting when the history or the other entries
  change, and the reverse:
  - An `answered` answer by a person decider that R68 reads as verified counts, whichever session
    it was recorded in and whoever the question waits on.
  - Any other `answered` answer counts only when the question waits on a session or anchor and the
    recording session is that addressee or continues it through a confirmed succession. This
    covers a session's answer and a person's answer read as a remote session's (R68), which is
    judged as an answer of the session holding it. A session's answer to a question that waits on a
    person never counts.
  - A `withdrawn` or `settled` entry counts only as R77 allows.
- **R73.** An entry that breaks a rule is returned by every read that returned it before, never
  dropped, and never takes effect. `koto decisions show` and `list --format v1` add to every
  `answer` and `readdress` entry `counted` (true or false) and, when it is false, a `reason`
  naming the rule; to every closing entry they add `governs`, true on the one entry in effect
  (R74) and false on every other. A pass-up counts when it is in the question's addressee history.
  The stored status of an entry is unchanged by this judgment.
- **R74.** Of a question's closing entries that count, one is in effect. A person's answer counted
  under R72's first rule outranks every other entry, whichever was recorded first: of several, the
  one no other of them cites in its `basis` is in effect, and of several uncited, the latest by
  time, then recording session name, then identifier. With no such answer, the earliest counted
  entry by the same order is in effect. Exactly one closing entry of a closed question reads
  `governs` true, and an open question has none.
- **R75.** A pass-up may name a person, or the session or anchor that is the recording session's
  dispatcher as `koto session origin` reports it (for a session continuing another through a
  confirmed succession and having none of its own, the dispatcher of the session it continues). A
  session with no dispatcher can pass a question only to a person. The pass-up records the
  dispatcher's name when it is written, and the rule is judged from that recorded name, so a later
  change to the recorder's links moves no question; a pass-up written before this requirement is
  judged from the dispatcher its authority recorded. A stored pass-up that breaks the rule is left
  out of the addressee history and reads `counted` false.
- **R76.** `koto decisions answer` refuses, before any write, every entry that would not count
  when written: a session's `answered` answer to a question that waits on a person, or recorded in
  a session that isn't the question's addressee and doesn't continue it; and a session's
  `answered`, `settled` or `withdrawn` entry aimed at a closed question, with a message naming the
  entry in effect. A person's `answered` answer is taken while the question is in effect, closed
  or not, since it outranks what closed it. Each refusal exits 2.
- **R77.** A question's raiser closes it itself with `koto decisions answer --outcome withdrawn`
  or `--outcome settled`, recorded `--in` the raising session, matched by session id, or a session
  continuing it, with a session decider. A withdrawal needs a reason and takes no evidence. A
  settle needs a text and evidence: for a question that waits on a person, the question needs a
  `label` and a `--basis` naming an `answered` answer of the same person that R68 reads as verified,
  that is in effect on its own question, whose question has the same `label`, and that was recorded
  no later than the settle; for a question that waits on a session or anchor, a `--basis` naming a
  decision that reads back, or an `--evidence event:<session>:<seq>` pointer. A basis names a
  session and an identifier, resolved when the settle is judged, so a decision a later session
  writes under a removed session's name and identifier is judged on its own and never read as the
  removed session's. A stored `withdrawn` or `settled` entry from any other session, by a person,
  with no reason, of a need (R78), or whose basis no longer meets these rules reads `counted`
  false. A `reverse` aimed at a `settled` or `withdrawn` entry is refused as one aimed at an
  answer is (R50).
- **R78.** An `escalate` may carry `need=decision`, `need=credential` or `need=reserved-step`
  (any other value is refused on `subject.need`) and a `label` of at most 128 bytes (a longer one
  is refused on `subject.label`). Every element of `koto decisions open --person` and
  `open --at` carries `need`, reading `decision` for a question raised with `need=decision` or
  with no `need`, including one stored before this requirement. A need is listed, passed up and
  answered like any question, and its raiser can withdraw it but can't settle a `credential` or
  `reserved-step` need. No read selects by `label`.
- **R79.** No write adds a twenty-first open question waiting on a person to a session's run.
  The run's count is of the questions raised in the session or in the sessions it continues
  through a confirmed succession, matched by session id, that are open and whose current addressee
  is a person, whatever the handles. While it reads twenty, an `escalate` with a `person:`
  recipient is refused on `subject.recipient`, and a pass-up of one of the run's questions to a
  person is refused on `to`, whoever records it; both messages name the limit of twenty and
  nothing is written. A pass-up of a question that already waits on a person to another person is
  not refused, since it adds nothing to the count, and an `escalate` or pass-up to a session or
  anchor never is. The count follows the current addressee, so a question passed from a person to
  a session no longer counts; a closed or reversed question doesn't count; and an entry that
  doesn't count frees no place. On one host, the writers that can add to the count hold a lock on
  the run from the count to the write, so writes run at once can't pass twenty. What the limit
  doesn't cover: the count reads this host's store, so writers on two hosts are not serialized and
  can together pass twenty; and a read never refuses, so questions can reopen past twenty without
  any write by the run, as when twenty settles resting on one person's answer stop counting
  because that person answers the earlier question again.

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
- **R48.** Following a run forward (R39) reads one supersede marker for each chain session that has
  one, and the reversals in each later member it reaches; it touches no session off the chain and
  its runs. R36's 50 ms limit still holds at a chain depth of five when two of the five chain
  sessions have each been fenced twice.

## Acceptance Criteria

**Reading an unattended unit after it finished**

- [ ] A worker dispatched by a live coordinator through a request leg runs unattended with
  `CLAUDE_CODE_SESSION_ID` and `KOTO_ANCHOR` set and no acting agent reported. It accepts a
  document, passes a gate with `koto overrides record` and records a merge, then reaches a success
  terminal. With the coordinator's workflow still not terminal and the worker's session finished
  and kept by retention, listing from the coordinator's session the worker's decisions returns three, each with
  decider kind `session` naming the worker session and its harness identifier labelled `observed`,
  agent `unknown`, run mode `unattended` (handed with `--run-mode unattended`), authority `kind` `dispatch` with `dispatcher` the
  coordinator, and a chain whose first link is the coordinator labelled `leg`.
- [ ] In the same run, each of the three decisions exists as keys in the worker's session, and no
  file under `~/.koto/` outside `sessions/` is created or changed holding a decision, a reversal or
  a gate answer. This criterion is checked on a koto that has removed its run journal; before
  then, an automated test may check it in an isolated koto home with `run_journal.enabled = false`,
  and in both cases the records koto sends to its telemetry sink are the same as with the journal
  on. No host is told to switch its journal off early. With `~/.koto/_decider_ledger.jsonl` replaced by a directory before the run, every
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

**Succession**

The fixture for this group: P is a coordinator dispatched by D through a request leg. P dispatches
W through a request leg, and W records a `judgment` and keeps running. P records a `judgment` of
its own, and a person enters a `judgment` through P with `--by person:<handle> --surface
terminal`. Then `koto init S --continues P --supersede` fences P.

- [ ] After the fence, `koto decisions record`, `koto decisions reverse` and `koto decisions import`
  addressed to P each exit non-zero with `session_superseded` naming S, and P gains no key;
  listing P still returns its three earlier decisions. A fence and a `record` on one session,
  started from a shared barrier in 20 trials, each end with either the decision written before the
  fence or the record refused, never a decision after the fence.
- [ ] Under the cloud backend, with S's fence pushed from a second host while P's own host records
  a `judgment` it hasn't seen the fence for, the judgment's identifier is above the marker's
  highest identifier, and a listing of P and `show` of the judgment both read it `refused`; a
  reversal written in P the same way blocks nothing below P.
- [ ] P holds a supersede marker naming S's session name and id, the fence's actor, its time and
  the highest decision identifier P held. Reading it takes one read and no read of P's log
  (checked by a read counter in tests). `koto context add` and `koto context remove` on it are
  refused naming it. Under the cloud backend it appears under P's bucket prefix.
- [ ] From S, reversing W's judgment is accepted with standing `verified` and recorded in S. Its
  standing path names the hop from P to S with the fence's actor and time. W's next `koto next`
  blocks naming S's reversal, and W's, P's and D's keys and logs are byte-identical to before.
- [ ] From S, reversing P's own session `judgment` is accepted. From S alone, reversing the person's
  `judgment` entered through P is refused and recorded `refused` in S. With `--by
  person:<handle>` through S it's accepted with standing `verified` and the hop in its path.
- [ ] With S itself fenced by S2, reversing W's judgment from S2 is accepted, and the standing path
  names both hops in order. Listing below S2 returns W's decisions.
- [ ] From S, reversing a decision D made, and one made by W2 (a session D dispatched beside P), are
  each refused and recorded `refused` in S. With S also linked under another session X by `koto
  session link`, W's chain read lists no X, and X's attempt to reverse W's judgment is refused
  and recorded `refused` in X: the walk continues upward from P's frozen links, never S's own.
- [ ] With P's marker removed by a test fixture (the fence event left in P's log), and in a second
  run with the marker naming a session id other than S's, S's reversal of W's judgment exits
  non-zero with `reversal_refused` reason `supersede_unconfirmed`, and a `refused` decision is
  recorded in S. Neither run records standing `asserted`.
- [ ] With P's marker holding malformed JSON, W's `koto next` blocks naming P as unreadable, and a
  listing of W's decisions shows the judgment `unreadable`.
- [ ] After the person reverses their own ruling through S, S's `koto next` isn't blocked by it, and
  showing the ruling from S reads `reversed` with the winning reversal's hop.
- [ ] A listing of reversals filtered on fenced session P returns exactly the reversals whose path
  names the P-to-S hop.
- [ ] R48's tick benchmark, at chain depth five with two chain sessions each fenced twice, measures
  under 50 ms.

**Reading one decision and the chain**

- [ ] M is dispatched by D through a request leg, and D records a person `judgment` through D. From
  M, `koto decisions show D <id>` returns the decider kind `person` with its handle, surface and
  entering session D; the chain snapshot with each link's kind; the time, subject and reason; and
  status `in-effect`. An unrelated session on the same host gets the same answer. M's and D's keys
  and logs are byte-identical before and after.
- [ ] After a person reverses that judgment through D, `show` from M returns status `reversed` with
  the winning reversal's session, identifier, time, decider, reason, standing and standing path. A
  `show` of a decision in a session koto can't read returns `unreadable`.
- [ ] Reading W's chain from the succession fixture lists P labelled `leg`, W's anchor labelled
  `held-by`, and P's hop to S with its confirmation, successor, actor and time. Reading P's chain
  reports P's own marker. With the marker naming the wrong session id, the hop is listed
  unconfirmed. No session gains a key or event from the read.
- [ ] `koto decisions record`, `list`, `show` and `reverse` each act on the session named in their
  arguments. Run from a process whose `KOTO_ANCHOR` and `KOTO_SESSION_*` variables name another
  session, `record` still writes into the named session only.

**Standing rulings and a person's own block**

- [ ] A person records a ruling through a coordinator session as a `judgment` with outcome
  `settled`, and it lists with kind `judgment`; recording a ruling needs no other kind.
- [ ] C is live, and a person enters a `judgment` through C, then reverses it through C. C's next
  `koto next` blocks. A new `judgment` naming the reversal, entered by a person through C, clears
  the block, found in C's own aimed area. In a second run, the same replacement recorded by C with
  no person decider leaves C blocked. Both runs pass under the cloud backend too.

**The harness id and the tick's driver**

- [ ] With `CLAUDE_CODE_SESSION_ID` set to a value that isn't id-shaped and `KOTO_ANCHOR` unset, a
  gate pass with no `--by` is refused with `decider_missing`, and a decision recorded with `--by
  person:<handle>` records an empty harness identifier. With an id-shaped value, the decision's
  harness identifier is the value, labelled `observed`.
- [ ] For one process ticking a session past a gate with `koto next --to`, the harness identifier
  its `gate-pass` decision records equals, byte for byte, the driver id koto's driver rule derives
  for that process, the value #600 hands the tick's gates. While the run journal exists, it also
  equals the driver the journal records for that tick.

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

**Questions and answers**

- [ ] An `escalate` recorded with `recipient=person:owner` shows its recipient as kind `person`
  and handle `owner`; `session:` and `anchor:` recipients show their own kind and name; one stored
  as `recipient=coord` shows as untyped with value `coord` and is open for no person (R49).
- [ ] `koto decisions open --person owner` returns a question raised in a session with no parent,
  anchor or link, with its identity, session, identifier, text and recipient, and
  `--person someone-else` returns nothing (R52).
- [ ] With a question raised in A to `session:B`, `koto decisions answer A <id> --in B --text ...
  --by session` writes exactly one file under B's `ctx/koto/aimed/<A's session id>/` and leaves
  every file in A byte-identical; afterwards the question is no longer open (R50, R51, R53).
- [ ] An answer aimed at a missing decision, at a decision that isn't an `escalate`, at an
  untyped or reversed `escalate`, from the asking session, or with `--by person:<handle>` and no
  `--prompt` exits 2 and writes nothing in either session (R50, R62).
- [ ] After the answering session is removed and a new session of the same name answers another
  question with the same answer identifier, the asker's earlier citation doesn't end the report of
  the new answer (R54).
- [ ] With q1 and q2 raised to `session:B` and q3 to `session:C`, B's answer to q1 alone leaves q2
  and q3 open and reports only q1 on the asker's `koto next` (R53, R54).
- [ ] After the asking session is removed and a session of the same name is created, the earlier
  answer closes none of the new session's questions and isn't reported to it (R50).
- [ ] The asker's `koto next` reports the answer on every call, on a terminal workflow too, and on
  its successor's first `koto next` after `--continues A --supersede`; a call that only reports
  leaves both sessions byte-identical; once a decision in the run cites the answer, the response
  has no `questions` key (R54).

**Passing a question up**

- [ ] With `worker` dispatched by `manager` and `manager` by `director`, and q1 raised in `worker`
  to `session:manager`, `koto decisions readdress worker <id> --in manager --to session:director
  --by session` exits 0, prints an entry of kind `readdress` and outcome `passed` whose target is
  q1's identity, writes one file under `manager`'s `ctx/koto/aimed/<worker's session id>/`, and
  leaves `worker` and `director` byte-identical; `koto decisions show worker <id>` prints the same
  entry before and after (R55, R56).
- [ ] A pass-up aimed at a missing decision, at one that isn't an `escalate`, at an untyped,
  reversed or answered `escalate`, with a `--to` that is untyped, badly named or the current
  addressee, or with `--by person:<handle>` exits 2 and writes nothing; `record --kind readdress`
  is refused and a `reverse` aimed at a pass-up exits 2 (R55).
- [ ] After `manager` has passed q1 to `director`, a second pass-up recorded in `manager`, or in a
  session that never was q1's addressee, exits 2 and writes nothing; one recorded in `director`
  or in `worker` exits 0, and the one from `worker` is stored in `worker`; a session created with
  `--continues director --supersede`, or with `--continues worker --supersede`, may pass q1 up in
  their place (R56, R57).
- [ ] A pass-up whose stored time reads earlier than the pass-up it follows still moves the
  question: after `manager` passes q1 to `director` and `director` passes it to `person:owner`,
  with `director`'s entry dated before `manager`'s, `--person owner` returns q1 with all three
  addressees (R58).
- [ ] After q1 goes from `manager` to `director` and on to `person:owner`, every open read that
  returns it shows `addressee` as person `owner` and `addressees` as `session:manager`,
  `session:director`, `person:owner`, with `recipient` still `session:manager`; q2, raised beside
  it and never passed up, shows one addressee (R58, R59).
- [ ] `koto decisions open --at manager` and `--at director` each return q1 exactly once before
  and after both pass-ups, and neither returns a question raised in an unrelated pair of sessions;
  a question from an unlinked session raised to `session:director` is returned by `--at director`
  until it is passed on, and never by `--at manager`; `--at` a session created with `--continues
  manager --supersede` returns what `--at manager` did; `--at` an anchor returns the questions of a
  session it dispatched or holds, and so does `--at` the anchor that dispatched it; a session held
  by an anchor and dispatched by another session is below both (R60).
- [ ] `--person owner` returns q1 only once it waits on `owner`; a question raised to
  `person:someone-else` and passed by its raiser to `person:owner` is returned for `owner` and not
  for `someone-else`; `koto next worker` carries no `questions` key while q1 is passed up and
  unanswered, and reports the answer once `director` answers it (R61).
- [ ] `koto decisions open` with both `--person` and `--at`, or with neither, or with `--at` naming
  no session, exits 2; both reads leave every session byte-identical and, under the cloud backend,
  make no request to the bucket (R60).

**A person's answer**

- [ ] With a record copied in structure from a real accepted question prompt, a person's answer
  citing it is recorded with the result's answer text even when `--text` says otherwise, shows
  `verified` labelled `observed` and a decider handle labelled `handed`, and each of three
  positions in one call verifies its own answer once (R62, R66, R68).
- [ ] A missing call, a call to another tool, another question's header, a position outside the
  call, a denied call, a permission source outside the allow-list (`userSettings`, `user_abort`),
  a call or result in a side conversation, a call made before the question was raised, a call in
  another harness session's record, a record under a redirected `HOME`, and a harness session
  other than the one koto recorded for the answering session each exit 2 with the check named,
  write nothing, and print no record text (R63, R64, R66, R71).
- [ ] A shown question text with one byte changed, with trailing whitespace or in another Unicode
  normalization, and labels with one byte changed, in another case, dropped, added or swapped,
  each fail (R65).
- [ ] A renamed field in the call, a missing field in the result, a result with no answer for the
  question, an invalid JSON line holding the call id, a result before its call, a call id on two
  lines and a line over 16 MiB each fail with `shape` (R66).
- [ ] A second answer citing a used position fails with `call-position-reused` and leaves the
  first unchanged; of two answers started together on one position, from one answering session
  or from two, twenty times over, exactly one is recorded each time (R67).
- [ ] On the recording host the answer reads `verified` after its session syncs and after the
  record is removed; a pulled copy on another host, holding the same bytes, reads "a remote
  session's answer" with mark `remote`; an imported copy, on another host or under a new name on
  the recording host, never reads it `verified`; nor does the sessions directory copied with its
  note to another koto root, or a hand-written body named by a note line, bare or carrying a real
  line's seal; the `questions` on `koto next` give the same reading (R68).
- [ ] Content of exactly 16 KiB is stored and 16 KiB plus one byte is refused naming the cap; a
  second write is refused and the first bytes stay; content that doesn't hash to the `packet` is
  refused; `context add` under `koto/packets/` is refused; a read from another session prints the
  bytes written (R69, R70).

**Which entries count and which is in effect**

With `worker` dispatched by `manager`, `manager` by `director`, and an unrelated `other-lead` with
`other-worker` below it:

- [ ] With q1 raised in `worker` to `session:manager`, a session's answer `--in manager` exits 0
  and `--in director` or `--in other-lead` exits 2 naming the addressee rule; after `manager`
  passes q1 to `director`, `--in manager` exits 2 and `--in director` exits 0; a question waiting
  on `person:owner` takes no session's answer from any session and takes a person's answer
  recorded in any session; a person's answer closes a question waiting on a session (R72, R76).
- [ ] A session's answer stored in `other-lead` for a question waiting on `manager` lists with
  `counted` false and a `reason`, the question stays in every open read, and `koto next worker`
  reports nothing; a stored answer by `manager` reads `counted` false once the history has moved
  to `director`; a person's answer in a session whose note of checked prompts is missing counts
  only when that session is the addressee; a question holding only such entries still takes a
  pass-up and an entitled answer (R72, R73).
- [ ] A session's answer and a later person's answer both read `counted` true and the person's
  alone `governs`; with the person's answer first, a session's answer exits 2 naming the entry in
  effect, and one stored later reads `counted` true and `governs` false; of two counted session
  answers the earlier governs; of two person answers the one citing the other governs whatever
  their stored times, and of two citing neither the later governs; exactly one entry governs
  (R74).
- [ ] `koto next worker` reports only the entry in effect, reports q1 again with the owner's answer
  after `worker` cited `director`'s, and stops once the owner's is cited; a question the run
  withdrew or settled itself is not reported (R54).

**Where a pass-up may go**

- [ ] With q1 raised in `worker` to `session:manager`, a pass-up `--in manager` to
  `session:other-lead` or `session:worker`, or `--in worker` to `session:director`, exits 2 naming
  the rule and the dispatcher it would accept; `--in manager` to `session:director` or
  `person:owner` and `--in worker` to `person:owner` exit 0; `--in director`, with no dispatcher,
  exits 2 to `session:manager` and 0 to `person:owner` (R75).
- [ ] A stored pass-up to a session that isn't its recorded dispatcher, one recorded by a session
  that was neither raiser nor addressee, one naming the current addressee and the later of two
  written against one addressee each list `counted` false with a reason and are left out of
  `addressees`; each pass-up in the history lists `counted` true (R73, R75).

**Withdrawing and settling**

- [ ] `--outcome withdrawn --reason ...` recorded `--in` the raiser or its successor exits 0 and
  closes the question; without a reason, `--in` any other session or by a person it exits 2 and
  writes nothing; a stored withdrawal from another session reads `counted` false (R77).
- [ ] With an owner's answer in effect on q1 labelled `cache`, a settle of q5, waiting on
  `person:owner` and labelled `cache`, citing it exits 0; with no basis, citing a session's
  answer, citing an answer that no longer governs, of another handle, read as remote, from a
  removed session's reused name and identifier, of a question with another label or none, it
  exits 2; once a later owner's answer replaces the cited one, the settle reads `counted` false
  and q5 is open again (R77).
- [ ] A settle of a question waiting on a session exits 0 with a basis that reads back or an event
  pointer and 2 with neither; a settle or withdrawal of a closed question exits 2 naming the entry
  in effect; `record --kind answer` is refused for every outcome and a `reverse` of a settle or a
  withdrawal exits 2 (R76, R77).

**Needs and the limit of twenty**

- [ ] `need=credential`, `need=reserved-step` and `need=decision` are recorded, another value is
  refused on `subject.need` and a 129-byte `label` on `subject.label`; the open reads carry `need`;
  a need is answered by a person or its addressee; settling a `credential` need exits 2 naming it,
  a stored settle of it reads `counted` false and a withdrawal exits 0 (R78).
- [ ] Fifteen open questions to `person:owner` and five to `person:someone-else` refuse the next
  `escalate` to either handle naming the limit of twenty and writing nothing, while one to a
  session or anchor and another session's twenty are recorded; a person's answer, a withdrawal or
  a settle frees one place and an entry that doesn't count frees none; a question passed up to a
  person counts toward its raiser and one passed from a person to a session doesn't; a successor
  is refused while its predecessor holds twenty, a new session under a removed one's name starts
  at zero, and a reversed `escalate` doesn't count (R79).
- [ ] With the raiser's run at twenty, a pass-up of one of its questions to a person exits 2 on
  `to` naming the limit, from the addressee and from the raiser, writing nothing; a pass-up to a
  session is recorded; a question already waiting on a person passes to another person; and six
  `escalate` commands to a person started at once at nineteen record exactly one (R79).

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
  session and its decisions. The one change to succession is R38's marker, which the fencing
  command writes beside the fence it already appends.
- **Who may fence a session.** Anyone who can fence a session can already cancel or change it,
  locally or through the bucket credential. A rule limiting who may fence belongs to succession,
  not to this record; until one exists, whoever fences a session takes its standing (Known
  Limitations).
- **A carry that rides the fence.** `koto init --continues` without `--supersede` fences nothing,
  and copying keys from a predecessor confers no standing. Making a carry ride the fence would be a
  change to succession; this record only reads the fence.
- **Dispatch links and requesters koto observes itself.** `init` and `link` links are written by a
  session's own opener and stay as they are. A consumer that trusts only koto-written links reads
  the `leg` kind each chain link already carries (R12).
- **A verified identity for a person decider.** A handle stays `handed` text. Verifying it needs an
  identity koto doesn't hold, which R8 rules out.
- **The tick's driver variable.** Handing gates the tick's driver id is #600's to build. This
  record says only what it expects of that value (R47).
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
- **The rest of a question's life.** R49 to R79 cover raising, passing up, answering, settling
  and withdrawing a question. How long questions and answers are kept is not covered yet, and
  neither is a read that selects earlier answers by `label` or one question's whole history as a
  read of its own.

## Known Limitations

- koto's run journal, `~/.koto/_run_journal.jsonl`, relays gate overrides and decider answers for
  telemetry. koto's event-sink work sends those records to the telemetry sink directly and removes
  the journal on its own schedule, which a host archive's cutover depends on. Until it does, R4
  holds only in an isolated test with the journal switched off; this feature never switches a
  host's journal off and changes nothing the telemetry sink receives.

- Overrule counts cover only sessions koto still keeps. A hierarchy whose sessions retention has
  removed no longer counts; a durable monthly reading is the reporting work's to keep.
- An `observed` session identifier can be forged by anything running as the operator. The record
  makes misattribution visible, not impossible.
- Until the harness reports which agent is acting, decisions record the agent as `unknown`.
- A reversal by a session whose chain to the decider passes through a session another host holds
  can't be judged from this host and records as `asserted`.
- R19 orders reversals by their recorded times; on one host the clock is shared, and across hosts a
  skewed clock can change which reversal wins, the same for every reader.
- A fence written by a koto release older than R38 has no marker, so it confirms no hop, and a
  successor of that fenced session gets no standing from it. Reversals below such a fence go to
  the session or person above, as before this record.
- Whoever fences a session takes its `verified` standing over everything below it, including the
  power to stop a live worker. The fence already lets them stop the session itself.
- Under the cloud backend, a decision recorded on the fenced session's host after the fence listed
  its decisions but before the fence's push lands is reported written, then reads `refused` (R37).
  The caller learns of it only from a later read. A fence with no marker records no cutoff, so a
  decision written there from a host that hasn't seen the fence reads like any other.
- A person decider is named with `--by`, which koto records and doesn't verify. An agent in the
  deciding session that names a person can clear a block a person's reversal through that same
  session caused (R22). The replacement names the decider it claimed, so the claim is visible, as
  any other forged person decision is.
- A person's answer is verified against the harness's own record of its question prompts, a file
  format koto observes and doesn't own. The `shape` check refuses a record whose format changed,
  but anything running as the operator can write a record that passes, in the account's own
  `.claude/projects`, and can set `CLAUDE_CODE_SESSION_ID`; the evidence shows the prompt happened
  as recorded, not that no one wrote the record.
- The verified reading protects against a session mistaking a relayed or typed remark for an
  answered prompt, and against copies that arrive by import or pull. It is not protection against a
  process running as the same user that sets out to forge: reading the koto root's `host-id` is
  enough to seal a hand-written answer that was never prompted, with no harness record at all. A
  copy of the whole koto root, `host-id` included, on another machine still reads verified, and so
  does a session directory restored in place.
- A harness configured with `CLAUDE_CONFIG_DIR` keeps its records outside `~/.claude/projects`,
  so a person's answer there always fails the `record` check. koto doesn't honour the variable,
  since any process can set it.
- R67's marker is per host. Two hosts answering from copies of one harness record aren't
  serialized. Once the session holding the answer that used a position is removed from the host, no
  stored answer cites it and the position can verify another answer.
- `koto session import` carries the decisions its source recorded itself, not the ones the source
  carried from an earlier import: a copy of a copy holds only its immediate source's own decisions.

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
  is too early to call.
- **Any reversed decision blocks a live workflow, not only a gate pass.** This is a product choice,
  made here on a reading of the planning's three-level scenario, where the grandchild "decides and
  keeps working" and a reversal from the top blocks its next step whatever kind the decision was.
  The first build blocked only after a reversed gate pass. The reason for widening it: a worker
  that keeps going on a reversed judgment, acceptance or escalation builds on it exactly as it
  would on a reversed gate pass, and a reversal that changes nothing in a live run is a note, not
  an overrule. The cost is that a reversal of a minor call stops the worker too; R22's rewind and
  the reverser's replacement decision are the ways out. The alternative, blocking only gate
  passes, would leave the scenario's judgment reversal with no effect on the running worker.
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
- **A successor stands where its predecessor stood, read from the fence.** A `continues` link is
  written by the successor itself and grants no authority, so standing can't come from it. The
  supersede fence is the one record tying predecessor to successor that the successor doesn't just
  assert: koto appends it to the predecessor's own log, under the predecessor's own lock, with one
  winner. Without it, once a coordinator is replaced, only the person at the top could overrule a
  long-lived worker, the run's routine reversals of its own earlier calls would be refused, and a
  ruling recorded at the root could never be reversed. Re-linking live workers to the successor
  was rejected: a stopped worker never re-links, decisions recorded before the re-link keep their
  old snapshot, and the predecessor's own decisions aren't covered.
- **An unconfirmed hop refuses rather than asserting.** An `asserted` reversal never stops a live
  worker or clears a `verified` one, so a hop that could only assert would leave the run unable to
  stop its own workers. Refusing makes the coordinator escalate, which it already does.
- **Reading needs no standing.** Who decided what is attribution, and every reader should be able
  to check it, including a manager citing its director. Standing governs reversing only. Limiting
  the read to sessions above would protect nothing: any process on the host can already read a
  session's files, and a decision body travels with its session wherever the session goes. A
  body that has to be kept from other sessions doesn't belong in a decision. Revisit this if koto
  gains a per-session read permission, or if a consumer needs a decision's reason or evidence kept
  inside its hierarchy.
- **A standing ruling is a person's `judgment`.** A ruling that binds a session until it's reversed
  is a judgment call a person settled. A separate kind would add nothing a reader can't already
  read from the decider and the subject.
- **A person can lift a block they caused through the same session.** R22 keeps a reversed session
  from clearing its own block because doing so would overrule the session above it. When a person
  reversed their own decision through that session, nobody above is overruled, so a person's
  replacement through it clears the block. The session alone still can't.
- **The record expects the driver id and doesn't build it.** A gate that checks who is ticking
  compares a tick's driver with a decision's decider. The record's part is making the values the
  same string; #600 hands the value to gates.
