---
schema: prd/v1
status: Done
problem: |
  Agents running koto workflows answer every branch decision themselves, even
  narrow closed-set ones over inputs koto already stores. Each costs a turn and
  context, varies between runs, and leaves no confidence record, and template
  authors have no safe way to find out whether a cheaper typed decider could
  settle a given decision, or to let it once the evidence says it can.
goals: |
  Template authors can declare a decision once and, on machines where a user
  has opted in, have koto consult a typed decider for it: first in shadow,
  where nothing changes for the run, then automatically for the specific
  answers whose recorded agreement clears a stated bar. Runs without opt-in,
  or with an unsure decider, behave exactly as today, and shirabe ships its
  first eligible decisions.
absorbed:
  - docs/briefs/BRIEF-jev-decision-offload.md
---

# PRD: Decisions koto can settle without the agent

## Status

Done

Absorbed [BRIEF-jev-decision-offload](docs/briefs/BRIEF-jev-decision-offload.md); carried in Absorbed Brief.

## Absorbed Brief

The feature was framed around a gap between what a workflow decision costs and
what it needs. Many koto branches are narrow closed-set judgments over inputs
koto already stores, yet every one costs the agent a turn and context, varies
between runs, and leaves no record of how sure the call was. Template authors
had no way to find out whether a cheaper typed decider could take a given
decision without risking the run. That problem is this document's Problem
Statement.

The outcome it asked for is that an author declares a decision once. Agents
then move past it once a decider has shown it agrees with them, or answer it
exactly as today when there's no decider or the decider is unsure. Maintainers
see every automatic decision and its agreement record. Those are this
document's Goals and Success Measures.

Four situations grounded it, and they survive as the User Stories: a shirabe
maintainer declaring the residual drift judgment; an agent passing the
issue-type decision without being asked; a koto maintainer promoting only the
common answer after reading agreement; and an offline contributor with no key
who must see ordinary prompts. The boundary it drew is the one Requirements and
Out of Scope hold: decisions only, never text or gates, no standalone command,
trust earned per answer rather than toggled per run, and prose-only skills left
for later.

## Problem Statement

When a koto workflow reaches a branch it can't resolve from gates, it stops
with `evidence_required` and the agent picks a value. Many of these branches
are narrow: one value out of a small closed set, judged from inputs koto
already holds in its context store. shirabe's `/work-on` asks on every
plan-backed issue whether the plan outline item is clear enough to code
against. `/execute` asks whether upstream drift changed a plan's intent, and
`/work-on` asks whether an issue is code, docs, or task work.

Each of these costs the agent a turn and pulls criteria prose into a context
it needs for the real work. The answers can differ between runs on the same
input, and the session log records the value chosen but not how clear-cut the
choice was. Hosted typed decision models (Jev is the first widely available
one) now answer this kind of question with calibrated probabilities in a few
hundred milliseconds, without generating text. koto can't use them. It has
nowhere to declare that a decision is eligible, no way to run a decider
without risking the run, and no record from which a maintainer could decide
that a decider is safe to trust for a given answer.

Some shirabe "decisions" are worse still: they aren't judgments at all. The
agent re-types facts a gate already computed, such as whether every child in
a batch succeeded, and a mistyped answer silently misroutes the run.

## Goals

- A template author describes a decision once, and that declaration serves
  both the agent and the decider.
- Without explicit opt-in, workflows behave exactly as they do today and
  nothing leaves the machine. The only visible difference is that each answer
  arrives with its description.
- A maintainer can run a decider in shadow on real traffic, read how often it
  agreed with agents per question and per answer, and enable automatic
  settling only for answers that clear the R22 bar. From then on, agents move
  past those decisions without being asked.
- Terminal exits, confirmation-guarded steps, gate-conditioned routes, and
  failed gates stay out of any decider's reach whatever a template declares.
- shirabe's first three eligible decisions run in shadow, and its
  mechanically computable "decisions" become gates.

### Success Measures

`koto decider report` (R21) computes these per question from the ledger.
They're reported rather than release-gating:

- **Agent stops removed**: the count of visits settled automatically, the
  stops an agent would otherwise have answered.
- **Coverage**: of the consultations, the share whose winning value met its
  threshold and wasn't the escape. A question below 30% coverage after 30
  consultations in `auto` is flagged as not worth its latency.
- **Directive bytes not delivered**: summed from the size recorded on each
  auto-settled consultation. This is a lower bound on context saved, because
  koto can't see reference files an agent would have read.

## User Stories

These carry the four journeys summarized in Absorbed Brief.

- As a **shirabe maintainer reshaping `/execute`'s drift check**, I want to
  declare the remaining judgment as a typed question with per-answer
  descriptions, an escape answer, and the stored facts it reads, so that it
  can be settled by a decider later without changing what users without one
  see.
- As an **agent driving `/work-on` where a user opted in and an answer has
  been promoted**, I want koto to settle the issue-type decision and hand me
  the next real step, so that I never load the classification criteria or
  spend a turn on it.
- As a **koto maintainer**, I want to read per-question, per-answer agreement
  between the decider and agents from shadow runs and golden fixtures, so
  that I can enable automatic settling for the common answer while leaving
  the rare, expensive one with the agent.
- As an **open-source contributor with no API key working offline**, I want
  every eligible decision to reach me as an ordinary prompt, so that nothing
  in my run depends on a network call or a paid service.

## Definitions

- **State visit**: the span that begins when a session enters a state and
  ends when it leaves it. It begins with a `transitioned` or
  `directed_transition` event targeting the state (including a self-loop),
  with a `rewound` event targeting it, or with workflow initialization into
  it. It ends with the next such event. A visit is identified by the session
  name, the state name, and the sequence number of the event that began it.
- **Consultation**: one attempt to get the decider's answer for one state
  visit, covering every declared field on that state. An attempt that stops
  before sending a request (R13) is still a consultation.
- **Winning value**: for an enum field, the value with the highest
  probability. For a boolean field, `true` or `false` when its threshold is
  met per R2, and otherwise none, which is recorded as `escape`.
- **Confidence**: the winning value's probability as reported by the
  provider.
- **Opted in**: the effective global mode (R17) is `shadow` or `auto` and an
  API key is available (R16).

## Requirements

### Declaration (koto templates)

- **R1.** A template may declare an `accepts` field of type `enum` as
  decider-eligible with a field-level `decider` block. The block carries: a
  description for every value in `values`; exactly one escape value that is
  not in `values`, with its own description; the inputs; and, per value, a
  mode (R3) and a threshold. The field `description` is the question. All
  new keys live inside the field; no new state-level or frontmatter key is
  introduced.
- **R2.** A `boolean` field may declare a `decider` block with the
  proposition as the field description, a mode and threshold for `true` and
  for `false`, and the inputs. There's no escape value: the winning value is
  `true` when P(true) is at or above the `true` threshold, `false` when
  P(false) is at or above the `false` threshold, and otherwise the result is
  treated as the escape. If both qualify, the result is also the escape.
- **R3.** Per-value modes are `off`, `shadow`, `auto`, and `never`. `never`
  is template-only and means the value may be consulted but never applied.
  A value with no declared mode is `shadow`. A value with no declared
  threshold uses 0.9.
- **R4.** Inputs are a list of entries, each naming a context-store key, a
  template variable, or a captured value, with a label and a byte budget
  (default 8192). A context key written by an earlier state's
  `default_action` is a valid input.
- **R5.** The compiler rejects a declaration that: lacks a description for
  any value; on an enum field, lacks the escape or its description, or names
  an escape value that's also in `values` or appears in any `when` clause; declares a threshold outside [0.5, 1.0];
  names an input that isn't a declared variable, capture, or a context key
  the template writes or gates on; or sits on a state that has another
  required field without a `decider` block.
- **R6.** For each value `v` declared `auto`, the compiler examines every
  transition whose `when` clause includes the field with value `v`. It
  rejects the declaration if any such transition: targets a terminal state;
  targets a state whose `default_action` has `requires_confirmation: true`;
  or has a `when` clause that also tests any `gates.*` key. This is the
  floor, and nothing else in the template can relax it.
- **R7.** The compiled template omits every new field when it's unset, so a
  template with no `decider` block keeps its existing compiled form and
  `template_hash`.

### Agent-facing contract

- **R8.** For a declared field, `koto next` and `koto status` add two keys to
  the field's `expects` entry: `description` (the question) and
  `value_descriptions` (an object mapping each value to its description).
  The escape value never appears. Evidence that submits the escape value is
  rejected like any value outside `values`. Responses for fields without a
  declaration are byte-identical to today.
- **R9.** No new `NextResponse` variant and no new required response field is
  introduced. A state settled by the decider doesn't appear in the response:
  the agent receives the next stop, with `advanced: true`.

### Consultation (koto runtime)

- **R10.** Only `koto next` advancing a session may consult. `koto status`,
  retrieval, export, and every other read-only path never do.
- **R11.** koto consults for a state only when all of these hold: the user
  has opted in; the state would otherwise return `evidence_required`; it has
  at least one declared field; no gate on the state failed on this tick; at
  least one declared value has an effective mode other than `off`; and this
  visit hasn't been consulted yet.
- **R12.** A visit gets at most one consultation, whatever its outcome,
  including errors, timeouts, and `input_unavailable`. The result is reused
  on every later `koto next` for that visit. A tick skipped because a gate
  failed doesn't use up the visit's consultation. Consultation happens while
  koto holds the session's state-file lock, so two concurrent `koto next`
  calls on one visit produce one consultation.
- **R13.** Before consulting, koto assembles the inputs. If any context key
  is unset, or any input exceeds its byte budget, koto doesn't call the
  provider. It records the consultation with outcome `input_unavailable` and
  falls back.
- **R14.** koto applies the answer only when, for every declared field on
  the state: the winning value's effective mode is `auto`; its confidence
  meets its threshold; and it isn't the escape. The resulting evidence must
  also match exactly one conditional transition. When all of that holds,
  koto records the fields together as one evidence submission marked
  decider-sourced, and advances. Otherwise it applies nothing.
- **R15.** When the answer isn't applied, `koto next` returns exactly the
  response it would have returned for a user who hasn't opted in. That
  covers every mode and threshold outcome, the escape, `input_unavailable`,
  a timeout, a connection failure, an HTTP error, and a malformed or
  mismatched response. With no API key the user isn't opted in, so no
  consultation happens and nothing is recorded. The decider's answer is never shown to the agent.
- **R16.** The API key comes only from the `KOTO_DECIDER_API_KEY`
  environment variable or `decider.api_key` in user config. The endpoint
  comes only from `KOTO_DECIDER_ENDPOINT` or user config, defaulting to the
  provider's public endpoint. koto ignores both keys in project config when
  loading configuration. The key never appears in the event log, the
  ledger, error text, or `koto config` output (including `koto config get`,
  which prints `<set>`).
- **R17.** The effective mode of a value is the minimum, in the order
  `off` < `shadow` < `auto`, of: the global mode from `KOTO_DECIDER` (when
  set) or else user config `decider.mode` (default `off`); project config
  `decider.mode` (when set); and the template's mode for that value, where
  `never` counts as `shadow` for consulting and is never applied. An
  unrecognised `KOTO_DECIDER` or `decider.mode` value is treated as `off`
  and koto prints a warning to stderr.
- **R18.** A consultation sends the provider only the question text, value
  and escape descriptions, and the assembled inputs, with each input already
  within its budget. It sends nothing else from the session.
- **R19.** Each consultation has a timeout, configurable as
  `decider.timeout_ms` in user config, defaulting to 2000 ms. There's no
  retry. A single `koto next` performs at most 4 consultations. Past that,
  each further decider-eligible state is handled as if the user hadn't
  opted in.
- **R20.** The decider sits behind a provider-neutral interface. Jev is the
  first and only shipped provider. Every consultation records the provider
  name and the model version string the provider returns, or `unknown` when
  the response carries none.

### Record and evaluation

- **R21.** Every consultation appends one `decider_consulted` event to the
  session log and one `consulted` record to the ledger. Both carry:
  - the visit identifier, state, and fields;
  - provider and model;
  - the declaration hash (R23) and a SHA-256 of the assembled input;
  - per-field probabilities, the winning value, and confidence;
  - effective modes;
  - an overall outcome: `applied`, `not_applied`, `input_unavailable`, or
    `error`;
  - for each field, its own outcome: `qualified` (winning value in `auto`
    and at or above threshold), `shadow` (winning value not in `auto`),
    `never`, `below_threshold`, or `escape`;
  - error class, latency in milliseconds, and the byte size of the directive
    and details the agent would have received.

  When the agent later submits evidence for a consulted visit that wasn't
  applied, koto appends an `answered` record to the ledger with the visit
  identifier and the agent's values.

  The ledger is `_decider_ledger.jsonl` in the koto home directory, next to
  the terminal index. It's append-only, one JSON object per line, each line
  at most 4 KiB, and neither session cleanup nor workspace pruning deletes
  it. Neither the event nor the ledger contains input content or
  credentials. The event is additive: the schema version isn't bumped, and
  the session-feed contract lists it. A failed ledger write prints a warning
  to stderr and never fails `koto next`.
- **R22.** `koto decider report` reads the ledger and joins `consulted` and
  `answered` records by visit identifier, skipping and counting malformed
  lines. Terms, per declaration hash:
  - a **paired observation** is a visit with both records;
  - **recall** for value `v` is, among paired observations where the agent
    chose `v`, the share where the decider's winning value was `v` at or
    above threshold (against fixtures, the fixture label replaces the agent's
    value);
  - **coverage** for `v` is the share of consultations that reached the
    provider and got a well-formed answer (so excluding `input_unavailable`
    and `error`) whose winning value was `v` at or above threshold, and
    question coverage is the sum over values.

  It prints, per declaration hash and per value:
  - paired observations (visits with both records);
  - a confusion matrix;
  - recall;
  - coverage;
  - disagreements, meaning paired visits where the decider's winning value
    met its threshold and differed from the agent's value;
  - fallback and error rates;
  - latency p50 and p95;
  - the Success Measures.

  With `--fixtures <path>` it also runs a golden fixture set against the
  configured provider and marks a value promotion-eligible when all of these
  hold:
  - the set holds at least 10 cases labelled with every value (the escape
    needs none; for booleans the values are `true` and `false`) and 40 in
    total;
  - no fixture labelled otherwise is answered with that value at or above
    its threshold;
  - macro recall exceeds always choosing the most frequent label;
  - the ledger holds at least 30 paired observations for the question under
    the current declaration hash, with at most one disagreement where the
    decider chose that value.

  The fixture file is JSON Lines. Each line holds the input map by label and
  the expected value. Running fixtures requires opt-in and network access,
  and without them the command says so and exits non-zero. The report never
  changes a mode.
- **R23.** The declaration hash covers the question, the values, the value
  and escape descriptions, and the inputs (sources, labels, budgets). It
  excludes modes and thresholds, so promoting a value doesn't discard the
  evidence that justified it. Changing any covered part changes the hash.

### shirabe

- **R24.** shirabe declares three decisions, each with golden fixtures that
  meet R22's minimums, stored beside the template:
  - `/work-on`'s `plan_validation` verdict reads the `context.md` outline
    item. `proceed` is `shadow` and `exit` is `never`.
  - A new single-field `issue_type` routing state in `/work-on` replaces
    today's double submission. It reads `context.md` and a changed-paths
    context key written by a new script. `code` is `shadow`; `docs` and
    `task` are `never`.
  - `/execute`'s `worktree_discipline_check` reads the facts key and the
    plan-intent key that R25's script writes. Every value is `never`.
- **R25.** Before the drift question, `/execute` computes its facts in a
  script: whether main advanced, which changed paths overlap the paths the
  plan references, and which referenced paths were deleted. The script
  writes them to a facts context key. It also copies the PLAN's goal and
  scope sections to a plan-intent context key. With no overlap, the run routes to `none`
  without asking anyone. The existing gate on the agent-written impact file
  is replaced by a gate on the facts key.
- **R26.** `/execute` routes `batch_outcome` from the `children-complete`
  gate's `all_success` and `needs_attention` fields instead of asking the
  agent. `/work-on` drops the `context_gathered` field, which routes nowhere.
- **R27.** Every changed shirabe template compiles on koto v0.12.2, and a
  scripted run of it there takes the same transitions as it does with every
  `decider` block removed. shirabe records v0.12.2 as its minimum koto
  version where it states one.

### Non-functional

- **R28.** A consultation adds at most its timeout plus 250 ms to `koto
  next`. A tick that doesn't consult makes no network call and writes no
  `consulted` record.
- **R29.** No new async runtime or HTTP crate. The provider client uses the
  synchronous HTTPS client koto already links.
- **R30.** koto's test harnesses run with `KOTO_DECIDER=off` unless a test
  opts in, so an exported key never reaches a provider during `cargo test`.
- **R31.** The `koto-author`, `koto-user`, and `koto-adhoc` skills document
  the declaration, modes, opt-in, and the report. `scripts/run-evals.sh
  --all` passes at or above each skill's pass rate before the change, and
  the pull request records the results.

## Acceptance Criteria

Unless stated otherwise, each criterion runs in CI against a local stub
decider reached through `KOTO_DECIDER_ENDPOINT`. The stub counts requests,
records payloads, and can return any answer, delay, or error.

### Declaration and compatibility

- [ ] A template with valid enum and boolean declarations compiles.
- [ ] Each R5 violation fails compilation with a message naming the field
      and the rule. Cases covered: a missing value description, a missing
      escape description, an escape inside `values`, an escape in a `when`
      clause, a threshold of 0.4, a threshold of 1.1, an unknown input, and
      a sibling required field without a declaration. Thresholds of exactly
      0.5 and 1.0 compile. A boolean declaration with no escape compiles.
- [ ] `auto` on a value whose transition targets a terminal state fails
      compilation, as does `auto` on a value whose target has a
      confirmation-required `default_action`, and `auto` on a value in a
      `when` clause that also tests `gates.*`. The same templates with that
      value in `shadow` compile.
- [ ] A value with no declared mode is recorded as `shadow`, and one with no
      threshold is compared against 0.9. An input with no declared budget
      falls back as over-budget at 8193 bytes and not at 8192.
- [ ] A declaration whose input is a context key written by an earlier
      state's `default_action` compiles, and at runtime the stub payload
      carries that key's content under its label. A template variable and a
      captured value work as inputs the same way.
- [ ] A template with no `decider` block has the same `template_hash` and the
      same `koto next` and `koto status` JSON before and after the feature,
      and the existing response baseline tests pass unchanged.
- [ ] A CI job downloads the koto v0.12.2 release binary, compiles a template
      containing enum and boolean declarations, and runs a scripted session
      through it. The transitions match the same script run with the
      declarations removed, and the escape value never appears in its
      responses.

### Agent-facing contract

- [ ] For a declared field, the `expects` entry carries `description` and a
      `value_descriptions` object with one key per value, in both `koto next`
      and `koto status`, and never the escape value.
- [ ] Submitting the escape value as evidence is rejected with the same error
      as any value outside `values`.

### Opt-in, configuration, and data

- [ ] With a key set and no mode configured, a scripted run over declared
      states makes zero stub requests.
- [ ] With the user mode `off`, or `KOTO_DECIDER=off`, a template declaring
      `auto` makes zero stub requests.
- [ ] With the user mode `shadow` and the template `auto`, the answer is not
      applied. With the user mode `auto` and project config `shadow`, it is
      not applied either. With project config `auto` and the user mode
      `shadow`, it is not applied, which shows project config can't raise
      the mode.
- [ ] A key or endpoint present only in project config is ignored: with the
      user endpoint pointing at stub A and project config naming stub B,
      stub B receives nothing.
- [ ] `KOTO_DECIDER=never` and `KOTO_DECIDER=bogus` each make zero stub
      requests and print a warning to stderr.
- [ ] `koto config get decider.api_key` prints `<set>`, and the key string
      appears nowhere in the session log, the ledger, or stderr during a run
      that includes an HTTP 401 from the stub.
- [ ] The payload the stub receives contains exactly the question, the value
      and escape descriptions, and the labelled inputs, and no other session
      content.
- [ ] Running `cargo test` with `KOTO_DECIDER_API_KEY` exported makes zero
      requests to a stub configured as the default endpoint.

### Consultation behavior

- [ ] In shadow mode, with the stub returning each value in turn (including
      a confident wrong one), the `transitioned` events and every
      agent-visible response are identical to a run with the user mode
      `off`.
- [ ] In auto mode, an answer at or above threshold for an `auto` value
      advances past the state without an `evidence_required` response. The
      log shows one `decider_consulted` event with outcome `applied`, then
      one decider-sourced evidence submission, then the transition.
- [ ] On a state with two declared fields, where one field's answer
      qualifies and the other's is below threshold, neither is applied, and
      the event records `qualified` for the first field, `below_threshold`
      for the second, and `not_applied` overall.
- [ ] An `auto` answer with confidence exactly equal to its threshold is
      applied.
- [ ] For a boolean field with `true` at 0.9 and `false` at 0.9, P(true) of
      0.95 applies `true` in auto, 0.05 applies `false`, and 0.5 is not
      applied and records `escape`. `auto` on a boolean value whose
      transition targets a terminal state fails compilation.
- [ ] A state whose declared values are all `off` makes no stub request.
- [ ] A declared state reached by auto-advance within a `koto next` is
      consulted, not only the state the call started on.
- [ ] An `auto` answer that matches no conditional transition, or matches
      more than one, is not applied, and the response equals the opted-out
      response.
- [ ] Each of these returns the opted-out response and records its outcome:
      a timeout, a refused connection, an HTTP 500, an HTTP 401, a malformed
      body, a response naming an undeclared value, below threshold, the
      escape, a `never` value, an unset input context key, and an input over
      its byte budget. No stub request is made in the last two cases.
- [ ] With the mode set to `auto` and no API key, the run makes zero stub
      requests and records no `decider_consulted` event.
- [ ] With no `decider.timeout_ms` set, a stub delaying 2500 ms produces
      outcome `error` with class `timeout`.
- [ ] With a 200 ms timeout and a stub that delays 10 s, `koto next` returns
      the opted-out response in under 2 s, the event records outcome `error`
      with class `timeout`, and its recorded latency is at most 450 ms.
- [ ] A provider response with no model version records model `unknown`.
- [ ] Three `koto next` calls within one visit make exactly one stub request,
      including when that request timed out or stopped at
      `input_unavailable`. A rewind into the state, and a self-loop back into
      it, each make one more.
- [ ] Two `koto next` processes started concurrently on the same visit make
      exactly one stub request between them.
- [ ] A state whose gate failed makes no stub request. After the gate passes
      on a later tick, the visit is consulted once.
- [ ] `koto status` on a declared state makes no stub request.
- [ ] A template chaining five consecutive auto-eligible states makes at most
      four stub requests in one `koto next`, and the fifth state returns
      `evidence_required`.
- [ ] The `decider_consulted` event carries the provider name and the model
      string the stub returned.
- [ ] A `koto next` on a state with no declaration makes no stub request and
      appends no `consulted` record.
- [ ] `cargo tree` shows no `tokio`, `hyper`, `reqwest`, or `ureq` in the
      dependency graph.

### Record and report

- [ ] A parent run with two decider-consulting children, taken to terminal
      with default cleanup, leaves one `consulted` record per child
      consultation in the ledger, plus an `answered` record for each one
      the agent answered.
- [ ] No event or ledger line contains any input string from the payload,
      and every ledger line is at most 4 KiB.
- [ ] In a single non-child session, a shadow consultation followed by an
      agent answer leaves a `consulted` and an `answered` record with the
      same visit identifier in `_decider_ledger.jsonl` in the koto home
      directory. The `consulted` record carries the declaration hash, input
      SHA-256, directive byte size, latency, and effective modes. The file
      is still present after `koto workspace prune`, and the session header's
      schema version is unchanged.
- [ ] With the ledger file made read-only, a consulting `koto next` succeeds,
      returns the same response, and prints a warning.
- [ ] `docs/reference/session-feed.md` lists `decider_consulted`.
- [ ] `koto template validate-feed` accepts a log containing
      `decider_consulted`, and koto v0.12.2 runs `koto status` and `koto next`
      on that session without error.
- [ ] Given a fixture ledger with known pairs, `koto decider report` prints
      the expected per-value paired counts, confusion matrix, recall,
      coverage, disagreements, fallback rate, latency percentiles, and
      Success Measures.
- [ ] Starting from a fixture set and ledger that meet every R22 condition
      (the value is marked eligible), each single change below makes it
      ineligible: 9 cases for the value; 39 cases in total; 29 paired
      observations; 2 disagreements where the decider chose the value; one
      fixture labelled otherwise answered as the value at threshold; macro
      recall equal to the majority baseline.
- [ ] Running the report leaves every template and config file unchanged.
- [ ] A ledger containing a malformed line produces a report that skips it
      and prints the skipped count.
- [ ] Changing a value description, the question, the escape description,
      or an input budget changes the reported declaration hash. Changing a
      threshold or a mode doesn't.
- [ ] `koto decider report --fixtures` with no opt-in prints that fixtures
      need an opted-in decider, and exits non-zero.

### shirabe

- [ ] The three shirabe declarations compile, and their modes match R24
      exactly. Each has a fixture file with at least 10 cases per value and
      40 in total.
- [ ] With no overlap between upstream changes and plan paths, `/execute`
      reaches `spawn_and_await` without an agent answer about drift. With
      overlap, the agent is asked, and the facts key lists the overlapping
      paths. With a deleted referenced path, the facts key lists it.
- [ ] `/execute` no longer asks the agent for `batch_outcome`: a batch whose
      gate reports `all_success: true` routes to the success path, and one
      reporting `needs_attention: true` routes to the attention path. `/work-on`'s
      `accepts` blocks no longer contain `context_gathered`, and `issue_type`
      is requested once per run.
- [ ] The CI job from the compatibility section runs each changed shirabe
      template on koto v0.12.2 with the same transitions as with its
      `decider` blocks removed, and shirabe's documented minimum koto version
      reads v0.12.2.

### Documentation

- [ ] The three koto skills describe the declaration, modes, opt-in, and the
      report. The pull request shows `scripts/run-evals.sh --all` results at
      or above the prior pass rate for each skill.

## Out of Scope

- **Per-item question sets and aggregation.** Asking one question per
  acceptance criterion or finding, and combining answers (counts, argmax,
  score banding). Several large wins live there, but they need their own
  design, and the score primitive waits with them.
- **Porting prose-only shirabe skills** (explore, plan, review-plan, the
  document validation juries) onto koto templates.
- **Automatic promotion,** and sampled shadow after promotion.
- **Any decider use that produces text,** or that evaluates gates.
- **A standalone command for asking the decider questions** outside a
  workflow step.
- **Promoting any v1 shirabe value to `auto`.** v1 ships everything in shadow
  or `never`, and promotion follows the data.
- **Other providers, local models, and vendor comparison.**
- **Cross-machine ledger aggregation, ledger compaction, and harvesting
  golden cases from real inputs.**
- **The remaining computable shirabe "decisions"** (`pause_decision`,
  `cascade_status`, retry caps, verification-map matching). Each needs koto
  routing features (variable equality, visit counts) or its own script.
- **`task_validation` and `staleness_check` as targets,** and fixing how a
  batch classifies `/work-on`'s `validation_exit` terminal.

## Known Limitations

- Organic traffic yields about one or two observations per question per week,
  and almost none for rare answers. Golden fixtures carry the recall
  evidence, so promotion is only as good as the fixtures.
- Agreement with the agent measures imitation, not correctness. For v1's
  targets, maintainers assign golden labels against the artifact.
- The ledger is local to one machine. Evidence gathered on another machine
  isn't merged.
- Inputs can include agent-written text, so semantic injection is possible.
  The R6 floor and per-value `never` bound what a manipulated answer can do,
  but they don't prevent it.
- `/work-on`'s `validation_exit` terminal counts as success in a batch
  today. R6 keeps a decider away from it, but the routing itself is
  unchanged.
- Demoting a promoted value reaches users only when they update shirabe. The
  user-level `KOTO_DECIDER` switch is the immediate lever.

## Decisions and Trade-offs

- **Evidence provider, not a gate or a transition resolver.** The decider
  answers the state's own `accepts` field, so one set of `when` clauses
  serves both paths and the fallback is today's response. A gate type would
  leak probabilities into `blocking_conditions`. A resolver would bypass the
  evidence model.
- **Field-level declaration only, with the escape outside `values`.** Older
  koto binaries fail to compile an unknown state-level key but silently drop
  unknown field-level keys. Keeping the escape out of `values` means an older
  koto never offers it to the agent.
- **Opt-in is global and off by default.** An exported key alone does
  nothing. A user must choose `shadow` or `auto`, because consulting sends
  repository content to a third party.
- **Shadow by default in templates, per-value auto.** Every v1 target has one
  cheap answer and one expensive one, and a single threshold per question
  can't express that.
- **The floor is structural.** koto can't see "irreversible" or "override" as
  concepts. Terminal targets, confirmation-guarded targets, and
  gate-conditioned routes are checkable stand-ins, and per-value `never`
  covers the rest.
- **One consultation per visit, whatever the outcome.** Retrying on later
  ticks would re-bill on every poll and make the answer depend on timing.
- **An append-only ledger with two record kinds.** Child and abandoned
  sessions never reach a terminal tick, so records are written when they
  happen, and the report joins them.
- **The hash excludes modes and thresholds,** so promotion edits keep the
  evidence that justified them.
- **Naming.** koto already uses "epoch" and `koto decisions` for other
  things, so this feature says "state visit", and the event and verb are
  `decider_consulted` and `koto decider report`.
- **No new `condition_type` value.** The session-feed contract fixes those
  values, so the decider source is carried on the evidence and the event
  instead.
- **Raw agreement and wall-clock are not goals.** Labels are almost all the
  common answer, so a decider that always answers `proceed` would score near
  100%, and each decision takes a few seconds. The bar is built on per-answer
  recall and coverage instead.
