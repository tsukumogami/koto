---
schema: prd/v1
status: Done
problem: |
  Two protocol steps that koto-backed workflows rely on are the agent's job,
  carried in directive prose: waiting on a check that settles later than the
  tick that asks (CI on a pull request), and clearing the context keys a
  state's gates read when the workflow comes back to that state for a retry.
  koto has no gate that can say "not yet" and no way to clear keys on entry,
  so the prose can't be deleted, and an agent that skips or botches either
  step fixes a build that was only pending or passes a gate on stale work.
goals: |
  A template declares which context keys belong to a state's attempt, and koto
  clears them whenever the workflow enters that state again, recording the
  clearing as one log event. A template declares a polling gate over a
  command, and koto re-evaluates it until the command reports done, failed or
  still pending, within a stated deadline, reporting pending as a wait and a
  failure through the command-gate failure payload koto already has. Templates
  that use neither compile and run unchanged, and koto v0.14.1 reads the new
  logs.
absorbed:
  - docs/briefs/BRIEF-koto-ci-wait-stale-keys.md
---

# PRD: koto owns the CI wait and stale-key clearing

## Status

Done

Absorbed [BRIEF-koto-ci-wait-stale-keys](docs/briefs/BRIEF-koto-ci-wait-stale-keys.md); carried in Absorbed Brief.

## Absorbed Brief

The feature was framed around two protocol steps koto-backed workflows leave
to the agent in directive prose: waiting on a check that settles later than
the tick that asks about it, CI on a pull request being the common case, and
clearing the context keys a state's gates read when the workflow comes back
to that state for a retry. Both are mechanical, and while the agent does them
from prose the prose can't be deleted, and a skipped or botched step either
sends the agent to fix a build that was only pending or lets a stale key pass
a gate on the retry.

The outcome it set is for a template to declare the wait and the keys and for
koto to do both: a tick says whether the check is done, failed or still
pending, and a retry into a state starts with that state's keys cleared and
recorded in the log, with no forge knowledge or credential in koto and no
change for templates that use neither feature.

Four journeys framed it, carried as the User Stories below: a retry loop that
starts clean, an agent waiting on CI, a CI repair loop whose wait restarts on
each fix, and a log reader reconstructing afterwards why a gate passed. The
boundary it drew held in the per-state clearing declaration and its log
record, the polling gate with its interval, deadline and failure payload, the
session-feed contract text with v0.14.1 compatibility, and a map of the
shirabe prose each feature makes deletable. It pushed out any shirabe change,
a built-in forge gate, retry caps and escalation, and routing on variable
values. The three framing questions it left open -- whether a pending gate
holds the turn, which entries clear, and how a command says "pending" -- are
settled under Decisions and Trade-offs.

## Problem Statement

A koto template enforces order with gates, and two gate patterns only work
when the agent does something by hand first.

**Waiting.** A command gate runs its command once per evaluation and reports
passed or failed. A check that settles later -- CI on a pull request is the
case every shirabe workflow hits -- has three answers, not two: done, failed,
or still running. Templates today gate on a one-shot CI-status command and
tell the agent in prose to keep re-ticking until the checks settle, how to
tell "still running" from "failed", and when to give up. An agent that reads
a pending run as a failure goes off to fix a build that isn't broken; one that
stops early leaves the run parked.

**Clearing.** A state whose gate asks whether a context key exists passes as
soon as the key is present. When the workflow loops back into that state --
review failed, the agent fixed something, the workflow returns to review --
the key from the last attempt is still there, and the gate passes on stale
work unless something removes it first. Today that something is an
executable block the agent runs before each retry: remove each key, then
check it's really gone. shirabe's work-on template carries seven copies of
it, about 4.8 KB, the largest piece of protocol prose it holds back from
deletion. A path that misses a copy, or an agent that skips the block, turns
a failed check into a silent pass.

Both steps are mechanical, and koto already has the pieces they build on: the
command-gate failure payload, per-state attempt counts and the session-feed
contract from the failure-reporting change. What's missing is a gate that can
answer "not yet" and a declaration that tells koto which keys to drop on
entry.

## Goals

- A template can say, on a state, which context keys to clear when the
  workflow enters it again, and the retry-clearing prose becomes deletable.
- A template can declare a polling gate over any command, and the CI-polling
  prose becomes deletable, without koto learning anything about a forge or
  holding a credential.
- Both features show up in the session log in documented fields, so a reader
  of the log can tell a clean retry from a stale one and a pending wait from
  a failure.
- Nothing changes for templates that use neither feature, and an older koto
  keeps reading the logs.

## User Stories

- As a template author, I want to list the keys a state's attempt produces so
  koto clears them on every re-entry, so that I can delete the remove-then-
  verify block from each retry path.
- As an agent running a workflow, I want a tick on a CI-watching state to tell
  me whether CI is done, failed or still pending, so that I wait when it's
  pending and repair only when it failed.
- As an agent in a CI repair loop, I want the deadline and the attempt count
  to follow the workflow's own re-entries, so that each pushed fix gets a
  fresh wait without template prose telling me how.
- As someone reading session logs, I want each clearing and each polling
  evaluation recorded in fields the session-feed contract names, so that I can
  reconstruct why a gate passed or blocked without reading a transcript.
- As a maintainer running an older koto on a session a newer koto wrote, I
  want the log to read cleanly and cleared keys to stay cleared.

## Definitions

- **Entry into a state.** A `transitioned`, `directed_transition` or
  `rewound` event whose target is the state. The `transitioned` event
  `koto init` writes for the initial state (with no `from`) is the session's
  first entry and is excluded from clearing, because nothing can precede it.
- **Epoch.** The events after the most recent entry into the current state.
  A self-transition starts a new epoch. This is the boundary koto already
  uses for gate overrides and submitted evidence.
- **Visit.** The events since the most recent entry from a different state or
  rewind; a self-transition doesn't start one. `visit_attempt` counts within
  a visit.
- **Pending exit code.** The exit status a polling gate's command uses to say
  "still pending". Every other non-zero status means failed.

## Requirements

### Clearing on entry

- **R1.** A state may declare `clear_on_entry:`, a non-empty list of context
  keys. Each must be a literal key that passes the context-key grammar, with
  no `{{VAR}}` reference and no duplicates; a terminal state may not declare
  it. The compiler rejects anything else with an error naming the state and
  the key.
- **R2.** The compiler rejects a `clear_on_entry` key that any transition in
  the template writes through `context_assignments`, naming both. (This is
  what keeps an older koto from restoring a cleared key; see R8.)
- **R3.** Clearing happens on every entry into a declaring state except the
  session's first: arrival from another state, a self-transition (including a
  `skip_if` one), a directed transition (`koto next --to`), a rewind, and the
  rewind a batch retry writes on a child. A gate override is not an entry and
  clears nothing; re-ticking inside the same epoch clears nothing.
- **R4.** A key written after the entry -- a `context_added` of that key with
  a higher sequence number than the entry event, other than one a cloud sync
  pull logged (writer `sync`) -- is not cleared. Every other declared key is
  removed from the context store.
- **R5.** koto decides what to clear from the event log alone. It reads no
  key's content or presence to decide, so clearing appends no `context_read`
  events.
- **R6.** Each clearing appends exactly one `context_cleared` event carrying
  the state, the keys removed, and the sequence number of the entry it
  belongs to. It carries key names only, never values, hashes or sizes.
  `entry_seq` names the entry on the epoch boundary (any entry, self-
  transitions included), not the visit boundary. One entry produces at most one `context_cleared` event,
  however many ticks follow it. When every declared key was written after the
  entry, no event is appended.
- **R7.** Clearing completes before koto runs the state's `default_action` or
  evaluates its gates, and before `koto next`, `koto next --to` or
  `koto rewind` returns after making the entry. Keys are removed before the
  event is appended, so an interrupted clearing is finished by the next tick
  rather than recorded as done.
- **R8.** koto's own log-based readers treat `context_cleared` as a removal of
  each key it names: the store repair that restores transition assignments,
  and the terminal-result read of the failure-reason key.
- **R8a.** Clearing removes keys through the context store's ordinary removal,
  the one `koto context remove` uses, on every backend. On the cloud backend
  that means the local copy goes first and the remote delete follows; a
  session-version conflict fails the clearing (no event, so the next tick
  retries it), and any other remote failure warns and continues, as
  `koto context remove` does today.

### Polling gate

- **R9.** A `command` gate may declare `poll:` with `interval_secs` (1 or
  more), `timeout_secs` (1 or more), optional `hold_secs` (0 or more, at most
  `timeout_secs`, default 0) and optional `pending_exit_code` (1 to 255,
  default 75). `poll:` on any other gate type, or on a gate in a state whose
  `default_action` declares `polling:`, is a compile error.
- **R10.** The command's exit status decides: 0 is done, the pending exit code
  is pending, and anything else is failed. A run killed by the gate's own
  per-run `timeout` keeps the outcome a command gate reports for it today
  (`timed_out`) and a spawn failure keeps `error`; both are failed for
  polling (`poll.status: "failed"`), never pending.
- **R11.** Every tick that evaluates the gate runs the command at least once.
  While it reports pending, koto sleeps `interval_secs` and runs it again,
  but starts no run that would begin after `hold_secs` have passed since the
  tick's first run or after the deadline in R12. A signal ends the hold like
  any other interrupt. Only the tick's last run is recorded.
- **R12.** The polling window starts at the moment of the first run of that
  gate in the current epoch, and that moment is recorded on every recorded
  evaluation as `poll.since`. A tick reads `since` from the epoch's earliest
  recorded evaluation of the gate, or takes its own first run's start when
  there is none. The deadline is `since` plus `timeout_secs`. A new entry
  into the state starts a new window.
- **R12a.** A run that reports done or failed is taken at its word whenever
  it finishes, deadline or not. Only a pending answer at or past the deadline
  becomes the timeout in R15.
- **R13.** A pending result blocks the state as a wait: its outcome is
  `pending` and its blocking condition has status `pending`, category
  `temporal`, is not agent-actionable, carries no `failure` object and no
  koto-written fallback finding, and carries a `poll` object with `status: "pending"`,
  `retry_after_secs` (the interval), `elapsed_secs` and `timeout_secs`. Its
  gate output is the command gate's usual `{"exit_code", "error"}`.
- **R14.** A failed result is reported exactly as a failed command gate is
  today -- outcome, output, category `corrective`, and the `failure` payload
  with findings and captured output -- plus a `poll` object with
  `status: "failed"`.
- **R15.** When the deadline passes with the command still pending, the gate's
  outcome is `timed_out`, its category `corrective`, and its `failure` payload
  carries a koto-written finding saying the check was still pending after the
  timeout; `poll.status` is `"timed_out"`.
- **R15a.** A polling gate can be overridden like any command gate unless it
  declares `overridable: false`, whether its last result was pending, failed
  or timed out. The override substitutes the command gate's default output,
  no command runs, and no `gate_evaluated` (so no `poll` object) is appended
  for it while the override stands, which is until the next entry into the
  state. The override doesn't move the polling window.
- **R16.** The attempt is the evaluation where the poll resolves -- done,
  failed or timed out. A pending evaluation carries no `attempt`,
  `visit_attempt` or `rule_counts` and doesn't advance either count, so a
  retry loop sees one attempt per CI result, not one per tick. koto enforces
  no cap on polling attempts.
- **R17.** koto knows nothing about any forge: no built-in GitHub gate, no
  credential handling. What "done" means is entirely the command's.

### Log, contract and compatibility

- **R18.** `gate_evaluated` gains an optional `poll` object on polling gates:
  `status` (`done`, `pending`, `failed` or `timed_out`), `evaluations` (runs
  since the window opened, summed across ticks), `since` (RFC 3339, the window
  start in R12) and `elapsed_secs` (from `since` to the end of the recorded
  run). A pending evaluation has the new outcome value `pending`, which only
  sessions whose template declares a polling gate ever write. A polling
  timeout keeps the existing `timed_out`, told apart from a per-run timeout by
  `poll.status`.
- **R19.** Every new field and event is optional or new, the one new
  `outcome` value is emitted only by polling gates, `schema_version` stays 1,
  and each is documented in `docs/reference/session-feed.md`.
- **R20.** A template that declares neither `clear_on_entry` nor `poll`
  compiles to the same JSON and template hash as before and runs unchanged.
- **R21.** koto v0.14.1 runs `koto status`, `koto next` and `koto context get`
  on a session the new koto wrote while using both features, exits 0, reports
  the same state, and doesn't bring back a cleared key. A CI job proves it.
- **R22.** The template-format reference, the koto-author and koto-user
  skills, and the gate-authoring guide describe both features.

### Adoption record

- **R23.** The design lists every back edge in shirabe's koto templates, what a
  `clear_on_entry` declaration would clear on each, and the prose each feature
  makes deletable, as spans shirabe can remove later. Nothing in shirabe
  changes here.

## Acceptance Criteria

### Clearing on entry

- [ ] A template whose state declares `clear_on_entry` with an invalid key, a
  `{{VAR}}` reference, a duplicate, or on a terminal state fails to compile
  with an error naming the state and key.
- [ ] A template that clears a key some transition assigns fails to compile,
  naming the key, the state and the transition.
- [ ] A session that loops review -> fix -> review with the verdict key
  declared on review finds the key absent on re-entry, and its log holds one
  `context_cleared` event naming the key and the entry's sequence number.
- [ ] On that re-entry the review state's `context-exists` gate on the key
  blocks on the first tick, and a `default_action` that prints whether the key
  exists reports it absent, so clearing ran before both.
- [ ] The same holds for a self-transition, a `skip_if` self-transition, a
  `koto next --to` into the state, a `koto rewind` into it, and the rewind a
  batch retry writes on a child; `koto context exists` right after
  `koto next --to` or `koto rewind` returns reports the key absent.
- [ ] After a clearing, the new koto's store repair doesn't restore the key,
  and a cleared failure-reason key is absent from the terminal result.
- [ ] On a cloud-backed store whose version check reports a conflict, the
  clearing fails without appending `context_cleared`.
- [ ] Recording a gate override and re-ticking in the same epoch clears
  nothing and appends no `context_cleared`.
- [ ] A key written with `koto context add` after the entry and before the
  first tick survives that tick; a key written before the entry doesn't,
  and neither does one whose only later write is a `context_added` with
  writer `sync`.
- [ ] A clearing appends no `context_read` event.
- [ ] Ticking the state several times after one entry leaves exactly one
  `context_cleared` event for that entry.
- [ ] When the store removal fails, no `context_cleared` event is appended and
  the next tick clears and records it.

### Polling gate

- [ ] `poll:` on a non-command gate, with `interval_secs` or `timeout_secs` of
  0, with `hold_secs` greater than `timeout_secs`, with a pending exit code
  of 0 or above 255, or beside a polling `default_action`, fails to compile.
- [ ] A command that exits with the pending code yields a temporal,
  non-actionable blocking condition with `poll.status: "pending"` and no
  `failure`; one that exits 0 passes; one that exits 1 yields a corrective
  condition with the command-gate `failure` payload and `poll.status:
  "failed"`.
- [ ] With `hold_secs` set, a command that reports pending twice and then
  done passes within one `koto next`, and the log holds one `gate_evaluated`
  with `poll.evaluations: 3`.
- [ ] A command killed by its per-run `timeout` yields outcome `timed_out`
  and a command that can't spawn yields `error`, both with `poll.status:
  "failed"` and category `corrective`, never pending.
- [ ] A pending evaluation is logged with outcome `pending`, `poll.status:
  "pending"`, `evaluations`, `since` and `elapsed_secs`, and with no
  `attempt`, `visit_attempt`, `rule_counts` or fallback finding, even when
  the command printed an error finding; every evaluation in one epoch
  carries the same `since`, and the resolving evaluation's `evaluations` is
  the total run count across the pending ticks before it.
- [ ] A command that stays pending past `timeout_secs` across ticks yields
  outcome `timed_out`, `poll.status: "timed_out"` and a koto-written finding;
  one that reports done after the deadline passes; a new entry into the
  state starts a new window with a new `since`.
- [ ] Overriding a pending polling gate passes the state without running the
  command or appending `gate_evaluated`; a gate declared
  `overridable: false` refuses the override.
- [ ] Three pending ticks followed by a done tick leave one `attempt`: the
  done evaluation carries `attempt` one higher than the state's previous
  resolved attempt, and the pending ones carry none.
- [ ] Every fixture template without a polling gate produces logs with no
  `pending` outcome, checked by the existing v0.14.1 compatibility job.
- [ ] No gate source under `src/` names a forge or reads a credential for
  polling: a CI grep of the polling code for `github`, `gh ` and `token`
  finds nothing.

### Contract and compatibility

- [ ] Every fixture template compiles to the same template hash as under
  v0.14.1.
- [ ] The v0.14.1 compatibility job drives a session through a clearing and a
  polling gate with the new koto, then runs v0.14.1's `koto status`,
  `koto next` and `koto context get` on it: all exit 0, the state matches,
  and the cleared key stays absent. A self-test shows the job fails when the
  checked events are removed.
- [ ] `koto template validate-feed` accepts a log carrying `context_cleared`
  and `gate_evaluated.poll`, and the session-feed contract documents both.
- [ ] Each criterion above that a script can check ships with the script and
  a CI job that runs it.
- [ ] The template-format reference, both koto skills and the gate-authoring
  guide mention `clear_on_entry` and `poll:`, and `cargo test --test
  doc_names` passes.
- [ ] The design lists every back edge of shirabe's koto templates and the
  deletable spans with file and line ranges.

## Out of Scope

- Any change to shirabe, including raising its koto floor. shirabe adopts
  these features later.
- A built-in CI or forge gate, and any credential handling.
- Retry caps and escalation on attempt counts. For reference, the settled caps
  shirabe will enforce later are three CI fix pushes and two blocking retries
  for review panels.
- Routing on variable values, the escalation ladder, and a rule registry.
- Waking a session when a polled command would change its answer. Nothing
  outside koto rings a session's wake file for a CI result.

## Decisions and Trade-offs

**A pending polling gate returns rather than holding the turn until done.**
koto wakes a waiting session only when a request leg changes (the per-session
wake file rings on request-store writes). Nothing rings it when CI finishes,
because koto can't know without running the command. Holding the turn for the
whole wait would keep one `koto next` running for as long as CI takes, past
the tool-call limits agent harnesses impose, and a killed process records
nothing. So a tick holds for at most `hold_secs` -- a template sets that to
what its harness tolerates -- and then returns the pending result as a
temporal wait with `retry_after_secs`. The deadline lives in the log, so it
spans ticks and survives a killed process.

**Clearing follows the epoch, not the visit.** `visit_attempt` deliberately
doesn't reset on a self-transition, because it counts tries within one stay
in a state. Clearing has the opposite need: shirabe's analysis state retries
through a self-loop and must start that retry without the keys it wrote the
first time. So clearing fires on every entry, self-transitions included,
which is the boundary koto already uses for overrides and evidence.

**The first entry doesn't clear.** The `transitioned` event `koto init`
writes has nothing before it, so there's nothing to clear and no reason to
log an event on every session start.

**Keys written since the entry are spared.** A `koto context add` between a
`koto rewind` and the next tick belongs to the new attempt. Deciding from the
log (the write's sequence number against the entry's) keeps that write
without reading the store.

**Clearing is recorded as a new event, not on the entry event.** A new
`context_cleared` event can be written by whichever process finishes the
clearing, including the next tick after an interrupted one, while a field on
the entry event would have to be written by every site that appends one, and
in the same append. An older koto skips the unknown event; R2 is what makes
that safe, because the only thing an older koto would restore from the log is
a transition assignment, and a cleared key can't be one.

**Pending is exit code 75 by default.** 75 is `EX_TEMPFAIL`, the conventional
"try again later" status, and it isn't tied to any tool. A template whose
command can't use it sets `pending_exit_code`.

**The polling window starts at the first run, and the log records it.**
Recording only each tick's last run keeps the log to one event per tick, so
the window start can't be read off the first event's timestamp without
drifting by up to `hold_secs`. Writing `since` on every evaluation makes the
start explicit and survives a killed process after the first record.

**Pending is its own outcome value.** `failed` means the check judged the
work and found it bad; a pending tick judged nothing, and a downstream
measurement consumer counting failed checks would otherwise count every tick
of a CI wait as a failure. The cost is one value added to the `outcome` enum,
which a reader pinned to the published list must add; only sessions whose
template declares a polling gate ever write it, so templates without one stay
byte-identical and their logs stay readable by koto v0.14.1. For the same
reason a pending evaluation gets no fallback finding, no rule counts and no
attempt number: the attempt is the evaluation where the poll resolves.

## Known Limitations

- An agent still has to tick again after a pending result; koto says when,
  but it doesn't wake the session.
- A polling gate's in-tick evaluations other than the last aren't logged, so
  the log shows one record per tick, not per run.
- On the cloud backend a remote delete that fails with anything but a version
  conflict only warns, so a later sync pull can bring the key back, exactly
  as it can after `koto context remove` today.
- A key written before the state is entered is cleared on that entry. A
  template that writes a key in one state for a gate in the next declares the
  clearing on the state that writes it.
