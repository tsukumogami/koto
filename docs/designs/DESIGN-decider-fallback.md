---
schema: design/v1
status: Accepted
upstream: docs/prds/PRD-decider-fallback.md
problem: |
  koto's check evaluator turns every decider-check criterion that gets no
  verdict into a passing gate, so a criterion nobody judged lets the state
  advance, and nothing it writes to the run journal counts these fallbacks
  or records the slice sizes behind them.
decision: |
  A criterion with no verdict and no applicable agent answer becomes a
  fallback: the gate is failed with the criterion under a new
  output.awaiting list and a warning finding that carries the criterion's
  question, and the agent answers with `koto decider answer`, which appends
  a decider_check_answered event the next evaluation reads back by slice
  hash. Each fallback appends a decider_fallback event, which the run
  journal relays as a decider_fallback record with koto.state, koto.check,
  koto.rule.id, koto.decider.mode, koto.decider.reason and the new
  koto.decider.input_bytes.
rationale: |
  Findings already reach the agent on a blocked response, so the fallback
  needs no response-contract change, and a dedicated subcommand records one
  criterion's answer without touching the template's evidence schema or the
  override path. Looking the answer up before every unanswered path makes
  every reason fall back by construction, and binding it to the slice hash
  reuses the evaluator's existing reuse key. A distinct event is a direct
  count of fallbacks and gives the answer command something to cite, and
  every new attribute is a sibling of an existing leaf, so none collides.
decision_provenance: inline-resolved
---

# DESIGN: decider-check fallback to the agent

## Status

Accepted

## Context and Problem Statement

`CliCheckEvaluator::evaluate` (`src/cli/check_evaluator.rs`) grades a
`decider-check` gate one criterion at a time. Each criterion ends in a
`CheckOutcome`: a verdict (`Pass`, `Fail`, `Escape`), `NotGraded` for an
empty slice, or `Unanswered(UnansweredReason)` for the six faults in
`src/decider/check.rs`: `OverBudget` and `ExtractionFailed` before anything
is sent, `ProviderError` and `UnreadableResponse` after two attempts,
`CapSpent` when the shared per-call budget is gone, and `Busy` when the
decider lock stays held past the wait. `blocks(outcome, mode)` is true only
for a veto `Fail`, and `result_for` returns `passed` whenever no criterion
blocks, so every unanswered criterion leaves the gate passed. The engine
(`src/engine/advance.rs`) blocks on a decider check only when
`output.failed` is non-empty.

The PRD (`docs/prds/PRD-decider-fallback.md`) reverses the effect for
unanswered criteria: a criterion with no verdict falls back to the agent,
in veto and shadow, and the agent's answer decides the gate. The technical
problem has four parts:

1. The agent has to learn which criterion waits on it, and record an answer
   koto can read back, with no change to the template's evidence schema.
2. The evaluator has to find that answer on the next evaluation of the same
   slice, and hand the criterion to the agent for any reason it got no
   verdict, including reasons added later.
3. The gate's outcome, output and findings have to keep the state stopped
   while a criterion waits, let the agent's answer decide it, and keep the
   existing records and overrides meaning what they mean today.
4. Each fallback has to reach the run journal as one record that a Datadog
   pipeline can admit, under `koto.*` names that collide with nothing the
   journal already writes.

## Decision Drivers

- **Never pass on nobody's judgment** (PRD R1, R2). Every unanswered path,
  present and future, ends at the agent.
- **Decider verdicts unchanged** (R7). A veto fail blocks and is
  overridable; pass, escape, `not_graded` and every shadow verdict don't
  block; `decider_checked` and ledger lines are written as today.
- **No response-contract change.** `tests/compat_baseline_test.rs` lets a
  failing `koto next` response gain keys only; the agent-facing channel
  should be one koto already uses.
- **Existing templates stay valid** (R8, R16). The decider-check output
  schema in `gate_type_schema` is enforced on `override_default`, so new
  output keys can't join it.
- **Telemetry names admitted by Datadog** (R11, R12). `koto.*`-prefixed,
  reusing existing names where the meaning is the same, and no new name
  equal to or a dot-prefix of another. `koto.check` is already both a leaf
  and the parent of `koto.check.outcome`; the archive renames it on
  `check_evaluated`, and no new name may widen that.
- **Older readers** (R17). koto v0.14.1 must read logs holding the new
  events, which the untagged `EventPayload` deserializer allows when an
  unknown type degrades to `Unknown`.
- **No content leaves the log** (R18). Slices, questions and descriptions
  stay out of events and journal records.
- **Small engine surface.** The engine never sees a provider, the lock or
  the ledger today (decider-checks design, D6), and shouldn't start.

## Considered Options

### Decision 1: How the agent is asked and how it answers

Resolved inline.

The agent needs the criterion's question, its pass, fail and `unclear`
descriptions, the reason, and a way to record one of three answers.
koto already returns a blocked gate as a `blocking_conditions` entry whose
`failure.findings` the koto-user skill teaches agents to read, and it
already has one agent-run recording command, `koto overrides record`.

Key assumptions:

- The escape value is `unclear` for every criterion (`ESCAPE_VALUE`).
- An agent can run a koto subcommand named in a finding, as it already
  runs `koto overrides record`.

#### Chosen: warning findings and a `koto decider answer` subcommand

A waiting criterion makes its gate `failed`, and the gate's result carries
one finding per waiting criterion, at level `warning` with
`message_source: koto`, whose message holds the reason, the question, the
three descriptions and the exact command line. The agent records an answer
with:

```
koto decider answer <session> --gate <check> --criterion <criterion id> --answer <pass|fail|unclear>
```

which appends one `decider_check_answered` event. The command follows
`koto overrides record`'s load, state, gate and append flow and its JSON
error shape on stdout, and never consults the decider or takes its lock.

#### Alternatives Considered

- **Evidence with a reserved key.** The answer would ride on `koto next
  --with-data`, the way `gates` is reserved. The state's `accepts` schema
  would have to admit or ignore it, `handle_next` would need a carve-out,
  and routing could see it. Rejected: it couples a check's answer to the
  template's evidence contract.
- **`koto overrides record --with-data`.** An override replaces the whole
  gate's output for the epoch, logs a candidate false fail, and can't name
  one criterion. Rejected: wrong grain and wrong record.
- **A `questions` field on `BlockingCondition`.** Structured, but it changes
  the `koto next` output contract for one gate type while findings already
  carry text to the agent. Rejected: contract change with no capability
  findings lack.

### Decision 2: Where the fallback happens and how an answer is matched

Resolved inline.

`evaluate` already reads the visit's events under the decider lock to find
a reusable verdict, keyed by state, gate, rule id, declaration hash and the
slice's `input_sha256`. An answer has to be found the same way, and the
fallback has to cover every reason without a per-reason rule.

Key assumptions:

- koto's visit (`arrival_index`) is the right scope for an answer, as it is
  for a reused verdict.

#### Chosen: answer lookup before every unanswered path, keyed by slice

Each criterion resolves in this order:

1. Empty slice: `NotGraded`, as today. (The early outcome is computed as
   today: a failed extraction first, then an empty slice, then over
   budget.)
2. A reusable decider verdict for the same slice: that verdict, as today.
   An early `OverBudget` or `ExtractionFailed` skips this step, as today.
3. An applicable agent answer: the latest `decider_check_answered` in the
   visit with the same state, gate, rule id, declaration hash and
   `input_sha256` (two absent hashes match). Its answer is the outcome and
   nothing is appended for the criterion.
4. Otherwise the existing path: the early `OverBudget` or
   `ExtractionFailed`, `Busy` without the lock, `CapSpent` without budget,
   or a consultation.

This restructures the evaluator's per-criterion `match`: today the early
outcome is its first arm, so steps 2 and 3 have to move ahead of it for
every early outcome except `NotGraded`.

Any criterion that ends step 4 `Unanswered` is a fallback. Because step 3
precedes every unanswered branch, a reason added to `UnansweredReason`
falls back with no further code.

A criterion that waits is evaluated again on every `koto next` until the
agent answers: an unanswered outcome is never reused, so the provider is
asked again, and a fresh missing verdict is a fresh fallback with its own
`decider_fallback` event. The journal's record count is therefore the
number of times the agent was asked. The number of distinct fallbacks is
the number of distinct `koto.session.id`, `koto.visit.seq`, `koto.check`
and `koto.rule.id` groups, and the first record of a group is the first
time the agent was asked.

An answer binds to the slice of the latest fallback, so an agent that
changes its work after the fallback and then answers has answered about
the old slice: the next evaluation of the new slice asks the decider, and
falls back again if it still can't answer. An answer about old work
never applies to new work.

#### Alternatives Considered

- **Binding an answer to one fallback event.** Each evaluation of a slice
  the provider still can't grade would make a new fallback and ask again
  for something already answered. Rejected.
- **Consulting the decider before reading the answer.** A provider that
  recovers within the visit could then grade the slice the agent already
  answered for, and veto it. But during an outage every tick would pay up
  to two provider timeouts per criterion and spend the shared cap, and
  for `over_budget` and `extraction_failed` nothing is sent anyway. The
  PRD (R6) makes the agent's answer the outcome for its slice. Rejected;
  a changed slice still goes back to the decider.
- **Looking up answers in the engine.** The engine would need to read
  decider records it never sees today. Rejected on the small-engine driver.

### Decision 3: The gate's outcome, output, findings and blocking rule

Resolved inline.

The gate has to stop the state while a criterion waits, let an agent fail
block in either mode, and leave overrides, rule counts and the measurement
readers' fields meaning what they mean.

Key assumptions:

- A check that logged `passed` while a criterion went unanswered will now
  log `failed` until the agent answers. The Datadog measurement
  definitions need a new version to count that; this design leaves the
  counting to their owners.

#### Chosen: `failed` while anything blocks, two new lists outside the schema

The gate is `failed` when `output.failed` or `output.awaiting` is
non-empty, and `passed` otherwise. The output becomes:

| Key | Meaning |
|---|---|
| `failed` | Criteria that block on a fail: a decider veto fail, or an agent `fail` in either mode. |
| `unanswered` | Unchanged meaning: every veto criterion with no decider verdict this evaluation, waiting or agent-answered included. |
| `awaiting` | New. Criteria waiting on the agent, any mode. |
| `agent_answered` | New. Criteria whose outcome this evaluation came from an agent answer. |
| `error` | Unchanged: `""`, `missing_spec` or `log_unreadable`. |

Lists are in declaration order. `awaiting` and `agent_answered` stay out of
`gate_type_schema`, so an `override_default` with only the old three keys
stays valid, and an override value without them doesn't block.

Findings: a decider veto fail keeps its `error` finding with
`message_source: decider`. An agent `fail` gets an `error` finding with
`message_source: koto`, so it counts as a rule like any violation. A
waiting criterion gets a `warning` finding with `message_source: koto`, so
it never counts as one: `error_rule_ids` in `src/engine/advance.rs` counts
only `error` findings, so a warning never reaches `rule_counts` or a
`check_rule` record. The other readers of a decider check's result take
it as it is: `blocking_conditions_from_gates` renders any non-passed gate
with its output and findings, and the journal's `with_rules` writes rules
from `rule_counts` alone.

The engine's decider-check blocking rule becomes "`failed` or `awaiting` is
non-empty", still ignoring the evidence fallback. `override_records` writes
a candidate false fail only for a `failed` id not in `agent_answered`.

A check with an unreadable log is `failed`, lists every criterion under
`awaiting`, keeps `error: log_unreadable` and `unanswered` as today, holds
one `warning` finding named for the check, and appends nothing. A check
with no spec still passes with `missing_spec`.

#### Alternatives Considered

- **Outcome `pending`.** It means "judged nothing yet", but it brings the
  `temporal` category, `agent_actionable: false`, and `check_pending`
  journal records whose poll fields don't apply. Rejected.
- **Outcome `error`.** The pre-koto#300 shape, which measurement reads as a
  violation plus a checker fault. Rejected on koto#297's ruling.
- **Waiting criteria under `failed`.** `override_records` would log
  candidate false fails for criteria nobody failed, and the measurement
  readers would see violations. Rejected.

### Decision 4: How a fallback is recorded and exported

Resolved inline.

The run journal relays committed session-log events, one set of records per
event (`relayed()` in `src/run_journal/mod.rs`), and its reference names
every attribute.

Key assumptions:

- Attributes under `koto.check.` stay on `check_evaluated`, where the
  archive renames `koto.check`, and this design adds none there.

#### Chosen: a `decider_fallback` event and journal record

Each fallback appends a `decider_fallback` session-log event after that
evaluation's `decider_checked` events and before its `gate_evaluated`, in
declaration order. The journal relays it as one `decider_fallback` record.
`decider_checked` records gain the slice size, and decider-check
`check_evaluated` records gain the two new list lengths. The answer event
stays in the session log only.

#### Alternatives Considered

- **Deriving the record from `decider_checked` with outcome `unanswered`.**
  No new event, but the journal would write a record no event stands for,
  breaking the one-event-per-relay rule and the `koto.seq` join, and the
  answer command would have no fallback to cite. Rejected.
- **A `koto.fallback.*` namespace.** A fresh root with no join to the
  `koto.decider.*` attributes Datadog already admits. Rejected.
- **New counts under `koto.check.`.** `koto.check.awaiting_count` would
  start with `koto.check` followed by `.`, the collision PRD R12 forbids.
  Rejected; the counts sit under `koto.decider.` instead.

## Decision Outcome

The four choices meet at the evaluator. Decision 2 decides, per criterion,
whether the outcome is a decider verdict, an agent answer or a fallback;
Decision 3 turns those outcomes into the gate's outcome, lists and
findings, and the engine blocks on two lists instead of one; Decision 1's
warning finding is how a fallback reaches the agent and its subcommand is
how the answer gets back; Decision 4's event is what the subcommand cites
and what the journal counts.

A fallback's life:

1. The evaluator gets no verdict for criterion `c` on slice `S` and finds
   no answer for `S`. It appends `decider_checked` (outcome `unanswered`,
   `blocked: false`, as today) and `decider_fallback`, then
   `gate_evaluated` with `c` under `awaiting`. The gate is `failed`; the
   state stops; the agent reads the warning finding.
2. The agent runs `koto decider answer <s> --gate g --criterion c --answer
   fail`. koto finds the latest `decider_fallback` for `g`/`c` in the visit,
   copies its hash fields, and appends `decider_check_answered`.
3. `koto next` re-evaluates. Slice still `S`: the answer applies, `c` is
   under `failed` and `agent_answered` with an `error` finding, and nothing
   is appended for it. The agent fixes the work.
4. Slice is now `S'`. No answer applies, so the decider is asked again. A
   verdict applies by mode; another missing verdict is a new fallback.

### The exact names, for the Datadog lane's review

Record kind `decider_fallback`. Its attributes, after the journal's common
fields (`kind`, `v`, `at`, `session`, `koto.session.id`, `koto.run.id`):

| Attribute | Type | Present | New? | Meaning |
|---|---|---|---|---|
| `koto.seq` | integer | always | reused | Sequence number of the `decider_fallback` event. |
| `koto.visit.seq` | integer | when logged | reused | The visit, by koto's rule. |
| `koto.state` | string | always | reused | The state the check belongs to. |
| `koto.check` | string | always | reused | The decider check's gate name. |
| `koto.rule.id` | string | always | reused | The criterion's id, or its digest. |
| `koto.decider.mode` | string | always | reused | The criterion's effective mode, `veto` or `shadow`. |
| `koto.decider.reason` | string | always | reused | Why no verdict: today `provider_error`, `unreadable_response`, `over_budget`, `extraction_failed`, `cap_spent` or `busy`; an open vocabulary. |
| `koto.decider.input_bytes` | integer | except with `extraction_failed` | new | The slice's size in bytes, after redaction, as captured. Capture stops at 64 KiB, so 65,536 means at least that. |

Added to existing kinds:

| Kind | Attribute | Type | Present | Meaning |
|---|---|---|---|---|
| `decider_checked` | `koto.decider.input_bytes` | integer | when the extraction produced output | As above, for the slice that criterion was graded on. |
| `check_evaluated` | `koto.decider.awaiting_count` | integer | decider checks | Length of `output.awaiting`, 0 included. |
| `check_evaluated` | `koto.decider.agent_answered_count` | integer | decider checks | Length of `output.agent_answered`, 0 included. |

Collision check: the journal's existing names under `koto.decider.` are
`provider`, `model`, `outcome`, `error_class`, `input_tokens`,
`output_tokens`, `unread_usage_attempts`, `latency_ms`, `directive_bytes`,
`mode`, `reason`, `blocked` and `attempts`, all leaves, and no name equals
`koto.decider`. The three new names are leaves beside them; none equals an
existing name, none starts with an existing name followed by `.`, and no
existing name starts with one of them followed by `.`. A test enforces
this over every attribute the journal writes.

Left open for the definitions owner: a decider check's gate was only
`passed` or `failed`, and a shadow check's always `passed`. With this
design a check logs `failed` while a criterion waits or after an agent
`fail`, in shadow too, and an agent `fail` produces a `check_rule` for the
criterion. Counting that needs a new definitions version (version 2 or
later); the `awaiting` and `agent_answered` counts above are there to make
the new outcomes separable without reading `output`.

## Solution Architecture

### Overview

The change lives in five places: the record types in `src/decider/check.rs`,
two event variants in `src/engine/types.rs`, the evaluator in
`src/cli/check_evaluator.rs`, one blocking rule and one append loop in
`src/engine/advance.rs`, the new subcommand under `src/cli/decider.rs`, and
the relay in `src/run_journal/mod.rs`. Templates, the compiler and the
routing decider are untouched.

### Components

- **`src/decider/check.rs`.** Adds `AgentAnswer { Pass, Fail, Unclear }`
  with `as_str` and `parse`; `DeciderFallback` (the `decider_fallback`
  payload) and `DeciderCheckAnswered` (the `decider_check_answered`
  payload). `blocks()` is unchanged: it still governs decider verdicts and
  the `blocked` field of `decider_checked`.
- **`src/engine/types.rs`.** `EventPayload::DeciderFallback` and
  `EventPayload::DeciderCheckAnswered`, declared before
  `InstructionsDelivered`, with `type_name` arms and deserialize arms that
  degrade an unparseable payload to `Unknown`, like `decider_checked`.
- **`src/gate.rs`.** `StructuredGateResult` gains a skipped field
  `decider_fallbacks: Vec<DeciderFallback>` beside `decider_checks`.
- **`src/cli/check_evaluator.rs`.** `applicable_answer()` beside
  `reusable()`; `Graded` records where its outcome came from (decider,
  agent, or waiting with a reason); `result_for` builds the five-list
  output, the findings and the gate outcome; the `log_unreadable` path
  returns the failed shape; `override_records` skips `agent_answered` ids.
- **`src/engine/advance.rs`.** The decider-check `blocks` closure reads
  `failed` or `awaiting`; after a result's `decider_checks`, the loop
  appends its `decider_fallbacks`, failing the tick if an append fails, as
  `decider_checked` does.
- **`src/cli/decider_answer.rs`** (new), wired as
  `DeciderCommand::Answer` in `src/cli/decider.rs` and dispatched from
  `src/cli/mod.rs` with a session backend.
- **`src/run_journal/mod.rs`.** `relays()` and `relayed()` gain
  `DeciderFallback`; `decider_checked()` adds `koto.decider.input_bytes`;
  the `GateEvaluated` arm adds the two counts when the output has
  `unanswered` (a decider check).

### Key Interfaces

Session-log payloads (all keys also go in the session-feed contract):

```
decider_fallback:
  state: string, visit_seq: integer, gate: string, rule_id: string,
  declaration_hash: string, mode: "shadow"|"veto",
  reason: string (an UnansweredReason value),
  input_sha256: string (optional), input_bytes: integer (optional)

decider_check_answered:
  state: string, visit_seq: integer, gate: string, rule_id: string,
  declaration_hash: string, input_sha256: string (optional),
  answer: "pass"|"fail"|"unclear", fallback_seq: integer
```

`koto decider answer`:

| Case | Exit | Output |
|---|---|---|
| Recorded | 0 | `{"status":"recorded","gate":G,"criterion":R,"answer":A}` |
| Answer not `pass`, `fail` or `unclear` | 2 | `{"error":{"code":"invalid_answer",...},"command":"decider answer"}` |
| Gate isn't a decider check of the current state, or doesn't declare the criterion | 2 | `unknown_criterion` |
| No `decider_fallback` for the criterion in the current visit | 2 | `no_fallback` |
| Session missing, log unreadable, template unreadable | 1 or 3 | As `koto overrides record` reports them |

Each typed error carries `code`, `message`, `gate`, `criterion` and `state`
(when known). The three codes go in `docs/reference/error-codes.md`.

The waiting finding's message, with the template's text substituted:

```
criterion <id> is waiting on your answer: the decider gave no verdict
(<reason>). Record it with `koto decider answer <session> --gate <gate>
--criterion <id> --answer <pass|fail|unclear>`, then run `koto next
<session>` again. <question> Answer pass if: <pass>. Answer fail if:
<fail>. Answer unclear if: <escape>.
```

Each substituted session, gate and criterion value is written as-is when
it matches `^[A-Za-z0-9._:/@-]+$` and single-quoted otherwise (see
Security Considerations).

A finding's message is capped at 1,000 bytes (`MESSAGE_MAX_BYTES`), and a
criterion's question and descriptions have no length limit of their own, so
the command comes before the template's text: a cut loses the end of the
descriptions, never the command.

The agent-fail finding: `criterion <id> failed: you answered fail for the
<label>: <fail description>`.

### Data Flow

For one evaluation of a decider check:

1. Run the extraction command; decide the early outcome (`not_graded`,
   `over_budget`, `extraction_failed`) and the slice hash and size, as
   today.
2. Take the lock when there's no early outcome, as today; read the log. If
   it can't be read or names no state, return the unreadable-log shape.
3. For each criterion in declaration order: `NotGraded`; else a reusable
   decider verdict; else an applicable agent answer; else the early
   outcome, `Busy`, `CapSpent` or a consultation. A consulted or early
   outcome gets a `decider_checked` record as today; an `Unanswered` one
   also gets a `DeciderFallback` record.
4. Release the lock; append ledger `checked` lines as today.
5. Build the output lists, findings and outcome (Decision 3).
6. The engine appends `decider_checked` events, then `decider_fallback`
   events, then `gate_evaluated`; the journal relays each after commit.
7. The engine blocks the state if `failed` or `awaiting` is non-empty.

## Implementation Approach

One pull request, built in this order so each step compiles and tests
green.

### Phase 1: Records and events

The payload types, the two `EventPayload` variants, the session-feed
contract entries, and the validate-feed test list. Nothing writes them yet.

Deliverables:
- `src/decider/check.rs`, `src/engine/types.rs`
- `docs/reference/session-feed.md` (frontmatter and body)
- `src/cli/validate_feed.rs` test list

### Phase 2: The fallback in the evaluator and engine

Phase 2's tests build `decider_check_answered` events directly, since the
subcommand lands in Phase 3; both phases land in the same pull request, so
no released build stops a state without a way to answer.

Answer lookup, the new output and findings, the unreadable-log shape,
`decider_fallbacks` on the result, the engine's blocking rule and append
loop, and `override_records`. The tests that pinned "unanswered passes"
move to the new rule, each listed in the pull request.

Deliverables:
- `src/cli/check_evaluator.rs`, `src/gate.rs`, `src/engine/advance.rs`
- `tests/decider_check_run.rs`, `tests/decider_check_fixtures.rs`

### Phase 3: The answer subcommand

`koto decider answer`, its typed errors, and their documentation.

Deliverables:
- `src/cli/decider_answer.rs`, `src/cli/decider.rs`, `src/cli/mod.rs`
- `docs/reference/error-codes.md`, `docs/guides/cli-usage.md`

### Phase 4: The journal

The `decider_fallback` record, the new attributes, the collision test, and
the run-journal reference.

Deliverables:
- `src/run_journal/mod.rs`, `src/run_journal/tests.rs`,
  `tests/run_journal_doc_test.rs`, `tests/run_journal_checks_test.rs`
- `docs/reference/run-journal.md`

### Phase 5: Compatibility, docs and skills

The v0.14.1 compat script's `nv` session now stops, is answered and moves
on, with mutations for both new events; the remaining docs and the
koto-user and koto-author skills; the koto-user missing-verdict eval
scenario, checked with `scripts/check-evals-exist.sh`; superseding notes in
the decider-checks PRD and design.

Deliverables:
- `test/compat/decider-checks-v0_14_1.sh`
- `docs/guides/decider-authoring.md`, `docs/workspace-layout.md`
- `plugins/koto-skills/skills/koto-user/SKILL.md`, its command reference
  and `evals/evals.json`;
  `plugins/koto-skills/skills/koto-author/references/template-format.md`
- `docs/prds/PRD-koto-decider-checks.md`,
  `docs/designs/current/DESIGN-koto-decider-checks.md`

## Security Considerations

koto downloads nothing new and runs no new command itself, adds no
dependency, and asks for no permission beyond writing the session log the
agent's own koto commands already write. The finding does ask the agent to
run one koto command. What the design changes is who can move a gate, what
text reaches the agent, and what the journal says.

- **Self-grading.** The agent answers for its own work, and could answer
  `pass` to move on. The maintainers' 2026-10-14 ruling, recorded in the
  PRD's motivating context, accepts this: it's what the
  agent did before a decider existed, and the answer is a recorded event a
  reviewer can read beside the gate. It never reaches the routing decider
  or the ledger's accuracy counts. An answer leaves a thinner trail than an
  override, which requires a rationale; the answer takes none on purpose,
  since a one-word answer is all a closed question needs, and the
  `agent_answered` list and count make every agent-decided gate visible.
- **Trusted template text.** The waiting finding carries template-authored
  text (question, descriptions, label) to the agent. That text is trusted
  authoring input, like a state's directive, and koto quotes it after its
  own fixed words and the command, so a description can't displace the
  command line. The slice, which is the agent's own output and may hold
  instructions aimed at a model, never enters the finding, the events or
  the journal; a test feeds marker strings through the slice and asserts
  none appears in any new event, finding or record.
- **Answer command input.** Arguments reach koto through clap as literal
  strings. The gate and criterion must match the template's declarations,
  the answer must be one of three words, and nothing else the agent types
  is recorded, so an event can't carry free text. The agent copies the
  command line from the finding into a shell, so koto writes each
  substituted value (session, gate, criterion) as-is only when it matches
  `^[A-Za-z0-9._:/@-]+$`, and otherwise single-quoted with any `'` written
  as `'\''`. A rule id with a space or a shell metacharacter can't turn
  the line into a different command; a test covers one.
- **An agent can cause its own fallback.** The agent controls its work,
  so it can push the slice over budget or break the extraction, then
  answer `pass`. That's not a regression, since those cases passed with
  no answer at all before, but it means a veto check can be reduced to a
  self-grade on purpose. The `decider_fallback` records make it visible:
  a session whose fallbacks are all `over_budget` or `extraction_failed`
  is a signal to alert on. Whether the ruling covers this deliberate case
  is raised with the maintainers rather than decided here.
- **Unreadable log.** With no readable log, nothing is appended, so
  `koto decider answer` finds no fallback and refuses with `no_fallback`,
  and `koto overrides record` can't read the log either. The state stays
  stopped until the log is repaired. That's the safe direction for a
  session koto can't read.
- **Integrity of the binding.** An answer applies only to the slice hash
  and declaration hash it was recorded against, in the visit it was
  recorded in, so an answer can't be replayed onto changed work or a
  changed criterion. The residual case is `extraction_failed`, where there
  is no slice hash: the answer stands for the rest of the visit while
  extraction keeps failing, even if the work changes. That's accepted and
  documented in the authoring guide; the agent can answer again, and a
  template whose extraction keeps failing is itself a defect the
  `decider_fallback` records surface by reason.
- **Telemetry.** The new journal record carries names, ids, a closed
  vocabulary and a byte count. Names outside the journal's name shape are
  left out, and rule ids outside it are digested, as today.
- **Concurrency.** The subcommand appends without the decider lock. An
  evaluation in progress doesn't see the answer, and the next one does;
  both writers append whole events to the same log as `koto overrides
  record` already does. An answer that lands between the engine's
  `decider_fallback` and `gate_evaluated` appends is harmless: lookups key
  on state, visit and hashes, never on adjacency. A slice that changes and
  changes back finds the answer recorded for it, by design: the same slice
  gets the same answer. Two concurrent ticks can each record a fallback for
  the same slice when the lock wait runs out (`busy`); each is a real
  fallback the agent was shown, so the journal counts both, and one answer
  covers both since it binds to the slice.

## Consequences

### Positive

- No decider-check criterion passes because nobody judged it, in either
  mode, for any present or future reason.
- A fallback is a countable journal record with its slice size, so budget
  and slice tuning can read Datadog instead of reconstructing from logs.
- A reviewer can tell decider verdicts, agent answers and waiting criteria
  apart from the gate's output alone.

### Negative

- A provider outage now stops every opted-in run at each decider check
  until its agent answers, instead of passing silently. That's the point,
  but it costs agent turns.
- A shadow check can now log `failed`, and the measurement definitions
  count decider-check gates differently until a new definitions version
  lands.
- An agent that doesn't know the subcommand stays stopped until someone
  answers or records an override.
- With `extraction_failed` there's no slice hash, so an answer stands for
  the rest of the visit while extraction keeps failing.

### Mitigations

- The waiting finding names the exact command, and the koto-user skill and
  its eval teach it.
- The new counts on `check_evaluated` let a definitions version separate
  waiting and agent-decided gates without reading `output`.
- The agent can answer again at any time; the latest answer wins.
