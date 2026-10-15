---
schema: prd/v1
status: In Progress
problem: |
  A decider-check criterion that gets no verdict passes its gate. Six reasons
  lead there today (over budget, extraction failed, provider error, unreadable
  response, cap spent, busy), and in every one the criterion is judged by
  nobody, in veto as in shadow. Nothing exported counts these cases or records
  the slice sizes that caused them, so budgets can't be tuned from the run
  journal.
goals: |
  A criterion the decider can't answer is answered by the agent, and that
  answer decides the gate. A log reader can tell a decider verdict from an
  agent answer from a criterion still waiting on one. Every fallback leaves
  one run-journal record naming the check, the criterion, the reason and the
  slice size, under names that collide with nothing the journal writes.
motivating_context: |
  koto#297 ruled that a missing verdict never blocks, and its fix (koto#300,
  released since v0.15.0) made every missing verdict pass. The maintainers
  ruled on 2026-10-14 that the pass is a defect: a missing verdict still
  shouldn't block on its own, but it no longer counts as a pass. The check
  falls back to the agent, the way the routing decider already does.
absorbed:
  - docs/briefs/BRIEF-decider-fallback.md
---

# PRD: decider-check fallback to the agent

## Status

In Progress

Absorbed [BRIEF-decider-fallback](docs/briefs/BRIEF-decider-fallback.md); carried in Absorbed Brief.

## Absorbed Brief

A decider check grades the agent's own work, and since koto#300 a criterion
the decider can't answer passes. Over budget, a failed extraction, a provider
outage, an unreadable answer, a spent cap and a busy lock all end with nobody
judging the criterion, even under veto. And nothing exported says how often
that happens or how big the slices were, so the people tuning budgets can't
see it.

The outcome this feature exists for has two halves. The agent doing the work
is asked any criterion the decider couldn't grade, using the template's own
question and descriptions, in shadow and veto alike, and its answer decides
the gate; a veto fail from the decider still blocks as before. A maintainer
tuning slice budgets counts every fallback by check, criterion and reason,
with each slice's size, wherever the run journal is read.

The journeys that frame it, carried as the User Stories below: an agent whose
slice is over budget answers the criterion and moves on; an agent hit by a
provider outage answers fail, fixes its work and answers pass, never passing
a check nobody graded; a maintainer plots over-budget slice sizes and raises
a budget; a reviewer reads a run's log and sees that the agent, not the
decider, passed a bad change.

The boundary, carried as the Requirements and Out of Scope below: every
missing-verdict reason falls back, the answer is recorded and gates the
state, each fallback is one exported record under collision-free names, docs
describe it and older readers still read the log. Out: the routing decider,
budget and criteria changes, shirabe adoption, how downstream consumers of
the journal count it,
ledger and report counting, and the release.

## Problem Statement

A decider check grades the agent's own work. koto runs the check's extraction
command, sends the slice it prints to the decider with one closed question per
criterion, and reads back pass, fail or the criterion's escape. In veto mode a
fail blocks the state. Since koto#300, any outcome that isn't a verdict leaves
the gate passed: the criterion is listed under the gate's `output.unanswered`,
its `decider_checked` event records the reason with `blocked: false`, and the
state moves on.

The code has six reasons for a missing verdict. With `over_budget` and
`extraction_failed` nothing is sent. With `provider_error` and
`unreadable_response` the provider was asked and nothing usable came back.
With `cap_spent` and `busy` koto didn't ask. None of them is a judgment about
the work, and in all six nobody judges the criterion: the decider didn't, and
the agent is never asked. An oversized slice passes silently even under veto,
and a provider outage turns every opted-in run's checks into passes.

The people who tune slice budgets and criteria can't see this either. The run
journal relays `decider_checked` with its reason but without the slice's size,
and nothing marks the moments a criterion was handed on as a countable group.

## Goals

- No decider-check criterion passes because nobody judged it. When the
  decider gives no verdict, the agent doing the work answers the criterion,
  and its answer decides the gate.
- The decider's own verdicts keep their current effect: a veto fail blocks
  and is overridable, a pass, an escape and an empty slice never block, and a
  shadow verdict never blocks.
- A reader of a session's log can tell, per criterion, whether the decider
  judged it, the agent answered it, or it is still waiting on the agent.
- Every fallback is one exported run-journal record that names the check, the
  criterion, the reason and the slice size, so budgets and slices can be tuned
  by downstream consumers of the journal.
- Nothing changes for a template that declares no decider check, for a user
  who isn't opted in to the decider, or for the routing decider.


## User Stories

- As an agent running a workflow whose decider check went over budget, I want
  to be told which criterion is waiting on me, with its question and what
  pass, fail and the escape mean, so that I can judge my own work and move on
  without an override.
- As an agent whose veto check hit a provider outage, I want my fail answer to
  hold the state until I fix the work, and my later pass on the changed work
  to release it, so that the outage never lets a failing change through.
- As an agent whose check runs in shadow mode, I want a criterion the decider
  couldn't grade to come to me just as it would in veto, so that shadow mode
  never means a criterion passes with nobody judging it.
- As a template author who declares a decider check, I want the fallback to
  show the agent my criterion's own question and descriptions, so that the
  text I already write for the decider is all the agent needs.
- As a maintainer tuning a check's slice budget, I want to count fallbacks by
  check, criterion and reason from the run journal and see each fallback's
  slice size,
  so that I can tell a budget that's too tight from a provider that keeps
  failing.
- As a pull request reviewer auditing why a bad change passed a veto check, I
  want the session log to show that the decider gave no verdict and the agent
  answered pass, so that I raise the miss with the right owner.

## Definitions

These terms carry the requirements. The DESIGN owns every shape beyond what
is pinned here, and must use these names.

- **Slice**: the redacted output of the check's extraction command. Two
  slices are the **same slice** when the SHA-256 koto records for them
  (`input_sha256`) is equal. A failed extraction has no slice; two absent
  slices count as the same slice.
- **Visit**: koto's existing visit, from an arrival in a state until the
  session next arrives in another state, is rewound, reaches a terminal or is
  cancelled. A transition from a state to itself stays in the visit.
- **Effective mode**: a criterion's existing per-criterion mode: `veto` only
  when the template says `veto` and the user's effective decider mode is
  `auto`, otherwise `shadow`.
- **Verdict**: a decider answer of pass, fail or the escape (`unclear`).
- **Fallback**: one evaluation of one criterion that got no verdict and has no
  applicable agent answer.
- **Waiting criterion**: a criterion whose latest evaluation in the visit was
  a fallback.
- **Agent answer**: one `pass`, `fail` or `unclear` the agent recorded for a
  criterion with `koto decider answer`.
- **Applicable agent answer**: for one evaluation, the latest agent answer
  recorded in the current visit for the same state, check, criterion,
  declaration hash and slice. An answer recorded in an earlier visit, or for
  another slice, never applies.
- **Unreadable log**: the session log can't be read when the check is
  evaluated, or it names no current state.

Pinned names: the answer command is `koto decider answer <session> --gate
<check> --criterion <criterion id> --answer <pass|fail|unclear>`; the
session-log events are `decider_fallback` and `decider_check_answered`; the
gate output's new lists are `awaiting` and `agent_answered`; the run-journal
record kind is `decider_fallback`.

## Requirements

### Functional

**R1. Every missing verdict falls back.** In one evaluation, a criterion that
gets no verdict, for any `UnansweredReason` koto has (today `over_budget`,
`extraction_failed`, `provider_error`, `unreadable_response`, `cap_spent` and
`busy`), and has no applicable agent answer, is a fallback, in veto and in
shadow alike. A reason added later falls back without further change: the
mapping from reason to fallback has no exceptions.

**R2. A waiting criterion never passes.** While any criterion of a check is
waiting, the check's gate is not `passed` and the state doesn't advance,
whether or not it accepts evidence; evidence the agent submits is handled
exactly as for a decider check blocking on a veto fail today. Unless a
criterion also failed, that evaluation's outcome is `pending`, in veto and in
shadow: it judged nothing yet, so it stamps no attempt and writes no rule
counts, as a polling gate's pending evaluation doesn't. A waiting criterion
produces a finding at level `warning`, never at level `error`. The one way
past a waiting criterion other than answering is `koto overrides record`
(R9).

**R3. What the agent is told.** For each waiting criterion, the response that
stops the state carries one finding naming the check, the criterion's id, the
reason the decider gave no verdict, the criterion's question, the template's
descriptions of pass, fail and `unclear`, and the `koto decider answer`
command line for that check and criterion. Neither the response nor the
finding contains the slice.

**R4. How the agent answers.** `koto decider answer` records one agent answer
for one criterion of one check in the session's current state. It succeeds
only when that criterion had at least one fallback in the current visit. It
reads the latest such fallback and binds the answer to that fallback's slice
and declaration hash. The agent may answer again; the latest answer wins. It
refuses, with a typed error, a documented code and no event appended: a gate
that isn't a decider check of the current state, or a criterion it doesn't
declare (`unknown_criterion`); a criterion with no fallback in the current
visit (`no_fallback`); and an answer other than `pass`, `fail` or `unclear`
(`invalid_answer`). It never consults the decider and never takes the
decider lock: an evaluation already running doesn't see the answer, and the
next one does.

**R5. The answer is recorded.** Each agent answer appends one
`decider_check_answered` event naming the state, the visit, the check, the
criterion, the declaration hash, the slice's hash (absent with no slice), the
answer, and the sequence number of the fallback it answers.

**R6. The answer decides the gate.** When an evaluation finds an applicable
agent answer for a criterion that has no reusable decider verdict, the answer
is the criterion's outcome, and the decider isn't consulted, nothing is
appended for that criterion (no `decider_checked`, no fallback event, no
ledger line), and the criterion is listed under `agent_answered`. `pass` and
`unclear` don't block. In veto, `fail` blocks the state with a finding at
level `error` that says the agent answered fail and quotes the criterion's
fail description. In shadow, `fail` is recorded under `agent_answered` and
doesn't block, as a shadow decider fail doesn't. A veto fail stops blocking
when the agent answers again with `pass` or `unclear`, when the slice
changes (the decider is then consulted again, and another missing verdict
falls back again), or when an override is recorded.

**R7. Decider verdicts are unchanged.** A veto fail still blocks, reaches the
agent as a finding, and can be overridden. A decider pass or escape, an empty
slice (`not_graded`), and every shadow verdict still never block. A reusable
decider verdict for the same slice is still reused within the visit, ahead of
any agent answer. A consulted criterion still appends its `decider_checked`
event and ledger `checked` line as today, an unanswered one still with
`blocked: false`.

**R8. The gate's output tells the three apart.** A decider check's gate
output keeps `failed`, `unanswered` and `error`, and always carries two more
lists of criterion ids in declaration order: `awaiting`, the waiting
criteria, and `agent_answered`, the criteria whose outcome this evaluation
took from an agent answer. `failed` lists every criterion that blocks on a
fail: a veto criterion the decider or the agent failed. `unanswered` keeps
meaning "got no verdict": the veto criteria with neither a decider verdict
nor an applicable agent answer, so an agent-answered criterion is listed
under `agent_answered` only. The gate is `failed` when `failed` is
non-empty, else `pending` when `awaiting` is non-empty, and `passed`
otherwise; a shadow check is therefore never `failed`. A template's existing `override_default`
for a decider check stays valid without the two new lists, and an override
value without them doesn't block.

**R9. Overrides.** `koto overrides record` still moves past a decider check
whose gate is `failed` or `pending`, waiting criteria included. A candidate
false fail is recorded only for a criterion the decider failed: never for one
listed in `agent_answered` or `awaiting`.

**R10. One event per fallback.** Each fallback appends exactly one
`decider_fallback` event, after that evaluation's `decider_checked` events and
before the gate's `gate_evaluated`, in criterion declaration order. It names
the state, the visit, the check, the criterion, the declaration hash, its
effective mode, the reason, the slice's hash, and the slice's size in bytes.
The hash and size are present for every reason except `extraction_failed`,
which has no slice; for `over_budget` the size is the full slice koto
captured.

**R11. The fallback is exported.** koto relays each committed
`decider_fallback` event to the run journal as one `decider_fallback` record,
the way it relays its other events, naming the check, the criterion, the
mode, the reason and the slice size. A decider check's `check_evaluated`
record, and the `check_pending` record of a decider check waiting on the
agent, carry the length of `awaiting` and of `agent_answered`, including 0.
`decider_checked` records gain the slice size too. The agent answer event is
not relayed.

**R12. Names.** The DESIGN names the exact attributes. Each is
`koto.`-prefixed. An attribute this feature reuses keeps its name, its JSON
type and its documented meaning in the run-journal reference. For every new
attribute N and every other attribute M the run journal writes in any record
kind, N differs from M, N doesn't start with M followed by `.`, and M
doesn't start with N followed by `.`.

**R13. An unreadable log never passes.** A decider check with an unreadable
log is `pending`, lists every criterion under `awaiting`, keeps
`output.error: log_unreadable`, and appends nothing; its one finding, at
level `warning` and named for the check, says the session log couldn't be
read. With no readable log the agent can't answer
either, so the state stays stopped until the log is repaired or an override
is recorded.

**R14. The routing decider is unchanged.** The routing decider's
consultation and its existing fallback to the agent behave exactly as before.

**R15. Old logs aren't reinterpreted.** Events an earlier koto wrote keep
their meaning. A session that passed a check on a missing verdict before the
upgrade stays past it, and an `unanswered` `decider_checked` from before the
upgrade isn't treated as a fallback.

### Non-functional

**R16. Unchanged where nothing is declared.** A template that declares no
decider check compiles to the same JSON and hash as before, and its sessions
log the same events and outputs. A user who isn't opted in still sees a
template without its checks.

**R17. Older readers.** koto v0.14.1 reads a session log holding both new
events with `koto status` and `koto next`, without an error, and
`koto template validate-feed` from this koto accepts it. The proof is the
`decider-checks-compat-v0-14-1` CI job, whose self-test fails when either new
event is dropped from the log.

**R18. No content leaves the log.** The two new events and the new journal
record hold names, ids, hashes, counts and closed vocabularies only: never
the slice, a question, a description or anything the agent wrote beyond its
one-word answer. With every name at the journal's 128-byte name limit, a
`decider_fallback` record fits the journal's 4,096-byte line.

**R19. Docs.** Each of these describes the fallback, the answer command and
the new events, and none says a missing verdict passes: the run-journal
reference, the session-feed contract, the decider authoring guide, the
error-code reference (the three codes of R4), the CLI usage guide, the
workspace layout, the koto-author template reference, and the koto-user
skill with its command reference. The koto-user eval about a missing verdict
expects the agent to run `koto decider answer`. The decider checks' PRD and
design each get a note that this PRD supersedes their missing-verdict-passes
rule.

## Acceptance Criteria

- [ ] For each of the six reasons, in veto and in shadow, a criterion with no
      verdict leaves its gate `pending` with the criterion under `awaiting`,
      the state stopped, and one `warning` finding that contains the
      criterion id, the reason, the question, the three descriptions and the
      line `koto decider answer <session> --gate <check> --criterion <id>`;
      neither the response nor the log holds the slice. A test covers each
      reason in each mode.
- [ ] A test that enumerates every `UnansweredReason` value asserts each one
      produces a fallback.
- [ ] A state that accepts evidence, with a waiting criterion, doesn't advance
      on submitted evidence matching a transition.
- [ ] A waiting check's `gate_evaluated` (outcome `pending`) has no attempt
      stamp and no `rule_counts`, and the journal writes a `check_pending`
      record for it, with no `check_evaluated` or `check_rule`.
- [ ] Answering `pass`, then evaluating the same slice, passes the gate, lists
      the criterion under `agent_answered`, makes no provider call, and
      appends no `decider_checked`, `decider_fallback` or ledger line for it;
      the same holds for `unclear`.
- [ ] Answering `fail`, then evaluating the same slice, blocks in veto with an
      `error` finding that quotes the fail description, listing the
      criterion under `failed` and `agent_answered`; in shadow the gate is
      `passed` with the criterion under `agent_answered` only.
- [ ] An agent-answered criterion is never listed under `unanswered`.
- [ ] After a `fail` answer, answering `pass` for the same criterion passes
      the gate on the next evaluation of the same slice.
- [ ] After any answer, a changed slice consults the decider again; a verdict
      applies by its mode, and another missing verdict falls back with a new
      `decider_fallback` event.
- [ ] An answer recorded for one slice doesn't apply to an evaluation of
      another slice, and an answer from an earlier visit doesn't apply after
      the state is left and re-entered.
- [ ] An answer stops applying once the criterion's declaration hash changes,
      and a reusable decider verdict for a slice is used ahead of an agent
      answer recorded for the same slice.
- [ ] An answer recorded while another `koto next` holds the decider lock is
      appended at once and applies to the next evaluation.
- [ ] With `extraction_failed`, an answer applies to later evaluations in the
      visit while extraction keeps failing.
- [ ] A check with three criteria, one failed by the decider in veto, one
      answered `pass` by the agent and one waiting, reports `failed: [first]`,
      `agent_answered: [second]`, `awaiting: [third]`, outcome `failed`, an
      `error` finding for
      the first and a `warning` finding for the third; answering only the
      third with `pass` leaves the gate blocked on the first.
- [ ] `koto decider answer` refuses an unknown check or criterion
      (`unknown_criterion`), a criterion with no fallback in the current visit
      (`no_fallback`) and an answer outside `pass`, `fail` and `unclear`
      (`invalid_answer`), each with exit code 2, its code in the JSON error,
      and no event appended; each code is in the error-code reference.
- [ ] A recorded answer appends one `decider_check_answered` event with the
      state, visit, check, criterion, declaration hash, slice hash, answer and
      fallback sequence number; a test pins every field.
- [ ] A veto fail from the decider still blocks, becomes a finding and can be
      overridden with a candidate false fail recorded for it.
- [ ] An override recorded on a gate with an agent-answered fail, or with a
      waiting criterion, moves the state past the gate and records no
      candidate false fail for those criteria.
- [ ] A template whose decider check declares an `override_default` holding
      only `failed`, `unanswered` and `error` still compiles, and an override
      with that value moves past a waiting criterion.
- [ ] Each fallback appends exactly one `decider_fallback` event, after the
      evaluation's `decider_checked` events and before its `gate_evaluated`;
      the event has the slice hash and size for every reason but
      `extraction_failed`, where both are absent, and for `over_budget` the
      size equals the captured slice's length.
- [ ] The run journal writes exactly one `decider_fallback` record per
      fallback event with the attributes the DESIGN names, and a test pins
      each attribute's value; `check_evaluated` for a decider check, and
      `check_pending` for a waiting one, carry both new counts, 0 included,
      and `decider_checked` carries the slice size.
- [ ] A test collects every attribute name the run journal writes across all
      record kinds and fails if a new name equals another or either is the
      other followed by `.`.
- [ ] A decider check with an unreadable log is `pending`, lists every
      criterion under `awaiting`, has `output.error: log_unreadable` and
      appends no `decider_checked` or `decider_fallback`.
- [ ] A session log that holds an `unanswered` `decider_checked` and a passed
      gate written before the upgrade replays to the same state, and the
      next evaluation of that check consults the decider rather than reading
      the old event as a fallback.
- [ ] The routing decider's existing tests pass without edits.
- [ ] `tests/compat_baseline_test.rs` passes without edits.
- [ ] The `decider-checks-compat-v0-14-1` job reads a session that fell back,
      was answered with `koto decider answer`, and moved on; koto v0.14.1 runs
      `koto status` and `koto next` on it without error, and the job's
      self-test fails when the `decider_fallback` or `decider_check_answered`
      event is dropped.
- [ ] Every key the two new events write is declared in the session-feed
      contract, and `koto template validate-feed` accepts a log holding them.
- [ ] No new event or journal record holds the slice, a question or a
      description: a test feeds a slice, question and descriptions with
      unique marker strings and asserts none appears in the log or journal.
- [ ] A `decider_fallback` record built from 128-byte state, check and
      criterion names is at most 4,096 bytes.
- [ ] Each document R19 lists names `koto decider answer` and
      `decider_fallback`; every line in them matching `missing verdict` or
      `unanswered` is read, and none says such a criterion passes or doesn't
      block; and
      `tests/doc_names.rs` and `tests/run_journal_doc_test.rs` pass.
- [ ] The koto-user eval about a missing verdict asserts that the response
      runs `koto decider answer` for the waiting criterion, and the repo's
      eval shape check passes. The full eval run belongs to the release's
      eval precondition, not to this change.
- [ ] The pull request lists every existing test it edits and the old rule
      each one pinned.

## Out of Scope

- The routing decider's fallback, which already works (R14).
- Changing any check's slice budget, criteria or thresholds. That's the
  slice-contract work, which needs this fallback first.
- shirabe adopting the fallback in its templates. It needs a koto release and
  a floor raise before it can use the answer command.
- How downstream consumers of the journal admit and count the new record
  and attributes. They own that; this PRD keeps the journal's existing
  counting rules true (see Known Limitations).
- Counting fallbacks or agent answers in the decider ledger or `koto decider
  report`. The report keeps counting no-verdicts per criterion from the
  ledger, which is unchanged; the run journal carries this feature's new
  telemetry.
- Relaying the agent's answer as a run-journal record of its own. The gate's
  `check_evaluated` (its `agent_answered` count) and `check_rule` records
  show its effect.
- Cutting a koto release.

## Known Limitations

- **Gate counting stays as it was.** Since koto#300 a decider check's gate
  is `passed` or `failed`, and a shadow check's is always `passed`. With
  this feature a waiting check logs `pending`, which judged nothing and
  writes a `check_pending` journal record like a polling gate's wait, and a
  shadow check is still never `failed`. The new case is an agent `fail` in
  veto, which logs `failed` with the criterion as its rule, like a decider
  veto fail.
- **The agent grades its own work.** An agent answer is self-assessment, and
  the 2026-10-14 ruling accepts that: it's what the agent did before a
  decider existed, and the answer is recorded where a reviewer can see it.
- **An escape answer passes.** `unclear` doesn't block, as a decider escape
  doesn't. It's recorded and visible, so it isn't silent, but nothing checks
  it.
- **No slice, no change detection.** With `extraction_failed` there is no
  slice to hash, so an answer stands for the rest of the visit while
  extraction keeps failing, even if the work changes. The agent can answer
  again to change it.
- **An agent that can't answer stays stopped.** A headless agent that doesn't
  know the answer command, or an unreadable log, leaves the state stopped
  until someone answers or records an override.

## Decisions and Trade-offs

- **Shadow mode falls back too.** Alternatives: fall back only in veto, or
  only for veto criteria withheld from the agent. Chosen: both modes, per the
  2026-10-14 ruling. Shadow means koto doesn't act on the decider's verdict;
  it never meant a criterion nobody judged should pass.
- **An agent fail blocks only in veto.** Alternative: block on a shadow
  agent fail too. Chosen: shadow never blocks on a fail and its gate is never
  `failed`, whoever answered, so a shadow check keeps meaning "recorded, not
  acted on" and its gate's counting doesn't change.
- **A waiting check is `pending`, not `failed`.** Alternative: `failed`,
  which renders a corrective blocked gate. `pending` says what happened (the
  check judged nothing yet), stamps no attempt and writes no rule, so the
  journal's counting rules stay true while the state still stops.
- **The answer is its own command and event.** Alternatives: route the answer
  through the state's evidence, or through `koto overrides record`. Evidence
  belongs to the template's schema and would need a reserved key; an override
  skips the whole gate and logs a candidate false fail. A dedicated command
  records one criterion's answer and nothing else.
- **An answer binds to the slice, not to one fallback.** Alternative: bind to
  the fallback event, so every new fallback needs a new answer. Each
  re-evaluation of a slice the provider still can't grade would then ask
  again for something already answered. Binding to the slice asks once per
  slice per visit.
- **A waiting criterion is a `warning`, not an `error` finding.**
  Alternative: the `error`-level finding koto wrote before koto#300. That
  counted every checker fault as a rule violation, which koto#297 ruled
  against and which still holds.
