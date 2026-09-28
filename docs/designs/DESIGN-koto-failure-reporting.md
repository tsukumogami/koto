---
schema: design/v1
status: Planned
problem: |
  A failed koto check tells the agent almost nothing: a failing command gate
  reaches it as a bare exit code because koto discards the output it already
  captured, and the session log can't say how many attempts a state or a rule
  took, what context was read, or who wrote it. A consumer exporting gate
  events has no field-level contract to read.
decision: |
  Check scripts report findings as `::koto-finding::{json}` lines on stdout;
  a failed corrective check with none gets one koto-written finding. Captured
  output is redacted once at the capture point, then returned in a new
  `failure` object beside each blocking condition's unchanged `output`, with
  a top-level `attempts` object. `gate_evaluated` and `default_action_executed`
  gain optional attempt stamps, findings, per-check rule counts, bounded output
  and a duration; a new `context_read` event and a `writer` field on context
  writes record lineage. All of it is additive and `schema_version` stays 1.
rationale: |
  Keeping new data out of `output` leaves routing, overrides and existing
  templates untouched by construction. Putting counts on the existing check
  events makes attempt records as durable as the gate result, and deriving
  them from the log leaves nothing to drift. Redacting at the capture point
  means no later cut or consumer can expose a known credential. Reusing
  `context_added` for silent writes keeps an older koto correct on a newer log.
upstream: docs/prds/PRD-koto-failure-reporting.md
user_visible_surface: true
---

# DESIGN: koto failure reporting

## Status

Planned

## Context and Problem Statement

When a check fails, koto has already done most of the work needed to explain
the failure and then drops it. `run_shell_command` (`src/action.rs`) drains a
command gate's stdout and stderr into separate buffers bounded at
`MAX_ACTION_OUTPUT_BYTES` (64 KiB each), and `command_gate_result`
(`src/gate.rs`) keeps neither: the gate's output is
`{"exit_code": N, "error": ""}`, with stderr surviving only on a spawn or wait
failure. The `default_action` failure path (`src/engine/advance.rs`,
`action_condition`) already returns both streams, a truncation flag and the
template's fallback prose under the reserved `__action__` condition, so the
engine has one failure path that informs the agent and one that doesn't.

Nothing in the engine counts attempts. `derive_visit_counts`
(`src/engine/persistence.rs`) counts state-entry events for display, and the
decider's `visit_seq` uses a boundary a self-transition resets, which is the
wrong window for retries. The context store's per-key metadata (`KeyMeta` in
`src/session/context.rs`) has no writer, reads leave no trace, and three
internal writers append no event at all. The session-feed contract
(`docs/reference/session-feed.md`) describes `gate_evaluated.output` as
gate-type-specific and lists an `outcome` enum narrower than what koto writes.

The PRD (`docs/prds/PRD-koto-failure-reporting.md`) sets the requirements:
linter-style findings on every failed corrective check (R1-R8), captured and
bounded command-gate output that stays out of routing (R9-R12), value-based
redaction ahead of every cut and every write (R13-R16), four attempt counts
with a visit boundary that a self-transition doesn't reset (R17-R20), logged
context reads and recorded writers (R21-R23), and an additive gate-event
schema written out in the session-feed contract that leaves existing
templates, feed readers and callers unchanged (R24-R30). This design settles
how.

## Decision Drivers

- **Additive only.** New optional fields on existing events and new event
  type names, `schema_version` stays 1, no new `koto next` response variant,
  and compiled templates that don't use the feature stay byte-identical
  (PRD R26, R28, R29; `docs/STABILITY.md`).
- **Routing stays put.** Nothing new enters the gate-output schema that
  `when` clauses and `override_default` are validated against
  (`gate_type_schema` in `src/template/types.rs`; PRD R12).
- **One failure path.** Command gates and `default_action` should produce the
  same payload shape, so an agent and an exporter learn one thing.
- **No template change to benefit.** An existing check script's plain output
  must already produce a useful finding (PRD R5).
- **An exporter reads the contract, not the code.** Every field is named,
  typed and bounded in `docs/reference/session-feed.md` (PRD R25).
- **Secrets never reach disk or the response.** Redaction happens once, at
  the point output enters koto, before any cut (PRD R13-R16).
- **Log volume is bounded per attempt** because the log is written on every
  attempt and read by exporters (PRD R24).
- **Observability never breaks the workflow.** A failed append of a new event
  type warns and continues (PRD R30).

## Considered Options

### Decision 1: How a check hands findings to koto

A check is a shell command koto already runs, so whatever carries findings has
to be something a script can print. The PRD fixes the outer shape: findings
arrive on standard output, standard error is never parsed, a line that doesn't
parse is ordinary output, and finding lines stay in the captured text the agent
sees (R4). What's left is the line grammar and the rules for the fallback
finding koto writes when a failed check reports no `error` (R5).

Key assumptions:

- Rule ids and references are compared as decoded JSON string values, with no
  trimming or normalization.
- Most authors emit finding lines from a wrapper (`jq`, a few lines of Python,
  a converter from a linter's JSON), not from hand-written `echo`.

#### Chosen: one JSON object per line after a `::koto-finding::` prefix

A finding line is a stdout line whose first bytes are `::koto-finding::`,
followed by exactly one JSON object and nothing else except trailing
whitespace (one trailing CR is allowed). The object's keys:

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `rule_id` | string, non-empty | yes | What the finding violated. Opaque to koto. |
| `level` | `"error"`, `"warning"` or `"info"` | yes | Severity. Doesn't change the check's outcome. |
| `message` | string | yes | The rule's own message. May contain `\n`. |
| `path` | string | no | Where the problem is. |
| `line` | integer >= 1 | no | Only with `path`. |
| `column` | integer >= 1 | no | Only with `line`. |
| `rule_ref` | string | no | Opaque pointer to the rule's full text. |
| `effect_landed` | boolean | no | The check's own claim for this finding (R3). |

`null` counts as absent and unknown keys are ignored, so the format can grow.
Any type error, missing required key, empty `rule_id`, unknown level, or a
`line` without `path` (or `column` without `line`) makes the whole line
ordinary output. If stdout was cut at the capture bound, its unterminated last
fragment is never parsed. Parsing runs on redacted stdout (Decision 5), and the
decoded string fields pass through the redactor once more, because JSON
escaping (a `\u` escape such as `\u0041`, or `\/`) can spell a known value that the raw-byte pass
couldn't see.

```text
::koto-finding::{"rule_id":"E501","level":"error","message":"line too long (104 > 88)","path":"src/app.py","line":12,"column":89,"rule_ref":"https://docs.example.org/rules/E501"}
::koto-finding::{"rule_id":"W291","level":"warning","message":"trailing whitespace","path":"src/app.py","line":40}
```

The fallback finding is added to a failed corrective check (command,
context-exists, context-matches, `default_action`) when none of its parsed
findings has level `error`. Its `rule_id` is the gate's name, or `__action__`;
its level is `error`; its `effect_landed` is koto's value; it has no location
and no `rule_ref`. Its message is the first of: the last non-blank line of
stderr (on a timeout that's koto's own `command timed out after N seconds`
note; on a spawn or wait failure it's koto's error text); the last non-blank
stdout line that isn't a finding line; or a sentence koto writes from the
outcome (`command exited with status N`, `context key 'KEY' is not set`,
`context key 'KEY' does not match pattern 'PATTERN'`, `command exited 0 but its
output could not be delivered as NAME`, and, for a context gate whose outcome
is `error`, that gate's own `output.error` text). koto's truncation note is
never chosen as the message. Gate types other than command, context-exists,
context-matches, children-complete and request-leg produce no findings. The
text goes through the existing
`one_line_reason` (`src/engine/terminal_result.rs`): whitespace runs fold to one
space and anything over 500 characters is cut to 497 plus `...`.

The response keeps at most 100 findings and the log at most 50. When a
check's findings fit, they keep emission order with the fallback last. When
they don't, koto picks them by level: errors, then warnings, then info, then
any level it doesn't know, each level in emission order, filling the cap in
that order, and sets `findings_truncated`. The fallback is an error, and it's
written only when no parsed finding is one, so an overflowing list starts with
it and always keeps it. One helper applies both caps. An error is never cut in
favour of warnings, since the point of the list is telling the agent what
failed. Per-rule counts use every parsed finding, not the capped list. The
fallback is also judged over every parsed finding, so a check that prints 100
warnings, then its only error, and fails gets no fallback; its error leads the
capped list, followed by 99 warnings, with `findings_truncated: true`.

#### Alternatives Considered

**GitHub Actions workflow-command lines** (`::error file=..,line=..::msg`).
Rejected because the format has no field for a rule id, a rule reference, the
`info` level or effect-landed, so koto would invent properties; and lines that
existing tools already print for CI would turn into findings with guessed rule
ids, which R4 forbids. It can still be added later as an adapter.

**A findings file named by an environment variable** (JSON Lines or SARIF).
Rejected because R4 puts findings on stdout and keeps them in the captured
output, and a file adds a temporary-file lifecycle, a second input for the size
bound and redaction to cover, and one more variable through the
command-environment rules.

**SARIF on stdout.** Rejected because it's a whole document rather than lines:
any other output mixed in makes it unparseable, and a cut at 64 KiB loses every
finding instead of one line.

**Compiler-style `path:line:col: level: message` parsed by pattern.** Rejected
because it turns ordinary output into findings with guessed rule ids, breaking
R4, and tool formats vary too much for one pattern authors could rely on. R5's
fallback already covers plain output.

### Decision 2: Where the log records findings, output and attempt counts

The PRD wants each recorded check evaluation's event to carry the check's
findings, a failed command gate's captured output, and the attempt counts
(R19, R24), with an exporter able to read all of it from the published
contract. Two check events exist today: `gate_evaluated`, appended by the
advance loop after a state's gates are evaluated, whose append failure fails
the command; and `default_action_executed`, appended by the CLI's action
closure before any gate runs, whose append failure is ignored. A new event type
would be best-effort under R30.

Key assumptions:

- A state entered twice in one invocation (A to B to A) makes one attempt per
  entry, as the PRD's definition of an attempt states.
- A `working_dir` rejection, which fails the action without spawning or
  appending `default_action_executed`, isn't an attempt, as the PRD defines it.
- Temporal gates that append `gate_evaluated` count as attempts but never raise
  per-rule counts, because they carry no findings.

#### Chosen: optional fields on `gate_evaluated` and `default_action_executed`, no new event

Each check event of an attempt carries the same attempt stamp, `attempt`
(session) and `visit_attempt` (visit), so events sharing `state` and `attempt`
form one attempt. Each event carries its own check's `findings`, and a failed
check's event carries `rule_counts` for the rules that check reported at
`error`. Counts are kept per check and rule id: the check is the event's
`gate`, or `__action__` for `default_action_executed`, so two gates that both
report `E501` keep two separate counts. A failed command gate's `gate_evaluated` carries the leading 4 KiB of
each stream beside its unchanged `output`. The full field tables are in
Solution Architecture.

Counts are derived when the event is appended, from the log koto already loads
each tick plus the events this invocation has appended so far. For a state
entry that will evaluate a check:

1. `attempt` = 1 + the highest `attempt` on any earlier event for this state,
   or 1.
2. `visit_attempt` = 1 + the highest `visit_attempt` on events for this state
   inside `delivery_window` (`src/engine/persistence.rs`,
   `Boundary::ArrivalFromElsewhere`, made `pub(crate)`), or 1.
3. For each failed check C of this attempt and each distinct rule id R that C
   reported at `error` (among all its parsed findings and koto's own finding,
   whose rule id is the gate's name or `__action__`): `rule_counts[R].visit` on C's event = 1 + the highest
   stored `visit` for R on C's events for this state in the window, and
   `rule_counts[R].session` = 1 + the highest stored `session` for R on C's
   events for this state across the log.

"One plus the highest stored value" rather than counting events means a rule
a check reports several times in one attempt can't inflate a count, and an
attempt split across a failed append still numbers correctly. The session
maximum is taken per state, like every other count. The stamp is computed when
the loop enters the state and reused on every event for that entry. If nothing
appends, it's discarded.

The `default_action_executed` append moves from the CLI's action closure into
the advance loop. The closure returns the command's result, including a
capture failure it detected, and the loop appends the event once it knows the
findings and counts. That is what lets the event carry `rule_counts` for a
failed action, lets a capture failure's finding reach the log, and puts the
event in the in-tick list so a state re-entered in the same invocation numbers
its next attempt correctly.

`gate_evaluated`'s append failure stays fatal, as today. A failed
`default_action_executed` append, silent today, becomes a warning on stderr,
with the command's outcome unchanged.

#### Alternatives Considered

**A new attempt-level event only** (`attempt_recorded`, carrying each check's
findings, output and counts). Rejected because R30 makes a new event
best-effort: one failed append loses a whole attempt while `koto next`
succeeds, which the hard-failing `gate_evaluated` can't do. It also misses
R24's wording, which puts findings on the check's own event.

**Both: fields on check events plus a thin per-attempt summary.** Rejected
because the summary would be best-effort, so an exporter still has to group by
`(state, attempt)`; its one unique job, a second record for an attempt that
failed at the action, goes to the same file as the event it backs up; and it
needs a finish step on every exit path of the advance loop, where a missed path
looks like a lost append. It can be added later without changing anything here.

**Persisted running counters** (a side file or per-key metadata). Rejected
because it's a second source of truth that can drift from the log, breaks "from
the session feed alone", and saves no I/O, since koto already reads the whole
log each tick.

### Decision 3: Where the failure payload sits in the `koto next` response

A gate's `output` value is used in four places: as the `gates.<name>` evidence
that `when` clauses resolve, verbatim as `gate_evaluated.output`, as an
override's `actual_output`, and as `BlockingCondition.output`. The PRD requires
that nothing new become routable or break an existing `override_default`
(R12, R28), and that every response addition be an optional field in an
existing structure (R29).

Key assumptions:

- The top-level response object counts as an existing structure for R29; the
  conditional `leg` and `leg_abandoned` fields are the precedent.
- Carrying a failed `default_action`'s output twice (its existing `output` keys
  and the new `failure.captured`) is worth one failure shape; it costs at most
  128 KiB, only when an action fails.

#### Chosen: a sibling `failure` object on each blocking condition, and a top-level `attempts` object

`BlockingCondition` gains an optional `failure` next to `output`, present only
on a failed corrective check: `findings` (at most 100), `findings_truncated`,
and, for command gates and `__action__`, `captured` with `stdout`, `stderr`,
`stdout_truncated` and `stderr_truncated`. `output` doesn't change, so for a
command gate it's still `{"exit_code", "error"}` plus `failure_kind` where koto
emits it now.

The response gains an optional top-level `attempts` object when
`blocking_conditions` is non-empty: `visit` and `session` for the blocked
state, and `rules`, keyed first by check (the gate's name, or `__action__`)
and then by rule id, each `{visit, session}`, for every check and rule pair
with a non-zero visit count, including pairs reported on earlier attempts of
this visit, at most 100 pairs with a `rules_truncated` flag when more exist
(the most recently counted pairs are the ones kept).
A passing or evidence-only response is byte-identical to today's.

Internally, `StructuredGateResult` (`src/gate.rs`) gains a typed
`failure: Option<GateFailure>` beside `output`. The evidence map,
`gate_evaluated.output` and `actual_output` all keep reading `result.output`,
and `gate_type_schema` (`src/template/types.rs`) is untouched, so routing and
`override_default` validation can't see the new data by construction.

#### Alternatives Considered

**New keys inside `output`.** Rejected because `output` is routing evidence,
the logged gate output and an override's `actual_output`: captured output would
enter the log unbounded (against R24's 4 KiB cap), sit in every override record
and reach routing evidence, leaving R12 resting on compile-time checks alone.

**Keys inside `output`, stripped before evidence, log and override.** Rejected
because it makes two views of one value that at least four sites must filter
correctly, and any future site must remember to; one missed strip leaks
unbounded output into routing or the log.

**Counts inside each `failure`.** Rejected because counts belong to the state,
so they'd repeat on every failing condition, and per-rule counts include pairs
no current condition reports.

**A top-level `failures` map keyed by condition name.** Rejected because it
splits one check's failure across two places an agent has to join by name.

### Decision 4: How context reads and writers are recorded

The PRD asks that every read of a context key append an event with no content
(R21), that every write record its writer in the log and in the key's stored
metadata (R22), and that a reader of the log alone can name the writer and log
position of the write a read saw (R23). Today `KeyMeta` has no writer, reads
leave nothing, and three internal writers (the batch final view, the workflows
publish key, and cloud sync pulls) append no event. `koto context add` writes
the store before it appends its event.

Key assumptions:

- Two processes appending to one log at once was already possible and read
  events make it likelier, so appends take a short per-session lock.
- A `transitioned` event's assignments need no writer field: the event type
  names the writer.

#### Chosen: a `context_read` event, a `writer` field on writes, and a key-plus-hash join

A new `context_read` event carries `key`, `reader` (`gate`, `cli`, `result`,
`decider`), `state`, `present`, and, when present, `hash`; plus `access`
(`content` or `presence`) and, for gate reads, `gate`. It never carries content
or size. The process that performs the read appends it, through the same path
`koto context add` uses today, so a gate script calling `koto context get`
inside a tick doesn't collide with the tick's lock. Gate reads are held on the
evaluation result, outside `output`, and appended just before that
evaluation's `gate_evaluated`; an evaluation that isn't recorded (a polling
re-evaluation) drops them. koto writes `access` on every read it logs, so
"absent means `content`" only covers events from other writers.

`context_added` and `context_removed` gain an optional `writer` (`agent`,
`transition`, `koto`, `sync`). The three silent writers now append
`context_added` with `writer: "koto"` or `"sync"`. Reusing `context_added`
rather than a new type matters for mixed versions: v0.14.1 already treats a
later `context_added` as superseding a transition assignment when it repairs
the store from the log, so an older koto reading a newer log won't restore a
stale assigned value over a koto or sync write. `KeyMeta` gains an optional
`writer`, and every production write goes through a new
`ContextStore::add_with_writer` whose default body calls `add`, so test
doubles keep compiling.

The join, written into the contract: the write that produced a read of key K
with hash H is the write of K with the highest `seq` below the read's whose
hash equals H (`context_added.hash`, or the SHA-256 of a `transitioned`
assignment's string). Its writer is its `writer` field, `transition` for a
`transitioned` assignment, or unknown for a pre-feature event with no `writer`.
No match means unknown: the value came from a write the log doesn't hold.

#### Alternatives Considered

**The read carries the producing write's `seq`.** Rejected because
`koto context add` stores before it logs, so a read in that window would name
the previous write with nothing to show it's wrong; the seq isn't known before
the append; and on the cloud backend every read would need the whole log.

**A positional join with no hash check.** Rejected because it silently names
the wrong writer after a failed append, a pre-feature silent write, or a read
in the add window, where a hash mismatch says "unknown" instead.

**A new `context_written` type for koto and sync writes.** Rejected because
v0.14.1 doesn't know it and could restore a stale transition value over it.

**Folding reads into `gate_evaluated` and `decider_consulted`.** Rejected
because R21 asks for one event per read, CLI reads have no host event, and
putting reads in gate output would put them in front of routing.

### Decision 5: How known credentials are kept out of captured output

Captured output reaches the response (64 KiB per stream), the log (4 KiB per
stream on the new fields, 64 KiB on `default_action_executed`), finding
messages cut to 500 characters, and `capture_stdout_as` variables. The PRD
requires that no fragment of 8 or more bytes of a known value survive any of
those cuts (R13-R16).

Key assumptions:

- "Configured API keys" means every value koto's configuration supplies for
  them, not only the effective one: a config-file value a later layer or an
  environment variable overrides is searched for too.
- `pass_env:` values count only when they actually reach the command, after
  `build_command_env`'s name filter.

#### Chosen: lookahead retention and one redaction pass at the capture point

Redaction runs once, on raw bytes, inside `run_shell_command`
(`src/action.rs`), after the reader threads finish and before decoding.
`CommandOutput.stdout` and `stderr` become a `RedactedText` type that only the
redactor can construct, so every consumer (gate output, finding parsing, the
fallback message, the log copies, `default_action_executed`,
`capture_stdout_as`) sees redacted text, and a new consumer can't reach raw
capture without a compile error.

The known set is built once per tick beside the command environment, dropping
values shorter than 8 bytes, first source winning on duplicates: the live
values of `CREDENTIAL_CARRIERS` (`src/engine/command_env.rs`), plus the
password from the userinfo of any proxy URL among them, both percent-decoded
and as written; the template's `pass_env:` values; and every configured value
of koto's decider key and cloud access and secret keys: the one koto resolved,
and each value any config file sets, whichever layer it's in and whether or
not a later layer, an environment variable or the project-file rule for the
decider key sets it aside. `KOTO_DECIDER_API_KEY`, `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY` are also read from the environment directly, so they
stay in the set when the configuration fails to load. A legacy session, whose
commands inherit the caller's whole environment, looks the carriers and
`pass_env:` names up in that environment. Each value is also added in its JSON
string spelling (as `serde_json` escapes it, and again with `/` written as
`\/`) when that differs from the raw value, so a finding line can't carry an
escaped copy through the captured text.

The marker is `[REDACTED:<source>]`, where the source is a variable name,
or a configuration setting name (which always contains a dot) for a key read
from the config file. A source name is at most 64 bytes, both when koto writes
a marker and when it recognizes one, so marker-shaped text a command prints
with a longer name isn't treated as a marker. The marker is ASCII, safe inside a JSON string, and fails
the variable-value pattern, so a capture that would store one is refused with a
new `redacted` case instead of being substituted into later commands.

The algorithm, per stream: the reader keeps `limit + L - 1` bytes (L is the
longest known value; exactly `limit` when the set is empty) and counts every
byte read; an Aho-Corasick automaton finds all overlapping matches, which merge
into spans; each span becomes one marker; output is emitted by raw offset up to
`limit`, with a span that starts before `limit` emitted whole; a final cut at
`limit` backs off to the start of any marker it would split and to a character
boundary. When koto killed the process (timeout, wait failure) and the reader
kept everything the stream carried, a trailing suffix of 8 or more bytes that
is a prefix of a known value is masked too. (The condition is on what the
reader kept, not on the capture bound: a value that starts before the bound
and is cut off by the kill just past it would otherwise leave a fragment.)
Each stream reports its own truncation flag, set when the stream carried more
than the bound or its markers pushed it past the bound; in the rare case of a
marker running to the end of a stream just over the bound, the flag says cut
when nothing is missing, which errs toward telling the reader to look. With an empty known set the output is
byte-identical to today's.

#### Alternatives Considered

**Streaming redaction in the reader threads with a held-back window.**
Produces the same bytes, but splits matching state across two threads that
must be proven correct for any read size, to save at most L-1 bytes. Rejected.

**Capture exactly `limit` bytes, then mask a trailing prefix of any known
value.** Rejected because it can't tell a real split from a coincidence, so cut
output ending in `ghp_` or `https://` would carry a marker falsely claiming a
credential.

**Redact at the sinks (response serializer and event appender).** Rejected
because it runs after the 64 KiB, 4 KiB and 500-character cuts, so a value
split at any of them can't be found (R14), and raw capture would stay in memory
where later consumers, such as decider inputs, could reach it (R15).

**Also match base64 or URL-encoded forms.** Rejected: the PRD limits the set to
known literal values and puts pattern detection out of scope, and more
encodings mean more false markers.

## Decision Outcome

A check that fails now explains itself. The command gate keeps the output it
was already reading; the redactor strips known credentials from it before
anything else touches it; the finding parser reads `::koto-finding::` lines out
of the redacted stdout; and a check that printed none gets one finding written
by koto from its last line of output. The agent gets all of that in a
`failure` object next to the condition's unchanged `output`, plus an `attempts`
object saying which try this is, overall and per rule. The session log gets the
same findings, a 4 KiB copy of the output and the attempt stamp on the check's
own event, and every context read and write says who did it.

The pieces fit because each keeps the others' assumptions true. Redaction at
the capture point means Decision 1 can parse, Decision 2 can cut to 4 KiB and
Decision 3 can return 64 KiB without any of them thinking about secrets. The
sibling `failure` field means routing, overrides and `gate_evaluated.output`
never see the new data, so templates that don't use it compile and route
exactly as before. Putting counts on existing check events means the attempt
record is as durable as the gate result, and deriving them from the log means
there's nothing else to keep in sync. Reusing `context_added` for the silent
writes keeps an older koto correct on a newer log.

Nothing here changes `schema_version`, adds a response variant, or adds a
template field. A consumer that ignores the new fields sees today's koto.

## Solution Architecture

### Overview

```text
 run_shell_command ── raw bytes ──> Redactor ──> RedactedText (+ per-stream truncated)
        │                                             │
        │                                  findings::parse(stdout)
        │                                             │
 command gate / default_action ──> StructuredGateResult { outcome, output, failure }
                                                      │
 advance loop: attempt stamp (delivery_window) ─> rule_counts ─> gate_evaluated / default_action_executed
                                                      │
 koto next response: blocking_conditions[].failure + attempts

 context store: add_with_writer(writer) ──> KeyMeta.writer + context_added{writer}
 every logged read ──> context_read{key, reader, state, present, hash}
```

### Components

- **`src/redact.rs` (new).** `Redactor` (known set, Aho-Corasick automaton,
  source names, `Debug` printing names only), `RedactedText`,
  `redact_capture(raw, raw_total, killed, limit, &Redactor)`, `redact_str`,
  `RedactedText::koto_note(&str)` for text koto writes itself (timeout,
  polling and `working_dir` notes), and one marker-safe cut helper used for
  the 4 KiB log copies, the per-field finding caps and, inside
  `one_line_reason`, the 500-character fold. `aho-corasick` becomes a direct
  dependency in `Cargo.toml`.
- **`src/action.rs`.** Reader threads retain `limit + L - 1` bytes and count
  `raw_total`; `CommandOutput` gains `stdout_truncated` and `stderr_truncated`
  (keeping `truncated` as their OR) and carries `RedactedText`.
- **`src/engine/command_env.rs`.** Builds the `Redactor` with the command
  environment and stores it on `CommandEnv`; `for_tick` takes the resolved
  config keys from its caller.
- **`src/findings.rs` (new).** `Finding`, `parse_findings(&RedactedText,
  stdout_truncated, &Redactor)` (the redactor is needed for the second pass
  over decoded strings), `fallback_finding(...)`, and `cap_findings`, which
  applies the 100/50 caps and, past a cap, keeps errors first. A
  command's output records whether stderr ends with koto's own note (timeout,
  wait error), because a polling timeout can report a nonzero exit while
  stderr ends with that note, and the fallback's `message_source` depends on
  it.
- **`src/gate.rs`.** `StructuredGateResult` gains `failure:
  Option<GateFailure>` and `context_reads: Vec<ContextReadRecord>`, both
  `#[serde(skip)]` so its serialized form and existing literal constructions
  change only by `..Default::default()`; `command_gate_result` fills `failure`
  for command gates whose outcome isn't `passed`; context gates fill it with
  their fallback finding and push their reads onto `context_reads`. A passing
  command gate's parsed findings ride on a third skipped field, `findings`, so
  the advance loop can log them on `gate_evaluated` (R8) without putting a
  `failure` on the response.
- **`src/engine/advance.rs`.** In-tick event list; attempt stamp per state
  entry; `effect_landed` filled on every finding the check left unset (true
  when this invocation appended `evidence_submitted` for the state, or the
  state's `default_action` exited 0 and delivered its capture; false
  otherwise, including a capture failure after exit 0; submitted evidence
  belongs to the state it was submitted for, so it counts only before the
  tick's first transition, and the CLI passes whether it recorded evidence
  into the advance loop); `rule_counts` computed
  once per attempt; the `default_action_executed` append, moved here from the
  CLI; new fields written on `gate_evaluated`; gate reads appended before it
  through a best-effort append; `action_condition` fills `failure` for
  `__action__`; `AdvanceResult` gains `attempts: Option<AttemptCounts>`.
- **`src/cli/mod.rs`.** The action closure returns the redacted
  `CommandOutput`, the capture result and whether it spawned, and no longer
  appends; `mark_truncated` uses the per-stream flags.
- **`src/engine/advance.rs` capture check.** `prepare_capture`, where a
  capture is already checked, refuses a value holding a marker with the new
  `redacted` case, after the empty check and before the size and allowlist
  checks, so the author sees the real cause rather than a size or character
  error the marker would also trip.
- **`src/cli/next_types.rs`.** `BlockingCondition.failure`, skipped when
  absent. `attempts` is added to the serialized JSON envelope beside `leg`, the
  way `leg` is added today, whenever the advance result carries counts and the
  response has blocking conditions, so no `NextResponse` variant or combinator
  changes.
- **`src/engine/types.rs`.** New optional fields on `GateEvaluated`,
  `DefaultActionExecuted`, `ContextAdded`, `ContextRemoved`; new
  `EventPayload::ContextRead`.
- **`src/session/context.rs`, `local.rs`, `cloud.rs`, `sync.rs`.**
  `KeyMeta.writer: Option<String>`; `ContextStore::add_with_writer(session,
  key, content, writer)` defaulting to `add`; `ContextStore::meta(session,
  key) -> Option<KeyMeta>` defaulting to `None`, used for presence-read hashes
  (on the cloud backend it falls back to the remote manifest for a remote-only
  key); `CloudBackend::get` appends `context_added {writer: "sync"}` when
  `pull_context_if_newer` reports that it wrote; `reconcile`'s repair writes
  record `transition`.
- **Context-read sites.** `koto context get`/`exists` append from the CLI
  process (`reader: "cli"`, which is also how a gate script's own call
  appears); `terminal_record`, not the read-only status path, appends
  `reader: "result"` reads, including the `failure_reason` read; the decider
  port appends `reader: "decider"` reads just before `decider_consulted`.
  Every new append, and the three newly logged silent writes, go through one
  best-effort helper that warns on stderr (R30). Tests make appends of new
  event types fail through a `cfg(test)`-only hook in that helper.
- **`src/workflows_surface/`.** The publish-location write appends
  `context_added` with `writer: "koto"`; `materialize_after_commit` gains a
  re-entrancy guard so that nested append doesn't materialize again.
- **`src/engine/persistence.rs`.** `delivery_window` becomes `pub(crate)`;
  every append path (`append_event` and `append_event_idempotent_in`) takes
  an exclusive lock on a dedicated per-session sidecar file, held only around
  reading the last seq, writing and syncing, never while a command runs, and
  never the state-file lock a batch tick holds. Where `flock` isn't available
  appends proceed unlocked, as today.
- **`docs/reference/session-feed.md`, `docs/guides/`, and the koto-skills
  plugin.** The contract, a finding-format section in the gate-authoring
  guide, and the `koto-user` and `koto-author` skills' descriptions of
  `blocking_conditions`.

### Response shape

A lint gate fails on the third attempt of this visit, after the agent submitted
evidence:

```json
{
  "action": "evidence_required",
  "state": "lint",
  "blocking_conditions": [
    {
      "name": "ruff",
      "type": "command",
      "status": "failed",
      "category": "corrective",
      "agent_actionable": true,
      "output": {"exit_code": 1, "error": ""},
      "failure": {
        "findings": [
          {"rule_id": "E501", "level": "error", "message": "line too long (104 > 88)",
           "path": "src/app.py", "line": 12, "column": 89,
           "rule_ref": "https://docs.example.org/rules/E501", "effect_landed": true,
           "message_source": "check"}
        ],
        "findings_truncated": false,
        "captured": {
          "stdout": "::koto-finding::{\"rule_id\":\"E501\",...}\nFound 1 error.\n",
          "stderr": "",
          "stdout_truncated": false,
          "stderr_truncated": false
        }
      }
    }
  ],
  "attempts": {
    "visit": 3,
    "session": 5,
    "rules": {"ruff": {"E501": {"visit": 2, "session": 4}}}
  }
}
```

Other response fields are as today and omitted here. The script reported an
`error` finding, so koto wrote no fallback. `effect_landed` is `true` because
the evidence submitted on this invocation was recorded.

### Gate-event schema

All new fields are optional and absent on events written before this feature.
Nested shapes are written out in prose tables in the contract, as
`decider_consulted.fields` already is, because `koto template validate-feed`
checks top-level fields; the frontmatter lists each top-level field with its
type. Conventions the contract states once for all of these fields:

- "A failed check" means a check whose `outcome` is anything other than
  `passed`.
- A field described as present "on" some condition is absent otherwise, and
  an absent boolean means `false`.
- Byte bounds (4 KiB is 4,096 bytes) are measured on the redacted UTF-8 text,
  and a cut never splits a character or a redaction marker. A marker is
  `[REDACTED:<source>]`, where `<source>` is an environment variable name or,
  when it contains a dot, a koto configuration setting.
- `reader`, `writer`, `access`, `message_source` and `level` are open
  vocabularies: consumers tolerate values they don't know, and the frontmatter
  declares them as strings without an `enum`, so validate-feed doesn't reject
  a later value.
- A gate's `context_read` events come immediately before that evaluation's
  `gate_evaluated`. A gate script that calls `koto context get` itself appears
  as `reader: "cli"`.

**`gate_evaluated`** keeps `state`, `gate`, `output`, `outcome` and
`timestamp`; the contract's `outcome` enum becomes `passed`, `failed`,
`timed_out`, `error` (R27). Added:

| Field | Type | Meaning |
|-------|------|---------|
| `attempt` | integer >= 1 | The state's session attempt number, including this attempt. The same on every check event of one attempt. Never resets. |
| `visit_attempt` | integer >= 1 | The state's attempt number in the current visit, including this attempt. Returns to 1 on the first attempt after arriving from a different state or being rewound; a self-transition doesn't reset it. Present whenever `attempt` is. |
| `findings` | array of finding objects | The check's findings, passed or failed, at most 50. When they fit, they're in emission order with the koto-written finding last when there is one; when there are more, koto keeps errors (the koto-written finding after any parsed ones), then warnings, then info, then any other level, each in emission order. Absent when there are none. |
| `findings_truncated` | boolean | `true` when the check produced more findings than `findings` holds. Absent means `false`. |
| `rule_counts` | object | On a failed check that reported at least one finding at `error`: keys are the distinct rule ids this check reported at `error`, taken from every parsed finding rather than only the logged ones; each value is `{"visit": int, "session": int}`, the attempts on this state in the current visit and in the session in which this same check (this event's `gate`) failed and reported that rule at `error`, including this one. A key can name a rule that isn't in `findings` when `findings_truncated` is `true`; that is expected, not corruption. At most 50 keys, first reported first. |
| `rule_counts_truncated` | boolean | `true` when the check reported more distinct rule ids at `error` than `rule_counts` holds. Absent means `false`. |
| `duration_ms` | integer >= 0 | For a command gate: the command's wall-clock run time in milliseconds, measured over the same span the timeout covers. Absent for gates that run no command. |
| `stdout` | string | On a command gate whose `outcome` isn't `passed`: the leading 4 KiB of redacted standard output. |
| `stderr` | string | The same for standard error. |
| `stdout_truncated` | boolean | `true` when `stdout` holds less than the command printed, whether the 64 KiB capture bound or the 4 KiB log cut removed it. |
| `stderr_truncated` | boolean | The same for `stderr`. |

**`default_action_executed`** gains `attempt`, `visit_attempt`, `findings`,
`findings_truncated`, `rule_counts`, `rule_counts_truncated` and `duration_ms`,
with the same meanings;
its check name for `rule_counts` is `__action__`. For an action with `polling:`, `duration_ms`
covers the whole polling loop. Its existing
`stdout`, `stderr` and `truncated` keep their definition (leading 64 KiB per
stream) and now carry redacted text.

**Finding object** (in the response and the log):

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `rule_id` | string | yes | What the finding violated; the gate's name or `__action__` on a koto-written finding. Opaque. |
| `level` | string | yes | `error`, `warning` or `info`. |
| `message` | string | yes | The rule's message, or the koto-written message. |
| `effect_landed` | boolean | yes | Whether the change attempted on this invocation was recorded (R3). |
| `message_source` | string | yes | `check`, `output` or `koto`, as below. |
| `path` | string | no | Location, when known. |
| `line` | integer >= 1 | no | Only with `path`. |
| `column` | integer >= 1 | no | Only with `line`. |
| `rule_ref` | string | no | Opaque pointer to the rule's full text. |

`message_source` is `check` for a finding the check printed, `output` for a koto-written finding whose message is
a line lifted from the check's output, and `koto` for one whose message is
koto's own sentence. Every string has been redacted, and each is capped after
redaction on a character boundary that never splits a marker: `rule_id` 128
bytes, `path` and `rule_ref` 512 bytes, `message` 1,000 bytes (the
koto-written message is already folded to 500 characters). A finding whose
`rule_id` had to be cut keeps the cut value; counts key on the cut value.
These strings come from the check's output and may contain control characters
or terminal escapes; consumers render them as data.

**`context_read`** (new, `tier: 2`). It is the highest-volume event this feature
adds, one per logged read, and consumers that don't need context lineage may
skip it like any `tier: 2` event:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `key` | string | yes | The key read. |
| `reader` | string | yes | `gate`, `cli`, `result` or `decider`. Consumers tolerate unknown values. |
| `state` | string | yes | The workflow's current state at the read. |
| `present` | boolean | yes | Whether the key existed. |
| `hash` | string | no | Lowercase hex SHA-256 of the content, present exactly when `present` is true. |
| `access` | string | no | `content` or `presence` (a context-exists gate or `koto context exists`). Absent means `content`. |
| `gate` | string | no | The gate's name when `reader` is `gate`. |

A presence read of a key that exists but has neither stored metadata nor readable content has no hash to record, so it logs nothing rather than a `present: true` event without `hash`.

**Reserved and aligned names.** The contract reserves the field name
`escalation` on `gate_evaluated` and `default_action_executed` for a later
feature that acts on attempt counts (a retry cap or an escalation step), so it
can be added as an optional field without renaming anything here; readers
ignore it until it's defined. If a check is ever answered by a model rather
than a command, its event names the model with the same `provider` and `model`
fields `decider_consulted` already uses, so the two event families don't
diverge in shape. This feature adds neither field, and it adds no header or
event field identifying a host or agent session.

**`context_added`** and **`context_removed`** gain `writer` (string: `agent`,
`transition`, `koto`, `sync`; consumers tolerate unknown values). The contract
stops saying `context_added` comes only from `koto context add`.

**Reading attempts from the log.** One attempt is the check events sharing
`(state, attempt)`. Per-rule counts are keyed by `(state, check, rule_id)`,
where the check is the event's `gate` or `__action__`; an attempt's counts are
its events' `rule_counts`, each under its own check. Events without `attempt` predate the feature
and are skipped. If the only event of an attempt that failed at its action is
lost, the next attempt reuses its number and the log shows no gap; the contract
says so.

**Worst-case size.** With the field caps above, one check event adds at most
about 120 KiB to the log (50 findings of about 2.2 KiB, 8 KiB of gate output,
50 rule-count keys), and one blocked condition adds at most about 350 KiB to a
response (100 findings and 128 KiB of captured output). Both are stated in the
contract so a consumer can size its buffers.

### Data flow for one failing gate

1. The tick builds the command environment and the `Redactor`.
2. The advance loop enters the state, computes the attempt stamp from the log
   plus this tick's events, and passes it to the action closure.
3. The gate's command runs; `run_shell_command` redacts and returns
   `RedactedText` with per-stream flags.
4. `command_gate_result` sets `output` exactly as today and builds `failure`:
   parsed findings, the fallback when needed, the 64 KiB capture.
5. With all of the state's results in hand, the loop computes `rule_counts`,
   appends any held context reads, then `gate_evaluated` with the new fields.
6. The response builder copies `failure` onto the blocking condition and adds
   `attempts`.

## Implementation Approach

### Phase 1: Compatibility baseline and redaction

Land the compatibility tests first, so every later phase runs against them:
fixture templates compile to the same JSON as under v0.14.1; a v0.14.1 binary
reads a log the new koto writes; a failing response differs from v0.14.1's
only by added optional fields. Then add `src/redact.rs`, the per-stream
truncation flags, lookahead retention in `run_shell_command`, the `Redactor`
built in `command_env.rs`, the `capture_stdout_as` `redacted` refusal, and the
contract text for the redaction marker. Existing `default_action` output and
`default_action_executed` become redacted with no other visible change.

### Phase 2: Findings and the response payload

Add `src/findings.rs`, `StructuredGateResult.failure`, `failure` on
`BlockingCondition` for command, context and `__action__` conditions,
`effect_landed` filling in the advance loop, the finding-format section of the
gate-authoring guide, and the `koto-user` skill's description of `failure`.
Dependencies: Phase 1.

### Phase 3: Attempt counts and the log fields

Add the in-tick event list, the attempt stamp, `rule_counts`, the move of the
`default_action_executed` append into the advance loop, the new fields on
`gate_evaluated` and `default_action_executed`, `AdvanceResult.attempts` and
the `attempts` envelope field, and the contract text for every field this
phase adds, including the corrected `outcome` enum. Dependencies: Phase 2
(counts read findings).

### Phase 4: Context reads and writers

Add `context_read`, `writer` on `context_added`/`context_removed` and
`KeyMeta`, `add_with_writer` and `meta`, events for the three silent writers
(with the re-entrancy guard in `materialize_after_commit`), the best-effort
append helper, the sidecar append lock, and the contract text for the join
rule. Dependencies: none on Phases 1-3; it can run beside them, but it and
Phase 3 both edit `src/engine/persistence.rs`, so land them in sequence.

### Phase 5: Skills and closing checks

Update the `koto-author` skill, run `cargo test --test doc_names` over the new
guide and contract text, and extend the compatibility tests from Phase 1 with
a log that exercises every new field against `koto template validate-feed`.
Dependencies: Phases 1-4.

## Security Considerations

**Credential exposure through captured output.** Command-gate output that koto
used to discard now reaches the agent, the session log (4 KiB per stream per
event, plus findings) and, where cloud sync is on, remote storage. Redaction at
the capture point (Decision 5) replaces every known credential before any
consumer sees the text: the live values of the credential-carrier variables and
the passwords embedded in proxy URLs, `pass_env:` values that reach the
command, and every configured value of koto's own decider and cloud keys,
in any config layer, whether or not a later layer or an environment variable
overrides it. Legacy sessions, whose
commands inherit the whole environment, build the same set from that
environment. Each value is also matched in its JSON-escaped spelling, so a
finding line can't carry an escaped copy through the captured text or the log.
The feature also redacts `default_action` output, which koto writes to the log
unredacted today, so it reduces an existing exposure.

**What redaction does not cover.** A secret koto doesn't know about (a password
read from a file, a token another tool keeps in its own config, a `.env` a
failing test dumps) is now persisted and, with cloud sync, uploaded, where
before a command gate's output was discarded. Encodings other than JSON
escaping (base64 in a Basic auth header, URL encoding, hex, arbitrary `\u`
escapes in raw captured text), values a tool wraps or colors mid-token, and
values under 8 bytes are not matched. Check authors remain responsible for what
their scripts print, and the gate-authoring guide says so beside the finding
format. Redaction keeps known secrets out of the response, the log and remote
storage; it isn't a barrier against a process that can already read the
environment, and an agent that can influence what a check echoes could use the
marker as an equality test.

**Fragments at cuts.** Every cut (64 KiB, 4 KiB, the 500-character fold, the
per-field finding caps) runs after redaction and never splits a marker, and
lookahead retention means a value that starts before the capture bound is seen
whole. When a stream ends mid-value because koto killed the process, a trailing
suffix of 8 or more bytes that is a prefix of a known value is masked; up to 7
bytes of a value's start can remain.

**No new secret surface.** The `Redactor` holds known values for one tick, the
lifetime `CommandEnv` already gives them. Nothing that contains its automaton
derives `Debug`; its own `Debug` prints source names only. Markers name a
variable or setting, never a value or its length. `aho-corasick` becomes a
direct dependency at the 1.x version already in `Cargo.lock` through `regex`,
so no crate is added to the build.

**Untrusted text reaching the agent.** A check is trusted, but what it prints
often isn't: linters quote source lines, test runners print third-party
assertion messages, and tools print pull-request text. That text now reaches
the agent inside `failure`, including as the message of a koto-written finding.
`message_source` tells koto's own sentences apart from lines lifted from the
check's output. koto never interpolates finding fields into commands,
templates or directives, and `failure` stays outside `output`, so routing,
overrides and decider inputs can't see it. The `koto-user` skill tells agents
that `failure` content is the check's output, not instructions, and that a
`rule_ref` isn't to be fetched automatically.

**Context reads.** `context_read` carries a SHA-256 of content, never content
or size, and is written only for keys that pass the key grammar. The same hash
is already in `context_added` and the store's manifest, so no value becomes
easier to guess. Reads appended from a gate's child process never feed an
advance-loop decision.

**Log and response volume.** Finding strings, findings per event, rule-count
keys and captured output are all capped, so one attempt adds a bounded amount
to the log and one blocked condition a bounded amount to the response (the
worst cases are stated under Gate-event schema). A template that loops a
failing state still grows the log linearly with attempts, as it does today
with `default_action_executed`.

**Concurrent appends.** Read events make it likelier that a tick and a
`koto context get` append at once. Every append path takes an exclusive lock on
a dedicated per-session sidecar file, held only around reading the last seq,
writing and syncing and never while a command runs, so a gate's own
`koto context get` can't wait on the tick that is running it, and the state-file
lock a batch tick holds is untouched. The lock is per host; the cloud backend's
cross-host ordering is unchanged. Where `flock` isn't available, appends
proceed unlocked, as they do today.

## Consequences

### Positive

- An agent sees why a check failed, where, and on which try, from the failing
  response alone, with no template change.
- Command gates and `default_action` fail the same way, with one payload shape.
- `default_action` output stops reaching the log with credentials in it.
- An exporter gets per-check and per-attempt records, context lineage, and a
  field-level contract, with no new event it has to treat specially except
  `context_read`.
- A long-standing drift in the contract (the `outcome` enum) is fixed.

### Negative

- Every `pass_env:` value is treated as a credential, so a non-secret
  pass-through value of 8 or more bytes (a region or stage name) is replaced in
  output, and a `capture_stdout_as` that would capture one is now refused.
- Log size grows by up to 50 findings, 8 KiB of gate output and a rule map per
  check event, and by one event per logged context read.
- On the cloud backend, each `koto context get` now appends, which uploads the
  state file.
- A lost `default_action_executed` for an attempt that failed at its action is
  an undetectable undercount.
- A failed `default_action`'s output appears twice in the response.

### Mitigations

- The `pass_env:` behavior is the PRD's stated rule (R13) and errs toward
  hiding; the capture refusal names the variable, so an author sees why. No
  template in this repository's tests or in the published shirabe templates
  declares `pass_env:` and captures one of its values, so the refusal breaks no
  existing template, which keeps the compatibility promise. A per-variable
  opt-out from redaction is later work and can narrow the rule without
  changing anything here.
- The per-event bounds keep log growth proportional to attempts, and exporters
  that don't want findings can skip the field.
- The contract states the undercount case, and it requires an append failure
  on a single event, which already prints a warning.
- The cloud-backend cost of a read event is the same append cost
  `koto context add` already pays; a later change can batch read events into
  the next append if it matters in practice.
- The duplicated action output is bounded at 128 KiB and only on failure; the
  legacy keys can be retired in a later major release.
