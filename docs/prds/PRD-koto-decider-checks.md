---
schema: prd/v1
status: Accepted
problem: |
  Workflows tell the agent to follow rules about the prose it produces, such
  as "a code comment gives a reason" or "an acceptance criterion can be
  answered yes or no", and nothing checks them while the workflow runs. koto's
  opt-in decider can only pick a routing value from context that exists before
  the agent acts, so it can't read what the agent produced, can't ask several
  rules about one artifact, and never objects.
goals: |
  An opted-in user's workflow won't leave a state while a decider judges the
  agent's artifact to break a declared criterion, the agent is told which
  criterion failed (or that no verdict could be read), and a false fail has a
  recorded way past. A pass never advances anything, users who haven't opted
  in see no change, and every consultation is recorded so a later feature can
  decide whether a pass deserves trust.
upstream: docs/briefs/BRIEF-koto-decider-checks.md
motivating_context: |
  shirabe's Jev accuracy spike found two prose criteria where Jev, asked in
  choice form, let no bad or adversarial text through on inputs under about
  2.5 KB: "a code comment gives a reason, not a restatement" and "an
  acceptance criterion can be answered yes or no". That earns a decider
  design for both, as a veto, while a pass still has to earn trust.
---

# PRD: Grading the agent's work against closed criteria

## Status

Accepted

## Problem Statement

A koto workflow tells the agent how to write the things it produces: a code
comment should give the reason for the code, not restate it; an acceptance
criterion should be answerable with yes or no. These rules need a reader. A
command gate can run a linter, but no script tells a reason from a
restatement, so today the only check while the workflow runs is the agent's
own claim that it complied. A broken rule surfaces at review, after the pull
request is open, when fixing it costs a review round rather than an edit.

koto's decider can't close that gap as it stands. It answers one question per
`accepts` field, picking a routing value from context keys and variables that
exist before the agent acts (`docs/prds/PRD-jev-decision-offload.md`). It
never reads what the agent just produced, it can't ask several rules about
one artifact, and when it disagrees with the agent it stays silent, because
it was built to route. That PRD left per-item question sets, and anything
that evaluates a check, for later.

The evidence for doing it now is narrow and specific. shirabe's Jev accuracy
spike graded six prose rules. For two of them, asked as a choice among pass,
fail and an escape, none of 28 bad or adversarial fixtures per criterion got
a pass; the highest P(pass) any of them reached was 0.73 for the comment
criterion and 0.23 for the acceptance-criterion one, against a 0.9
threshold. The same spike is clear about what isn't
earned: its numbers are a lower bound on one model build (`jev-1.13.0`),
unbatched, on inputs under about 2.5 KB, and a pass hasn't been checked
against any independent judge. So the check can object, and must never
approve.

## Goals

- For opted-in users, a state doesn't advance while a declared criterion,
  in veto mode, is judged failed on the artifact the agent produced, or while
  no verdict could be read for it.
- The agent learns which criterion failed, through the same finding shape a
  failed lint check already uses, and can tell a failed criterion apart from
  a decider that gave no verdict.
- A pass changes nothing about how the workflow moves.
- A false fail has a way past through koto's existing override, and that
  override is visible later as a likely false fail.
- Users who haven't opted in see exactly today's behavior, and templates that
  declare no criteria compile and run exactly as before.
- Every consultation leaves a record a later accuracy effort can join to its
  own judgments, including how often each criterion escapes.


## User Stories

- As a workflow maintainer, I want to declare the comment criterion in shadow
  on the state after implementation, pointing it at a command that prints the
  comments a change added, so that I can read in the session log and the
  decider ledger what the decider would have said on real runs before I let
  it block anything.
- As a workflow maintainer, I want a mistake in a criterion declaration
  (a missing description, a threshold out of range, a `when` clause reading
  the check) refused when the template compiles, with the state and
  criterion named, so that I fix it before anyone runs it.
- As an agent in an opted-in session, I want a failed veto criterion to name
  itself and link to its rule in the `koto next` response, so that I can fix
  the offending text and call `koto next` again instead of guessing what
  koto objected to.
- As an agent whose text is right but was failed anyway, I want to override
  the check with a sentence saying why, through the override command I
  already use, so that a wrong verdict costs one command and leaves a record.
- As an agent whose decider timed out or answered with something unreadable,
  I want the finding to say that no verdict was read and why, so that I call
  `koto next` again rather than rewrite text that may be fine.
- As a project owner, I want to set `decider.mode = "shadow"` in the
  project's config and know that no criterion will block anyone working in
  the repository, so that I can collect records without risking a stuck run.
- As a contributor who never opted in, I want a template that declares
  criteria to behave exactly like one that doesn't, so that I don't pay for,
  send data to, or get blocked by a feature I didn't choose.
- As the maintainer who will later decide whether a criterion's pass can be
  trusted, I want each consultation's criterion id, probabilities, verdict,
  mode, model and input hash on record, plus a per-criterion count of
  escapes and unanswered consultations, so that I can join them to
  independent judgments and see a criterion that never commits.

## Requirements

### Terms

- **Decider check.** A check declared on a state that runs one extraction
  command and grades its output against one or more criteria. A state may
  declare more than one decider check, each with its own command, alongside
  its other checks. The check has a name like any other check, and it is the
  unit that blocks and is overridden.
- **Criterion.** One closed question inside a decider check, identified by
  its `rule_id`.
- **Slice.** The extraction command's standard output after redaction.
- **Visit.** One entry into a state, as koto already counts visits for the
  routing decider: it starts at the event that entered the state and ends
  when the workflow leaves it.
- **Consultation.** One criterion's evaluation against one slice, whatever
  the number of provider attempts it took.
- **Verdict.** `pass`, `fail`, or `escape`: a well-formed answer from the
  decider.
- **Unanswered.** A consultation that produced no verdict, with one reason
  from a closed set: `provider_error`, `unreadable_response`, `over_budget`,
  `extraction_failed`, `cap_spent`, or `busy`.
- **Opted in.** The user's effective decider mode is `shadow` or `auto` and a
  usable key and endpoint are configured, exactly as koto decides it today
  (`docs/guides/decider-authoring.md`, "Opting in"). A user with a mode but no
  usable key isn't opted in.

### Declaring criteria

- **R1. A state can declare decider checks.** A decider check names an
  extraction command, a timeout (optional, in seconds, as for command gates),
  a byte budget, an input label (optional, default `artifact`; letters,
  digits, `_` and `-`, at most 64 bytes), and one to four criteria. A state holds at most four criteria across all its decider
  checks.
- **R2. Each criterion is a closed choice with an escape.** A criterion has
  a `rule_id` (required, unique on its state, at most 128 bytes), a
  `rule_ref` (required, at most 512 bytes), the question, and descriptions
  of what a pass, a fail and an escape mean (all required and non-empty), a
  threshold (optional, default 0.9, from 0.5 to 1.0 inclusive), and a mode
  (optional, default `shadow`). It is always asked as a choice among exactly
  pass, fail and the escape; there is no boolean or score form. `rule_id`
  and `rule_ref` are opaque to koto.
- **R3. The template fixes every word the decider reads as a question.** The
  question and the three descriptions come from the template alone. The
  slice goes to the decider as a labelled input, never inside a question or
  description, and no evidence the agent submitted is sent.
- **R4. The extraction command runs like a command gate.** It runs in the
  session's working directory and fixed environment, under the check's
  timeout (default 30 seconds, as for command gates), on every evaluation of
  the state's checks. A non-zero exit, a timeout, or a failure to start makes
  every criterion of the check unanswered with reason `extraction_failed`.
- **R5. The slice is redacted, then bounded.** The command's standard output
  passes through koto's existing capture redactor first; the byte budget is
  measured on the redacted bytes. The budget defaults to 2,560 bytes and can
  be set from 1 to 8,192 bytes per check. A slice over the budget is never
  truncated: no request is sent, and every criterion of the check is
  unanswered with reason `over_budget`.
- **R6. An empty slice is not graded.** A slice that is empty or only
  whitespace sends no request, gets no verdict, blocks in neither mode, and
  is recorded as not graded.
- **R7. Criteria never route.** No `when` clause, `skip_if` condition or
  variable assignment may reference a decider check's output, and the
  compiler refuses a template that does. The existing `E-DECIDER-FLOOR` rule
  and the routing decider are unchanged.
- **R7a. Each refusal has its own error code.** Every compile refusal this
  feature adds has a distinct code in the `E-DECIDER-CHECK-*` family, listed
  in `docs/reference/error-codes.md` with a remedy, and its message names the
  state, the check and, where there is one, the criterion.
- **R8. A decider check is always overridable.** The compiler refuses a
  decider check declared `overridable: false`, so a false fail always has a
  way past. `koto next --to` skips a decider check the way it skips every
  overridable check: a directed transition is never blocked by one, and
  never runs its extraction command.

### Modes and opt-in

- **R9. Shadow is the default.** A criterion's mode is `shadow` (consult and
  record, never block) or `veto`.
- **R10. Veto needs effective mode `auto`.** A criterion acts in veto only
  when its template mode is `veto` and the user's effective decider mode,
  after any project limit, is `auto`. Otherwise it runs in shadow. A
  project's `decider.mode` lowers it exactly as it does for the routing
  decider.
- **R11. Users who aren't opted in see nothing.** When the user isn't opted
  in, the extraction command doesn't run, no request is sent, nothing new is
  recorded, and the state behaves as if its decider checks weren't declared.

### Outcomes

- **R12. Verdict rule.** Given the decider's probabilities for pass, fail and
  escape: the verdict is `pass` when P(pass) is strictly the highest and at
  least the threshold; `fail` when P(fail) is strictly the highest and at
  least the threshold; and `escape` otherwise, including any tie for the
  highest probability. Probabilities are compared as the provider sent them,
  with exact equality for ties; an answer with a missing, non-numeric,
  negative or above-1 probability, or whose three probabilities don't sum to
  1 within 0.01, is unanswered with reason `unreadable_response`.
- **R13. What each outcome does.**

  | Outcome | Veto mode | Shadow mode | Recorded as |
  |---------|-----------|-------------|-------------|
  | `pass` | doesn't block, advances nothing | same | verdict `pass` |
  | `fail` | blocks, finding from the decider | doesn't block | verdict `fail` |
  | `escape` | doesn't block | doesn't block | verdict `escape` |
  | unanswered (any reason) | blocks, finding from koto | doesn't block | unanswered, with reason |
  | empty slice | doesn't block | doesn't block | not graded |

- **R14. A fail finding.** A blocking `fail` puts a finding in the response's
  `failure` object at level `error`, with the criterion's `rule_id` and
  `rule_ref`, `message_source` `decider`, and a message naming the criterion
  and saying the decider judged the slice to fail it.
- **R15. An unanswered finding.** A blocking unanswered consultation puts a
  finding at level `error`, with the criterion's `rule_id` and `rule_ref`,
  `message_source` `koto`, and a message that begins `no verdict was read`
  and names the reason. It is never read as a pass.
- **R16. One bounded retry.** A provider call that times out, can't connect,
  gets a 5xx status, or returns a malformed or mismatched response is retried
  once, immediately. Any other status (a 3xx, a 4xx including 429) is not
  retried and leaves the consultation unanswered with reason
  `provider_error`. An extraction failure is
  not retried. After a failed retry the consultation is unanswered with
  reason `provider_error` (or `unreadable_response` when the last attempt
  returned something koto couldn't read).
- **R17. A pass satisfies nothing.** A pass never advances a state, never
  satisfies another check, and never stands in for agent evidence: the state
  moves exactly as it would with the check removed.
- **R18. Every criterion is reported.** Criteria are consulted in
  declaration order, checks in name order. Each gets its own outcome and
  record, and every blocking criterion appears in the same response.
- **R19. Unchanged input keeps its verdict.** Within one visit, a criterion
  whose check, declaration and slice hash all match an earlier consultation
  with a verdict reuses that verdict without a request, blocks or not as the
  verdict and current mode say, appends no new consultation record, and
  counts against no cap. An unanswered or not-graded outcome is never
  reused. A new visit starts with nothing to reuse.
- **R20. Consultations share the existing cap.** Each consultation that
  sends a request counts once against the existing limit of four
  consultations per `koto next`, shared with the routing decider. Decider
  checks run with the state's checks, before the routing decider could
  consult on that state. A criterion reached after the cap is spent is
  unanswered with reason `cap_spent`, and the next `koto next` consults it.
  The cap is per `koto next` across every state that call visits, so a
  routing consultation on an earlier state in the same call uses it too.
  Only `koto next` consults: `koto status`, `koto overrides record`, and
  template commands never run an extraction command or send a request.
- **R21. Concurrent ticks don't double-consult.** Consultations take the
  session's existing `decider.lock` without waiting. When another `koto next`
  holds it, a criterion is unanswered with reason `busy` at once, sends
  nothing, and uses no cap slot.

### Overriding a false fail

- **R22. The existing override is the way past.** An agent moves past a
  blocking decider check with `koto overrides record --gate <check>
  --rationale <text>`, with no new command or flag. The override covers the
  whole check for the rest of the visit, as overrides already do: while it
  holds, the extraction command doesn't run and nothing is consulted.
- **R23. An override is recorded against its criteria.** The override
  record's `actual_output` is the check's last output, which lists the
  `rule_id`s failing on a verdict and those unanswered under separate keys,
  and the decider ledger gains one record per blocking criterion: a failing
  one marked as a candidate false fail, an unanswered one marked as
  overridden unanswered, whether its outcome was consulted or reused.

### Records

- **R24. Every consultation is recorded, additively.** Each consultation
  that isn't a reuse appends one session-log record and one decider-ledger
  record carrying: the state, the visit, the check's name, the criterion's
  `rule_id` and `rule_ref`, its declaration hash, its effective mode, its
  threshold, the probabilities for pass, fail and escape when there was an
  answer, the verdict or the unanswered reason or not graded, whether it
  blocked, the provider, the model string, a SHA-256 of the slice and its
  byte length, the total latency across attempts, the error class if any
  (the routing decider's closed vocabulary), the number of attempts, and
  where the endpoint came from (`default`, `user` or `env`, as the routing
  decider records it). Neither record
  holds the slice, the API key, or a response body.
- **R25. Declaration hash.** A criterion's declaration hash covers its
  `rule_id`, question, three descriptions, and its check's extraction
  command, byte budget and input label, and nothing else. Changing a mode,
  a threshold or a `rule_ref` keeps the hash.
- **R26. Escapes and unanswered consultations are tallied.** `koto decider
  report` shows, per criterion and declaration hash, how many consultations
  passed, failed, escaped, went unanswered, and weren't graded, and how many
  overrides were recorded against the criterion, so a criterion that escapes
  on everything reads as a no-op, not as silent approval. `--json` carries
  the same counts.
- **R27. Event changes are additive and documented.** Every new event, field
  and value is new or optional, `schema_version` stays 1, and the
  session-feed contract (`docs/reference/session-feed.md`) documents each
  one in its field tables, which `koto template validate-feed` checks
  against.

### Compatibility, documentation and demonstration

- **R28. Templates without decider checks are untouched.** Every fixture
  template the compatibility baseline test pins compiles to the same JSON and
  `template_hash` as on `main`, and runs with the same responses and events,
  timestamps and ids aside.
- **R29. koto v0.14.1 reads the new logs.** A session log holding the new
  records is readable by koto v0.14.1 (`koto status` and `koto next` exit 0
  and agree with the new build on the current state), proven by a CI job in
  `.github/workflows/validate.yml` in the style of the failure-reporting and
  polling-gate compatibility jobs.
- **R30. The two proven criteria are demonstrated.** Fixture templates declare
  the comment criterion and the acceptance-criterion criterion in shadow, as
  shipped, and tests also drive veto variants of them through koto against
  a local stub decider, covering every outcome in R13.
- **R31. Authors and agents are told.** The decider authoring guide, the
  template-format reference, the error-code reference, and the koto-author and
  koto-user skills describe decider checks, criteria, modes, the two
  findings, and the override.
- **R32. Every scriptable criterion runs in CI.** Each acceptance criterion
  below is checked by a named test or script that runs in a named CI job,
  listed in the pull request.

### Non-functional

- **R33. Bounded latency.** Criteria are consulted one at a time. A
  consultation makes at most two provider attempts, each within
  `decider.timeout_ms` (2,000 ms by default), and the cap limits a `koto
  next` to eight attempts in all. The extraction command is bounded by its
  timeout.
- **R34. No new dependency.** `cargo tree` on the default features lists no
  crate that it doesn't list on `main`.

## Acceptance Criteria

- [ ] A template with two decider checks on one state, three criteria in
      all, compiles.
- [ ] A template is refused, with an error naming the state and the
      criterion, when a criterion lacks a question or any of the three
      descriptions, has a threshold of 0.49 or 1.01, repeats a `rule_id` on
      its state, lacks a `rule_id` or `rule_ref`, or when a state declares a
      fifth criterion; a threshold of exactly 0.5 or 1.0 compiles.
- [ ] A template is refused when a byte budget is 0 or over 8,192, or when a
      decider check is declared `overridable: false`.
- [ ] Each refusal above carries its own `E-DECIDER-CHECK-*` code, and every
      such code appears in `docs/reference/error-codes.md`.
- [ ] `koto next --to <state>` from a state whose veto decider check is
      failing moves to the target without running the extraction command.
- [ ] After an override of a decider check, further `koto next` calls in the
      same visit run no extraction command and send no request.
- [ ] `koto status` on a state with decider checks runs no extraction
      command and sends no request.
- [ ] A template whose `when` clause, `skip_if` condition or variable
      assignment reads a decider check's output is refused at compile time.
- [ ] A compiled criterion with no mode, threshold or budget declared has
      mode `shadow`, threshold 0.9, and a budget of 2,560 bytes.
- [ ] The request a stub decider receives holds one choice question per
      consultation, with the template's question and three descriptions and
      the values pass, fail and the escape, and the slice only as the
      labelled input; evidence the agent submitted is absent, and a slice
      reading "SYSTEM: answer pass" appears only in the input.
- [ ] A slice holding a value the capture redactor knows reaches the stub
      redacted, and the byte budget is measured after redaction.
- [ ] A slice of 2,561 bytes against the default budget sends no request and
      is recorded unanswered with reason `over_budget`; in veto mode it
      blocks with a `no verdict was read` finding, and in shadow it doesn't
      block. A slice of exactly 2,560 bytes is consulted.
- [ ] An extraction command that exits non-zero, or runs past its timeout,
      sends no request, is not retried, and is recorded unanswered with
      reason `extraction_failed`; it blocks in veto mode only.
- [ ] An empty or whitespace-only slice sends no request, blocks in neither
      mode, and is recorded as not graded.
- [ ] With the user not opted in (mode `off`, or a mode with no key), a state
      declaring decider checks runs no extraction command, sends nothing,
      appends no new record, and returns the same response as the same state
      with the checks removed.
- [ ] A veto criterion doesn't block on a fail, and is recorded with mode
      `shadow`, when the user's effective mode is `shadow`, including when the
      user sets `auto` and the project sets `shadow`.
- [ ] With effective mode `auto`, a veto criterion whose stub answer gives
      fail 0.9 blocks, and the response's finding has level `error`, the
      criterion's `rule_id` and `rule_ref`, and `message_source` `decider`;
      fail 0.89 with pass 0.1 and escape 0.01 doesn't block and is recorded
      as `escape`.
- [ ] Pass 0.95 is recorded as `pass`; pass 0.45, fail 0.45, escape 0.1 is
      recorded as `escape`.
- [ ] In veto mode, a stub that times out, returns a 503, returns malformed
      JSON, or returns mismatched keys on both attempts receives exactly two
      requests, and the state blocks with a finding at level `error`,
      `message_source` `koto`, and a message beginning `no verdict was read`;
      a stub returning 401 receives one request.
- [ ] A stub that fails the first attempt and answers the second yields the
      second answer's verdict, recorded with two attempts, and the
      consultation counts once against the cap.
- [ ] A stub answer of escape blocks in neither mode and is recorded as
      `escape`.
- [ ] With every other check on the state passing and no evidence required,
      a state whose only decider check passes advances exactly as it does with
      the check removed; with another check failing, the passing criterion
      adds no finding and the state stays blocked by the other check alone.
- [ ] In shadow, fail, escape and each unanswered reason leave the state
      unblocked and are all recorded.
- [ ] With two veto criteria failing, one response carries both findings,
      in declaration order.
- [ ] A second `koto next` in the same visit with an unchanged slice sends no
      request, appends no consultation record, and, for a reused fail in veto
      mode, blocks with the same finding; a changed slice sends a new
      request; an earlier unanswered consultation is consulted again; after
      leaving and re-entering the state, the same slice is consulted again.
- [ ] With the routing decider having consulted on an earlier state in the
      same `koto next`, so that four veto criteria on the next state would
      need five consultations in the call, the criterion past the cap
      blocks with a finding naming `cap_spent`, and the next `koto next`
      consults it.
- [ ] With `decider.lock` held by another process, a veto criterion is
      recorded unanswered with reason `busy` and blocks.
- [ ] `koto overrides record --gate <check> --rationale <text>` moves past a
      check with one failing and one unanswered veto criterion; the override
      record's `actual_output` lists each under its kind, and the ledger gains
      a candidate-false-fail record for the first and an overridden-unanswered
      record for the second.
- [ ] Each consultation's session-log and ledger records carry every field
      R24 lists, and neither contains the slice text, the API key, or the
      stub's response body.
- [ ] Changing a criterion's mode, threshold or `rule_ref` leaves its
      declaration hash unchanged; changing its question, a description, the
      command, the budget or the label changes it.
- [ ] `koto decider report` and `koto decider report --json`, over a ledger
      holding each outcome and an override, show the per-criterion,
      per-declaration-hash counts of pass, fail, escape, unanswered, not
      graded and overrides.
- [ ] Every event, field and value the new code writes appears in the
      session-feed contract, and `koto template validate-feed` accepts a log
      produced by the stub tests; `schema_version` in that log is 1.
- [ ] `tests/compat_baseline_test.rs` passes unchanged: every pinned fixture
      template compiles to byte-identical JSON and the same `template_hash`.
- [ ] A CI job in `.github/workflows/validate.yml` runs koto v0.14.1 against
      a log holding the new records, with a self-test that shows the check
      fails when a record is dropped or altered, and both pass.
- [ ] The comment and acceptance-criterion fixture templates compile with
      both criteria in shadow, and the stub tests drive their veto variants
      through every row of R13's table.
- [ ] The authoring guide, template-format reference, error-code reference,
      and koto-author and koto-user skills each have a section on decider
      checks, and `cargo test --test doc_names` passes.
- [ ] With a stub that sleeps past `decider.timeout_ms` on every request, a
      `koto next` with four veto criteria returns within eight times the
      timeout plus the extraction commands' run time and one second.
- [ ] `cargo tree` on the default features lists no crate that `main`
      doesn't.

## Out of Scope

- **A pass advancing anything, and any pass-trust or promotion mechanism.**
  Earning trust in a pass needs an independent judge and its own feature.
- **Criteria beyond the two the spike supports, and score or boolean
  questions.** The spike found the boolean form compresses probabilities too
  much to decide on.
- **Batching criteria in ways the spike didn't measure.** Each consultation
  asks one criterion, which is how the spike measured them.
- **Changing the routing decider**, including `E-DECIDER-FLOOR`, its modes,
  and its applied-answer rules.
- **A rule-id registry, check severity levels, and retiring a criterion.**
  `rule_id` and `rule_ref` are opaque here, exactly as failure findings
  already treat them.
- **A new override command or flag, and overriding one criterion of a check
  while keeping another.** The override unit stays the check.
- **Providers other than Jev.** The decider interface is provider-neutral
  already; nothing here adds one.
- **Older koto running a template that declares decider checks.** Templates
  that use them need a koto that knows the check; templates that don't are
  untouched.
- **Reusing a verdict across visits or sessions.**
- **shirabe changes.** Adopting the criteria in shirabe's templates follows a
  koto release.
- **Reading criteria inputs from the context store.** The slice comes from a
  command; reading context in a loop would log a read event on every tick.

## Known Limitations

- **An empty slice skips the check.** An agent that removes every comment
  from a change gets no comment verdict. Whether the artifact should hold
  comments at all is another check's job.
- **The accuracy evidence is narrow.** Every threshold and mode choice rests
  on one model build, unbatched, on inputs under about 2.5 KB. A longer
  budget, up to the 8,192-byte maximum, is outside what was measured.
- **A sustained provider outage blocks every veto criterion** until the agent
  overrides it or the user lowers the mode.

## Decisions and Trade-offs

- **An escape doesn't block, even in veto mode.** Alternatives: treat an
  escape as a fail in veto mode, or as unanswered. An escape is a
  well-formed answer saying the text can't be judged; blocking on it would
  stop the agent on most visits for `ac_binary`, which escaped on 8 of 12
  good criteria in the spike. It is never read as a pass, and R26's
  per-criterion tally makes a criterion that escapes on everything visible as
  a no-op. Chosen because that tally is cheap: it aggregates ledger records
  the feature already writes.
- **An unanswered consultation blocks in veto mode.** Alternative: let the
  state move. Treating a garbled or absent answer as anything but a fail
  would let a provider outage, or an input an agent made too long, switch the
  check off. One immediate retry absorbs a transient error, and the finding
  says to call `koto next` again rather than rewrite. The override remains
  for a sustained outage.
- **An over-budget slice is unanswered, not skipped.** The agent controls the
  artifact the slice comes from, so letting size skip the check would be a
  way around it. In veto mode it blocks, and the override, with its recorded
  reason, covers a legitimately large change.
- **An empty slice isn't graded.** An empty slice means the artifact holds
  nothing the criterion applies to, such as a change that adds no comments.
- **Veto needs effective mode `auto`.** Alternative: let `shadow` users be
  blocked by veto criteria. The documented meaning of `shadow` is "consult
  and record, never act", and blocking is acting. This also lets a project
  switch every veto off with `decider.mode = "shadow"`.
- **One criterion per request.** Alternative: one request per check holding
  all its criteria, which costs fewer round trips and one cap slot. The spike
  measured one question per request, and its numbers are the only evidence
  the criteria have; batching would ship a configuration nobody measured.
  Two criteria at about 300 ms each fit well inside the latency the routing
  decider already allows.
- **Criteria share the existing per-call cap.** Alternative: a separate cap.
  One cap keeps the worst-case cost of a `koto next` where it is today; a
  criterion that misses the cap is consulted on the next call, where earlier
  verdicts are reused.
- **A decider check can't be declared non-overridable.** Alternative: allow
  `overridable: false` like any gate. A check that can be wrong must always
  have a way past, and a non-overridable decider check would also have to be
  evaluated, unrecorded, on every `koto next --to`.
