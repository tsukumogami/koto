---
schema: design/v1
status: Planned
problem: |
  koto has no gate that can answer "not yet" and no way to drop a state's
  context keys when the workflow comes back to it, so templates keep prose
  that tells the agent to poll CI and to run a remove-then-verify block
  before every retry. Entries into a state are written from six places in
  the code, the context store is a projection an older koto repairs from the
  log, and a command gate's result is already routing evidence, a logged
  event and an override target, so both features have to fit around those
  without changing what existing templates see.
decision: |
  A state declares `clear_on_entry: [keys]`. At the top of each advance-loop
  iteration, and right after `koto next --to` and `koto rewind` append their
  entry, koto works out from the log alone whether the current entry has been
  cleared yet; if not, it removes the declared keys not written since the
  entry and appends one `context_cleared` event, removal first. A command gate
  declares `poll:`; the advance loop re-runs a pending polling gate on an
  interval for at most `hold_secs`, then reports pending as a temporal block
  with a `poll` object, times out against a window whose start is logged as
  `poll.since`, and reports failure through the existing command-gate payload.
rationale: |
  Deciding clearing from the log at a few read points, rather than stamping
  it on every entry event at its six write sites, keeps one rule in one
  function, lets the next tick finish an interrupted clearing, and needs no
  change to entry-event constructors. Routing the store removal through the
  CLI's existing append closure keeps the advance loop's signature intact.
  Polling in the advance loop, next to the attempt stamp and the log, lets the
  window and the hold read the same events the counts do, and returning
  pending instead of holding the turn fits koto's wake model, which only
  rings on request-store writes.
upstream: docs/prds/PRD-koto-ci-wait-stale-keys.md
user_visible_surface: true
---

# DESIGN: koto owns the CI wait and stale-key clearing

## Status

Planned

## Context and Problem Statement

The PRD (`docs/prds/PRD-koto-ci-wait-stale-keys.md`) sets the requirements:
a per-state clearing declaration with one log event per clearing (R1-R8a), a
polling command gate with a done/failed/pending answer, a hold, a deadline and
override behaviour (R9-R17), additive log and contract changes with v0.14.1
compatibility (R18-R22), and a map of the shirabe prose each feature makes
deletable (R23). This design settles how.

Four facts about today's code shape it.

**Entries are written in several places.** A state is entered by a
`transitioned` event from the advance loop (two sites: `skip_if` and
`take_transition` in `src/engine/advance.rs`), a `directed_transition` from
`koto next --to` (`src/cli/mod.rs`), a `rewound` from `koto rewind`
(`handle_rewind`) and from a batch retry of a child (`write_rewound_event` in
`src/cli/retry.rs`), and the initial `transitioned` that `koto init` writes.
`entry_index` in `src/engine/persistence.rs` already finds the latest entry
into a state under two boundaries: any entry (the epoch) and arrival from a
different state (the visit window that `visit_attempt` uses).

**The context store is a projection of the log.** Transition
`context_assignments` ride the `transitioned` event and are written to the
store afterwards; `reconcile` in `src/engine/context_assign.rs` restores any
assigned key whose latest writer in the log is a transition. koto v0.14.1
does the same, and it reads an event type it doesn't know as `Unknown` and
carries on.

**A command gate runs once.** `evaluate_command_gate` in `src/gate.rs` runs
the command and maps its exit onto `Passed`, `Failed`, `TimedOut` or `Error`,
with the `failure` payload beside `output`. The one existing loop,
`default_action.polling` (`execute_with_polling` in `src/cli/mod.rs`), holds
the whole tick until the state's gates pass or its timeout runs out.

**Waiting sessions are only woken by the request store.** The per-session
wake file (`DESIGN-koto-leg-wake.md`) rings when a request leg changes.
Nothing rings it when CI finishes.

## Decision Drivers

- **D1. Deletable prose.** Each feature must do the whole step the prose does
  today, including failing closed: shirabe's blocks refuse to submit when a
  removal can't be confirmed.
- **D2. Nothing changes for templates that don't opt in.** Same compiled
  JSON, same template hash, same responses, same log (PRD R20).
- **D3. Additive log.** New optional fields and one new event type;
  `schema_version` stays 1; v0.14.1 reads the log and doesn't bring back a
  cleared key (R19, R21).
- **D4. No reads to decide.** Every context read now appends and, on the
  cloud backend, uploads; clearing must decide from the log (R5).
- **D5. One rule, one place.** Six entry sites must not each carry their own
  copy of the clearing rule.
- **D6. Forge-neutral.** No knowledge of GitHub, no credentials (R17).
- **D7. Bounded ticks.** A `koto next` must return within a time the caller
  can predict, because agent harnesses kill long tool calls.

## Considered Options

### Decision 1: Which entries clear, and on what boundary

**Chosen: every entry except `koto init`'s, on the epoch boundary.** A key
listed in `clear_on_entry` is removed on each `transitioned`,
`directed_transition` or `rewound` event into the state, self-transitions
included, unless the log shows a `context_added` of that key after the entry.
The initial `transitioned` (no `from`) is skipped. This is the boundary
`epoch_slice` already cuts at, and deliberately not `visit_attempt`'s.

The reason it differs from `visit_attempt`: that count answers "which try is
this within one stay in the state", so a self-transition must not reset it.
Clearing answers "has this try started clean", and a self-loop retry is a new
try. shirabe's analysis state retries through a self-loop
(`scope_changed_retry`) and has to drop the `plan.md` it wrote the first
time. A gate override adds no entry, so an override followed by a re-check
clears nothing; a rewind always clears.

*Alternative: the visit boundary (arrival from elsewhere and rewinds only).*
It would share one boundary with `visit_attempt`, which reads well in the log.
Rejected because the analysis self-loop would keep its stale `plan.md`, and
shirabe's prose clears on that edge today, so the prose couldn't go.

*Alternative: re-entries only (skip the first entry into each state, not just
the session's first).* Saves one event on each state's first entry. Rejected
because a key some earlier state wrote before the first entry would survive,
and the log would need a second scan to find "first entry into this state".
Skipping only `koto init`'s entry costs one event on a first entry into a
declaring state and nothing else.

### Decision 2: Where clearing is decided and recorded

**Chosen: decide from the log at the read points; record with a new
`context_cleared` event.** One function,
`pending_clearing(events, state, keys)`, finds the state's latest entry, skips
it when it's `koto init`'s, returns nothing when a `context_cleared` for that
entry's sequence number already follows it, and otherwise returns the keys
not written since. It is called at the top of every advance-loop iteration
(after the terminal check, before the integration, action and gates), and
right after `koto next --to` and `koto rewind` append their entry. A batch
retry's child rewind is cleared on the child's next tick. Keys are removed
first and the event appended second, so a crash in between leaves no event
and the next call finishes the job.

*Alternative: a `context_cleared` field on the entry events, written in the
same append.* Atomic by construction, like `context_assignments`. Rejected
because all six write sites would have to compute the field (and every test
constructor of three event variants would change), `koto init`'s and the
batch retry's sites don't hold the template's state list in the right form,
and a failed store removal after the append would have nothing to retry it:
the entry event would already claim the clearing.

*Alternative: virtual clearing (readers treat a key as absent when the log
shows a clearing after its last write).* No store writes at all. Rejected
because every context read -- `koto context get` from a gate script, the
cloud backend, v0.14.1 -- would need the log, and the store would disagree
with the log for anyone reading it directly.

### Decision 3: How the store removal reaches the store

**Chosen: through the append closure.** The advance loop only appends
events; it has no store handle. The CLI's append closure for a tick already
wraps the backend; when the payload is `context_cleared`, it first calls the
store's ordinary `remove` for each key (the same call `koto context remove`
makes, so the cloud backend's local-then-remote order and its conflict error
apply), and appends only if every removal succeeded. A removal error becomes
the append's error, which fails the tick as a persistence error: no event, a
clear message, and the retry is the next tick. `koto next --to` and
`koto rewind` call the same helper directly.

*Alternative: a new closure parameter on the advance loop.* Clearer types,
but it changes `advance_until_stop` and its three wrappers and ten callers,
for a behaviour that only ever pairs one removal with one append. Rejected.

### Decision 4: Does a pending polling gate hold the turn?

**Chosen: it holds for at most `hold_secs`, then returns.** koto can't learn
that CI finished without running the command, and the wake file never rings
for it (koto#250 wired wakes to request-store writes only). Holding until done
would keep `koto next` running for as long as CI takes, past harness tool
limits, and a killed process records nothing. So each tick runs the command
at least once, re-runs it every `interval_secs` while it's pending, and stops
starting runs once `hold_secs` or the deadline would be passed. A pending
result then comes back as a temporal block with `retry_after_secs`. The
template sets `hold_secs` to what its harness tolerates; the default, 0, is a
single run per tick.

*Alternative: hold until done or timeout, like `default_action.polling`.*
Simplest for the agent, which ticks once. Rejected on D7: a 20-minute CI run
is a 20-minute tool call.

*Alternative: never hold; one run per tick, always.* Simplest for koto.
Rejected as the only mode because an agent then needs to wait between ticks
on its own, and some harnesses make that awkward; `hold_secs: 0` keeps this
behaviour available.

### Decision 5: How a command says "pending", and where polling lives

**Chosen: an exit code, default 75, and a loop in the advance loop.** 0 is
done, `pending_exit_code` (default 75, `EX_TEMPFAIL`) is pending, anything
else is failed. The advance loop evaluates the state's gates once as today,
then, for polling gates whose result is pending, sleeps and re-evaluates only
those gates through the same `evaluate_gates` closure, checking the shutdown
flag every 100 ms. It then classifies each polling result, attaches a `poll`
report, and appends one `gate_evaluated` per gate as today.

*Alternative: a stdout marker line (`::koto-status::pending`).* Richer, and
consistent with finding lines. Rejected because it makes a one-line shell
wrapper impossible (`gh pr checks` already exits 8 for pending; mapping an
exit code is `case $?`), and a marker parser adds a second output grammar.

*Alternative: poll inside `evaluate_command_gate`.* Keeps gate code together.
Rejected because the window start comes from the log and the hold needs the
shutdown flag, and `gate.rs` has neither; the loop would have to take both.

*Alternative: a new gate type (`command-poll`).* Rejected: the output schema,
override default, failure payload and routing are the command gate's, and a
second type would duplicate every place that switches on `command`.

### Decision 6: How a pending evaluation reads in the log

**Chosen: a new outcome value, `pending`, emitted only by polling gates.** A
tick whose polling gate is still pending logs `gate_evaluated` with
`outcome: "pending"`. `failed` keeps meaning "the check judged the work and
found it bad"; a pending tick judged nothing. The value appears only in logs
of sessions whose template declares a `poll:` gate. A template without one
never emits it, compiles byte-identical, and writes logs koto v0.14.1 reads
as it does today; the existing compatibility job proves that for every
fixture template, and the new job below covers a template that does poll.
koto v0.14.1 stores `outcome` as a plain string, so it reads a `pending`
event without error.

A pending evaluation is not a failed check, so it gets none of what a failed
check gets:

- no `failure` object and no koto-written fallback finding, in the log or the
  response;
- no `rule_counts`, whatever the command printed, now or in any later
  version: a pending event never counts toward a rule;
- no `attempt` and no `visit_attempt`. The attempt is the evaluation where
  the poll resolves -- done, failed or timed out -- so retries, loops and fix
  turns see one attempt per CI result, not one per tick. The next resolving
  evaluation takes `1 +` the highest stamp in the log, exactly as today,
  because pending events carry none. Other checks of the same state that
  resolve in a pending tick keep their stamps as usual. For example, a state
  with a pending polling gate and another gate that fails in the same tick
  stamps the failed gate `attempt: N`, and the CI result, when it resolves on
  a later tick, takes `N+1`; that is the intended behaviour, not a gap.

The wait's cost stays visible on the resolving event: its `poll.evaluations`
counts every run since the window opened (summed across the pending ticks
before it) and its `poll.elapsed_secs` is the time since `poll.since`.

A poll that runs out of time stays `outcome: "timed_out"`, the value a
command gate's per-run timeout already uses, with `poll.status: "timed_out"`
on it so a CI timeout can be told apart from a command that hung.

*Alternative: `outcome: "failed"` plus `poll.status: "pending"`.* This keeps
the published `outcome` enum closed, the way an open request leg's failed
`gate_evaluated` does, so no reader validating against the enum meets a new
value. Rejected because a downstream measurement consumer counting failed
checks would count every pending tick of a CI wait as a failure (a 20-tick
wait reads as 20 failures) unless it learns a nested field, and because
"failed" would then mean two things. The cost of the chosen value is one
enum entry that a reader pinned to the published list must add; it's
confined to sessions that use polling gates.

## Decision Outcome

A template gets two declarations. On a state:

```yaml
states:
  implementation:
    clear_on_entry: [scrutiny_results.json, review_results.json, qa_results.json, summary.md]
```

On a command gate:

```yaml
gates:
  ci:
    type: command
    command: "{{PLUGIN_ROOT}}/scripts/ci-status.sh"
    timeout: 60
    poll:
      interval_secs: 30
      timeout_secs: 3600
      hold_secs: 240
```

Clearing is decided from the log wherever koto is about to act on the current
state, removes keys through the ordinary store removal, and leaves one
`context_cleared` event per entry. A polling gate re-runs within a bounded
hold, reports pending as a wait, fails through the command-gate payload, and
times out against a window it writes into the log. Neither feature touches a
template that doesn't use it, and both leave an older koto reading the log.

## Solution Architecture

### Components

- **`src/template/types.rs`.** `TemplateState.clear_on_entry: Vec<String>`
  (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`).
  `Gate.poll: Option<PollSpec>` with `interval_secs`, `timeout_secs`,
  `hold_secs` (default 0) and `pending_exit_code` (default 75), skipped when
  `None`. Both are added to the exhaustive field lists
  (`substitutable_fields`/`literal_fields`) as non-substitutable. Validation:
  R1, R2 and R9, with errors naming the state, key, gate and transition.
- **`src/template/compile.rs`.** The source-YAML mirrors of both fields.
- **`src/engine/types.rs`.** `EventPayload::ContextCleared { state, keys,
  entry_seq }`, type name `context_cleared`. `GateEvaluated` gains
  `poll: Option<PollRecord>` (`status`, `evaluations`, `since`,
  `elapsed_secs`), skipped when `None`.
- **`src/engine/clear_on_entry.rs` (new).** `pending_clearing(events, state,
  keys) -> Option<Clearing>` and `apply(store, session, clearing, append)`,
  which removes then appends. Unit-tested against synthetic logs for every
  entry kind.
- **`src/engine/advance.rs`.** At the top of each iteration, after the
  terminal check: when the state declares keys and `pending_clearing` returns
  one, append `ContextCleared`. After the first gate evaluation: the polling
  hold (Decision 4 and 5), classification into done / pending / failed /
  timed out, and the `poll` report on each polling result.
- **`src/gate.rs`.** `GateOutcome::Pending`, logged as `pending`.
  `StructuredGateResult.poll: Option<PollReport>` (`#[serde(skip)]`), and a
  helper that turns a pending result into the temporal shape (outcome
  `Pending`, no `failure`, no fallback finding, parsed findings kept in
  `findings` for the log only) and a timed-out one into `TimedOut` with a
  koto finding (`command still pending after N seconds`). Every `match` on
  `GateOutcome` gains the arm; the attempt-stamp and rule-count code skip
  `Pending`.
- **`src/cli/mod.rs`.** The tick's append closure performs the store removal
  for a `ContextCleared` payload before appending. `koto next --to` and
  `handle_rewind` call `clear_on_entry::apply` after their append;
  `handle_rewind` gains the context store and the compiled template as
  parameters, which it doesn't take today. The append closure's contract
  (remove the keys a `ContextCleared` names, then append) is documented on
  the advance loop, since test closures that only record events skip the
  removal.
- **Sequence numbers.** `entry_seq` is the persisted sequence number of the
  entry event, so it joins across exports. The tick's in-memory log gives
  events it appends synthetic numbers (`advance_until_stop_recording`), which
  can differ from the persisted ones when another process appends during the
  tick; the append path therefore returns the persisted number and the tick
  log records it, and `pending_clearing` reads only persisted numbers.
- **`src/cli/next_types.rs`.** `BlockingCondition.poll` (skipped when
  absent); `blocking_conditions_from_gates` sets category `temporal` and
  `agent_actionable: false` for a pending polling result.
- **`src/engine/advance.rs` attempt counting.** `check_fields` writes no
  `attempt`, `visit_attempt` or `rule_counts` for a pending result, and the
  entry stamp isn't computed for a tick whose only check is pending.
- **`src/engine/context_assign.rs` and `src/engine/terminal_result.rs`.**
  `outstanding_assignments` and the failure-reason read treat each key a
  `context_cleared` names as removed (R8).

### Log and response shapes

```json
{"type":"context_cleared","payload":{"state":"implementation","keys":["review_results.json","summary.md"],"entry_seq":41}}
```

```json
{"type":"gate_evaluated","payload":{"state":"ci_monitor","gate":"ci","outcome":"pending",
 "output":{"exit_code":75,"error":""},
 "poll":{"status":"pending","evaluations":8,"since":"2026-09-28T20:00:00.000Z","elapsed_secs":392}}}
{"type":"gate_evaluated","payload":{"state":"ci_monitor","gate":"ci","outcome":"passed",
 "output":{"exit_code":0,"error":""},"attempt":4,"visit_attempt":2,
 "poll":{"status":"done","evaluations":23,"since":"2026-09-28T20:00:00.000Z","elapsed_secs":1180}}}
```

A pending blocking condition in the `koto next` response:

```json
{"name":"ci","type":"command","status":"pending","category":"temporal","agent_actionable":false,
 "output":{"exit_code":75,"error":""},
 "poll":{"status":"pending","retry_after_secs":30,"elapsed_secs":392,"timeout_secs":3600}}
```

`context_cleared` is `tier: 2` in the session-feed contract, beside the other
context events. `gate_evaluated.poll` is an optional object documented in a
prose table, as `findings` is. The contract's `outcome` enum gains `pending`,
with a note that only sessions whose template declares a polling gate emit it.

`context_cleared` carries key names only, never values, hashes or sizes: the
keys are removed, and the event records which, not what they held.

**`entry_seq` is the epoch boundary, not the visit boundary.** It is the
sequence number of the entry the clearing belongs to, which is any entry,
self-transitions included. A visit (the `visit_attempt` window) starts only
at an arrival from a different state or a rewind, so one visit can hold
several clearings. The mapping: the visit a `context_cleared` belongs to is
the one opened by the latest arrival-from-elsewhere or `rewound` event at or
before `entry_seq`; when the event at `entry_seq` is itself such an arrival,
the clearing opens that visit and the next resolving check in the state has
`visit_attempt: 1`; when it's a self-transition, the clearing sits inside the
running visit and `visit_attempt` keeps counting across it.

### Data flow: a retry into a cleared state

1. `review` fails; the agent submits `blocking_retry`; the advance loop
   appends `transitioned review -> implementation` (seq 41).
2. The next iteration is `implementation`. `pending_clearing` finds entry 41,
   no `context_cleared` for 41, and no `context_added` of the listed keys after
   41, so it returns all of them.
3. The append closure removes each key from the store, then appends
   `context_cleared {entry_seq: 41}`. A removal error fails the tick here.
4. `implementation` runs its action and gates against the cleared store.

### Data flow: a polling gate

1. The advance loop evaluates the state's gates. `ci` exits 75: pending.
2. It reads `since` from the epoch's earliest `gate_evaluated` for `ci` that
   carries `poll`, or takes the first run's start.
3. While pending and neither the hold nor the deadline would be passed by the
   next run, it sleeps `interval_secs` and re-runs `ci` alone.
4. It classifies the last run: exit 0 done (`passed`), 75 pending
   (`pending`, or `timed_out` if the deadline has passed), anything else
   failed as today.
5. It appends one `gate_evaluated` with `poll`, stamped with `attempt` and
   `visit_attempt` only when the poll resolved, and the response carries the
   matching blocking condition.

An override of `ci` works as for any command gate: the override value is
injected, the command doesn't run, and no `gate_evaluated` is written while
it stands (until the next entry).

### Attributes a consumer can export

Every attribute this design adds to the log, for a downstream measurement
consumer's definitions:

| Event | Attribute | Type | Present | Meaning |
|---|---|---|---|---|
| `gate_evaluated` | `outcome` value `pending` | string (enum value) | on a polling gate's pending evaluation | The check hasn't settled; nothing was judged. |
| `gate_evaluated` | `poll.status` | string: `done`, `pending`, `failed`, `timed_out` | on every polling gate evaluation | How the poll stood when this evaluation was recorded. |
| `gate_evaluated` | `poll.evaluations` | integer >= 1 | with `poll` | Command runs since the window opened, summed across ticks. |
| `gate_evaluated` | `poll.since` | string, RFC 3339 | with `poll` | Start of the polling window: the first run of this gate since the latest entry into the state. |
| `gate_evaluated` | `poll.elapsed_secs` | integer >= 0 | with `poll` | Seconds from `poll.since` to the end of the recorded run. |
| `context_cleared` | `state` | string | always | The state whose entry was cleared. |
| `context_cleared` | `keys` | array of strings | always | Names of the keys removed; never values. |
| `context_cleared` | `entry_seq` | integer >= 1 | always | Sequence number of the entry (any entry, self-transitions included) the clearing belongs to. |

A pending `gate_evaluated` carries no `attempt`, `visit_attempt`,
`rule_counts` or fallback finding; the resolving evaluation carries the
attempt stamps.

## Implementation Approach

Clearing comes first, since it's the part that makes the largest block of
shirabe prose deletable.

1. **Clearing: template and compile.** `clear_on_entry` field, validation
   (R1, R2), compile-cache byte-identity check for templates without it.
2. **Clearing: engine.** `context_cleared` event, `clear_on_entry.rs`, the
   advance-loop call, the append-closure removal, the `--to` and rewind
   calls, and the R8 readers. Integration tests for each entry kind, the
   written-since case, the one-event-per-entry case and the removal-failure
   case.
3. **Polling gate.** `poll` field and validation (R9), the hold loop and
   classification, `gate_evaluated.poll`, `BlockingCondition.poll`, and
   integration tests with a script that counts its runs.
4. **Contract, docs and skills.** `docs/reference/session-feed.md`, the
   template-format reference and gate-authoring guide, the `koto-author` and
   `koto-user` skills (a pending temporal block means wait and tick again
   after `retry_after_secs`).
5. **Compatibility and scripted checks.** A v0.14.1 compatibility script and
   CI job, modelled on `test/compat/failure-reporting-v0_14_1.sh`, with its
   own mutation self-test; a forge-neutrality grep script and CI step.

## Adoption Map for shirabe

Nothing in shirabe changes here. This section lists, against shirabe commit
`e34abda`, what each feature lets shirabe delete later and what stays, so that
adoption is mechanical. Per-skill changes under way in shirabe will move line
numbers, so adoption re-checks each span's lines against the commit it
adopts on. Where a span is keyed to a contradiction-settlement
identifier, the identifier is given so adoption doesn't collide with that
edit.

### Back edges in shirabe's koto templates

**work-on (`skills/work-on/koto-templates/work-on.md`)**, nine back edges:

| Edge | Fires on | Declaration that covers it | Prose it retires |
|---|---|---|---|
| analysis -> analysis | `scope_changed_retry` | `analysis: clear_on_entry: [plan.md, scrutiny_results.json, review_results.json, qa_results.json, summary.md]` | block in `phase-3-analysis.md` |
| implementation -> analysis | `scope_expanded_retry` | same declaration on `analysis` | block in `phase-4-implementation.md` |
| scrutiny -> implementation | `blocking_retry` | `implementation: clear_on_entry: [scrutiny_results.json, review_results.json, qa_results.json, summary.md, pre_pr.md]` | block in `phase-4a-scrutiny.md` |
| review -> implementation | `blocking_retry` | same declaration on `implementation` | block in `phase-4b-review.md` |
| qa_validation -> implementation | `blocking_retry` | same | block in `phase-4c-qa.md` |
| verification -> implementation | `failed` | same | the copy in `work-on.md` (verification directive) |
| finalization -> implementation | `issues_found` | same (this is the edge that needs `pre_pr.md`) | block in `phase-5-finalization.md` |
| implementation -> implementation | `partial_tests_failing_retry` | none needed; the declaration fires and finds nothing to clear | none today |
| pr_creation -> pr_creation | `creation_failed_retry` | none needed (its gate is a command gate) | none today |

`plan.md` must not be declared on `implementation`, which reads it, and
`impl_base` must never be declared. No declared key is a `context_assignments`
target (work-on assigns only `failure_reason`), so the compile rule in PRD R2
doesn't bite. The `changed_paths_record` state could also declare
`changed_paths.txt`, making the removal inside `record-changed-paths.sh`
redundant; optional.

At `e34abda` work-on's `ci_monitor` has no back edge: `failing_fixed` ends
the run at `done`. The settled `ci-fix-ends-run-unverified` policy changes
that: `failing_fixed` loops back to `ci_monitor`, so a fix is re-checked
rather than trusted. That adds a tenth row:

| Edge | Fires on | Declaration that covers it | Prose it retires |
|---|---|---|---|
| ci_monitor -> ci_monitor | `failing_fixed` (after `ci-fix-ends-run-unverified`) | none needed for evidence: `ci_outcome` and its `rationale` are submitted evidence, which koto already scopes to the epoch, so the self-transition drops them and the re-check can't read the previous verdict. If the adopted state records a verdict in a context key (for example a CI summary a later state gates on), that key goes in `ci_monitor: clear_on_entry: [...]`. | the "Re-check" step of phase-6-pr.md's CI Monitoring, and the agent's re-ticking while CI runs |

A polling gate on `ci_monitor` covers the wait on each lap: the self-transition
is a new entry, so each fix push gets a fresh polling window with a new
`poll.since`, and the gate re-evaluates until the new run is done, failed or
timed out. It replaces the one-shot `gh pr checks` gate and the agent's
re-ticking, but not the repair path (read the logs, fix, push).

**execute (`skills/execute/koto-templates/execute.md`)**, two back edges, both
`merge_route -> merge_readiness` on `recheck: waited` (one for a pending
verdict, one for no verdict). execute's `ci_monitor` itself has no back edge:
`passing`, `failing_fixed` and `pending` all go forward to `merge_readiness`,
which is where CI is re-read and waited on after a fix. The clearing there is
done by `record-merge-verdict.sh` in the state's `default_action`, not by
prose. A declaration `merge_readiness: clear_on_entry: [merge_verdict,
home_pr]` would express the part of it that isn't a `context_assignments`
target; `reason`, `step` and `waiting` are assigned by transitions elsewhere,
so the compile rule refuses them and the script keeps clearing those. The
wait (`Wait a minute or two, then submit recheck: waited`) is what a polling
gate on `merge_readiness` replaces, carrying the 1,800-second per-head-commit
deadline the script enforces today as `timeout_secs`.

**execute-coordinated, scope and deliver.** execute-coordinated has no back
edges; its CI wait is one bullet inside `coord_loop`'s agent loop and stays
until that loop is restructured. scope has seven back edges
(`full_run_blocked`, `exit_re_evaluation` and `exit_abandonment` self-loops,
`chain_proposal -> discovery`, and the `hop_design`/`hop_plan -> fold` hub
edges), and deliver has two (`scope_absent -> scope_run`,
`execute_absent -> execute_run`). None of their states gates on a
context-exists or context-matches key, so no declaration applies; their
retries are author decisions or request-leg waits, not polls. There is no
scope hop retry edge: a hop whose gate fails holds.

### Spans shirabe can delete

Lines are at shirabe `e34abda`.

- **`wh-work-on-retry-clearing-blocks`** (removable with `clear_on_entry`).
  The seven executable retry-clearing blocks, 4,821 bytes of shell, 18,544
  bytes with the paragraphs that exist only to justify them:
  `skills/work-on/references/phases/phase-3-analysis.md` 74-102,
  `phase-4-implementation.md` 146-176, `phase-4a-scrutiny.md` 43-73,
  `phase-4b-review.md` 41-69, `phase-4c-qa.md` 40-68,
  `phase-5-finalization.md` 141-174, and
  `skills/work-on/koto-templates/work-on.md` 1831-1855. Along with them go
  `skills/work-on/scripts/retry-clearing_test.sh` and the `koto context
  remove` requirement in `skills/work-on/requires.tsv`. What stays: the
  escalate outcomes each block names, reduced to one sentence each, unless
  shirabe relies on koto's failed clearing stopping the tick (it does: a
  removal failure fails the tick with no event); the retry-count prose; and
  phase-4a's instruction to re-spawn the reviewers.
- **`ci-fix-ends-run-unverified`**: the CI-watching part of
  `skills/work-on/references/phases/phase-6-pr.md`, lines 47-57 (CI
  Monitoring) and 69-73 (ci_monitor evidence), as that settlement leaves
  them. A polling gate replaces "re-check" and the waiting; the repair steps
  (read logs, fix, push) and the escalation to the user stay.
- **`retry-caps`**: the "up to 3" and "2-3 iterations" wording in
  phase-6-pr.md and work-on.md stays with that settlement; this feature adds
  no cap and deletes none of it.
- **ci_monitor directives.** `work-on.md` 2036-2055: line 2038's pointer to
  phase-6-pr.md for CI monitoring and 2054-2055, plus the comment at
  1268-1275 that the gate "polls the PR and may be stale". `execute.md`
  1452-1481, of which line 1477 ("Waiting on CI is bounded, and not here")
  is the polling text; the re-check wait in `merge_route`, 1585-1594 (line
  1591); and `merge_readiness`'s wait prose, 1569-1583. The fix path
  (1467-1475) and the DIRTY handling (1479-1481) stay.

## Security Considerations

**Commands.** A polling gate runs the template's command more often, not
differently: same shell, same fixed environment, same redaction of captured
output, same per-run timeout. koto adds no forge client and reads no
credential; a CI-status script that needs a token gets it the way any gate
command does, through `pass_env:`, and its output is redacted as today.

**Deletion.** Clearing removes only keys the template lists, each checked
against the context-key grammar at compile time and never built from a
variable, so no runtime value can steer a removal outside the session's
store. It goes through the store's ordinary removal, so the cloud backend's
version check applies.

**Denial of service.** A hold can keep one `koto next` running for at most
`hold_secs` plus one command run; the compiler caps `hold_secs` at
`timeout_secs`, and the interval floor of one second bounds the run rate.
Each tick appends one `gate_evaluated`, so the log grows per tick as it does
for any blocked gate.

**Concurrent writes during a clearing.** Clearing reads the log, removes the
keys, then appends its event. The per-session append lock covers only the
append, so a `koto context add` of a listed key that lands after the log read
and before the removal is deleted even though it came after the entry. This
window is accepted rather than closed. It lasts from the top of the
iteration to the removal, before the state's action or gates run, so a gate
script's own writes can't fall into it; only a write from outside the tick
can. The failure direction is safe: the lost key makes a gate block, never
pass. Holding the append lock across the store removal would close it, at the
cost of holding a lock around cloud I/O, which the failure-reporting design
ruled out for that lock.

**Sync writes don't count as "written since the entry".** A cloud sync pull
logs a `context_added` with writer `sync`. That write restores content from
the remote copy; it isn't new work for this attempt. So `pending_clearing`
ignores `context_added` events whose writer is `sync` when it decides which
keys were written since the entry: a stale key a pull brings back after the
entry and before the clearing is still cleared. Writes with writer `agent`,
`koto` or no writer (older logs) count.

**Command output on the poll path.** A polling gate's captured output and
findings reach the agent the same way a failing command gate's do today:
redacted, capped at the existing 64 KiB per stream in the response and 4 KiB
in the log, and inside `failure`, never `output`. A pending result carries no
`failure` at all, so nothing the command prints while pending reaches the
agent except through the log. The koto-user skill already tells agents that
`failure` content is the check's output, not instructions; the poll path adds
no new channel for it.

**Stale-state safety.** The point of the feature is removing a class of
false passes: a gate satisfied by a key from an earlier attempt. The residual
case is the cloud backend, where a remote delete that fails for a reason
other than a version conflict warns and continues (as `koto context remove`
does), so a later sync pull can restore the key. That is unchanged from the
removal shirabe's prose performs today.

## Consequences

### Positive

- The largest block of retry prose in shirabe, and the CI-polling
  instructions, become deletable, and rewinds and `koto next --to` entries
  get the clearing that prose never covered.
- The log shows each clearing and each polling evaluation with its window,
  in documented fields.
- A pending CI run is a wait, not a failure an agent tries to fix.

### Negative

- Every entry into a declaring state after the first appends a
  `context_cleared` event, even when the keys were already absent, because
  koto doesn't read the store to check.
- An older koto running a session that uses these features evaluates the
  polling gate once per tick and doesn't clear; it reads the log correctly
  but doesn't perform the new behaviour.
- A key written before the state is entered is cleared on entry; authors
  have to declare keys on the state that produces them.

### Mitigations

- The event is small and one per entry, not per key or per tick.
- The compatibility job states the downgrade behaviour and checks the part
  that matters for correctness: an older koto never brings a cleared key back.
- The template-format reference and the koto-author skill say where to
  declare keys, with the shirabe `implementation`/`analysis` split as the
  worked example.
