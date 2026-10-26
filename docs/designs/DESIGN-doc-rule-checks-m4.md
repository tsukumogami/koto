---
schema: design/v1
status: Accepted
upstream: docs/prds/PRD-doc-rule-checks-m4.md
problem: |
  koto's check evaluator sends a decider check's whole extraction output to
  the decider as one slice per criterion, under a four-consultation cap it
  shares with the routing decider, and its records, fallback and answer
  command know criteria but not units. A failed criterion can't hand the
  agent its rule, and nothing records which unit was graded, how confident
  the decider was, or which rule text went out at which level.
decision: |
  A decider check opts into unit mode with `units: jsonl`; the evaluator
  parses the extraction's JSON lines into units and grades each (unit,
  criterion) pair with one request under a separate per-call unit budget of
  32. An optional unit reference rides on findings, the three decider-check
  events, reuse and answer lookup, and `koto decider answer --unit`. Response
  assembly adds a `rules` entry per failed criterion, summary or full by the
  existing per-rule visit count, and appends a `rule_delivered` event after
  printing. The run journal relays unit keys, an integer confidence and the
  delivery level.
rationale: |
  Parsing in the evaluator keeps the engine free of I/O and reuses the
  extraction's capture, redaction and timeout. A second budget leaves the
  routing decider's allowance alone. Optional fields skipped when empty keep
  old templates, logs and ledgers byte-identical. The per-rule visit count
  already resets on a new visit and counts a reused failing verdict, so the
  summary-then-full ladder needs no new storage, and choosing the text at
  response assembly keeps it out of every record.
decision_provenance: inline-resolved
---

# DESIGN: per-unit decider checks that hand back the broken rule

## Status

Accepted

## Context and Problem Statement

`CliCheckEvaluator::evaluate` (`src/cli/check_evaluator.rs`) grades a
`decider-check` gate. It runs the gate's extraction command once, takes its
whole stdout as one slice, and asks the decider one choice question per
criterion with that slice as the only input. The slice is held to the
check's `max_bytes` (default 2,560, range 1 to 8,192): over it, every
criterion is `unanswered` with `over_budget` and falls back to the agent.
Consultations draw on `ConsultBudget`, a per-`koto next` counter capped at
four (`MAX_CONSULTATIONS_PER_CALL`) and shared with the routing decider; a
state holds at most four decider criteria for the same reason. Reuse within
a visit and the lookup of an agent's answer both key on state, gate,
criterion id, declaration hash and the slice's hash.

The requirements are in `docs/prds/PRD-doc-rule-checks-m4.md`. In technical
terms they ask for five changes.

1. **Grade units, not one slice.** A check that declares `units: jsonl`
   gets one JSON object per line from its extraction (`key`, `text`, and
   optional `document` and `section`), and each unit's `text` is graded as
   its own slice against each criterion (PRD R1, R2, R5, R6). Keys,
   documents and sections must fit koto's name shape so they can go into a
   command line and onto exported records as written (R3).
2. **Budget units apart from the shared cap.** A state's unit-mode checks
   may make up to 32 consultations per evaluation, and none of them may
   spend the four slots the routing decider and single-slice checks share
   (R4, R7). A unit past the bound, over budget, or unanswered falls back
   to the agent on its own while the other units keep their verdicts (R10).
3. **Carry the unit everywhere a criterion goes today.** Findings, the
   gate's output, the fallback, the agent's answer, verdict reuse and
   override records all know criteria only (R9, R11). Each needs the unit.
4. **Hand back the rule.** A failing veto criterion that declares a
   `summary` or `rule_text` puts the rule in the `koto next` response, the
   summary on the first failing evaluation of that check and rule in the
   visit and the full text after that, and nowhere else: not in finding
   messages, the session log, the decider ledger or the run journal (R12 to
   R15).
5. **Record it for telemetry.** Unit keys on the three decider-check
   records, the decider's confidence as an integer, and one record per rule
   delivery with its level, all relayed by the run journal, with a readout
   that reads only exported records (R16 to R21).

Three facts about the code shape the answer. The engine (`src/engine/`)
does no network or file I/O beyond appending events; the decider is reached
from the CLI's evaluator. Every new field on a compiled type, an event or a
ledger line must be skipped when empty, so templates, logs and ledgers that
don't use the feature stay byte-identical (R1, R22). And the run journal
copies only names, ids, counts and booleans field by field; its reference
promises it holds no probability, and its writer has no float builder.

## Decision Drivers

- **Unit grading under the validated configuration.** One question per
  request, one unit's text as the only input, the existing `max_bytes`
  budget per unit (PRD R5, R6).
- **Don't touch the shared cap.** The routing decider's four consultations
  per `koto next` stay four, and a unit-mode check can still grade ten or
  more units in one call (R7).
- **Never pass for lack of a verdict.** Every unit with no verdict, for any
  reason, goes to the agent (R10), including when a concurrent `koto next`
  holds the decider lock past the wait (R7).
- **Byte-identical when unused.** A template without the new fields
  compiles to the same JSON and template hash, an existing single-slice
  check produces the same findings, records and cap use, and koto v0.14.1
  still reads logs and ledgers holding the new records (R1, R22).
- **Rule text in the response only** (R14), and summary-then-full keyed on
  the same check, rule and visit (R13).
- **Telemetry a consumer can read without the source document.** Unit keys
  exported as written, a confidence the journal can carry, one record kind
  for rule deliveries, every name in `koto.` and colliding with nothing
  (R16 to R20).
- **Small engine surface.** New behavior lives in the CLI evaluator, the
  template compiler and response assembly; the engine gains event types
  and nothing else.

## Considered Options

### Decision 1: How units are produced, parsed and graded

Resolved inline.

The PRD fixes the wire contract: one extraction, one JSON object per line.
Left open is where parsing happens and how a unit's outcome is decided
before anything is sent.

Key assumptions:

- The extraction's output fits the existing stdout capture bound. In unit
  mode a truncated capture makes the evaluation an extraction failure,
  since a cut line can't be parsed.
- Unit text passes through the same redaction as a slice does today.

#### Chosen: the evaluator parses JSON lines and grades (unit, criterion) pairs in print order

`evaluate` keeps running the extraction through `run_shell_command`, with
the gate's timeout and the tick's redactor. In unit mode it then parses
stdout into `Vec<Unit { key, text, document, section }>`: blank lines are
skipped, unknown fields ignored, a `null` optional field read as absent.
Any line that isn't a JSON object with string `key` and `text`, any field
outside the name shape (`^[A-Za-z0-9._:@/-]{1,128}$`, not starting with
`/`, `~` or `-`), or a duplicate key makes the evaluation an extraction failure:
every criterion `unanswered` with `extraction_failed`, no unit attached,
exactly as a failed command is handled today.

Each unit then gets its own early outcome, in this order: past the check's
`max_units` (or the per-call unit budget, Decision 2) is `unanswered` with
`unit_limit`; empty or whitespace-only text is `not_graded`; text longer
than `max_bytes` is `unanswered` with `over_budget`. Units with no early
outcome are graded per criterion, units in print order and criteria in
declaration order, each with one `DecisionRequest` whose single input is
`{label: text}`. `input_sha256` is computed per unit from its request, so
two units' verdicts never collide even when reuse is keyed only by hash. No
units at all grades nothing: every criterion `not_graded`, gate passed.

#### Alternatives Considered

- **A per-unit loop.** Run the extraction once per unit key. The template
  would have to know the key list up front, or run a second command to list
  it, and a 30-unit section would spawn 30 processes per evaluation.
  Rejected.
- **A delimiter-separated format** (NUL or a marker line). Units are often
  multi-line Markdown, and there is nowhere to put the document and section
  without inventing a header syntax. Rejected for JSON lines, which every
  script language can print.
- **Splitting in koto from a document path.** koto would read the document
  and know Markdown structure. That puts file I/O and format knowledge in
  koto and moves the slicing contract out of the template author's hands.
  Rejected.

### Decision 2: How unit consultations are budgeted, and the decider lock

Resolved inline.

Key assumptions:

- Consultations run one after another at the measured latency, so ten
  units under one criterion take a few seconds per evaluation.
- A concurrent `koto next` that gives up after the existing 60-second wait
  cap is acceptable, because giving up falls units back rather than passing
  them.

#### Chosen: a compile-time per-state bound and a separate per-call unit budget

At compile time, `validate_state_decider_checks` sums `max_units` times
criteria over a state's unit-mode checks and refuses more than 32 with
`E-DECIDER-CHECK-UNITS`, as it refuses `max_units` below 1. A check that
doesn't declare `max_units` gets 32 divided by the number of criteria
across the state's unit-mode checks, rounded down, so defaults always fit.

At run time `handle_next` creates a `UnitBudget` (the same shared-counter
shape as `ConsultBudget`, capped at 32) beside the existing budget, and
hands it to `CliCheckEvaluator`. A unit-mode check draws a slot from
`UnitBudget` per (unit, criterion) request it is about to send, never from
`ConsultBudget`; single-slice checks and the routing port keep drawing from
`ConsultBudget` alone. A slot is taken only when a request is about to be
sent, never for a reuse, an agent answer, a `busy` unit or an early
outcome, as `ConsultBudget` works today. A unit that finds the unit budget spent, which can
only happen when one `koto next` auto-advances through several unit-mode
states, is `unanswered` with `unit_limit`.

The evaluator still takes the session's `decider.lock` for the whole
evaluation. `lock_wait_for` keeps its 60-second ceiling. A unit-mode
holder can outlast it, so a waiting `koto next` whose wait runs out marks
every unit it would have consulted `unanswered` with `busy`, and each
falls back. That is today's rule for criteria, applied per unit; nothing
passes for lack of a verdict.

#### Alternatives Considered

- **Raise `MAX_CONSULTATIONS_PER_CALL` to 36.** One counter, but it raises
  the routing decider's per-call allowance too and makes the lock-wait
  sizing meaningless. Rejected.
- **Units under the shared cap.** No new counter, but every unit past the
  fourth falls back with `cap_spent`, which defeats the feature. Rejected.
- **Size the lock wait to 36 consultations.** About two minutes at the
  default timeout, which holds an agent's turn for longer than the work
  saves. Rejected; falling back is the safe outcome.

### Decision 3: How unit identity reaches findings, output, fallback and answers

Resolved inline.

Key assumptions:

- With many failing units, the response's 100-finding and the log's
  50-finding caps can truncate; `findings_truncated` says so, errors first.

#### Chosen: an optional unit reference wherever a criterion is named

A new `UnitRef { key, document: Option, section: Option }` in
`src/decider/check.rs` is added, as an optional field skipped when `None`,
to `DeciderCheck` (the `decider_checked` event and ledger `checked` line),
`DeciderFallback`, `DeciderCheckAnswered` and `Finding`
(`src/findings.rs`). `reusable` and `applicable_answer` add the unit to
their match, so an answer or verdict for one unit never applies to
another.

`result_for` builds one error finding per failed (unit, veto criterion),
with the criterion's `rule_id` and `rule_ref` and the unit's `UnitRef`,
message `criterion <id> failed on unit <key>: <fail description>`, cut to
1,000 bytes. It builds one warning finding per waiting (unit, criterion),
whose message ends with the exact command, `koto decider answer <session>
--gate <gate> --criterion <id> --unit <key> --answer <pass|fail|unclear>`,
each word shell-quoted. The gate output keeps `failed`, `unanswered` and
`awaiting` as deduplicated criterion ids, and in unit mode gains `units`:
one object per printed unit with `key`, `failed`, `awaiting` and
`agent_answered` (criterion ids). `units` is not added to
`gate_type_schema`, so existing `override_default` values stay valid. The
gate's outcome follows the PRD's fixed rule: failed if any veto (unit,
criterion) failed on a verdict or on the agent's answer, else pending if
any (unit, criterion) waits, shadow included, else passed.

`koto decider answer` (`src/cli/decider_answer.rs`) gains `--unit <key>`.
`latest_fallback` matches on the unit. The command refuses with
`unit_required` when the criterion's pending fallbacks in the latest
evaluation name units and none was given, `unit_not_expected` when they
name none and one was given, `unknown_unit` when the key wasn't printed by
the latest evaluation, and the existing `no_fallback` when the unit was
printed but has no pending fallback for that criterion. The success JSON
gains `unit`. `override_records` writes one `check_overridden` per failed
(criterion, unit) read from `output.units`, falling back to per-criterion
records when `units` is absent.

#### Alternatives Considered

- **Unit-qualified rule ids** (`ac_binary/AC-3`). No new fields anywhere,
  but per-rule counts would split by unit, breaking the summary-then-full
  ladder and every consumer that joins on the rule id. Rejected.
- **Reusing `Finding.path` and `line`.** A document is usually a path, but
  a section isn't a line, and a consumer couldn't tell a unit from a
  linter's location. Rejected for an explicit field.

### Decision 4: Where the rule is delivered and how its level is chosen

Resolved inline.

Key assumptions:

- `attempts.rules[gate][rule].visit` is in the response whenever a decider
  check fails with an error finding. It is computed after the failing
  evaluation, so it already counts the evaluation being reported.
- The compiled template is in hand at response assembly.

#### Chosen: response assembly adds `rules`, levelled by the per-rule visit count; a `rule_delivered` event follows the print

`CheckCriterion` gains `summary: Option<String>` (1 to 300 bytes) and
`rule_text: Option<String>` (1 to 4,096 bytes), validated by
`validate_spec` with `E-DECIDER-CHECK-RULE`, both skipped when absent, and
left out of `declaration_hash` because neither is sent to the decider.

Response assembly in `src/cli/next_types.rs`, beside `attach_attempts`,
walks the blocking conditions. For a decider-check condition whose
`output.failed` names a criterion that declares a rule, it adds to that
condition a `rules` array with one object per such criterion: `rule_id`;
`level`, which is `summary` when `attempts.rules[gate][rule_id].visit` is 1
and `full` when it is 2 or more; `text`, the summary or full text, falling
back to the other when only one is declared (a summary-only criterion
always sends `summary`, a `rule_text`-only one always sends `full`); and in
unit mode `units`, the keys from `output.units` that failed it. The visit
count is koto's existing per-rule count: it resets when the workflow
arrives in the state from elsewhere or is rewound into it, counts every
failing evaluation in the visit, reused verdicts included, and is kept per
(state, check, rule), so a second rule or another check starts at 1. It
is per rule, not per unit: a unit failing for the first time under a rule
that already failed in the visit gets the full text, which is the PRD's
meaning of a repeat. Shadow criteria never appear in `output.failed`, so
they never deliver.

Two details keep the count honest. A pending evaluation writes no rule
counts, so the combining rule's order matters: an evaluation with both a
failed and a waiting unit is `failed`, which writes the count and delivers
the rule; letting `pending` win would lose both. And when `attempts` has
no entry for a failing rule (no attempts object, or the 100-pair response
cap or 50-rule log cap dropped it), `attach_rules` delivers `summary`, the
level that never over-delivers. `attach_rules` takes the `AttemptCounts`
already computed for the response rather than recomputing them.

After the response is printed, `handle_next` appends one `rule_delivered`
event per `rules` object, as it appends `instructions_delivered`: a crash
between print and append loses the record, never the delivery. The text
lives only in the compiled template and the printed response.

#### Alternatives Considered

- **Text in the finding message.** One finding per failing unit would
  repeat the text, overrun the 1,000-byte message bound, and copy it into
  `gate_evaluated` and the log. Rejected.
- **A `rule_text` field on `Finding`.** Additive in the response, but
  `Finding` is logged with the gate event, so the text would reach the log.
  Rejected.
- **The evaluator reads earlier `rule_counts` and decides the level.** It
  runs before the current evaluation's counts exist, so it would
  re-implement the count, and its result would still have to travel to the
  response without being logged. Rejected.
- **A top-level `rules` key.** It would separate a rule from the check
  that failed it when several checks fail at once. Rejected.

### Decision 5: The telemetry events, attributes and readout

Resolved inline.

Key assumptions:

- `rule_delivered` is koto's only rule-delivery record kind. Work that
  later records command-gate deliveries writes this kind and may add
  attributes to it, but adds no second kind.

#### Chosen: unit fields on the three decider-check records, an integer confidence, and one `rule_delivered` kind

| Record (event and journal kind) | When it's written | New attributes |
|---|---|---|
| `decider_checked` | each (unit, criterion) consultation, or early unit outcome, that isn't a reuse | `koto.unit.key`, `koto.unit.document`, `koto.unit.section` (unit mode only); `koto.decider.confidence_bp` whenever an answer was read, unit mode or not |
| `decider_fallback` | each (unit, criterion) handed to the agent | `koto.unit.key`, `koto.unit.document`, `koto.unit.section` |
| `decider_check_answered` | each `koto decider answer` for a unit | `koto.unit.key`, `koto.unit.document`, `koto.unit.section` |
| `rule_delivered` (new) | once per `rules` object, after the response prints | `koto.state`, `koto.check`, `koto.rule.id`, `koto.rule.delivery` (`summary` or `full`), `koto.rule.unit_count` (unit mode only), with `koto.seq` and `koto.visit.seq` like every check record |

The journal's values and their shapes:

| Attribute | Value | Shape |
|---|---|---|
| `koto.unit.key` | always the key's digest, `sha256:` and 32 hex characters, as rule ids outside the name shape are digested | id |
| `koto.unit.document` | the check's declared `unit_document`; else the digest of the printed `document`; absent when neither | name or digest |
| `koto.unit.section` | the check's declared `unit_section`; else the digest of the printed `section`; absent when neither | name or digest |
| `koto.decider.confidence_bp` | integer 0 to 10,000 | integer |
| `koto.rule.delivery` | `summary` or `full`, a closed vocabulary | name |
| `koto.rule.unit_count` | integer 1 to 32 | integer |

No raw value an extraction printed reaches the journal: paths and headings
can carry work names, the reason context keys never leave the host. The
session log keeps the raw unit (it stays on the host), so a local reader
can still map a digest back to its unit. A value outside its shape is
dropped silently by the journal writer, which is why each field above is
built to fit.

`koto.decider.confidence_bp` is the highest of the read probabilities for
`pass`, `fail` and `unclear`, times 10,000, rounded half up. It's an
integer, so the journal's existing writer carries it. The journal's
no-probability rule appears in four places in koto (the run-journal
reference, the module documentation in `src/run_journal/mod.rs` in two
spots, and the test that a planted probability isn't copied), and in the
telemetry definitions' matching requirement outside this repository; all
of them are amended, in the same changes, to name this one exception. The
answer itself is `koto.decider.outcome`, which the record already carries.
Every new name is checked in `tests/run_journal_doc_test.rs`'s collision
tests: none equals or is a dot-prefix of an existing name, `koto.unit`
and `koto.rule` exist only as prefixes, and the delivery level is named
`koto.rule.delivery` so it can't be read as a severity beside
`koto.finding.level`.

The session feed declares `rule_delivered` and the optional `unit` object
on the three events. Optional fields need no version bump, and koto
v0.14.1 skips an unknown event type, which the compatibility job proves.

The feature's evidence is read by one named telemetry query over the
exported run-journal records, the telemetry readout. Its text lives with
the telemetry definitions, outside this repository. It reads one record
kind per query, because grouping drops a record that lacks a grouped
field: unit-mode `decider_checked` records by document, section, unit,
rule and outcome with the confidence; `decider_fallback` records by unit,
rule and reason; and `rule_delivered` records by rule and delivery level;
each per host. Template identity comes from joining each record's session
id to that session's `session_started` record, which carries the template
name and hash. The readout reads no derived per-template series, so a
template version too new to have one still reads. koto writes the run
journal only; the exporter outside this repository decides which kinds and
fields leave the host, and stamps the event time, so koto writes neither.

#### Alternatives Considered

- **Delivery as fields on `check_evaluated`.** One record per evaluation,
  not per delivery, and a command gate's later delivery would need a
  different home. Rejected.
- **A float confidence.** It needs a new writer and unwinds the journal's
  rule outright rather than for one named value. Rejected.
- **New derived per-template series for the readout.** Those can come back
  empty for a new template version; a session join works for any version.
  Rejected.

## Decision Outcome

A template author adds `units: jsonl` to a decider check and an extraction
that prints one JSON line per unit. The evaluator parses the lines, gives
each unit its early outcome, and grades the rest one (unit, criterion)
request at a time against a 32-slot unit budget that never touches the
routing decider's four. Unit identity rides on findings, the gate's
`units` output, the fallback and the agent's answer, so every unit is
graded, handed back, and answered on its own, and a unit with no verdict
always goes to the agent. When a veto criterion fails, response assembly
reads koto's per-rule visit count, attaches the rule's summary on the
first failing evaluation in the visit and its full text after that, prints,
and only then records a `rule_delivered` event. The three decider-check
records carry the unit and an integer confidence, and the run journal
relays all of it under `koto.` names that collide with nothing, for a
readout that reads only exported records.

## Solution Architecture

### Overview

```
koto next ──► handle_next ── budgets: ConsultBudget(4), UnitBudget(32)
                 │
           advance loop ── gates ──► CliCheckEvaluator::evaluate
                 │                     units: jsonl?
                 │                       no ─► today's single-slice path
                 │                       yes─► parse JSON lines → Vec<Unit>
                 │                             early outcome per unit
                 │                             (unit_limit, not_graded, over_budget)
                 │                             lock → per (unit, criterion):
                 │                               reuse | agent answer | consult (UnitBudget)
                 │                             findings + output.units + records
                 ▼
           decider_checked × n, decider_fallback × m, gate_evaluated
                 │
           response assembly: attempts, rules (level from attempts.rules visit)
                 │
           print ─► append rule_delivered × k ─► run journal relays
```

### Components

- **`src/template/decider_check.rs`.** `DeciderCheckSpec` gains `units:
  Option<UnitsMode>` (only `Jsonl`), `max_units: Option<u32>`, and
  `unit_document` and `unit_section` (`Option<String>`, name-shaped
  template names the journal exports in place of printed values), all
  skipped when `None` and refused outside unit mode. `CheckCriterion` gains
  `summary` and `rule_text`, skipped when `None`. `validate_spec` checks
  `max_units >= 1`, the label shapes and the text lengths
  (`E-DECIDER-CHECK-UNITS`, `E-DECIDER-CHECK-RULE`). `declaration_hash`
  appends `units=jsonl` only when unit mode is set, so existing hashes
  don't move, and leaves `max_units`, the unit labels, `summary` and
  `rule_text` out. A helper resolves each check's effective `max_units`.
- **`src/template/types.rs` and `compile.rs`.** `SOURCE_GATE_KEYS` and
  `SOURCE_CRITERION_KEYS` gain the new keys, refused on other gate types.
  `validate_state_decider_checks` adds the per-state sum rule.
- **`src/decider/check.rs`.** `UnitRef`; `UnansweredReason::UnitLimit`
  (recorded `unit_limit`); the optional `unit` field on `DeciderCheck`,
  `DeciderFallback` and `DeciderCheckAnswered`; `confidence_bp(&probs)`;
  a `RuleDelivered { state, gate, rule_id, level, unit_count }` record.
- **`src/cli/check_evaluator.rs`.** `UnitBudget`; `parse_units`; the
  unit-mode branch of `evaluate`; `reusable` and `applicable_answer`
  matching on the unit; `result_for` building per-unit findings and
  `output.units`; `override_records` per (criterion, unit).
- **`src/findings.rs`.** `Finding.unit: Option<UnitRef>`, skipped when
  `None`; the 1,000-byte cut applied to decider-built messages.
- **`src/cli/decider_answer.rs`.** `--unit`, the three new refusal codes,
  `unit` in the success JSON.
- **`src/cli/next_types.rs` and `src/cli/mod.rs`.** `attach_rules` beside
  `attach_attempts`; the `UnitBudget` created per `koto next`; the
  `rule_delivered` appends after the print, on every path that prints a
  blocked response.
- **`src/engine/types.rs`.** `EventPayload::RuleDelivered`, type name
  `rule_delivered`.
- **`src/run_journal/`.** `decider_checked`, `decider_fallback` and
  `decider_check_answered` relay the unit's key digest, its declared or
  digested document and section, and the confidence;
  `rule_delivered` relays its record; `relays()` lists the new event.
- **`src/decider/report.rs`.** Counts per unit roll up into the existing
  per-criterion rows; a ledger line with a unit is read like any other.
- **Docs and skills.** The template-format reference and decider
  authoring guide (`units`, `max_units`, `summary`, `rule_text`, `rules`),
  the error-code reference, the run-journal and session-feed references,
  and the koto-user skill with its command and response references, per
  the PRD's table.

### Key Interfaces

Template declaration:

```yaml
gates:
  ac_binary:
    type: decider-check
    command: "scripts/acceptance-criteria-units.sh {{PRD_PATH}}"
    units: jsonl
    max_units: 20
    unit_document: prd
    unit_section: acceptance-criteria
    criteria:
      ac_binary:
        rule_ref: "https://example.org/rules/ac-binary"
        question: Can this acceptance criterion be checked pass or fail with no judgment?
        pass: The criterion names one observable outcome a tester can check.
        fail: The criterion needs judgment, or names no observable outcome.
        escape: The text isn't an acceptance criterion.
        summary: "Acceptance criteria must be binary: one observable pass/fail outcome."
        rule_text: |
          An acceptance criterion states one observable result ...
        mode: veto
```

Extraction output, one line per unit:

```json
{"key":"ac-3","document":"docs/prds/PRD-x.md","section":"acceptance-criteria","text":"- [ ] The export finishes quickly."}
```

A failing blocking condition in `koto next`:

```json
{"name":"ac_binary","type":"decider-check","status":"failed",
 "output":{"failed":["ac_binary"],"unanswered":[],"awaiting":[],"error":"",
           "units":[{"key":"ac-3","failed":["ac_binary"],"awaiting":[],"agent_answered":[]},
                    {"key":"ac-4","failed":[],"awaiting":[],"agent_answered":[]}]},
 "failure":{"findings":[{"rule_id":"ac_binary","level":"error","message_source":"decider",
   "message":"criterion ac_binary failed on unit ac-3: The criterion needs judgment ...",
   "unit":{"key":"ac-3","document":"docs/prds/PRD-x.md","section":"acceptance-criteria"}}]},
 "rules":[{"rule_id":"ac_binary","level":"summary",
           "text":"Acceptance criteria must be binary: one observable pass/fail outcome.",
           "units":["ac-3"]}]}
```

`rule_delivered` event payload: `{state, gate, rule_id, level,
unit_count?}`.

### Data Flow

1. `handle_next` creates `ConsultBudget(4)` and `UnitBudget(32)` and the
   evaluator.
2. A unit-mode gate runs its extraction; a bad line, bad name or duplicate
   key ends here with every criterion `extraction_failed`.
3. Each unit gets its early outcome; the evaluator takes the lock (or,
   when the wait runs out, marks the remaining units `busy`).
4. For each graded (unit, criterion): reuse a verdict for the same unit,
   criterion, declaration and text; else apply the agent's answer for that
   unit; else take a `UnitBudget` slot and consult, or mark `unit_limit`.
5. Records (`decider_checked`, `decider_fallback`) and ledger lines carry
   the unit; `result_for` builds findings, `output.units` and the outcome.
6. The engine appends the records and `gate_evaluated` with its
   `rule_counts`, as today.
7. Response assembly attaches `attempts`, then `rules` per failing decider
   condition from the compiled criteria and the visit counts.
8. The response prints; one `rule_delivered` event per `rules` object is
   appended; the run journal relays every record.

## Implementation Approach

### Phase 1: Declaration and compile rules

The new spec and criterion fields, `E-DECIDER-CHECK-UNITS` and
`E-DECIDER-CHECK-RULE`, the per-state sum and default, the hash rule, and
a decider-check fixture pinned in the compile-compatibility snapshots.

Deliverables:
- `src/template/decider_check.rs`, `types.rs`, `compile.rs` and their tests

### Phase 2: Per-unit grading, budget and unit identity

`UnitRef`, `UnitLimit`, `parse_units`, the unit-mode branch, `UnitBudget`,
reuse and answer matching, per-unit findings, `output.units`, the fixed
combining rule, override records, and `koto decider answer --unit` with its
refusals. Stub-driven integration tests for every PRD criterion on units,
bounds, fallback and answers.

Deliverables:
- `src/decider/check.rs`, `src/cli/check_evaluator.rs`,
  `src/cli/decider_answer.rs`, `src/findings.rs`, `src/cli/mod.rs`

### Phase 3: Rule delivery

`attach_rules`, the `rule_delivered` event with its session-feed
declaration and its run-journal relay, its append after the print on every
path that prints a blocked response (a failed append warns, as
`instructions_delivered`'s does), and tests for summary, full, reset on a
new visit, per-rule and per-check independence, summary-only and full-only
criteria, single-slice checks, shadow, a mixed failed-and-waiting
evaluation, and the sentinel test that the text reaches no record. Rule
delivery for single-slice checks needs only Phase 1, so this phase can run
beside Phase 2.

Deliverables:
- `src/cli/next_types.rs`, `src/cli/mod.rs`, `src/engine/types.rs`,
  `src/run_journal/`, `docs/reference/session-feed.md`

### Phase 4: Telemetry, compatibility and docs

Run-journal relays and names, the confidence exception, collision and doc
tests, the session-feed entries, the v0.14.1 compatibility job's new
cases, the reference docs and the koto-author and koto-user skills.

Deliverables:
- `src/run_journal/`, `tests/run_journal_doc_test.rs`,
  `docs/reference/run-journal.md`, `docs/reference/session-feed.md`,
  `docs/reference/error-codes.md`, `docs/guides/decider-authoring.md`,
  `test/compat/`, `plugins/koto-skills/`

The telemetry definitions, their test cases, the archive's export of the
new attributes and the readout's text are maintained with the telemetry
pipeline, outside this repository, and land beside Phase 4.

## Security Considerations

**Data leaving the machine.** Each request carries one unit's redacted text
and the criterion's question and descriptions, to the endpoint and under
the key rules the decider already uses. Per request that's no more than a
single-slice check sends; per evaluation it can be more, up to 32 requests
of `max_bytes` each, bounded by the unit budget and recorded with their
tokens. Repeated `koto next` calls in a session aren't capped beyond reuse,
which sends nothing for an unchanged unit. A user who hasn't opted in gets
the gates removed before anything runs. `summary` and `rule_text` are
never sent to the decider. The extraction's whole stdout, unit names
included, passes through the tick's redactor at capture, before parsing;
the redactor replaces known secret values only, so an extraction script
should print the narrow text a criterion judges, never a whole file.

**Injected text in a unit.** A unit is agent-authored and can try to steer
the grader. It travels only as a JSON string value in the request's
`state`, never in `instructions` or `criteria`, and each unit is graded
alone, so a steer in one unit can't reach another unit's verdict. A steered
`pass` does defeat a veto for that one unit; that is the same exposure a
single-slice check has, and it's why every criterion ships in shadow and
must show agreement with reviewers before it can block. A steered
`unclear` is an escape: a verdict that never blocks and never waits, counted
per criterion in the escape tally, where a pattern shows.

**Unit keys in a command line.** A key, document and section reach the
waiting finding's `koto decider answer` command and the run journal. The
evaluator refuses any value outside the name shape at parse time, including
one that starts with `-`, and the command line shell-quotes every word, so
a key can't inject shell syntax or read as an option. `koto decider answer`
validates `--unit` against the latest evaluation before it appends
anything, so a refused answer leaves no event.

**What unit names export.** Keys, documents and sections are exported on
the run journal's records as written. They are names the extraction script
chooses, so a script should print slugs and repository paths (`ac-3`,
`acceptance-criteria`), never text taken from the unit's content; the
template-format reference says so. A document value is a label: koto never
opens it, so a `..` in it reaches nothing.

**Nothing to grade.** An extraction that prints no units, or only empty
ones, grades nothing and the check passes, as an empty slice does today.
That is the honest outcome for a section that isn't written yet, and it is
visible: every criterion's `decider_checked` record says `not_graded`. It
is also a way past a veto for an agent that empties the section. While
criteria run in shadow nothing is passed that would otherwise block; a
setting that requires at least one unit is the natural guard once a
criterion is trusted to block, and is left to that decision.

**Evasion by the agent.** The agent controls the document an extraction
reads and, if the script lives in the tree it edits, the script. It can
split a criterion's subject into units past `max_units`, make one unit
huge, or print a malformed line. Each falls back to the agent (`unit_limit`,
`over_budget`, or `extraction_failed` for the whole check) and the agent
then answers for itself. That isn't safe, only visible: every fallback and
every agent answer is recorded per unit, so the readout shows how often a
criterion was settled by the agent rather than the decider, which is one of
the figures a criterion needs before it leaves shadow. Keeping extraction
scripts outside the paths the agent is asked to change remains the
guidance.

**Records.** No event, ledger line or journal record holds unit text, rule
text, a question or a probability beyond the one integer confidence;
units appear by key, document and section, all name-shaped. A sentinel
test asserts that neither rule text nor unit text reaches the session log,
the ledger or the run journal.

**Denial of progress.** A unit-mode evaluation can hold the decider lock
for up to 32 sequential consultations of at most two attempts each at the
provider timeout, with no further deadline. A concurrent `koto next` waits
at most 60 seconds and then falls its units back with `busy`; the agent can
answer them or retry, and nothing passes. An author who wants a shorter
worst case sets a lower `max_units`.

## Consequences

### Positive

- A section of any length is graded unit by unit under the budget the
  decider checks already use, in one state visit and one gate.
- An agent learns which unit failed and gets the rule, summary first, at
  the moment it acts, with no fetch and no record holding the text.
- Per-unit telemetry lets a later effort compare each criterion with
  reviewers unit by unit, which is what moving a criterion out of shadow
  needs.
- Nothing changes for templates, logs, ledgers or the routing decider that
  don't use unit mode.

### Negative

- A unit-mode evaluation can take up to 32 sequential decider round trips,
  seconds to tens of seconds, and holds the decider lock that long.
- One `koto next` can append dozens of `decider_checked` events, and a
  failed append fails the tick as it does today.
- Many failing units can push findings past the 50 logged and 100 returned,
  truncating the list.
- The journal's no-probability rule gains an exception.
- An agent answer doesn't survive an edit to its unit.

### Mitigations

- Latency is recorded per consultation, so the cost of a unit-mode check
  is readable; `max_units` lets an author bound it below 32.
- Errors are kept first when findings are truncated, and `output.units`
  lists every unit's state in full regardless of the finding caps.
- The confidence exception is one named integer, documented and tested,
  not a general relaxation.
- Every criterion still ships in shadow, where a unit fail blocks nothing.
