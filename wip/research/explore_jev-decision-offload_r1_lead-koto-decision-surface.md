# Lead: Where does koto hand a decision to the calling agent today, and which extension point would an embedded classifier use?

## Findings

### 1. The one place koto asks the agent to decide

Everything funnels through `advance_until_stop` in `src/engine/advance.rs`
(line ~545). Each loop iteration runs, in order: shutdown check, chain limit,
terminal, `integration`, `default_action`, gates, `skip_if`, then
`resolve_transition`. The loop hands a decision to the agent at exactly one
spot: `resolve_transition` returns `TransitionResolution::NeedsEvidence` and
the state has an `accepts` block, which yields `StopReason::EvidenceRequired`
(advance.rs ~1110). The CLI turns that into `NextResponse::EvidenceRequired`
(`src/cli/next_types.rs:64`) carrying `directive`, `details`, `advanced`,
`expects` (field schema plus `options`, the list of `{target, when}` pairs),
and `blocking_conditions`.

Other stops (`GateBlocked`, `ActionRequiresConfirmation`, `Integration*`,
`Terminal`, the capture-refusal stops) hand the agent a problem to fix or an
acknowledgement to give, not a branch choice. So the branch-decision surface
is narrow and well defined: "conditional transitions exist, no `when` matched,
and the state accepts evidence".

`NeedsEvidence` fires in three cases (`resolve_transition`, advance.rs ~1235):
no conditional `when` matched and there is no unconditional fallback; a gate
failed on a state with `accepts` (unconditional fallback suppressed); or the
state was reached by auto-advance with no fresh evidence and has conditional
transitions (the `fresh_evidence` guard). The third case is the one a
classifier most naturally fills: the engine chained into a decision state and
is about to stop only because nobody has answered yet.

### 2. How evidence and `when` routing work

- Template side: `accepts` is `BTreeMap<String, FieldSchema>` where
  `FieldSchema { field_type, required, values, description }`
  (`src/template/types.rs:178`). Types include `enum` (with `values`),
  `string`, `boolean`, `number`, `tasks`.
- Submission: `koto next --with-data` validates against `accepts` with
  `validate_evidence` (`src/engine/evidence.rs:45`, called at
  `src/cli/mod.rs:4282`) and appends `EventPayload::EvidenceSubmitted { state,
  fields, submitter_cwd }` (`src/cli/mod.rs:4364`).
- Merge: `derive_evidence` (`src/engine/persistence.rs:789`) takes every
  `EvidenceSubmitted` event for the current state since the last
  `Transitioned`/`DirectedTransition`/`Rewound` into it;
  `merge_epoch_evidence` does last-write-wins.
- Matching: `when` is exact JSON equality per key, with dot-path traversal
  (`gates.<name>.<field>`), plus two special matchers (`evidence.<f>: present`,
  `vars.<name>: {is_set: bool}`). There is no numeric comparison, so a `when`
  clause cannot express "confidence >= 0.85" today.
- Gate output is merged into the resolver's evidence map under `gates` only
  when the state has a `gates.*` reference in a `when` or `skip_if`
  (`has_gates_routing`), and it is injected regardless of pass/fail.
- In the loop, `current_evidence` is reset to empty after each auto-advance
  and `fresh_evidence` set false, so only the first state in a tick sees agent
  evidence.

Concrete shape in shirabe (`skills/work-on/koto-templates/work-on.md`): 30
`accepts` blocks, most of them a required `enum` verdict (`verdict: [proceed,
exit]`, `[ready, needs_design, exit]`, `status: [completed, override,
blocked]`) plus optional free-text `rationale`/`detail`. That enum-plus-values
schema maps directly onto a Jev `choice`; a `boolean` field maps onto `noul`.
`FieldSchema.values` has no per-option description, which Jev needs.

### 3. Gate evaluators

`src/gate.rs` has four built-in types (`command`, `context-exists`,
`context-matches`, `children-complete`), dispatched in `evaluate_gates`
(gate.rs:70), each returning `StructuredGateResult { outcome, output }`. Types
have fixed output schemas (`gate_type_schema`, types.rs:546) and built-in
override defaults. The CLI builds the gate closure (`src/cli/mod.rs` ~4480)
with field substitution first. Gate results are logged as `GateEvaluated`
events and failed gates surface to the agent as `blocking_conditions` with
their full `output`, and can be overridden with `koto overrides record`.

### 4. Existing precedent for koto calling out: `integration`

`TemplateState.integration: Option<String>` and step 4 of the loop already
exist, designed in DESIGN-unified-koto-next.md as a "processing integration"
bound to user config with graceful degradation when unavailable. The CLI
closure is a stub that always returns `IntegrationError::Unavailable`
(`src/cli/mod.rs:4508`). It is the wrong shape for this job: an integration
*stops* the loop and hands its output to the agent, which is the opposite of
"the agent never sees the decision". It is still useful precedent for
"declared in the template, bound by config, degrades when not configured".

### 5. Precedent for koto doing the agent's work: `default_action`

DESIGN-koto-runs-commands.md and BRIEF-koto-runs-commands.md set the pattern
this feature should copy:

- Success is invisible: the command runs, its captured stdout is delivered
  through an additive event (`VariableCaptured`) into a live overlay, and the
  loop keeps going. Decision 1 explicitly rejected threading per-state output
  through the `NextResponse` variants (five variants, hand-written Serialize,
  three combinators) because the acting state and the stopping state diverge
  on the auto-advance path.
- Failure becomes a handoff using existing response shapes: Decision 3/4 route
  failure through `GateBlocked` under a reserved condition `__action__` and
  splice author-written `fallback` prose onto the directive, rather than
  adding an eighth response variant.
- If override evidence already exists, the action is skipped
  (`ActionResult::Skipped`) — agent input wins over engine work.
- I/O is injected as closures into `advance_until_stop` for testability.

### 6. Which extension point fits

Four candidates, judged against the code:

**New gate type (`type: classifier`).** Fits the evaluator plumbing
(closure, `GateEvaluated` event, `gates.*` routing). But: gate output only
routes via `gates.<name>.choice` keys, so every decision state would need two
parallel sets of `when` clauses (one on `gates.cls.choice`, one on the agent's
`verdict`) with ambiguity risk; a low-confidence result has to be a "failed"
gate, which the agent then sees in `blocking_conditions` with the full
probability distribution (anchoring, and it reads as a corrective problem);
and the override mechanism (`koto overrides record`) would apply to a
decision, which is semantically odd. Gates judge whether work is done; this
is not that.

**Transition resolver (classifier picks the target directly).** Tempting but
it bypasses evidence entirely: the `when` clauses the agent path uses would
not be the ones the classifier path uses, the log would record a
`Transitioned` with no record of *why*, and replay/audit would diverge from
the evidence model.

**Evidence provider (recommended).** The classifier answers the state's own
`accepts` fields, exactly as the agent would, and the existing `when` clauses
route on the result. One set of transitions serves both paths; the
`expects` schema the agent would have seen is the question schema the
classifier is asked (enum `values` -> choice keys, `boolean` -> noul); the
answer is recorded as an ordinary `EvidenceSubmitted` event so
`derive_evidence`, `merge_epoch_evidence`, rewind, and resume need no change.
`EvidenceSubmittedPayload` is not `deny_unknown_fields`, and STABILITY.md
treats additive optional fields as non-breaking, so a
`source: {kind: "classifier", model, confidence, probabilities, ...}` field
(or a sibling additive event such as `ClassifierDecided` preceding it) is a
compatible audit record.

**Where it hooks in the loop.** Not as a new numbered step before
resolution, but in the `NeedsEvidence` arm of step 8 — i.e. at the exact
moment the engine would otherwise stop with `EvidenceRequired`. Conditions:
state declares a classifier decision, state has `accepts`, no gate failed
(`gates_failed == false`; a classifier should not paper over a failing gate),
and the classifier has not already been tried for this state in this tick.
Mechanically: call a new injected closure (`classify(state, decl) ->
Result<ClassifierAnswer, ClassifierError>`) alongside the existing four;
on a confident answer, validate it with `validate_evidence`, append
`EvidenceSubmitted` (with source metadata), insert the fields into
`current_evidence`, set `fresh_evidence = true`, and `continue` so the same
state re-resolves. `skip_if` (deterministic, free) keeps priority because it
runs first. Agent-submitted evidence also keeps priority, because on the
first iteration agent evidence resolves the transition before this arm is
reached.

### 7. What `koto next` does and returns

**High confidence** (answer's confidence at or above the template's
threshold, and the answer is not an escape-hatch key): the loop records the
evidence and transitions, then keeps advancing. The agent receives whatever
the next real stop is — typically the next state's `EvidenceRequired` or a
`GateBlocked`/`Terminal` — with `advanced: true`. Nothing in the response
mentions the decision. This matches the runs-commands rule that successful
engine work leaves the response contract untouched. The decision is visible
in the event log (`EvidenceSubmitted` with classifier source, then
`Transitioned`, whose `condition_type` could become `"classifier"` instead of
`"auto"` as an additive value), and therefore in the dashboard/session feed
and `koto decisions`-style listings.

**Low confidence, escape-hatch key, no key configured, network error, or
timeout**: the arm falls through to today's behaviour and returns the normal
`EvidenceRequired` for that state — same directive, same `expects`, same
`options`. The agent does the work as if the classifier did not exist. The
classifier's distribution should *not* be shown to the agent by default: it
would anchor the agent, and it would contaminate the agent's answer as an
independent label for later concordance measurement. The declined attempt
should still be logged (additive event, e.g. `ClassifierDeclined` with
confidence and reason) so repeated `koto next` calls at the same state do
not re-bill the API, and so shadow-mode evaluation has data. Mid-band
routing (Jev's 0.50-0.85 "human review" band) maps onto "ask the agent" here;
there is no third outcome in koto's model.

## Implications

- The classifier is best framed as "an automatic evidence submitter for
  states the engine would otherwise stop on", not as a gate or resolver. This
  keeps a single routing surface (`when` on `accepts` fields) and reuses the
  event model end to end.
- No new `NextResponse` variant or response field is needed for v1. That is
  consistent with the runs-commands design and keeps the agent-facing
  contract stable.
- Template schema needs additions the other leads should pin down: per-state
  opt-in (e.g. a `decide:`/`classifier:` block naming which `accepts` field to
  answer), per-option descriptions for enum values (Jev needs them;
  `FieldSchema.values` is a bare list), an explicit escape-hatch key, a
  threshold, and a declared state payload (context keys, captures, gate
  output, prior evidence). `when` cannot express thresholds, so the threshold
  lives in the classifier declaration, not in routing.
- Only required enum/boolean fields can be classifier-filled. Required
  free-text fields (e.g. `task_description`) make a state ineligible, and
  optional `rationale` fields would be left empty or filled with a synthetic
  "decided by classifier (p=0.93)" note.
- A classifier-eligible state must be a pure decision state whose inputs
  already live in koto. That drives the shirabe-side reshaping.

## Surprises

- Most shirabe decision states bundle "do the work" with "report the
  verdict": the directive says "validate the issue / research the codebase"
  and the `accepts` enum reports the outcome. If a classifier filled that
  enum, the agent would skip the directive's work entirely. So the scope's
  premise that decisions can be moved "inside `koto next`" only holds after
  templates split work states from decision states, with the work's output
  stored in the context store or a capture where koto can assemble it into
  the Jev state. This is the real coupling between the koto and shirabe
  halves of the question.
- The `integration` field and loop step already exist as a designed but
  unimplemented "koto calls something external" hook (the runner always
  returns `Unavailable`). It stops the loop rather than continuing, so it is
  not reusable as-is, but a reader might assume it is the obvious place.
- koto has no HTTP client dependency today (nothing in `Cargo.toml`); every
  external effect so far is a subprocess. An embedded network call is new
  territory (lead 5).
- Gate output routing only turns on when a `when`/`skip_if` references
  `gates.*`, and failed gates expose their whole `output` to the agent — a
  concrete reason the gate-type option leaks classifier internals on the
  fallback path.

## Open Questions

- Should a declined classifier attempt be retried on a later `koto next` at
  the same state if its inputs changed (e.g. a context key was updated)? A
  hash of the assembled state payload in the declined event would allow
  that.
- Should the classifier run when a gate failed on a state with `accepts`, or
  only when gates passed? Recommendation is "only when gates passed", but
  some templates route on gate output plus a verdict.
- Should the agent ever see the classifier's suggestion on fallback (opt-in
  per template, e.g. as `details`), or never? Never by default is safest for
  evaluation; some workflows may want a hint.
- Can the agent veto an automatic decision after the fact? `koto rewind` or a
  directed transition already exist; whether that needs a first-class
  "contest" path is open.
- Should the classifier also fill the initial state's evidence when the
  agent calls `koto next` with no `--with-data`, or only states reached by
  auto-advance? The `NeedsEvidence`-arm hook covers both; whether that is
  wanted for the first state is a policy question.
- Batch/child workflows: `koto next` on a coordinator ticks many sessions;
  parallel questions in one Jev request could batch decisions across
  children, which the per-state closure shape does not exploit.

## Summary

koto hands a branch decision to the agent in exactly one place: `resolve_transition` returns `NeedsEvidence` on a state with `accepts`, producing `EvidenceRequired`, and the agent answers with `--with-data` evidence that `when` clauses route on by exact match. The best fit for a classifier is an evidence provider hooked into that `NeedsEvidence` arm of `advance_until_stop`: it answers the state's own enum/boolean `accepts` fields, records an ordinary `EvidenceSubmitted` event with additive classifier metadata, and lets the loop continue, so on high confidence the agent just receives the next real stop with `advanced: true`, and on low confidence, escape-hatch, no key, or error it gets today's unchanged `EvidenceRequired` (following the runs-commands precedent of invisible success and handoff through existing response shapes). The big catch is that most shirabe decision states bundle work with the verdict, so templates must split pure decision states out, with their inputs stored where koto can build the classifier state, before any of this pays off.
