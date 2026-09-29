---
schema: design/v1
status: Current
problem: |
  koto's decider answers routing questions about context that exists before
  the agent acts. It has no way to read what the agent produced, grade it
  against several closed criteria, object with a finding the agent can act
  on, or record each verdict for later trust decisions, and nothing in the
  gate machinery can ask a decider anything.
decision: |
  A new `decider-check` gate type runs a template-declared extraction command,
  redacts and bounds its output, and asks the opted-in decider one choice
  question per criterion (pass, fail, and the escape `unclear`). A veto
  criterion under effective mode `auto` fails the gate on a fail verdict, with
  an `error` finding carrying the criterion's `rule_id` and `rule_ref`;
  everything else passes, including a criterion that got no verdict, which
  the gate's output lists as unanswered and the log records with its reason,
  and every shadow criterion. The gate's output never
  routes, it is always overridable, and opted-out users get a template view
  with the gates removed. Each consultation writes a `decider_checked` event
  and a ledger `checked` record, overrides add `check_overridden` records, and
  `koto decider report` tallies outcomes per criterion.
rationale: |
  A gate already blocks, reports findings, counts attempts, and is overridden
  with a recorded reason, so the veto, the finding and the way past come from
  code that exists. One request per criterion is the configuration the
  accuracy spike measured. Keeping all provider I/O in the CLI's gate
  evaluator leaves the engine I/O-free, and a separate event keeps
  `gate_evaluated` and `decider_consulted` unchanged for their readers.
upstream: docs/prds/PRD-koto-decider-checks.md
user_visible_surface: true
---

# DESIGN: Decider checks that grade the agent's work

## Status

Current

## Context and Problem Statement

The requirements are in `docs/prds/PRD-koto-decider-checks.md` (R1 to R34).
This design settles how koto meets them.

Two existing mechanisms each hold half of what the feature needs. The
routing decider (`docs/designs/current/DESIGN-jev-decision-offload.md`) holds
the provider client, opt-in and mode resolution, the per-call cap, the
per-session `decider.lock`, the `decider_consulted` event and the
append-only `~/.koto/_decider_ledger.jsonl`. It runs only in the
`NeedsEvidence` arm of the advance loop, only for `accepts` fields, and only
to supply evidence; it never blocks. Gates hold the other half. Every gate on
a state is evaluated on every tick through the CLI's `TickGates` closure; a
failed gate blocks the state, carries a `failure` object with findings
(`rule_id`, `level`, `message`, `rule_ref`, `message_source`), stamps attempt
counts and rule counts on `gate_evaluated`, and can be overridden with
`koto overrides record --gate <name> --rationale <text>`, which records the
gate's `actual_output` and injects a synthetic pass for the rest of the
visit.

What's missing is a check whose verdict comes from the decider. Three facts
constrain it. The engine (`src/engine/`) does no network or file I/O beyond
appending events; the routing decider reaches it through a port. Templates
that don't use the feature must compile to the same JSON and hash, so any
new field on a compiled type must be skipped when empty. And the measurement
effort that will judge criteria later reads `gate_evaluated`,
`decider_consulted` and the ledger, so every change to them is reported
before building and must be additive.

## Decision Drivers

- **D1. Veto, never approval.** A pass must advance nothing (R15, R17); a
  missing answer must never count as a pass (R12 to R16).
- **D2. Reuse the check machinery.** Blocking, findings, attempt stamps and
  overrides already exist for gates; the PRD forbids a new override
  mechanism (R22).
- **D3. Measured configuration only.** The spike measured one choice
  question per request, unbatched, under about 2.5 KB (R2, Out of Scope).
- **D4. No change for anyone else.** Opted-out users (R11) and templates
  without the feature (R28) see nothing new; the routing decider and
  `E-DECIDER-FLOOR` are untouched (R7).
- **D5. Additive, documented records** that a later accuracy effort can join
  (R24 to R27), readable by koto v0.14.1 (R29).
- **D6. The engine stays I/O-free**, as the routing decider's design
  required, and every behavior is testable against a local stub with no new
  crate (R34).

## Considered Options

### Decision 1: Where a criterion is declared

The declaration has to name an extraction command and several criteria,
block like a failed check, be overridable through the existing command, and
never route.

#### Chosen: a new gate type, `decider-check`

```yaml
states:
  review_comments:
    gates:
      comment_reasons:
        type: decider-check
        command: "scripts/added-comments.sh {{BASE_REF}}"
        timeout: 20            # optional, seconds, as for command gates
        max_bytes: 2560        # optional, 1..8192, default 2560
        label: comments        # optional, default "artifact"
        criteria:
          comment_reason:
            rule_ref: "https://github.com/tsukumogami/shirabe/blob/main/skills/work-on/references/phases/phase-4-implementation.md"
            question: Does each comment give the reason for the code rather than restate what it does?
            pass: Every comment says why the code is the way it is.
            fail: At least one comment restates what the code does.
            escape: The text holds no comment, or can't be judged from what is shown.
            threshold: 0.9     # optional, 0.5..1.0, default 0.9
            mode: shadow       # optional, shadow | veto, default shadow
```

A gate is the unit koto already blocks on, reports findings for, counts
attempts for, and overrides. The gate's name is the `--gate` argument; its
criteria are its findings. The source keys `max_bytes`, `label` and
`criteria` compile into one optional `Gate` field, `decider_check`, skipped
when absent, so no existing template's compiled JSON changes. The escape value is fixed as `unclear` and the choice values
as `pass` and `fail`, the words the spike asked with.

#### Alternatives considered

- **A `decider` block on an `accepts` field, as the routing decider uses.**
  It would reuse the existing declaration parser, but an `accepts` field
  supplies evidence and routes; making one that must never route or supply
  evidence inverts its meaning. It also can't block the state (the
  `NeedsEvidence` arm only runs when the state already stopped), and an
  override would need a new path. Rejected on D1 and D2.
- **A state-level `criteria:` block.** It reads well and avoids stretching
  `Gate`. But it needs its own evaluation point, its own blocking rule, its
  own findings plumbing and, above all, its own override path, which R22
  forbids. Older koto rejects it just as it rejects an unknown gate type, so
  it buys no compatibility. Rejected on D2.
- **Extending the `command` gate with an optional `criteria:` block.** Fewer
  gate types, but a command gate's own exit code and findings would then mix
  with decider verdicts, `gate_evaluated.output` would change shape for a
  type readers already parse, and routing on `gates.<name>.exit_code` would
  have to be refused for some command gates and not others. Rejected on D4
  and D5.

### Decision 2: How several criteria are asked

#### Chosen: one request per criterion

Each consultation sends one `DecisionRequest` holding one `Choice` question
(the criterion's question, `pass` and `fail` with their descriptions, the
escape `unclear` with its description) and one labelled input, the slice.
The `Decider` trait and the Jev client are used unchanged. Each consultation
counts once against the per-`koto next` cap of four, shared with the routing
decider (R20), and a state holds at most four criteria (R1), so a state's
criteria always fit one call when the routing decider hasn't used the cap
earlier in it.

This is the configuration the spike measured. Its accuracy numbers therefore
apply as stated, within their own limits (one model build, inputs under
about 2.5 KB). Two criteria cost about 600 ms at the spike's p50.

#### Alternatives considered

- **One request per check, all its criteria as questions.** Fewer round
  trips and one cap slot per check. Jev's documentation says each question
  is scored on its own, but the spike didn't check that on its data, so a
  batched verdict would rest on an unmeasured configuration (D3). The PRD
  allows batching only if the two criteria need it, and they don't: they
  grade different artifacts, so they sit in different checks anyway.
  Rejected; noted as the first thing to measure if cost ever matters.
- **Boolean (`Proposition`) questions.** The spike found the boolean form
  compresses probabilities too much to decide on. Rejected by R2.

### Decision 3: Where the consultation runs

#### Chosen: a check evaluator in the CLI's `TickGates`, records returned beside the result

`evaluate_gates_with_request_store` gains a `GATE_TYPE_DECIDER_CHECK` arm
that calls an optional check evaluator, the same way `children-complete`
calls its evaluator. `TickGates` supplies one, `CliCheckEvaluator`, which
owns the decider handle, the policy, a shared consultation budget, the
session backend and the ledger root. It runs the extraction command, reads
the local log for the visit and for reusable verdicts, consults, writes the
ledger, and returns a `StructuredGateResult` whose new skipped field
`decider_checks` carries one record per consultation. The engine appends
those as `decider_checked` events immediately before the gate's
`gate_evaluated`, exactly as it already appends a gate's `context_read`
events there, except that a failed append fails the command, as
`gate_evaluated`'s does.

The engine never sees a provider, a lock or the ledger (D6), and a decider
check is evaluated wherever the advance loop evaluates gates, with no new
loop step.

The engine does change in one place. A failed gate on a state that accepts
evidence normally falls through to the transition resolver, so evidence
matching a conditional transition still routes: the gate is one way
forward and the evidence another. For a decider check that would let the
agent's own evidence walk around a veto on the agent's own work, so a
blocking decider check stops the state whether or not it accepts evidence
(R11), and `koto overrides record` is the way past (R22). Every other gate
keeps the evidence fallback.

#### Alternatives considered

- **An engine-side consultation step through the routing `DeciderPort`.**
  It would reuse the port's lock and visit logic, but it would need a second
  hook in the loop's gate step, the port's single-consultation-per-visit
  rule doesn't fit several reusable criteria, and the port would grow a
  second responsibility. Rejected: more engine surface for no behavior the
  CLI evaluator can't provide.
- **Consulting in `handle_next` before the loop runs.** The loop may
  auto-advance through several states, and only it knows which state's
  gates are evaluated when. Rejected.

### Decision 4: What opted-out users get

#### Chosen: the engine sees a template with the decider checks removed

When `DeciderSettings::opted_in()` is false, `handle_next` hands the advance
loop and the `--to` guard a copy of the compiled template with every
`decider-check` gate removed from every state. The copy is made only when
the template declares one, so templates without the feature pay nothing.
Nothing runs, nothing is sent, and no `gate_evaluated` appears for the
removed gates: the state behaves exactly as if they weren't declared (R11).
`template_hash` is computed from the file, not from this view, so it is
unaffected. The view shadows `compiled` for the whole of `handle_next`, so
the response renderer, override validation, the `--to` guard and the
polling closure all see the same template. The same view is used when the
user is opted in but `build_decider` returns no provider: with nothing to
consult, a check can't be evaluated honestly. When a provider exists, one
handle is shared between the routing port and the check evaluator.

#### Alternatives considered

- **Evaluate the gate as an immediate pass.** Simpler, but it appends a
  `gate_evaluated` for every opted-out tick, which is a record the PRD says
  shouldn't exist, and it keeps the gate in the state's gate map, where it
  changes nothing today but would silently start mattering if gate presence
  ever does. Rejected on D4.

### Decision 5: The records and the gate's output

#### Chosen: a new `decider_checked` event, ledger `checked` and `check_overridden` records, and a gate output that lists failing and unanswered criteria

`gate_evaluated` for a decider check keeps its usual shape. Its `output` is
`{"failed": [<rule_id>...], "unanswered": [<rule_id>...], "error": ""}`:
`failed` lists the veto criteria that failed on a verdict, the only ones that
block, and `unanswered` the veto criteria that got no verdict, so an
override's `actual_output` already names them by kind (R23) and
`gate_override_recorded` needs no new field. It never carries `stdout` or
`stderr`: the slice is the agent's text and isn't recorded (R24).

**A missing verdict never blocks.** Fail-closed means only that a missing
or malformed answer is never read as a pass. It doesn't mean such an answer
blocks: an outage or a busy lock would then cost every opted-in run one
override per veto check, and fill the candidate false fails with overrides
that say nothing about a criterion's accuracy. So `blocks(outcome, mode)` is
true only for `fail` in veto. A criterion that got no verdict, for any
reason, passes; its `decider_checked` keeps outcome `unanswered` with the
reason and `blocked: false`, the gate's `output.unanswered` keeps listing it
on a passing gate, and `koto decider report` counts it per criterion, the
way it counts escapes. A reader of the log can therefore tell a pass that
was checked and met from one where nothing could be checked.

The event's `outcome` separates a judgment from everything else, because
the measurement effort counts violations from `gate_evaluated`:

- **`failed`** when any veto criterion failed on a verdict. The findings are
  one per failing criterion, with its `rule_id`; attempt stamps and rule
  counts follow as for any failed gate. Unanswered criteria, if any, appear
  in `output.unanswered` only.
- **`passed`** otherwise, with `output.unanswered` listing every veto
  criterion that got no verdict and no finding: a missing verdict is a
  checker fault, not a violation, so no finding ever carries its `rule_id`.
  In shadow mode a check's `gate_evaluated` is always `passed`, with empty
  lists and no findings, whatever the decider said. The verdict lives only
  in `decider_checked`, so shadow criteria never count as violations.

Builds before this rule wrote `error`, with koto's `no verdict was read`
finding named for the check, when no criterion failed but a veto criterion
went unanswered. koto no longer writes it for a decider check; old logs that
hold it still parse and validate, with no special handling.

`output.error` is empty except when the check couldn't grade anything at
all, and then the gate passes and says why. `missing_spec`: the gate has no
decider-check spec, which a validated template can't produce. A compiled
template read back from JSON isn't revalidated, so loading one refuses such
a gate with `E-DECIDER-CHECK-SPEC` before any session starts or resumes on
it; `missing_spec` stays in the evaluator only as a defence in depth, and the
lists are empty. `log_unreadable`: the session log couldn't be read, so there is no state or
visit to record under and no verdict to reuse; nothing is asked or
recorded, and every veto criterion is listed under `unanswered`. Neither
case writes a `decider_checked`, so nothing is ever recorded under an empty
state name.

The engine blocks on a decider check only when its output lists a failed
criterion: a criterion recorded as blocking, never the gate's outcome alone
and never a shadow check. A decider check that passes, or that an older log
recorded as `error`, doesn't count toward the state's failed gates.

Each consultation that isn't a reuse appends one `decider_checked` event,
one per criterion, holding the fields R24 lists. The ledger gains a
`checked` record (the envelope plus the same fields) and, written by
`koto overrides record` when the overridden gate is a decider check with a
failed criterion, one `check_overridden` record per failed criterion, with
`override_kind` `candidate_false_fail`. An unanswered criterion never
blocked, so an override doesn't move past it and records nothing for it.
Ledgers written before that rule also hold `overridden_unanswered` records,
which the report still reads and counts.
`koto overrides record` doesn't evaluate anything to write these: it reads
the gate's latest `gate_evaluated` output for the failed list, and takes each
criterion's `visit_seq` and `declaration_hash` from the latest
`decider_checked` for that `rule_id` in the visit (a reused verdict appends
none, but the consultation it reused is in the same visit). Older koto skips
both kinds as unknown (`report.rs` counts them under `unknown_kind`). A
`checked` line over the ledger's 4 KiB line bound drops its
`probabilities` first and is marked `trimmed`, as `consulted` lines are.

#### Alternatives considered

- **A `criteria` array on `gate_evaluated`.** One event per evaluation, no
  new type. But a reused verdict would repeat on every tick, the event's
  readers would see a new nested shape on a type they already parse, and a
  consultation's cost fields (latency, attempts, model) would be mixed into
  a check record. Rejected on D5.
- **Extending `decider_consulted`.** It is defined as at most one per visit,
  keyed by `accepts` field, with an `outcome` vocabulary about applying
  evidence; a criterion fits none of those. Rejected.
- **A new field on `gate_override_recorded` naming the failed criteria.**
  `actual_output` already carries them once the gate's output lists them,
  so the field would duplicate it. Rejected.

### Decision 6: Reuse, the cap, the lock and the retry

#### Chosen

- **Visit and reuse.** The evaluator reads the local log once per gate
  evaluation (`read_events_local`, no network) and finds the visit start
  through `entry_slice` with `Boundary::ArrivalFromElsewhere`, the window
  `visit_attempt` already counts: a visit opens at an arrival from a
  different state or at a rewind, and a self-transition doesn't open one.
  `visit_seq` is the persisted sequence number of that opening event. A criterion reuses the latest
  `decider_checked` in the visit with the same gate, `rule_id`,
  `declaration_hash` and `input_sha256` whose outcome is a verdict (R19). A
  reuse sends nothing, appends nothing, and uses no cap slot; its blocking is
  recomputed from the reused verdict and the current effective mode.
- **The cap.** A `ConsultBudget` (a shared counter) is created per `koto
  next` and given to both the routing port and the check evaluator. The port
  returns `Skipped` once it is spent, so the engine's own counter and the
  shared one agree. A criterion that finds it spent is unanswered with
  `cap_spent`.
- **The lock.** The evaluator takes the session's `decider.lock` for the
  whole gate evaluation and releases it before returning. If another process
  holds it, the evaluator tries again every 50 ms for up to the longest a
  holder can keep it: every consultation of the per-call cap with its
  retry, at the provider timeout, plus a second (4 x 2 x 2 s + 1 s = 17 s at
  the default timeout), capped at 60 s. Only if the wait runs out is every
  criterion of the gate unanswered with `busy`, sending nothing and using no
  cap slot (R21). The wait is what stops a second, concurrent `koto next`
  from passing a veto check the first one is still grading, now that a
  missing verdict passes: the second waits, then consults for itself. The
  60 s cap only bites above a provider timeout of about 7.4 s; there a
  holder still grading can outlast the wait, and the waiting tick's
  criteria go `busy` and pass.
  `busy` stays separate in the report's per-reason tally, so a pattern of
  lock-busy passes on one criterion shows.
- **The retry.** A `DeciderError` of class `timeout`, `connect`,
  `malformed` or `mismatched`, or `http_status` with a 5xx status, is retried
  once at once. Any other status is not. A consultation counts once however
  many attempts it took, and records `attempts`.
- **Other evaluation sites.** `koto next --to` evaluates only non-overridable
  gates, and a decider check can't be one (R8), so `--to` never consults.
  The polling `default_action` loop (`execute_with_polling`) evaluates gates
  unrecorded to decide when to stop; it treats a decider check as passed, so
  the loop can't spend consultations, and the advance loop's recorded
  evaluation that follows consults once. `koto status` and `koto overrides
  record` don't evaluate gates. In the polling loop the pass is synthetic:
  with no evaluator supplied, the `decider-check` arm returns `Passed`, and
  the gate stays in the map, because `execute_with_polling` switches to
  "succeed on exit code 0" when the map is empty.
- **Who counts.** The shared `ConsultBudget` is authoritative. A slot is
  taken only when a request is about to be sent (by the routing port inside
  `consult`, by the evaluator per criterion), never for a retry, a reuse, or
  an unanswered outcome that sent nothing. The engine's own counter can lag
  it and is advisory; the port's `Skipped` keeps the two consistent.
- **Lock scope.** `flock` locks belong to an open file description, so two
  descriptors on `decider.lock` in one process would contend. They never
  coexist: the evaluator opens, locks and closes the file within one gate
  evaluation, and the routing port locks only inside `consult`, which runs
  after the gate step. A concurrent `koto next` waits on the lock for up to
  eight attempts' worth of time plus a second (17 s at the default timeout)
  before its criteria go `busy`.
- **Cloud sessions.** Reuse reads the local log, which on the cloud backend
  can lag the remote one. The worst outcome is an extra consultation, never
  a wrong verdict. The loop's own transitions are appended before the gate
  step runs, so the visit scan sees the state being evaluated.

#### Alternatives considered

- **Reusing verdicts across visits.** Cheaper on loops, but a re-entered
  state is a new attempt, and the PRD scopes reuse to one visit. Rejected.
- **A separate cap for checks.** It would raise the worst-case cost of a
  `koto next`. Rejected with the PRD.
- **Failing fast on the lock.** The routing decider does, and this design
  first did too. Once a missing verdict passes, a second `koto next` run
  while the first holds the lock would pass a veto check the first is still
  grading, so the agent could skip its own check by running two ticks at
  once. Rejected for the bounded wait.
- **Waiting without a bound.** A holder that hangs past its own timeouts
  would hold the agent's turn indefinitely. The bound covers a holder that
  behaves, and the per-criterion `busy` tally shows one that doesn't.

### Decision 7: How modes and the escape resolve

#### Chosen

A criterion's effective mode is `veto` only when its template mode is `veto`
and `effective_global_mode(user, project)` is `Auto`; otherwise `shadow`. An
escape (including a tie) never blocks and is recorded as `escape`, and R26's
tally makes an always-escaping criterion visible. These are the PRD's
decisions (see its Decisions and Trade-offs); the design adds only that the
existing `effective_global_mode` is the one place the rule is computed, so a
project's `decider.mode` lowers it with no new code path.

## Decision Outcome

koto gains a `decider-check` gate type. At compile time the new
`validate_decider_checks` step refuses malformed declarations with
`E-DECIDER-CHECK-*` codes and refuses any `when`, `skip_if` or assignment
that reads a decider check's output. At run time, for an opted-in user, the
CLI's check evaluator runs the extraction command, redacts and bounds the
slice, and consults once per criterion (reusing a verdict for an unchanged
slice within the visit), under the shared per-call cap and the session's
decider lock, with one retry for transient provider errors. A veto
criterion under effective mode `auto` fails the gate on a fail verdict, with
a finding naming the criterion. Everything else passes, and a pass changes
nothing: a criterion that got no verdict is listed as unanswered and
recorded with its reason, and every shadow criterion passes. Each
consultation is recorded in the session log and the ledger, an override of
a failed check is recorded as a candidate false fail per failed criterion,
and `koto decider report` tallies it all per criterion, no-verdicts by
cause. Opted-out users see a template without the checks.

## Solution Architecture

### Overview

```
koto next ──► handle_next
                 │  opted in?  no ──► template view without decider-check gates
                 │             yes──► CliCheckEvaluator { decider, policy, budget, backend, ledger }
                 ▼
           advance loop ── step 6: evaluate gates via TickGates
                 │                       │
                 │                       ├─ command / context / request-leg (unchanged)
                 │                       └─ decider-check ──► CliCheckEvaluator::evaluate
                 │                               run extraction → redact → bound
                 │                               lock → read log → reuse or consult (≤2 attempts)
                 │                               ledger `checked` lines
                 │                               StructuredGateResult { outcome, output,
                 │                                 findings, failure, decider_checks }
                 ▼
           append `decider_checked` × n, then `gate_evaluated`
```

### Components

- **`src/template/types.rs`**: `GATE_TYPE_DECIDER_CHECK = "decider-check"`,
  added to `SUPPORTED_GATE_TYPES`; `Gate` gains one optional field,
  `decider_check: Option<DeciderCheckSpec>`, skipped when `None`, so every
  existing `Gate` literal changes by one line. `DeciderCheckSpec { max_bytes,
  label, criteria: Vec<CheckCriterion> }` keeps criteria in declaration
  order, and `CheckCriterion { rule_id, rule_ref, question, pass, fail,
  escape, threshold, mode }` holds every default resolved at compile time
  (both in `src/template/decider_check.rs`, with the declaration hash and the
  per-check bounds); `gate_type_schema("decider-check")` is `failed: Array,
  unanswered: Array, error: Str`, and `built_in_default` returns empty lists
  and an empty error. `validate_state_decider_checks` holds the per-state
  rules (limit, duplicates, routing). The strict-mode "gate has no `gates.*` routing"
  check skips decider checks, which must not route.
- **`src/template/compile.rs`**: `SourceGate` gains `max_bytes`, `label` and
  `criteria` (kept raw so errors can name state, gate and criterion);
  `compile_gate` refuses them on other gate types and refuses `poll:` and
  `overridable: false` on a decider check.
- **`src/decider/check.rs`** (new, pure): `CheckMode`, `CheckOutcome`
  (`Pass`, `Fail`, `Escape`, `Unanswered(Reason)`, `NotGraded`), `Reason`,
  `verdict(probabilities, threshold)`, `declaration_hash(rule_id, criterion,
  gate)`, `build_check_request(criterion, label, slice)`, `blocks(outcome,
  mode)`, and `DeciderCheck`, the record shared by the event and the ledger.
- **`src/cli/check_evaluator.rs`** (new): `CliCheckEvaluator` and
  `ConsultBudget`; the retry rule; the finding builders.
- **Hashing before substitution.** `TickGates::evaluate` substitutes gate
  fields before evaluation, so it computes each criterion's declaration
  hash from the unsubstituted gate first and passes the hashes to the
  evaluator; a changing `{{BASE_REF}}` doesn't change the hash or defeat
  reuse. The criteria's questions and descriptions are never substituted.
  `Gate::substitutable_fields` destructures `Gate` exhaustively, so it has to
  name the new fields (only `command` is substitutable), which is the
  tripwire that keeps a later field from being missed. The command is
  substituted exactly as a command gate's is, with the same rules for
  quoting variable values.
- **`src/gate.rs`**: the new arm (with no evaluator supplied it returns a
  synthetic `Passed`: fail-open is safe for a check that can only veto,
  unlike `children-complete`, which errors); `StructuredGateResult.decider_checks`
  (skipped); `MessageSource::Decider` (`"decider"`) in `src/findings.rs`.
- **`src/engine/advance.rs`**: append each `decider_checks` record as
  `EventPayload::DeciderChecked` before `gate_evaluated`, and block on a
  decider check only when its output lists a failed criterion.
- **`src/engine/types.rs`**: `EventPayload::DeciderChecked(DeciderCheck)`,
  type name `decider_checked`.
- **`src/decider/ledger.rs`**: `LedgerRecord::Checked` and
  `LedgerRecord::CheckOverridden`; the reader accepts both kinds.
- **`src/decider/report.rs`**: a `checks` section: per `(state, gate,
  rule_id, declaration_hash)`, counts of `pass`, `fail`, `escape`,
  `unanswered` (per reason, and per cause: `provider` for a provider error
  or an unreadable answer, `not_asked` for a spent cap or a busy lock,
  `input` for an over-budget slice or a failed extraction), `not_graded`,
  `candidate_false_fail` and `overridden_unanswered` (from older ledgers);
  rendered as a table and under `checks` in `--json`.
- **`src/cli/mod.rs`**: build the evaluator and the budget; the opted-out
  template view; `TickGates` carries the evaluator; `execute_with_polling`
  passes decider checks; `koto overrides record` writes `check_overridden`.
- **`src/cli/decider_port.rs`**: consult through the shared budget.

### Key interfaces

The check request, for one criterion:

```json
{"model":"jev-latest",
 "state":{"comments":"<the redacted slice>"},
 "questions":{"comment_reason":{"type":"choice",
   "instructions":"Does each comment give the reason for the code rather than restate what it does?",
   "criteria":{"pass":"Every comment says why ...","fail":"At least one comment restates ...",
               "unclear":"The text holds no comment, or ..."}}}}
```

The slice is only ever a value in `state`, a JSON string; the question and
criteria come from the template.

The outcome rules, in `src/decider/check.rs`:

| Outcome | Condition | Blocks in veto | Finding |
|---------|-----------|----------------|---------|
| `pass` | P(pass) strictly highest and ≥ threshold | no | none |
| `fail` | P(fail) strictly highest and ≥ threshold | yes | `message_source: decider`: "criterion \<id\> failed: the decider judged the \<label\> to fail: \<fail description\>" |
| `escape` | otherwise, ties included | no | none |
| unanswered | provider error after retry, unreadable answer, over budget, extraction failed, cap spent, busy | no | none; listed under `output.unanswered` |
| not graded | slice empty or whitespace | no | none |

The fail finding is level `error`, with `effect_landed` filled as for any
gate, and carries the criterion's `rule_id` and `rule_ref`. Shadow never
blocks and adds no finding. **Not graded** and **escape** are both "not checkable" and
stay distinct: not graded means koto had nothing to ask (an empty slice, no
request sent); escape means the decider read the slice and answered that it
can't be judged.

`decider_checked` event and ledger `checked` record fields:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `state` | string | yes | State evaluated. |
| `visit_seq` | integer | yes | Persisted sequence number of the event that opened the visit: the latest arrival from a different state, or rewind, into this state. Self-transitions don't open a visit. |
| `gate` | string | yes | The decider check's name. |
| `rule_id` | string | yes | The criterion's id. |
| `rule_ref` | string | yes | The criterion's reference. |
| `declaration_hash` | string | yes | SHA-256 over `rule_id`, question, the three descriptions, and the check's command text (before substitution), `max_bytes` and `label`. |
| `mode` | string | yes | Effective mode: `shadow` or `veto`. |
| `threshold` | number | yes | The criterion's threshold. |
| `outcome` | string | yes | `pass`, `fail`, `escape`, `unanswered` or `not_graded`. Consumers MUST tolerate other values. |
| `reason` | string | no | With `unanswered`: `provider_error`, `unreadable_response`, `over_budget`, `extraction_failed`, `cap_spent` or `busy`. |
| `blocked` | boolean | yes | Whether this criterion blocked the state: `true` only for `fail` in veto. `false` for every `unanswered` outcome; older builds wrote `true` for a veto `unanswered`. |
| `provider` | string | yes | `jev`. |
| `model` | string | yes | Model the provider reported, or `unknown`. |
| `probabilities` | object | no | `pass`, `fail`, `unclear`, rounded to four places; present when an answer was read. |
| `input_sha256` | string | no | SHA-256 of the labelled slice, as the routing decider hashes inputs; present whenever the extraction produced output. |
| `input_bytes` | integer | no | Byte length of the redacted slice; present with `input_sha256`. |
| `attempts` | integer | yes | Provider attempts made: 0, 1 or 2. |
| `input_tokens` | integer | no | Input tokens the provider reported, summed over every attempt that got a 2xx answer, including one koto couldn't use (it was billed). From Jev's `usage` block, which holds counts only; the rest of an unusable answer is never read. A non-2xx status or a transport failure adds nothing. Absent when no attempt reported usage. |
| `output_tokens` | integer | no | Output tokens, likewise. Jev reports no cache fields; a provider that does can add them later as optional fields. |
| `unread_usage_attempts` | integer | yes | Attempts that got a 2xx answer whose usage couldn't be read (a body that isn't JSON, or no usable `usage` block), so a remaining undercount is visible rather than guessed. Absent on records written before it existed; read as 0. |
| `latency_ms` | integer | yes | Total wall time of the attempts. |
| `error_class` | string | no | The last attempt's error class, when it failed. |
| `endpoint_origin` | string | no | Which configuration layer supplied the endpoint: one of the labels `default`, `user` or `env`, as the routing decider records it. Never a URL, host, query string or credential. |

The ledger `checked` record adds the envelope (`v`, `at`, `session`,
`session_id`). The ledger `check_overridden` record holds the envelope,
`state`, `visit_seq`, `gate`, `rule_id`, `declaration_hash` and `override_kind`
(`candidate_false_fail`; ledgers from older builds also hold
`overridden_unanswered`).

A criterion's `rule_id` is opaque to koto and declared by the template, but
it is the join key for later adjudications (a review finding or a CI failure
naming the same rule). Once a rule registry exists, a criterion's `rule_id`
must be that registry's id for the rule. A consumer keys decider rates by
`(rule_id, declaration_hash, mode)`, which every record carries.

### Attributes a consumer can export

| Event | Attribute | Type | Present | Meaning |
|---|---|---|---|---|
| `decider_checked` | `koto.decider.provider` (from `provider`) | string | always | The decider provider, `jev`. |
| `decider_checked` | `koto.decider.model` (from `model`) | string | always | The model build the provider reported, or `unknown`. |
| `decider_checked` | `rule_id`, `rule_ref`, `declaration_hash`, `mode` | strings | always | The criterion, its reference, its declaration, and its effective mode. |
| `decider_checked` | `outcome`, `reason`, `blocked` | string, string, boolean | `reason` with `unanswered` | What the consultation produced and whether it blocked. |
| `decider_checked` | `probabilities` | object of numbers | when an answer was read | P(pass), P(fail), P(unclear), four places. |
| `decider_checked` | `input_tokens`, `output_tokens` | integers | when the provider reported usage | The consultation's spend, over every billed (2xx) attempt. |
| `decider_checked` | `unread_usage_attempts` | integer | always | Billed attempts whose usage couldn't be read: the size of any undercount. |
| `decider_checked` | `attempts`, `latency_ms`, `error_class` | integer, integer, string | `error_class` when the last attempt failed | The consultation's cost and failure class. |
| `decider_checked` | `state`, `visit_seq`, `gate`, `input_sha256`, `input_bytes` | string, integer, string, string, integer | hash and bytes when the extraction produced output | Where it ran and what it judged, by hash. |
| `gate_evaluated` (decider check) | `outcome` | `passed`, `failed` (older logs also `error`) | always | `failed` is a violation; shadow is always `passed`. |
| `gate_evaluated` (decider check) | `output.failed`, `output.unanswered` | arrays of `rule_id` | always | Veto criteria that failed on a verdict, and veto criteria that got no verdict, on a passed gate as on a failed one; empty in shadow. |
| `gate_evaluated` (decider check) | `output.error` | `""`, `missing_spec`, `log_unreadable` | always | Why nothing could be graded; the gate passed. `missing_spec` is unreachable from a loaded template (`E-DECIDER-CHECK-SPEC`). |
| `gate_evaluated` (decider check) | finding `message_source` value `decider` | string | on a fail finding | The finding's message came from a decider verdict. |
| ledger `check_overridden` | `override_kind` | `candidate_false_fail` (older ledgers also `overridden_unanswered`) | always | What an override moved past. |

### Data flow for one gate evaluation

1. Run the extraction command with the gate's timeout in the tick's fixed
   environment; its stdout is already redacted at capture. A spawn failure,
   timeout or non-zero exit makes every criterion `unanswered /
   extraction_failed`.
2. Empty or whitespace slice: every criterion `not_graded`. Over
   `max_bytes`: every criterion `unanswered / over_budget` (hash and length
   recorded, no request).
3. Take `decider.lock`, waiting up to the bound in Decision 6; if it runs
   out every criterion is `unanswered / busy`.
4. Read the local log; if it can't be read, or names no state, pass with
   `output.error` `log_unreadable` and every veto criterion unanswered,
   recording nothing. Find the visit start; for each criterion in
   declaration order, reuse a matching verdict or, if the budget has room,
   consult (at most two attempts); else `unanswered / cap_spent`.
5. Append a ledger `checked` line per consultation; release the lock.
6. Build the output lists, the findings for blocking criteria, and the
   records. The gate's outcome is `failed` if any veto criterion failed on
   a verdict, else `passed`; either way `output.unanswered` lists the veto
   criteria that got no verdict.

## Implementation Approach

1. **Declaration and compile rules.** The gate type, the `Gate` and
   `SourceGate` fields, the schema and default, `validate_decider_checks`
   and its `E-DECIDER-CHECK-*` codes (`FIELD` for a missing or empty
   field and for decider-check fields on another gate type, `THRESHOLD`,
   `MODE`, `DUPLICATE`, `LIMIT`, `BUDGET`, `LABEL`, `OVERRIDABLE`, `POLL`,
   `ROUTE`), the
   strict-mode exemption, and the compatibility baseline.
2. **Pure check logic.** `src/decider/check.rs`: outcome rule, declaration
   hash, request builder, blocking rule, record type.
3. **The evaluator and the engine append.** `CliCheckEvaluator`,
   `ConsultBudget`, the gate arm, the `decider_checked` event, the opted-out
   template view, the polling-loop pass-through, and stub-driven integration
   tests for every outcome in both modes.
4. **Overrides, ledger and report.** `check_overridden`, the ledger kinds,
   the report's `checks` section.
5. **Contract, docs, skills and compatibility.** Session-feed entries, the
   error-code reference, the authoring guide and template-format reference,
   the koto-author and koto-user skills, the two demonstration fixtures, and
   the koto v0.14.1 compatibility job. Older koto can't compile a template
   with a decider check, so the job drives the new build through a
   decider-check state against the stub until the session has left it,
   then hands v0.14.1 that session: its log holds `decider_checked` events
   and a check-gate `gate_evaluated` in history, and its current state has
   no decider check. v0.14.1 must run `koto status` and `koto next` on it and
   agree on the state, and `koto decider report` on the ledger holding
   `checked` and `check_overridden` lines must exit 0.

## Security Considerations

**What the veto is.** The veto is an in-band guardrail on the agent's own
work, not an access control. The agent can override a check with its own
rationale, or leave the state with `koto next --to`, which skips every
overridable check and records no consultation. Both paths already exist for
every overridable gate; the override is at least recorded as a candidate
false fail, and a `--to` out of a state is visible in the log as a
`directed_transition`.

**Data leaving the machine.** For an opted-in user, one request per
criterion carries the criterion's question and descriptions and the
redacted slice, to the endpoint the routing decider already uses under the
same key and endpoint rules. The slice passes through koto's capture
redactor before it is measured, hashed or sent, so credential-carrying
variables, `pass_env` values and koto's own keys (the decider key included:
the evaluator uses the tick's redactor, which holds it) never leave in it.
The redactor replaces known values only. A slice is arbitrary repository
text, so a secret committed to a file, or held in a `.env` the extraction
command prints, is not recognized and would be sent. Extraction commands
should print the narrow text the criterion judges, such as the added
comment lines, never whole files. The
decider's input rule is the existing one: what the agent already sends to
its model provider may go to the decider, with no gating by repository
visibility. Opted-out users send nothing, because the gates are removed
from their view before anything runs.

**Injected text in the slice.** The slice is agent-authored and can hold
text written to steer the grader. It travels only as a JSON string value in
`state`, never in `instructions` or `criteria`. The spike's adversarial
fixtures for these two criteria got no pass, and a steered pass would still
advance nothing, because a pass never advances anything (R17). The worst a
successful steer can do is let one veto criterion through, which is exactly
the state of affairs without the feature. A steer toward the escape has the
same effect, since an escape never blocks; the per-criterion escape tally in
`koto decider report` is how such a pattern shows up.

**Evasion by the agent.** An agent controls the artifact the slice comes
from. An over-budget slice is unanswered, not skipped: it passes like any
missing verdict, so an agent that makes the slice too long gets past the
check, but the record says `over_budget` and the report tallies it per
criterion, so size can't switch a check off silently. An empty slice is not
graded; making a slice empty means
removing the thing the criterion judges (Known Limitations in the PRD). The
extraction command's string is fixed by the template, but a script it calls
may sit in the tree the agent edits, and then the agent can shape or empty
what the grader sees, the same exposure a command gate's script has. Keep
extraction scripts outside the paths the agent is asked to change, or treat
the check as advisory in repositories where that can't hold.

**Records.** Neither the event nor the ledger holds slice text, the API key,
a response body or error text; the slice appears only as its hash and
length. `gate_evaluated` for a decider check never carries streams. The
ledger keeps its mode 0600.

**Denial of progress.** A provider outage, a spent cap or a busy lock
doesn't block: each leaves its criteria unanswered and the gate passes, so
no outage costs an override. The cost moves to coverage instead: a criterion
that goes unanswered wasn't checked on that visit, which the gate's
`output.unanswered`, the `decider_checked` reason and the report's
per-criterion tally all show. The bounded lock wait (Decision 6) keeps a
concurrent `koto next` from turning a busy lock into a way past a check.
Reuse is keyed on the slice, not
the model; a verdict read earlier in the same visit from another model build
is reused, which is acceptable within one visit and visible in the records.

**Extraction command.** It runs with the same working directory, fixed
environment, timeout, and variable substitution rules as a command gate; it
adds no new execution surface.

## Consequences

### Positive

- A veto on the two proven criteria needs no new blocking, findings, attempt
  or override code: the gate machinery carries it.
- The records give the later trust effort per-criterion data joined by
  declaration hash and input hash, including escapes, unanswered
  consultations and candidate false fails.
- The accuracy numbers the spike measured apply to the configuration koto
  sends.

### Negative

- Templates that declare a decider check don't compile on older koto, unlike
  routing declarations, which older koto drops. Adoption in shirabe has to
  wait for a release and raise its minimum koto.
- One request per criterion costs more round trips than a batch, and a state
  with four criteria takes the whole per-call cap.
- During a provider outage no veto criterion is checked; the work passes
  ungraded, visibly so in the records.
- A second `koto next` that finds the lock held waits up to 17 s at the
  default timeout (60 s at most) before its criteria go `busy`.
- An agent can empty a slice by removing the thing it grades.

### Mitigations

- Every criterion ships in shadow; nothing blocks until a maintainer sets
  `veto` and a user opts in at `auto`.
- A criterion that goes unanswered is asked again on the next evaluation
  in the visit; verdicts already read are reused, so a retry only re-asks
  what got no answer.
- Batching is a measured change away if cost matters: the request shape
  already supports several questions.
