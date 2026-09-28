---
schema: design/v1
status: Proposed
upstream: docs/prds/PRD-koto-failure-reporting.md
problem: |
  placeholder
decision: |
  placeholder
rationale: |
  placeholder
---

# DESIGN: koto failure reporting

## Status

Proposed

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
output could not be delivered as NAME`). The text goes through the existing
`one_line_reason` (`src/engine/terminal_result.rs`): whitespace runs fold to one
space and anything over 500 characters is cut to 497 plus `...`.

The response keeps the first 100 findings in emission order, or the first 99
plus the fallback when there is one; the log keeps 50 (49 plus the fallback).
Per-rule counts use every parsed finding, not the capped list. The guide tells
authors to print errors before warnings, since a check that prints its only
error after its hundredth finding shows the agent warnings plus the fallback.

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
  entry. The PRD's "one attempt per invocation" was written about several
  gates evaluated together, and counting per entry keeps a re-entered state
  from showing a new visit with zero attempts.
- A `working_dir` rejection, which fails the action without spawning or
  appending `default_action_executed`, isn't an attempt, as the PRD defines it.
- Temporal gates that append `gate_evaluated` count as attempts but never raise
  per-rule counts, because they carry no findings.

#### Chosen: optional fields on `gate_evaluated` and `default_action_executed`, no new event

Each check event of an attempt carries the same attempt stamp, `attempt`
(session) and `visit_attempt` (visit), so events sharing `state` and `attempt`
form one attempt. Each event carries its own check's `findings`, and a failed
check's event carries `rule_counts` for the rules that check reported at
`error`. A failed command gate's `gate_evaluated` carries the leading 4 KiB of
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
3. For each distinct rule id R reported at `error` by a failed check of this
   attempt: `rule_counts[R].visit` = 1 + the highest stored `visit` for R in
   the window, and `rule_counts[R].session` = 1 + the highest stored `session`
   for R across the log.

"One plus the highest stored value" rather than counting events means two gates
reporting the same rule in one attempt can't inflate a count, and an attempt
split across a failed append still numbers correctly. The stamp is computed
before the action runs, passed into the action closure (a new argument), and
reused on every `gate_evaluated` for that entry. If nothing appends, it's
discarded.

`gate_evaluated`'s append failure stays fatal, as today. A failed
`default_action_executed` append changes from silent to a warning on stderr,
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
state, and `rules`, keyed by rule id, each `{visit, session}`, for every rule
with a non-zero visit count, including rules reported on earlier attempts of
this visit. A passing or evidence-only response is byte-identical to today's.

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
so they'd repeat on every failing condition, and per-rule counts include rules
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
re-evaluation) drops them.

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

- "Configured API keys" means the effective values koto resolved: a key in the
  config file overridden by an environment variable isn't searched for.
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
values of `CREDENTIAL_CARRIERS` (`src/engine/command_env.rs`); the template's
`pass_env:` values; and koto's resolved decider key and cloud access and secret
keys. The marker is `[REDACTED:<source>]`, where the source is a variable name,
or a configuration setting name (which always contains a dot) for a key read
from the config file. The marker is ASCII, safe inside a JSON string, and fails
the variable-value pattern, so a capture that would store one is refused with a
new `redacted` case instead of being substituted into later commands.

The algorithm, per stream: the reader keeps `limit + L - 1` bytes (L is the
longest known value; exactly `limit` when the set is empty) and counts every
byte read; an Aho-Corasick automaton finds all overlapping matches, which merge
into spans; each span becomes one marker; output is emitted by raw offset up to
`limit`, with a span that starts before `limit` emitted whole; a final cut at
`limit` backs off to the start of any marker it would split and to a character
boundary. When koto killed the process (timeout, wait failure), a trailing
suffix of 8 or more bytes that is a prefix of a known value is masked too. Each
stream reports its own truncation flag. With an empty known set the output is
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
  `redact_capture(raw, raw_total, killed, limit, &Redactor)`, `redact_str`, and
  the shared marker-safe cut helper used for the 4 KiB log copies and the
  500-character fold.
- **`src/action.rs`.** Reader threads retain `limit + L - 1` bytes and count
  `raw_total`; `CommandOutput` gains `stdout_truncated` and `stderr_truncated`
  (keeping `truncated` as their OR) and carries `RedactedText`.
- **`src/engine/command_env.rs`.** Builds the `Redactor` with the command
  environment and stores it on `CommandEnv`; `for_tick` takes the resolved
  config keys from its caller.
- **`src/findings.rs` (new).** `Finding`, `parse_findings(&RedactedText,
  stdout_truncated)`, `fallback_finding(...)`, and the 100/50 caps.
- **`src/gate.rs`.** `StructuredGateResult.failure: Option<GateFailure>`;
  `command_gate_result` fills it for failed command gates; context gates fill
  it with their fallback finding and record reads into a side list.
- **`src/engine/advance.rs`.** In-tick event list; attempt stamp per state
  entry; stamp passed to the action closure; `rule_counts` computed once per
  attempt; new fields written on `gate_evaluated`; gate reads appended before
  it; `action_condition` fills `failure` for `__action__`.
- **`src/cli/mod.rs`.** The action closure writes the stamp and findings on
  `default_action_executed`, reports whether it appended, and warns on a failed
  append; `mark_truncated` uses the per-stream flags; `capture_stdout_as`
  refuses a marker with the new `redacted` case.
- **`src/cli/next_types.rs`.** `BlockingCondition.failure` and
  `NextResponse`'s top-level `attempts`, both skipped when absent.
- **`src/engine/types.rs`.** New optional fields on `GateEvaluated`,
  `DefaultActionExecuted`, `ContextAdded`, `ContextRemoved`; new
  `EventPayload::ContextRead`.
- **`src/session/context.rs`, `local.rs`, `cloud.rs`, `sync.rs`.**
  `KeyMeta.writer`, `add_with_writer`, `meta`; cloud pull reports whether it
  wrote.
- **`src/engine/persistence.rs`.** `delivery_window` becomes `pub(crate)`;
  `append_event` takes a short per-session append lock.
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
           "rule_ref": "https://docs.example.org/rules/E501", "effect_landed": true}
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
    "rules": {"E501": {"visit": 2, "session": 4}}
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
checks top-level fields.

**`gate_evaluated`** keeps `state`, `gate`, `output`, `outcome` and
`timestamp`; the contract's `outcome` enum becomes `passed`, `failed`,
`timed_out`, `error` (R27). Added:

| Field | Type | Meaning |
|-------|------|---------|
| `attempt` | integer >= 1 | The state's session attempt number, including this attempt. The same on every check event of one attempt. Never resets. |
| `visit_attempt` | integer >= 1 | The state's attempt number in the current visit, including this attempt. Returns to 1 on the first attempt after arriving from a different state or being rewound; a self-transition doesn't reset it. Present whenever `attempt` is. |
| `findings` | array of finding objects | The check's findings, passed or failed: at most 50 in emission order, with the koto-written finding last when there is one. Absent when there are none. |
| `findings_truncated` | boolean | `true` when the check produced more findings than `findings` holds. Absent means `false`. |
| `rule_counts` | object | On a failed check that reported at least one finding at `error`: keys are those rule ids, each value `{"visit": int, "session": int}`, the attempts on this state in the current visit and in the session in which a failed check reported that rule at `error`, including this one. |
| `stdout` | string | On a failed command gate: the leading 4 KiB of redacted standard output, cut on a character boundary and never inside a marker. |
| `stderr` | string | The same for standard error. |
| `stdout_truncated` | boolean | `true` when `stdout` holds less than the command printed. Absent means `false`. |
| `stderr_truncated` | boolean | The same for `stderr`. |

**`default_action_executed`** gains `attempt`, `visit_attempt`, `findings`,
`findings_truncated` and `rule_counts`, with the same meanings. Its existing
`stdout`, `stderr` and `truncated` keep their definition (leading 64 KiB per
stream) and now carry redacted text.

**Finding object** (in the response and the log): `rule_id`, `level`,
`message`, `effect_landed` (always present), and `path`, `line`, `column`,
`rule_ref` when known. Every string has been redacted.

**`context_read`** (new, tier 2):

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `key` | string | yes | The key read. |
| `reader` | string | yes | `gate`, `cli`, `result` or `decider`. Consumers tolerate unknown values. |
| `state` | string | yes | The workflow's current state at the read. |
| `present` | boolean | yes | Whether the key existed. |
| `hash` | string | no | Lowercase hex SHA-256 of the content, present exactly when `present` is true. |
| `access` | string | no | `content` or `presence` (a context-exists gate or `koto context exists`). Absent means `content`. |
| `gate` | string | no | The gate's name when `reader` is `gate`. |

**`context_added`** and **`context_removed`** gain `writer` (string: `agent`,
`transition`, `koto`, `sync`; consumers tolerate unknown values). The contract
stops saying `context_added` comes only from `koto context add`.

**Reading attempts from the log.** One attempt is the check events sharing
`(state, attempt)`. Per-rule counts for an attempt are the union of
`rule_counts` over those events. Events without `attempt` predate the feature
and are skipped. If the only event of an attempt that failed at its action is
lost, the next attempt reuses its number and the log shows no gap; the contract
says so.

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

### Phase 1: Redaction at the capture point

Add `src/redact.rs`, the per-stream truncation flags, lookahead retention in
`run_shell_command`, the `Redactor` built in `command_env.rs`, and the
`capture_stdout_as` `redacted` refusal. Existing `default_action` output and
`default_action_executed` become redacted with no other visible change.
Dependencies: none. This lands first because every later phase handles
captured output.

### Phase 2: Findings and the response payload

Add `src/findings.rs`, `StructuredGateResult.failure`, `failure` on
`BlockingCondition` for command, context and `__action__` conditions, and the
finding-format section of the gate-authoring guide. Dependencies: Phase 1.

### Phase 3: Attempt counts and the log fields

Add the in-tick event list, the attempt stamp, `rule_counts`, the new fields on
`gate_evaluated` and `default_action_executed`, the top-level `attempts` field,
and the stderr warning on a failed `default_action_executed` append.
Dependencies: Phase 2 (counts read findings).

### Phase 4: Context reads and writers

Add `context_read`, `writer` on `context_added`/`context_removed` and
`KeyMeta`, `add_with_writer`, events for the three silent writers (with the
re-entrancy guard in `materialize_after_commit`), and the per-session append
lock. Dependencies: none on Phases 1-3; it can run in parallel with them.

### Phase 5: Contract, compatibility checks and skills

Write every new field into `docs/reference/session-feed.md` (prose and
frontmatter), fix the `outcome` enum, add the join rule and the attempt-reading
rules, and update the `koto-user` and `koto-author` skills. Add the
compatibility tests: fixture templates compile identically to v0.14.1; a
v0.14.1 binary reads a new log; a failing response differs from v0.14.1's only
by added optional fields. Dependencies: Phases 1-4.

## Security Considerations

**Credential exposure through captured output.** This feature starts
returning command-gate output that koto used to discard, and writes 4 KiB of it
to the session log, which is readable by anything that reads the session store
and is uploaded by cloud sync. Redaction at the capture point (Decision 5)
covers the credentials koto knows about: the carrier variables it already
treats as sensitive, `pass_env:` values, and its own API keys. It also applies,
for the first time, to `default_action` output, which koto already writes to
the log unredacted today, so the feature reduces existing exposure. The
residual risk is a secret koto doesn't know about (a password a script reads
from a file and prints). That stays the check author's responsibility and is
listed in the PRD's known limitations; the guide says so next to the finding
format.

**Fragments at cuts.** Every cut (64 KiB, 4 KiB, 500 characters) happens after
redaction, and the lookahead retention means a value that starts before the
capture bound is seen whole, so no cut can leave a fragment of 8 or more bytes.
The one exception is a stream koto killed mid-value on a timeout, where the
trailing-prefix rule masks a suffix of 8 or more bytes; up to 7 bytes of a
value's start can remain.

**No new secret surface.** The `Redactor` keeps known values in memory for the
duration of a tick, the same lifetime `CommandEnv` already gives them, and its
`Debug` output prints source names only. Markers name a variable or setting,
never a value or its length.

**Context reads.** `context_read` carries a SHA-256 of content, never content
or size. A hash of a low-entropy value (a yes/no flag) could be guessed from
the hash, but the same hash is already in `context_added` and in the store's
manifest, so nothing new is disclosed.

**Injection through findings.** Finding fields are data: koto never
interpolates them into commands, templates, or directive prose, and they reach
the agent inside JSON string values. A malicious check could print misleading
findings, but a check can already print anything it likes and decide the gate's
outcome, so no trust boundary moves.

**Log volume as denial of service.** Bounds per event (50 findings, 4 KiB per
stream) cap what one attempt can add. A template that loops a failing state
grows the log linearly with attempts, as it does today with
`default_action_executed`.

**Concurrent appends.** Read events make concurrent appends from a tick and a
`koto context get` more likely. The per-session append lock around "read last
seq, write, fsync" closes the duplicate-seq hazard that already existed.

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
  hiding; the capture refusal names the variable, so an author sees why. A later
  per-variable opt-out can narrow it without changing anything here.
- The per-event bounds keep log growth proportional to attempts, and exporters
  that don't want findings can skip the field.
- The contract states the undercount case, and it requires an append failure
  on a single event, which already prints a warning.
- The duplicated action output is bounded at 128 KiB and only on failure; the
  legacy keys can be retired in a later major release.
