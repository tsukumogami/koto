---
schema: prd/v1
status: In Progress
problem: |
  When a koto check fails, the agent running the workflow gets too little to
  act on. A failing command gate reaches it as a bare exit code: koto reads the
  command's output and throws it away. After the fact, the session log can't
  say how many attempts a state or a rule took, what context the agent read, or
  who wrote the context value a check tripped on, so template authors and
  anyone measuring workflow health reconstruct it from transcripts.
goals: |
  A failed check tells the agent, in the same response, which rule failed, the
  rule's own message, where the problem is when the check can say, how serious
  it is, and whether the attempted effect landed. The session feed records
  attempt counts per state and per rule, every context read, and the writer of
  every context key, in a gate-event schema written out field by field in the
  published session-feed contract. Templates that use none of this compile and
  run unchanged on the new koto, and nothing in it raises the koto version a
  consumer must run.
absorbed:
  - docs/briefs/BRIEF-koto-failure-reporting.md
---

# PRD: koto failure reporting

## Status

In Progress

Absorbed [BRIEF-koto-failure-reporting](docs/briefs/BRIEF-koto-failure-reporting.md); carried in Absorbed Brief.

## Absorbed Brief

The feature was framed around what an agent learns when a koto check fails,
which today is almost nothing it can act on. A failing command gate arrives as
a bare exit code while the check's own explanation is thrown away, and a
`default_action` failure, which does return bounded output and fallback prose,
still can't say which rule failed, how serious it is, or whether the agent's
change took effect. The same gap exists after the fact: the session log can't
say how many tries a state or a rule took, what context the agent read, or who
wrote the value a check read.

The outcome it set is for the agent to learn from the failing response what
failed, where, how serious it is and whether its change landed, so the next
attempt is a fix rather than a guess; for template authors to get that from
the check scripts they already write; and for anyone reading a finished
session, including a separate program that exports gate events, to count
attempts per state and per rule and trace context reads and writers from the
session feed alone.

Four journeys framed it, carried as the User Stories below: an agent fixing a
failing check on the first retry, a template author getting useful failures
without learning a new format, a maintainer finding the state that keeps
bouncing agents, and an exporter reading gate events without reading koto's
source. The boundary it drew held in the failure payload, bounded command-gate
output with secrets kept out, attempt counts with written reset rules,
context-read and writer records, and a gate-event schema in the published
session-feed contract that leaves existing templates unchanged. It pushed out
the escalation ladder that would act on the counts, a named exit for
unsatisfiable checks, the origin of rule ids and rule references, any
consumer-side change, CI waiting, stale-key clearing and value routing, and the
exporter itself.

## Problem Statement

A koto workflow holds an agent at a gate until a check passes, and the agent
is the one who has to fix whatever made it fail. Today the agent learns almost
nothing about what that is.

Command gates are the sharpest case. koto runs the gate's command, reads up to
64 KiB of its standard output and standard error, and then keeps neither: the
agent sees `{"exit_code": 1, "error": ""}` in `blocking_conditions`. Standard
error survives only when the command couldn't be started or waited on. A check
script that printed exactly which file broke which rule has done that work for
nothing, and the agent re-runs the script by hand, reads its source, or guesses.
A state's `default_action` already does better: when it fails, the agent gets
its bounded output and the template's fallback prose. Gates, where most checks
live, never got the same treatment, and neither path distinguishes a blocking
error from a warning or says whether the agent's change took effect.

The same gap exists after the fact. `gate_evaluated` records that a gate ran
and whether it passed. It doesn't record which rule inside a check failed, and
nothing in koto counts how many attempts a state took. Context reads leave no
record at all. Context writes are recorded for `koto context add` and transition
assignments, but not by writer, and some koto-internal writes leave no event.
A maintainer asking why a state takes six tries, or a separate program exporting
gate events for measurement, has no field-level contract to read and ends up
reading transcripts or koto's source.

This matters now because more koto workflows hand their deterministic checks to
scripts, which makes the check's own explanation the most useful thing the
agent could see, and because an effort to export gate events needs a stable
shape to build against before it starts.

## Goals

- An agent whose check failed knows, from the failing response alone, what
  failed, where, how serious it is and whether its change landed, so the next
  attempt is a fix rather than a probe.
- A template author gets structured failures from check scripts they already
  have, with no template change, and richer ones by printing a documented
  format.
- A reader of a finished session, a person or a program, can count attempts
  per state and per rule, see every context read, and name the writer of every
  context key, from the session feed alone.
- None of it costs anyone who doesn't use it: existing templates, existing
  feed readers and existing callers of `koto next` keep working.

## User Stories

1. As an agent running a workflow, when a lint gate fails after I submit
   evidence, I want the response to name the rule, the file and line, and the
   linter's message, and to tell me my evidence was recorded but the
   transition didn't happen, so that I fix that line and resubmit instead of
   re-running the linter to find out what's wrong.
2. As a template author wiring an existing check script into a command gate,
   I want the script's own output to reach the agent on failure, bounded and
   with secrets kept out, so that I get useful failures without learning a
   koto format; and when I later make the script print a rule id and a
   location, I want those to arrive as fields.
3. As a koto maintainer looking at a batch of sessions, I want to count
   attempts per state and per rule and follow the context reads before each
   attempt back to the writer of the key that was read, so that I can find
   the state that keeps bouncing agents and why.
4. As the author of a program that exports gate events, I want every gate
   event's fields written out with types and meanings in the published
   session-feed contract, so that I build the exporter from the contract and
   it keeps parsing sessions from templates that use none of this.

## Definitions

- **Check**: one gate of a state, or the state's `default_action`. Each check
  already ends in one of koto's outcomes: `passed`, `failed`, `timed_out`, or
  `error`.
- **Failed check**: a check whose outcome is anything other than `passed`.
  `timed_out` and `error` are failed checks for every requirement below.
- **Corrective check**: a check whose blocking condition koto classifies as
  `corrective` today: command, context-exists and context-matches gates, and
  `default_action`. Children-complete and request-leg gates are `temporal`:
  they fail while they wait, which isn't something the agent fixes.
- **Finding**: one reported problem inside a check. A check produces zero or
  more findings.
- **Rule id**: an opaque, non-empty string naming what a finding violated.
  koto never parses it or checks it against a list.
- **Rule reference**: an opaque string pointing to the rule's full text. koto
  carries it without resolving it.
- **Attempt**: for one state, one `koto next` invocation that evaluates at
  least one of that state's checks and records the evaluation in the session
  log (a `gate_evaluated` or `default_action_executed` event for that state).
  Several checks evaluated for the same state on one entry into it are one
  attempt. An invocation that passes through several states makes one attempt
  on each state whose checks it evaluated, and one that enters the same state
  more than once (leaving and coming back within one tick) makes one attempt
  per entry. Re-evaluations inside a polling
  loop that append no event are not attempts; the evaluation that ends the
  loop and is recorded is. A state whose checks are all overridden, or that
  has no checks, accumulates no attempts. A directed transition
  (`koto next --to`) evaluates nothing and is not an attempt.
- **Visit**: the span that starts at the event by which the workflow arrived
  at a state from a different state, or was rewound to it, and runs until the
  workflow leaves for a different state. A transition or directed transition
  from a state to itself does not start a new visit. This is the boundary koto
  already uses to decide when to re-send a phase's instructions, not the
  boundary that scopes evidence, which a self-transition resets.
- **Writer**: the kind of operation that put a context key's current value in
  place. One of: `agent` (`koto context add` or `koto context remove`),
  `transition` (a transition's context assignment), `koto` (a write koto makes
  on the workflow's behalf, such as the batch final view), or `sync` (a pull
  from a remote session store).

## Requirements

### Failure payload

- **R1.** For every failed corrective check on a `koto next` response, the
  check's blocking condition carries a list of findings. Each finding has a
  rule id, a message, a level, and an effect-landed value (R3), and may have a
  location (a path, and optionally a line and a column) and a rule reference.
- **R2.** The level is one of `error`, `warning`, or `info`. Levels describe
  findings; they don't decide the check's outcome, which stays decided exactly
  as today (for a command gate, by exit status).
- **R3.** Effect-landed is `true` when, on the invocation that produced the
  finding, koto recorded evidence the agent submitted for that state, or ran
  that state's `default_action` to a zero exit; otherwise `false`. A finding
  a check emits may state its own value, which then applies to that finding
  only; findings that don't state one take koto's.
- **R4.** A command gate or `default_action` can emit findings as lines in a
  documented format on its standard output. Standard error is never parsed for
  findings. A line that doesn't parse as a finding (including one with an
  empty rule id or an unknown level) is ordinary output: it produces no
  finding and no error. Finding lines stay in the captured output returned to
  the agent. The format is documented, with examples, in koto's user-facing
  guide for writing gates and actions.
- **R5.** A failed corrective check that reports no finding at level `error`
  gets one koto-written finding at level `error`, added to any findings it did
  report. Its rule id is the gate's name, or `__action__` for a
  `default_action`, and its message is, in order of preference: the last
  non-blank line of standard error; the last non-blank line of standard
  output that isn't a finding line; or a sentence koto writes naming the
  outcome (for example the exit status, the timeout, the absent context key,
  or the pattern that didn't match). The message is folded onto one line and
  cut to at most 500 characters, the bound koto already applies to a child's
  failure reason.
- **R6.** Rule ids and rule references are carried byte-for-byte as emitted.
  koto doesn't define where they come from, validate them against a list, or
  resolve references.
- **R7.** A response carries at most 100 findings per check, and says when it
  dropped any. When a check's findings fit, they're in the order emitted with
  the koto-written finding (R5) last. When they don't, errors are kept first
  (the koto-written finding after any the check emitted), then warnings, then
  info, then any other level, each in the order emitted, until the cap is
  reached.
- **R8.** Findings from a check that passed are not returned to the agent.
  They are recorded in the session log (R24) so warnings from clean runs stay
  countable.

### Command-gate output

- **R9.** A failed command gate's blocking condition carries its captured
  standard output and standard error, in addition to the existing `exit_code`
  and `error` keys and, where koto emits it today, `failure_kind`. Their values
  don't change, and a key absent today stays absent. This includes
  `timed_out` gates (whatever the command printed before it was stopped) and
  `error` gates (whatever was captured before the failure).
- **R10.** Captured output returned to the agent keeps the leading 64 KiB of
  each stream, the bound `default_action` output already has, cut on a
  character boundary, with one flag saying when either stream was cut.
- **R11.** A command gate that passes returns nothing new to the agent.
- **R12.** None of the new keys in a gate's output (captured output, findings,
  counts, flags) is part of the gate-output schema that `when` clauses and
  `override_default` are checked against. A `when` clause can't route on them,
  and an existing `override_default` stays valid without listing them.

### Secrets

- **R13.** Before captured output reaches the `koto next` response or the
  session log, every occurrence of a known credential value is replaced with a
  marker naming its source: a variable that holds it, or, for a key read from
  koto's configuration file, the configuration setting. The known set is: the values of the
  credential-carrying variables koto already refuses to record in a session's
  command environment; the values of every variable a template passes through
  with `pass_env:`; and the values of every API key koto itself is configured
  with (cloud sync and decider keys). When one value is held by several
  variables, the marker names one of them.
- **R14.** Replacement runs on the captured bytes before any cut koto makes
  for the response or the log, and an occurrence split by the capture bound is
  replaced along with the part that was captured, so no fragment of a known
  value survives a cut.
- **R15.** Findings, their locations and rule references, and the
  `default_action` output koto already returns and records, all pass through
  the same replacement. No path writes an unreplaced copy.
- **R16.** Values shorter than 8 characters, the minimum koto already uses
  when matching credentials in the command environment, are not searched for.

### Attempt counts

- **R17.** koto keeps four counts: attempts on a state in the current visit;
  attempts on a state across the session; and, for each pair of a check and a
  rule id, attempts on a state in the current visit and across the session in
  which that check failed and reported that rule id at level `error`. A check
  is named by its gate name, or `__action__` for a `default_action`, so the
  same rule id reported by two different checks is counted separately.
  Findings from a check that passed don't raise a per-rule count. A rule
  reported more than once by one check in one attempt counts once for that
  attempt. Per-rule counts are kept per state.
- **R18.** Visit counts reset to zero when a new visit to the state starts.
  Nothing else resets them: a self-transition, a passing check, an override,
  and an evidence submission don't. Session counts never reset.
- **R19.** Every recorded attempt carries, in the session log, the state's
  visit and session attempt numbers including that attempt, and the per-rule
  counts for every rule reported at level `error` on that attempt, so a
  reader gets counts without replaying koto's rules.
- **R20.** A `koto next` response that carries a failed check carries the
  same counts for that state: the attempt numbers, and the per-rule visit and
  session counts for every check and rule id pair with a non-zero visit
  count.

### Context reads and writers

- **R21.** Each of these reads of a context key appends one event: a context
  gate evaluation that is recorded in the log, `koto context get`,
  `koto context exists`, a terminal result's `${context.<key>}`, and a
  decider's context input. The event names the key, the reader (`gate`,
  `cli`, `result`, or `decider`), the current state, whether the key was
  present, and, when it was, the content hash of what was read. It never
  carries content. Reads koto makes for its own bookkeeping (checking an
  assignment before rewriting it, locating a published workflow surface) and
  reads inside polling re-evaluations that append no event are not logged.
- **R22.** Every write or removal of a context key records its writer in the
  session log, and every write records it in the key's stored metadata. This
  includes koto-internal writes and sync pulls that append no event today.
- **R23.** From the session log alone, a reader can name, for any logged
  context read, the writer and the log position of the write that produced the
  value read. A key written before this feature, with no recorded writer,
  reads as writer unknown rather than as an error.

### Gate-event schema and compatibility

- **R24.** Each recorded check evaluation's event carries the check's findings
  (for passed and failed checks), the attempt counts of R19, and the captured
  output of a failed command gate. The log copy keeps at most 50 findings,
  chosen and ordered as R7 describes, and the leading 4 KiB of each output
  stream per event, and says when it cut either.
- **R25.** Every new event and every new field is written out (name, type,
  required or optional, meaning, allowed values) in
  `docs/reference/session-feed.md`, in its prose and in its machine-readable
  frontmatter, so an exporter can consume them without reading koto's source.
  The frontmatter lists top-level fields; nested objects (a finding, a rule
  count) are written out in prose tables, as the contract already does for
  `decider_consulted`, because `koto template validate-feed` checks top-level
  fields.
- **R26.** The additions follow the session-feed contract's existing
  forward-compatibility rules: new fields on existing events are optional, new
  events have new type names, and the header's `schema_version` stays `1`.
- **R27.** The session-feed contract's `gate_evaluated.outcome` values match
  what koto writes: `passed`, `failed`, `timed_out`, `error`.
- **R28.** A template that uses none of this compiles, on the new koto, to the
  same compiled template it compiles to on koto v0.14.1, and routes the same
  way: the gate-output fields a `when` clause or an `override_default` can
  reference are unchanged. No template field is added as required, and no
  consumer has to raise the koto version it requires. One runtime change
  follows from R13 and is accepted: a `capture_stdout_as` whose captured value
  contains a known credential value (including a `pass_env:` value of 8 or
  more bytes) is refused rather than stored. No template in this repository's
  tests or in the published shirabe templates does this.
- **R29.** Every addition to the `koto next` response is an optional field
  inside an existing structure. No response variant is added and no existing
  field changes meaning.

### Non-functional

- **R30.** Failing to append a new event type never fails a koto command
  that would otherwise succeed; the failure is reported on standard error. An
  existing event that now carries new fields (such as `gate_evaluated`) keeps
  today's behavior when its append fails. A context read whose event wasn't
  appended is therefore missing from the log, and R23's join is only as
  complete as the log.

## Acceptance Criteria

### Failure payload

- [ ] A command gate whose script prints `boom` to stdout and exits 1 yields a
      blocking condition whose findings list holds exactly one finding: level
      `error`, rule id equal to the gate's name, message `boom`, and whose
      captured stdout contains `boom`.
- [ ] A command gate whose script prints the format guide's two-finding
      example (an `error` with path, line and rule reference, and a
      `warning`) and exits 1 yields both findings with every supplied field
      preserved and the rule id and reference byte-for-byte as printed, and no
      koto-written finding.
- [ ] A script that prints only a `warning` finding and exits 1 yields the
      warning plus one koto-written `error` finding with the gate's name as
      rule id.
- [ ] A script that prints a malformed finding line (empty rule id, and
      separately an unknown level) and exits 1 yields only the koto-written
      finding, and the malformed line appears in the captured stdout.
- [ ] A script that prints an `error` finding and exits 0 passes: the state
      advances, the response carries no findings, the log's event for that
      evaluation carries the finding, and no per-rule count rises.
- [ ] A script that prints 150 findings and exits 1 yields 100 findings in the
      response and the dropped flag set.
- [ ] A script that prints 101 warnings, then one error, and exits 1 yields
      100 findings in the response with the error first and the dropped flag
      set, and 50 in the log's event for that evaluation, the error first.
- [ ] A command gate that times out yields a koto-written finding naming the
      timeout and whatever the script printed before it was stopped.
- [ ] A failing context-exists gate yields a koto-written finding naming the
      absent key; a failing context-matches gate yields one naming the key and
      the pattern.
- [ ] A failing children-complete or request-leg gate carries no findings.
- [ ] A failing `default_action` yields a koto-written finding with rule id
      `__action__` and effect-landed `false`.
- [ ] Submitting evidence on a tick where a gate then fails yields findings
      with effect-landed `true`; the same gate failing on a tick with no
      evidence yields `false`; a tick whose `default_action` exits 0 and whose
      gate then fails yields `true`; a finding that states its own
      effect-landed value keeps it in every case.
- [ ] `docs/guides/` documents the finding format with the examples the tests
      above use.

### Command-gate output

- [ ] A failing command gate that prints to both streams returns both, with
      `exit_code`, `error` and `failure_kind` unchanged from koto v0.14.1 for
      the same script.
- [ ] A failing gate printing 100 KiB to stdout returns exactly its leading
      65,536 bytes (or fewer, ending on a character boundary when byte 65,536
      falls inside a multi-byte character) with the truncation flag set; the
      same for stderr; exactly 65,536 bytes returns all of it with the flag
      unset.
- [ ] A passing command gate's response is identical to koto v0.14.1's for
      the same template and script.
- [ ] A template with `override_default: {exit_code: 0, error: ""}` on a
      command gate compiles, and a `when` clause naming a new output key is
      refused at compile time as it would be for any unknown key.

### Secrets

- [ ] With `GH_TOKEN` set to a 40-character value, a failing gate that echoes
      it twice to stdout and once to stderr, and prints a finding whose
      message, path and rule reference each contain it, produces a response
      and a log in which the value appears nowhere and the marker naming
      `GH_TOKEN` appears in every place it was.
- [ ] The same holds for a variable declared in `pass_env:`, for the
      configured cloud sync key, and for the configured decider key.
- [ ] A failing `default_action` that echoes the value produces a response
      and a `default_action_executed` event without it.
- [ ] A value that straddles the 64 KiB capture bound leaves no fragment of 8
      or more of its characters in the response or the log.
- [ ] A 7-character value in a credential-carrying variable is not replaced;
      an 8-character value is.

### Attempt counts

- [ ] A state that fails three times and then passes records visit and
      session attempt numbers 1, 2, 3, 4 on its four attempts.
- [ ] Per-rule counts rise only on attempts that reported that rule at level
      `error`; a rule reported twice in one attempt adds one; a rule reported
      only as `warning` has no count.
- [ ] Two failing gates in one state on one invocation are one attempt.
- [ ] A self-transition, an override followed by a failing re-check, and an
      evidence submission each leave the visit count running; arriving from
      another state or rewinding restarts it at 1; the session count keeps
      rising in every case.
- [ ] The same rule id failing in two different states is counted separately
      per state, and the same rule id reported at `error` by two failing gates
      of one state on one attempt yields two counts, one per gate, each
      rising by one.
- [ ] A polling loop that re-evaluates a failing gate five times before it
      passes adds one attempt.
- [ ] A state with no checks, and a state whose only gate is overridden,
      record no attempts; a `koto next --to` records none.
- [ ] A failing response carries the state's attempt numbers and the visit
      and session count of every rule with a non-zero visit count.

### Context reads and writers

- [ ] `koto context get`, `koto context exists`, a recorded context-exists
      gate evaluation, a recorded context-matches gate evaluation, a terminal
      result's `${context.<key>}`, and a decider's context input each append
      one read event with the right reader value; reads of an absent key say
      so and carry no hash; no read event contains the key's content; the
      hash equals the SHA-256 of the content read.
- [ ] After `koto context add`, a transition assignment, a batch final view
      write and a sync pull, each key's stored metadata and the log name the
      right writer; `koto context remove` records `agent` as writer.
- [ ] A key written by a transition, overwritten by `koto context add`, and
      then read, joins to the `koto context add` write from the log alone.
- [ ] A key written by koto v0.14.1 and read by the new koto reads as writer
      unknown without error.

### Schema and compatibility

- [ ] `docs/reference/session-feed.md` lists every new event and field with
      type, requiredness and meaning, and `koto template validate-feed`
      accepts a log from a session that exercises all of them.
- [ ] The contract's `gate_evaluated.outcome` enum lists `passed`, `failed`,
      `timed_out` and `error`.
- [ ] A log event carrying more than 50 findings or more than 4 KiB of a
      stream carries the bounded amount and the cut flag.
- [ ] A session log written by the new koto has `schema_version: 1`, and
      koto v0.14.1's `koto status` and `koto next` read it without error,
      skipping the new events.
- [ ] Every template under the repository's test fixtures compiles to the
      same compiled template JSON under the new koto as under koto v0.14.1,
      and the existing functional test suite passes unchanged.
- [ ] A snapshot of the `koto next` response for a failing gate differs from
      koto v0.14.1's only by added optional fields.
- [ ] With appending new events made to fail by a test hook, `koto next`,
      `koto context get` and `koto context add` still exit as they would
      otherwise and print a warning on standard error.

## Out of Scope

- An escalation ladder that acts on attempt counts, such as stepping up
  guidance or handing off after a number of failures. The counts are recorded
  here; acting on them is a later feature.
- A named exit a workflow takes when a check can't be satisfied. Also later.
- Where rule ids come from and what a rule reference resolves to. A later
  rule-registry feature defines both; here they're opaque strings.
- Routing on findings or counts from `when` clauses or `override_default`
  (R12). Routing on them belongs with the escalation work.
- Findings from children-complete and request-leg gates, which fail while
  they wait rather than because something is wrong.
- Changes to shirabe or any other consumer's templates or skills. Consumers
  adopt the new fields in their own features.
- Waiting on CI from a gate, clearing stale context keys, and routing values
  between states. Each is a separate koto feature.
- The exporter that reads the gate events. This feature defines the shape it
  reads, not the program.
- Pattern-based secret detection for credentials koto doesn't know about.

## Decisions and Trade-offs

**What counts as an attempt.** Decided: one `koto next` invocation that
evaluates and records a state's checks, whether or not evidence came with it.
Alternatives: count evidence submissions only, which misses states with no
`accepts` block whose gates re-run on every tick; count every gate
evaluation including polling, which would inflate counts by the polling
interval rather than by anything the agent did. The chosen definition counts
what the agent can see and cause.

**When counts reset.** Decided: a visit starts when the workflow arrives from
another state or is rewound; a self-transition continues it. Alternative: the
epoch boundary koto uses for evidence, which a self-transition resets. Many
templates retry by looping a state onto itself, and resetting there would
report every retry as a first attempt. The rule matches the one koto already
uses for re-sending instructions. Session counts that never reset sit beside
it, so a reader who wants another window can compute one.

**Per-rule counts are keyed by check and rule id.** Two checks can emit the
same rule id (two linters both reporting `E501`), and a count keyed by rule id
alone would merge them so that no reader could separate them afterwards. The
check's name is already on every event, so keying by the pair costs nothing.

**Per-rule counts count errors only.** Warnings don't block, so counting them
would make "attempts that failed on this rule" disagree with what blocked the
agent. Warnings are still in the log (R8, R24) for anyone who wants them.

**A koto-written error finding on every failed corrective check.** It makes
every failure countable per rule without asking authors to change anything,
and it's what lets a warnings-only failing script still say why it blocked.
Its rule id names the check, not a rule, and doesn't define where real rule
ids come from.

**Errors survive the findings cap.** Decided: when a check's findings exceed
the cap, errors are kept first, then warnings, then info. The first version
kept findings in the order emitted, so a check that printed a hundred
warnings and then its only error showed the agent warnings only. An error
must never be cut in favour of warnings, since the point of the list is
telling the agent what failed. Within the cap, emission order is unchanged.

**Findings on stdout only.** Linters and test runners already print their
diagnostics there, and keeping stderr free for the script's own errors means
a failing script can't be mistaken for a clean one by accident.

**Output bounds.** 64 KiB per stream in the response, the bound
`default_action` already has, so the two failure paths behave the same. The
log keeps 4 KiB per stream and 50 findings per event, because the log is
written on every attempt and read by exporters; the response is read once.
100 findings in a response is more than an agent acts on in one attempt.

**Secrets.** Value-based replacement of known credentials, extended from the
list koto already uses for the command environment. Alternative: pattern
matching for token shapes, which misses credentials with no fixed shape and
mangles ordinary text that happens to match. It can be added later.

**Writer vocabulary.** The framing named a state's own action and a child
workflow as possible writers. Neither writes to the context store today: a
`default_action`'s captured output goes to variables, and child results reach
the parent through koto's own batch view. The vocabulary names what actually
writes, and `koto` covers writes made on children's behalf.

**Framing open questions.** The framing deferred three questions here: what
counts as an attempt and when counts reset (Definitions, R17-R18), the output
bound (R10, R24), and how secrets stay out (R13-R16; mechanism in the design).

## Known Limitations

- Replacement only catches values koto knows are credentials. A secret a check
  prints that isn't in the known set reaches the response and the log. Check
  authors remain responsible for what their scripts print.
- Effect-landed is koto's view of its own invocation unless a finding says
  otherwise. koto can't know whether something the agent did outside koto
  took hold.
- Attempt counts start from the first attempt after upgrade. Sessions already
  in flight have no counts for earlier attempts, and keys written before the
  upgrade have no writer.
- A read whose event couldn't be appended is missing from the log (R30).
