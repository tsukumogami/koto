# Run Journal Reference

The run journal is a local record of session identity and progress for
tooling outside koto. koto appends one JSON object per line as sessions
start, enter states, reach a terminal, are cancelled, or are driven by a new
Claude Code session, each time koto evaluates one of their checks or
consults the decider or the agent answers what the decider was asked, each
time a response hands back a failed criterion's rule, each time an override
or a dispute is recorded, once per state
visit for the context keys the visit read, and each time `koto next` or
`koto status` prints a state's directive. The journal is
append-only: koto never rewrites or reads back its records, so a
session's lines outlive the session itself, including sessions koto removes
at their terminal tick or on `koto session cleanup`.

It sits beside the decider ledger and the terminal index and, like them,
stays on the host that wrote it. It's derived, written from what sessions
commit, and it isn't session state: cloud sync and `koto session import`
don't carry it, and nothing koto does resumes from it. Deleting it changes
nothing koto does, but its lines can't be rebuilt once the sessions they
describe are gone.

## Location

The journal lives with the session store it describes:

| Store | Journal |
|-------|---------|
| The default store, `~/.koto/sessions` | `~/.koto/_run_journal.jsonl` |
| The cloud store, which keeps its local copies in `~/.koto/sessions` | `~/.koto/_run_journal.jsonl` |
| A store redirected with `KOTO_SESSIONS_BASE=<base>` | `<base>/_run_journal.jsonl` |

`~/.koto` is the koto home, `$HOME/.koto`. A store redirected with
`KOTO_SESSIONS_BASE` journals inside its own base, never into the real
home's journal, and journaling is always on for every store. The file sits
next to the session directories in that case; `koto workflows` and the
other session listings skip it.

koto creates the file with mode 0600 and refuses to write through a symlink
at the journal's path.

Each session directory also holds a sidecar, `run-journal.json`, that
caches the session's run id and the driver last recorded for it. It's for
koto's own use, not for readers: it goes when the session directory goes,
and koto rebuilds it from the session header when it's missing.

## Line format

Each line is one JSON object of at most 4096 bytes, newline included.
Records start with these fields:

| Field | Type | Description |
|-------|------|-------------|
| `kind` | string | The record kind (see below). |
| `v` | integer | The line format's version, currently `1`. |
| `at` | string | When the record was written: UTC, RFC 3339, millisecond precision, `Z` suffix, such as `2026-10-10T03:09:22.599Z`. |
| `session` | string | The session's name, such as `issue_42` or `parent.task-a`. |
| `koto.session.id` | string | The session's `session_id` from its header. Left out for a session whose header has no `session_id` (one created by a koto that predates the field). Such a session's records carry a `koto.run.id` only when it is a child whose root has a `session_id`. |
| `koto.run.id` | string | The run the session belongs to (see [Run ids](#run-ids)). Left out when it can't be resolved. |

A field with no value is left out, never written as `null` or an empty
string. Ids (every `*.id` field except `koto.rule.id`, and
`koto.template.hash`, `koto.context.first_hash` and `koto.context.last_hash`)
must match `^[A-Za-z0-9._:-]{1,128}$`. Names (`koto.template.name`,
`koto.state`, `koto.terminal`, `koto.check`, `koto.check.name`, `koto.context.key`, and the
string values of the check, decider, override, response and context kinds)
must match `^[A-Za-z0-9._:/@-]{1,128}$` and not start with `/` or `~`. A
value outside its shape is left out and the record is still written.
`koto.rule.id`, `koto.context.key`, `koto.context.reader` and
`koto.context.access` are never left out: a rule id outside the name shape,
a decider criterion's id included, and such a context value are written as
their digest (see
[Rule ids](#rule-ids)).

### Record kinds

| Kind | Its own fields | Written when |
|------|----------------|--------------|
| `session_started` | `koto.parent.session.id`, `koto.driver.session.id`, `koto.template.name`, `koto.template.hash`, `koto.fixture`, `koto.imported_from.session.id` | A session is created. |
| `state_entered` | `koto.state` | The session enters a state. |
| `terminal` | `koto.terminal` | The session arrives at a terminal state. |
| `cancelled` | none | The session is cancelled. |
| `driver_seen` | `koto.driver.session.id` | A command runs under a driving session other than the last one recorded for it. |
| `check_evaluated` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.check.outcome`, `koto.attempt`, `koto.visit_attempt`, `koto.check.rule_count`, `koto.check.rule_counts_truncated`, `koto.check.finding_count`, `koto.check.findings_truncated`, `koto.check.duration_ms`, `koto.poll.status`, `koto.check.unanswered_count`, `koto.check.error`, `koto.decider.awaiting_count`, `koto.decider.agent_answered_count`, `koto.check.reason`, `koto.check.not_ready`, `koto.check.pre_directive` | koto evaluates a gate and the result isn't pending, or runs a state's default action. |
| `check_rule` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.attempt`, `koto.visit_attempt`, `koto.rule.id`, `koto.rule.visit_count`, `koto.rule.session_count` | Once for each error rule of a check evaluation, after its `check_evaluated`. |
| `check_finding` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check.name`, `koto.attempt`, `koto.visit_attempt`, `koto.rule.id`, `koto.finding.level`, `koto.finding.count`, `koto.finding.source` | Once for each distinct rule id and level among a check evaluation's logged findings, passing or failing, after its `check_rule` records. |
| `check_pending` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.poll.status`, `koto.poll.evaluations`, `koto.poll.elapsed_secs`, `koto.decider.awaiting_count`, `koto.decider.agent_answered_count` | A polling gate's evaluation is still pending, or a decider check is waiting on the agent. |
| `check_disputed` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check.name`, `koto.attempt`, `koto.visit_attempt`, `koto.rule.id`, `koto.dispute.source` | `koto disputes record` records a dispute of a rule's finding at a gate. |
| `decider_consulted` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.decider.provider`, `koto.decider.model`, `koto.decider.outcome`, `koto.decider.error_class`, `koto.decider.input_tokens`, `koto.decider.output_tokens`, `koto.decider.unread_usage_attempts`, `koto.decider.latency_ms`, `koto.decider.directive_bytes`, `koto.decider.reason`, `koto.decider.input_label`, `koto.decider.input_bytes`, `koto.decider.field_count`, `koto.decider.field`, `koto.decider.answer`, `koto.decider.mode`, `koto.decider.threshold_met` | koto consults the decider about a state's evidence. |
| `decider_field` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.decider.consulted_seq`, `koto.decider.field`, `koto.decider.answer`, `koto.decider.mode`, `koto.decider.threshold_met` | Once for each field of a consultation that asked about more than one, after its `decider_consulted`. |
| `evidence_field` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.evidence.field`, `koto.evidence.value` | Once for each enum or boolean field of the agent's evidence submission (see [Evidence fields](#evidence-fields)). |
| `decider_answered` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.decider.field`, `koto.decider.agent_answer`, `koto.decider.consulted_seq` | The agent submits evidence for a field the decider was consulted about and didn't decide. |
| `decider_checked` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.rule.id`, `koto.unit.key`, `koto.unit.document`, `koto.unit.section`, `koto.decider.provider`, `koto.decider.model`, `koto.decider.mode`, `koto.decider.outcome`, `koto.decider.confidence_bp`, `koto.decider.reason`, `koto.decider.blocked`, `koto.decider.error_class`, `koto.decider.input_tokens`, `koto.decider.output_tokens`, `koto.decider.unread_usage_attempts`, `koto.decider.attempts`, `koto.decider.latency_ms`, `koto.decider.input_bytes` | koto grades one criterion of a decider check, on one unit for a unit-mode check. |
| `decider_fallback` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.rule.id`, `koto.unit.key`, `koto.unit.document`, `koto.unit.section`, `koto.decider.mode`, `koto.decider.reason`, `koto.decider.input_bytes` | A decider-check criterion that got no verdict is handed to the agent. |
| `decider_check_answered` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.rule.id`, `koto.unit.key`, `koto.unit.document`, `koto.unit.section`, `koto.decider.answer`, `koto.decider.reason`, `koto.decider.fallback_seq` | `koto decider answer` records the agent's answer to a criterion handed to it. |
| `rule_delivered` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check.name`, `koto.rule.id`, `koto.rule.extent`, `koto.rule.unit_count`, `koto.rule.via`, `koto.rule.delivery` | A `koto next` response hands back a failed criterion's rule, a command check reports a rule it printed or refused, or `koto next` prints a directive that carries a rule its state declares in `directive_rules`. |
| `check_overridden` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check` | An override is recorded for a gate. |
| `review_round` | `koto.seq`, `koto.visit.seq`, `koto.doc.type`, `koto.doc.format`, `koto.doc.schema`, `koto.review.round`, `koto.review.seat_count`, `koto.review.fallback` | `koto review record ... round-started` records that a document review round's seats were spawned. |
| `review_verdict` | `koto.seq`, `koto.visit.seq`, `koto.doc.type`, `koto.doc.format`, `koto.doc.schema`, `koto.review.round`, `koto.review.seat`, `koto.rule.id`, `koto.review.section`, `koto.review.verdict` | Once for each entry `koto review record ... verdicts` records. |
| `review_decision` | `koto.seq`, `koto.visit.seq`, `koto.doc.type`, `koto.doc.format`, `koto.doc.schema`, `koto.review.round`, `koto.review.decision` | `koto review record ... decision` records what follows a review round. |
| `doc_checked` | `koto.seq`, `koto.visit.seq`, `koto.doc.type`, `koto.doc.schema`, `koto.rule.id`, `koto.doc.check_outcome` | Once for each entry `koto review record ... doc-checks` records. |
| `doc_rule_checked` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.doc.format`, `koto.rule.id`, `koto.doc.rule_outcome`, `koto.doc.check_moment` | Once for each entry `koto review record ... rule-checks` records; one call's records share its `koto.seq`. |
| `doc_section_written` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.doc.format`, `koto.doc.section`, `koto.doc.section_write` | `koto review record ... section-written` records a document section written for the first time or rewritten. |
| `doc_fix_applied` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.doc.format`, `koto.rule.id`, `koto.doc.section`, `koto.doc.fix_answers`, `koto.doc.fix_round`, `koto.doc.fix_check_rule` | Once for each entry `koto review record ... fix` records; one call's records share its `koto.seq`. |
| `response` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.response.source`, `koto.response.directive_bytes`, `koto.response.details_bytes` | `koto next` or `koto status` has printed a non-terminal state's directive. |
| `context_reads` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.context.key`, `koto.context.reader`, `koto.context.access`, `koto.context.overflow`, `koto.context.read_count`, `koto.context.present_count`, `koto.context.first_hash`, `koto.context.last_hash`, `koto.context.read_by` | A state visit that read context keys ends, or a reader's anchor records a read of another session. |

The first five are the identity kinds. The next five are the check kinds,
described in [Check records](#check-records), the nine after them are the
decider, evidence, rule-delivery and override kinds, described in
[Decider and override records](#decider-and-override-records) and
[Evidence fields](#evidence-fields), `response`
is described in [Response records](#response-records), and `context_reads`
in [Context reads](#context-reads).

`session_started`'s fields:

| Field | Type | Description |
|-------|------|-------------|
| `koto.parent.session.id` | string | The parent session's id. Children only, and left out for a child created by a koto that didn't record it. |
| `koto.driver.session.id` | string | The Claude Code session that created this one (see [Drivers](#drivers)). |
| `koto.template.name` | string | The template's `name`. |
| `koto.template.hash` | string | The compiled template's hash, the header's `template_hash`. Two sessions from one template name compiled before and after an edit share the name and differ in hash. |
| `koto.fixture` | boolean | Whether the session looks like a test fixture (see [Fixture rules](#fixture-rules)). Always present. |
| `koto.imported_from.session.id` | string | For `koto session import`: the source session's id, the same value as the `from_session_id` of the `session_imported` event the import adds. |

A session is created by `koto init` (with `--template` or `--from-stdin`,
and with `--replace-terminal`), by every child spawn (batch tasks, skip
markers and retry respawns included), by `koto session start` (with or
without `--parent`), and by `koto session import`. `koto session rebind`
writes no `session_started`.

`state_entered` is written for the initial state, for each state a chained
tick passes through (in order), and for every transition, directed
transition and rewind, including a transition back into the current state.

`terminal` is written once per arrival, after that state's
`state_entered`, whether or not the terminal is a failure terminal and
whether the session is then kept or removed. A rewind out of a terminal and
a second arrival write a second `terminal`.

`cancelled` is written for `koto cancel`, with or without `--cleanup`, and
for a respawn fallback's cancellation. A cancelled session gets no
`terminal`.

### Example

```json
{"kind":"session_started","v":1,"at":"2026-10-10T03:09:22.599Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.driver.session.id":"3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60718","koto.template.name":"work-on","koto.template.hash":"a0c6f50ee8d1f172f6fed748e19214eb6907428f38b1b101918b6486a1848623","koto.fixture":false}
{"kind":"state_entered","v":1,"at":"2026-10-10T03:09:22.600Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.state":"analysis"}
{"kind":"terminal","v":1,"at":"2026-10-10T04:12:05.031Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.terminal":"done"}
```

## Check records

koto relays each check it evaluates from the event the session's log
commits for it: a gate's `gate_evaluated`, or a state's default action's
`default_action_executed`, whose check name is `__action__`. A record is
written after its event commits, and a failed commit writes none. Check
records copy names, counts and short vocabulary values out of the event
field by field. They never hold check output, a finding's message, path or
location, a rule reference or a command, and koto adds no filesystem path
of its own. A rule id is passed through as the check reported it, or as its
digest (see [Rule ids](#rule-ids)): the name shape admits `/` and `@`, so
an id such as `@typescript-eslint/no-explicit-any` is written unchanged.

Every check record carries two sequence numbers after `koto.run.id`:

| Field | Type | Description |
|-------|------|-------------|
| `koto.seq` | integer | The sequence number (`seq`) of the session-log event the record relays. The `check_rule` records of one evaluation share their `check_evaluated`'s. |
| `koto.visit.seq` | integer | The sequence number of the session-log event that began the visit the record belongs to. |

A visit runs from an arrival in a state until the session next arrives
somewhere else, is rewound, reaches a terminal or is cancelled. An arrival
is the state's first entry, a transition or directed transition from a
different state, or any rewind. A transition from a state to itself stays
in the visit. This is the boundary koto's own `visit_attempt` counts
within. `koto.visit.seq` is left out only for a session whose log holds no
state entry.

### `check_evaluated`

One record per check evaluation: a `gate_evaluated` whose outcome isn't
`pending`, or a `default_action_executed`.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the check belongs to. |
| `koto.check` | string | always | The gate's name, or `__action__` for a default action. |
| `koto.check.outcome` | string | always | For a gate, its logged outcome: `passed`, `failed`, `timed_out` or `error`. For a default action, `failed` when its exit code isn't 0 or it logged rule counts (a capture failure after exit 0 always does), otherwise `passed`. A default action that timed out or couldn't be spawned is `failed` too: its event carries no outcome of its own, and its exit code of -1 stands for a timeout, a spawn failure and a failed wait alike. |
| `koto.attempt` | integer | when logged | The state's attempt number in the session. Every check evaluation from koto v0.15.0 on carries it. |
| `koto.visit_attempt` | integer | when logged | The state's attempt number in the current visit. |
| `koto.check.rule_count` | integer | always | How many error rules the evaluation counted, the keys of its `rule_counts`; 0 when it has none. |
| `koto.check.rule_counts_truncated` | boolean | when `true` | The check reported more error rules than koto counts (50), so some have no `check_rule`. |
| `koto.check.finding_count` | integer | always, from this koto on | How many findings the evaluation's event logged, of every level and whatever the outcome, koto's own fallback finding included; 0 when it logged none. An event logs at most 50, so this is at most 50. Left out only by a koto that predates the field. |
| `koto.check.findings_truncated` | boolean | when `true` | The check reported more findings than its event logged (koto logs at most 50, keeping `error` findings first, then `warning`, then `info`), so `koto.check.finding_count` and the `check_finding` records undercount what it printed. |
| `koto.check.duration_ms` | integer | when logged | The check command's run time in milliseconds. Command gates, decider checks and default actions log it; context gates don't. |
| `koto.poll.status` | string | polling gates | Where the poll settled: `done`, `failed` or `timed_out`. |
| `koto.check.unanswered_count` | integer | decider checks | How many veto criteria got no verdict. |
| `koto.check.error` | string | decider checks, when set | Why no criterion could be graded, such as `log_unreadable` or `missing_spec`. |
| `koto.decider.awaiting_count` | integer | decider checks | How many criteria are waiting on the agent's answer (the length of the check's `awaiting` output list), 0 included. Non-zero here only beside a failed criterion; a check that only waits writes `check_pending`. |
| `koto.decider.agent_answered_count` | integer | decider checks | How many criteria this evaluation took from an answer the agent recorded (its `agent_answered` output list), 0 included. |
| `koto.check.reason` | string | when the outcome isn't `passed` | Why the check didn't pass (see the list below). Left out by a koto that predates the field. |
| `koto.check.not_ready` | boolean | always, from this koto on | The check said its input wasn't ready to judge: its command printed a line starting with `::koto-not-ready::`, or exited with a code its gate lists in `not_ready_exit_codes`. koto never sets it on its own. It is telemetry only: the outcome, routing and fallback finding are the same either way. |
| `koto.check.pre_directive` | boolean | always, from this koto on | No `koto next` had printed the state's directive in the current visit when the check ran: the check ran on arrival in the same invocation, on the first tick after `koto init` (which prints no directive), or after a rewind or a loop back. `false` once a `koto next` has printed the directive in the visit, with or without its details. |

`koto.check.reason` takes one of these values:

| Value | Meaning |
|-------|---------|
| `rule_failed` | The check printed at least one `error` finding of its own. A decider check's failed criterion counts. |
| `undecided` | The command exited 2 and printed no `error` finding of its own. |
| `no_finding` | Any other failure with no `error` finding of its own: another non-zero exit, a context gate whose key is missing or doesn't match, a default action whose capture couldn't be delivered. |
| `timed_out` | The command, or a polling gate's window, ran out of time. |
| `spawn_failed` | The command couldn't be started. |
| `error` | Anything else that kept the check from answering, such as a failed wait or a decider check's error. |

Unlike `koto.check.outcome`, the reason tells a default action's timeout or
spawn failure apart from a non-zero exit.

### `check_rule`

One record per error rule of a check evaluation, in the order koto first
saw each rule reported, right after the evaluation's `check_evaluated`.
koto counts error rules only on a check that didn't pass: the rule ids it
reported at level `error`, or koto's own fallback finding's id when none of
its findings was an error. A passing check writes no `check_rule`, whatever
it printed, and an evaluation writes at most 50. Every level of finding, a
passing check's included, is in its [`check_finding`](#check_finding)
records instead.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | As its evaluation's. |
| `koto.check` | string | always | As its evaluation's. |
| `koto.attempt` | integer | when its evaluation has it | As its evaluation's. |
| `koto.visit_attempt` | integer | when its evaluation has it | As its evaluation's. |
| `koto.rule.id` | string | always | The rule id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.rule.visit_count` | integer | always | The attempts in this visit, this one included, on which this check failed reporting this rule. |
| `koto.rule.session_count` | integer | always | The same count across the whole session. |

A rule whose `koto.rule.visit_count` keeps rising within one visit is a
check the agent isn't converging on; one that stops before a passing
`check_evaluated` in the same visit converged.

### `check_finding`

One record per distinct pair of rule id and level among the findings a
check evaluation's event logged, whether the check passed or failed, after
the evaluation's `check_evaluated` and `check_rule` records and in the same
write, so a reader never sees one without the other. koto's own fallback
finding, whose rule id is the check's name, is a finding like any other. The
order is `error` pairs first, then `warning`, then `info`, then any other
level, each level in the order its pairs were first logged. An evaluation
writes at most 50; past that, the pairs of the later levels are the ones
left out. A pending evaluation writes none.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | As its evaluation's. |
| `koto.check.name` | string | always | The gate's name, or `__action__`: its evaluation's `koto.check`, under the path a tool that nests dotted names gives that field, so a join reads one path. The record has no `koto.check` of its own. |
| `koto.attempt` | integer | when its evaluation has it | As its evaluation's. |
| `koto.visit_attempt` | integer | when its evaluation has it | As its evaluation's. |
| `koto.rule.id` | string | always | The rule id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.finding.level` | string | always | The findings' level: `error`, `warning` or `info` from this koto. The vocabulary is open, so a reader keeps a level it doesn't know rather than rejecting the record. |
| `koto.finding.count` | integer | always | How many of the logged findings had this rule id and level, at least 1. |
| `koto.finding.source` | string | always, from this koto on | `check` when the check printed the findings, `fallback` for the finding koto wrote because the check printed no `error` finding of its own. |

The counts of one evaluation's records sum to its `koto.check.finding_count`
unless more than 50 pairs were logged. A finding's message, path, location
and rule reference stay in the session's log.

### `check_pending`

One record per evaluation of a polling gate that is still pending, or of
a decider check waiting on the agent's answer (see
[`decider_fallback`](#decider_fallback)). A
pending evaluation judged nothing, so it carries no outcome, attempt or rule
field and never a `check_evaluated`. The evaluation where the poll settles,
or where the decider check stops waiting, is a check evaluation.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the gate belongs to. |
| `koto.check` | string | always | The gate's name. |
| `koto.poll.status` | string | polling gates | `pending`. |
| `koto.poll.evaluations` | integer | polling gates | The gate's runs since its polling window opened, at least 1. |
| `koto.poll.elapsed_secs` | integer | polling gates | Seconds since the polling window opened. |
| `koto.decider.awaiting_count` | integer | decider checks | How many criteria are waiting on the agent's answer, at least 1. |
| `koto.decider.agent_answered_count` | integer | decider checks | How many criteria this evaluation took from an answer the agent recorded, 0 included. |

A polling gate re-run several times inside one evaluation (a `hold_secs`
wait) writes one record for that evaluation, and `koto.poll.evaluations`
counts the runs.

### `check_disputed`

One record per committed `finding_disputed` event, which
`koto disputes record` appends when an agent or a person says a rule's
finding at a gate is wrong. A dispute changes no outcome, route or state;
this record says only that it was made. The reason given stays in the
session's log and is never copied.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state whose evaluation of the gate is disputed. It can be a state the session has left (`--state`), so it can differ from the state of the visit `koto.visit.seq` names, which is the one the session was in when the dispute was recorded. |
| `koto.check.name` | string | always | The gate's name, or `__action__` for a default action, under the same path as on `check_finding`. The record has no `koto.check` of its own. |
| `koto.attempt` | integer | when the disputed evaluation has it | The disputed evaluation's `koto.attempt`. |
| `koto.visit_attempt` | integer | when the disputed evaluation has it | The disputed evaluation's `koto.visit_attempt`. |
| `koto.rule.id` | string | always | The disputed rule's id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.dispute.source` | string | always | Who disputed it: `agent` or `person`. |

A dispute names the most recent settled evaluation of the gate in that
state, one with an outcome rather than pending. Its `check_evaluated` is the
one with the same `koto.session.id` and `koto.state`, a `koto.check` equal to
the dispute's `koto.check.name`, and the same `koto.attempt` and
`koto.visit_attempt`. A dispute of an evaluation from before attempt
numbers carries neither; its evaluation is then the latest such
`check_evaluated` with a lower `koto.seq`. koto doesn't require that
evaluation to have logged a finding for the rule, since its logged findings
may have been cut.

### Rule ids

koto doesn't restrict rule ids: a check can report `E501`,
`@typescript-eslint/no-explicit-any` or `no explicit any [x]`. A rule id in
the name shape is written unchanged. Any other, such as one with a space or
a bracket, one longer than 128 bytes, or one with a non-ASCII character,
is written as `sha256:` followed by the first 32 lowercase hex characters
of the SHA-256 of the id's UTF-8 bytes. The same id gives the same value in
every session and on every host, so a reader holding the original id can
compute it. A digest is for shape, not secrecy. A check that reports a
literal id of that form can't be told apart from a digest.

### What writes no check record

- A gate with a recorded override, which koto doesn't evaluate.
- The re-runs of a polling gate inside one evaluation, which
  `koto.poll.evaluations` counts.
- A gate or default action koto refused before running it, such as one
  reading a capture that hasn't been delivered yet.
- Events committed by a koto that predates check records. A session it
  started and a later koto continues gets check records only from then on,
  and an evaluation logged without attempt numbers gets a record without
  them.

### Example

```json
{"kind":"check_evaluated","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.check.outcome":"failed","koto.attempt":2,"koto.visit_attempt":2,"koto.check.rule_count":2,"koto.check.finding_count":4,"koto.check.duration_ms":840}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"E501","koto.rule.visit_count":2,"koto.rule.session_count":2}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"no-unused-vars","koto.rule.visit_count":1,"koto.rule.session_count":1}
{"kind":"check_finding","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check.name":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"E501","koto.finding.level":"error","koto.finding.count":2}
{"kind":"check_finding","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check.name":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"no-unused-vars","koto.finding.level":"error","koto.finding.count":1}
{"kind":"check_finding","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check.name":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"W291","koto.finding.level":"warning","koto.finding.count":1}
{"kind":"check_disputed","v":1,"at":"2026-10-10T03:15:40.020Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":16,"koto.visit.seq":9,"koto.state":"implement","koto.check.name":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"W291","koto.dispute.source":"agent"}
{"kind":"check_pending","v":1,"at":"2026-10-10T03:21:40.502Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":19,"koto.visit.seq":17,"koto.state":"ci","koto.check":"ci","koto.poll.status":"pending","koto.poll.evaluations":2,"koto.poll.elapsed_secs":31}
```

## Decider and override records

koto relays each decider consultation, each rule a response hands back and
each recorded override from the event the session's log commits for it,
after the commit, as it relays checks. The agent's answers to what the
decider was asked are written the same way, from the event that records
them. Like check records, they carry `koto.seq` and `koto.visit.seq` after
`koto.run.id`. `koto.visit.seq` is koto's visit, by the rule in
[Check records](#check-records). A decider event carries a `visit_seq` of
its own, which isn't copied.

These records copy names, counts and short vocabulary values field by
field. They never hold a probability, an input or declaration hash, a
threshold, the endpoint or where it came from, a criterion's rule
reference, question, descriptions, summary or rule text, an input's or a
slice's content, a unit's key, document or section as the extraction printed
it, a context key, an override's rationale, its values or the output it
moved past, or anything else the session wrote. There is one exception to
the first of those: `decider_checked`'s `koto.decider.confidence_bp`, the
highest of the criterion's answer probabilities as an integer number of
basis points. No other probability, and no other form of this one, is
written.

### Units

A decider check in unit mode grades each unit its extraction prints on its
own, so its `decider_checked`, `decider_fallback` and
`decider_check_answered` records name the unit. They carry it as three
fields, which no record of a check without units carries:

- `koto.unit.key` is always the digest of the unit's key, computed as for
  [Rule ids](#rule-ids): `sha256:` and 32 lowercase hex characters. The same
  key gives the same digest in every session.
- `koto.unit.document` and `koto.unit.section` are the check's declared
  `unit_document` and `unit_section`, names the template chose for the
  journal. When the check declares none, they are the digest of the document
  or section the extraction printed for the unit, and they are left out when
  it printed none.

A key, document or section as printed can be a path or a heading, so it
never reaches the journal. The session's log keeps the printed unit, so a
reader on the same host can map a digest back to its unit.

Every answer a record carries (`koto.decider.answer`,
`koto.decider.agent_answer`) is a value the template declares for the field
or criterion, or one of koto's own words for what happened; a field name and
an input label are the template's too. A declared name outside the name
shape is written as its digest, as a rule id is (see [Rule ids](#rule-ids)).
An agent's value that isn't one of the field's declared values is written
`other`, never as submitted.

### `decider_consulted`

One record per consultation of the decider about a state's evidence: a
committed `decider_consulted` event.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state consulted. |
| `koto.decider.provider` | string | always | The provider's name, such as `jev`, koto's built-in decider provider. |
| `koto.decider.model` | string | always | The model the provider reported, or `unknown`. |
| `koto.decider.outcome` | string | always | How the consultation ended: `applied`, `not_applied`, `input_unavailable` or `error`. |
| `koto.decider.error_class` | string | when logged | Why the provider call failed, such as `timeout`, `connect`, `http_status`, `malformed` or `mismatched`. |
| `koto.decider.input_tokens` | integer | when logged | Input tokens of the billed answer. |
| `koto.decider.output_tokens` | integer | when logged | Output tokens of the billed answer. |
| `koto.decider.unread_usage_attempts` | integer | always | Billed answers whose token usage couldn't be read, so an undercount of the two counts above is visible. 0 when the event doesn't log it. |
| `koto.decider.latency_ms` | integer | always | Wall time of the provider call, in milliseconds. |
| `koto.decider.directive_bytes` | integer | always | UTF-8 bytes of the directive plus its details (and the recovery pointer a state with details gets) that `koto next` would have returned to the agent for the consulted state, had the routing decider not been consulted; details already delivered in that visit don't count, and an applied consultation spares the agent these bytes. This differs from a `response` record's `koto.response.directive_bytes`, which counts only the printed directive. |
| `koto.decider.reason` | string | `input_unavailable` | Why a declared input stopped the consultation before anything was sent (on the check kinds the same key says why a criterion got no verdict, from another set of values): `unset` (the context key or variable it reads isn't set), `over_budget` (it is longer than its `max_bytes`) or `not_text` (the context key holds bytes that aren't UTF-8 text). Left out in the one case the request can't be built from the declaration, which a compiled template rules out. |
| `koto.decider.input_label` | string | with `koto.decider.reason` | The declared `label` of the input that stopped it, the first one in declaration order. Never its context key, a path or its content. |
| `koto.decider.input_bytes` | integer | `over_budget`, `not_text` | That input's size in bytes, before any budget. An unset input has none. The same key on the check kinds is a slice's size, so read it by kind. |
| `koto.decider.field_count` | integer | always | How many fields the consultation asked about. |
| `koto.decider.field` | string | one field | The field's name. |
| `koto.decider.answer` | string | one field | What the decider answered for it, the column `koto decider report` puts it in: the winning declared value (`"true"` or `"false"` for a boolean) when it met its threshold, whether or not it was applied; `below_threshold` when the winner fell short of its threshold; `escape` when no declared value won (the escape, a tie, or a boolean with neither or both sides at threshold); or `no_answer` when the consultation got no usable answer (`input_unavailable` or `error`). |
| `koto.decider.mode` | string | one field, when known | The declared mode for that answer, after the user's and project's modes: the winner's (`shadow`, `auto` or `never`) when it is a declared value, below threshold included; otherwise the mode every declared value of the field shares, and left out when they differ. |
| `koto.decider.threshold_met` | boolean | one field, when a declared value won | Whether the winning declared value met its threshold: `true` with that value as the answer, `false` with `below_threshold`. Left out with `escape` and `no_answer`, where no declared value won. |

The answer keys describe one field. A consultation of one field, which is
every shipped template's case, carries them itself. A consultation of
several carries none of them: one [`decider_field`](#decider_field) record
per field follows it instead, so the consultation's tokens and timings are
still written once.

### `decider_field`

One record per field of a consultation that asked about more than one, in
field-name order, right after its `decider_consulted`.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The consulted state. |
| `koto.decider.consulted_seq` | integer | always | The `koto.seq` of the `decider_consulted` record it belongs to, which is also its own `koto.seq`. |
| `koto.decider.field` | string | always | The field's name. |
| `koto.decider.answer` | string | always | As on `decider_consulted`. |
| `koto.decider.mode` | string | when known | As on `decider_consulted`. |
| `koto.decider.threshold_met` | boolean | when answered | As on `decider_consulted`. |

### `decider_answered`

One record per consulted field the agent answers: a committed
`evidence_submitted` from the agent that holds a field the visit's
consultation asked about, when that consultation wasn't `applied`. These
are the visits the decider ledger pairs (see
[Joining a consultation's answers](#joining-a-consultations-answers)). The
agent's answer is never known when the decider is consulted, since koto
doesn't consult once the agent's evidence holds a declared field, so it
comes in this later record. One case writes none: when the agent's
evidence lands while the provider call is in flight, koto records the
consultation after the evidence (as `not_applied`, never applied), so
nothing came before the answer to pair it with. The ledger writes no
`answered` record for that visit either, so the two agree.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the evidence was submitted on. |
| `koto.decider.field` | string | always | The field's name. |
| `koto.decider.agent_answer` | string | always | The agent's value when it is one of the field's declared values (`"true"` or `"false"` for a boolean), otherwise `other`. |
| `koto.decider.consulted_seq` | integer | always | The `koto.seq` of the `decider_consulted` record this answer pairs with. |

### Joining a consultation's answers

A `decider_answered` pairs with the `decider_consulted` that has the same
`koto.session.id` and a `koto.seq` equal to its
`koto.decider.consulted_seq`. The pair is the consultation of the
decider's own visit: a transition from the state to itself begins a new
one, so an answer after a self-loop pairs only with a consultation made
after it, even though both share one `koto.visit.seq`. With the
consultation's `koto.decider.answer` (or its `decider_field` records'), the
pairs give agreement, coverage and the below-threshold rate per field, as
`koto decider report` computes them from the ledger.

An agent that submits the same field more than once in the visit writes a
`decider_answered` for each submission. Its answer is the one with the
highest `koto.seq`, as the ledger keeps the latest.

A consultation the decider applied has no `decider_answered`: its answer
was submitted as the evidence. A visit the agent leaves with `koto next
--to` before answering has none either, and nothing in the journal records
that exit; the ledger's `directed_exit` records it.

### `decider_checked`

One record per criterion koto grades on a decider check, and for a
unit-mode check one per unit and criterion: a committed `decider_checked`
event. A decider check's records come before its gate's
`check_evaluated`, each with a lower `koto.seq`. A verdict koto reuses
within a visit appends no event, so it writes no record.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the check belongs to. |
| `koto.check` | string | always | The decider check's gate name. |
| `koto.rule.id` | string | always | The criterion's id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.unit.key` | string | unit mode, when a unit was graded | The digest of the unit's key (see [Units](#units)). Left out for a check without units, and for a unit-mode check whose extraction failed or printed no unit. |
| `koto.unit.document` | string | with `koto.unit.key`, when known | The check's declared `unit_document`, else the digest of the document printed for the unit; left out when neither exists. |
| `koto.unit.section` | string | with `koto.unit.key`, when known | The check's declared `unit_section`, else the digest of the section printed for the unit; left out when neither exists. |
| `koto.decider.provider` | string | always | The provider's name. |
| `koto.decider.model` | string | always | The model the provider reported, or `unknown`. |
| `koto.decider.mode` | string | always | The criterion's effective mode: `veto` or `shadow`. |
| `koto.decider.outcome` | string | always | `pass`, `fail`, `escape`, `unanswered` or `not_graded`. |
| `koto.decider.confidence_bp` | integer | when an answer was read | The decider's confidence: the highest of the probabilities it gave `pass`, `fail` and `unclear`, times 10,000, rounded half up, so 0 to 10,000. Written in unit mode or not, whatever the outcome the answer led to. Left out when no answer was read: nothing was sent (`not_graded`, `over_budget`, `extraction_failed`, `cap_spent`, `busy`, `unit_limit`) or the answer couldn't be read (`provider_error`, `unreadable_response`). The journal's only probability. |
| `koto.decider.reason` | string | when logged | Why a criterion got no verdict: `provider_error`, `unreadable_response`, `over_budget`, `extraction_failed`, `cap_spent`, `busy` or `unit_limit`. Later koto versions may add values; every value is a fixed identifier koto defines, never a message, a path or instruction text. |
| `koto.decider.blocked` | boolean | always | Whether this verdict blocked the state. |
| `koto.decider.error_class` | string | when logged | Why the last provider call failed. |
| `koto.decider.input_tokens` | integer | when logged | Input tokens, summed over the attempts that reported usage. |
| `koto.decider.output_tokens` | integer | when logged | Output tokens, likewise. |
| `koto.decider.unread_usage_attempts` | integer | always | Billed attempts whose token usage couldn't be read. |
| `koto.decider.attempts` | integer | always | Provider calls made for this criterion, 0 when none was sent. |
| `koto.decider.latency_ms` | integer | always | Wall time of those calls, in milliseconds. |
| `koto.decider.input_bytes` | integer | when the extraction produced output | The slice's size in bytes, after redaction, as captured. Capture stops at 64 KiB, so 65,536 means at least that many. |

A failing veto criterion is also one of its gate's error rules, so the
gate's `check_evaluated` that follows has a `check_rule` for it under the
same `koto.rule.id`.

To total decider tokens, sum `koto.decider.input_tokens` and
`koto.decider.output_tokens` over both decider kinds, by
`koto.decider.model` and `koto.run.id`, and keep `unknown` as a model of
its own. Sum `koto.decider.unread_usage_attempts` the same way: a nonzero
count means some billed answers' usage couldn't be read, so that model's
totals are a lower bound.

### `decider_fallback`

One record per decider-check criterion handed to the agent: a committed
`decider_fallback` event. A criterion falls back when it got no verdict
(the slice was over budget, the extraction failed, the provider failed or
answered something unreadable, the per-call cap was spent, or the decider
lock stayed held) and no answer the agent recorded for that slice applies.
The check then waits on the agent's answer, recorded with `koto decider
answer`, and its gate writes `check_pending` until the answer or the
decider decides it. A fallback's record comes after its evaluation's
`decider_checked` records and before the gate's `check_evaluated` or
`check_pending`.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the check belongs to. |
| `koto.check` | string | always | The decider check's gate name. |
| `koto.rule.id` | string | always | The criterion's id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.unit.key` | string | unit mode, when a unit fell back | The digest of the unit's key, as on `decider_checked`. |
| `koto.unit.document` | string | with `koto.unit.key`, when known | As on `decider_checked`. |
| `koto.unit.section` | string | with `koto.unit.key`, when known | As on `decider_checked`. |
| `koto.decider.mode` | string | always | The criterion's effective mode: `veto` or `shadow`. |
| `koto.decider.reason` | string | always | Why it got no verdict, as on `decider_checked`. A fixed identifier, never a message, a path or instruction text. |
| `koto.decider.input_bytes` | integer | except with `extraction_failed` | The slice's size in bytes, after redaction, as captured. Capture stops at 64 KiB, so 65,536 means at least that many. Absent when the extraction failed and there is no slice. |

A criterion still waiting is evaluated again on every `koto next`, and each
evaluation that hands it over again writes another record. So the number of
records is the number of times the agent was asked; the number of distinct
fallbacks is the number of distinct `koto.session.id`, `koto.visit.seq`,
`koto.check` and `koto.rule.id` groups. The agent's answer is the
[`decider_check_answered`](#decider_check_answered) record that names the
fallback's `koto.seq`; a gate it decided has
`koto.decider.agent_answered_count` above 0, and an answered `fail` in veto
mode is one of its gate's error rules, with a `check_rule` under the
criterion's `koto.rule.id`.

The record never holds the slice, its hash, the criterion's question or
descriptions, or anything the agent wrote.

### `decider_check_answered`

One record per committed `decider_check_answered` event: the agent's answer
to a criterion handed to it, recorded with `koto decider answer`.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the check belongs to. |
| `koto.check` | string | always | The decider check's gate name. |
| `koto.rule.id` | string | always | The criterion's id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.unit.key` | string | unit mode | The digest of the answered unit's key (`koto decider answer --unit`), as on `decider_checked`. |
| `koto.unit.document` | string | with `koto.unit.key`, when known | As on `decider_checked`, from the fallback answered. |
| `koto.unit.section` | string | with `koto.unit.key`, when known | As on `decider_checked`, from the fallback answered. |
| `koto.decider.answer` | string | always | The agent's answer: `pass`, `fail` or `unclear`. |
| `koto.decider.reason` | string | when the log holds the fallback | Why the criterion fell back, from the `decider_fallback` it answers, with the same values. |
| `koto.decider.fallback_seq` | integer | always | The `koto.seq` of the `decider_fallback` record it answers. |

To set the agent's answer beside the decider's, join it to the
`decider_checked` records with the same `koto.session.id`,
`koto.visit.seq`, `koto.check` and `koto.rule.id` and a higher `koto.seq`:
those grade the criterion later in the same visit. The journal carries no
slice hash, so it can't say whether such a grading was of the slice the
agent answered; a slice that changed between the two is graded afresh. For
a unit-mode check, add `koto.unit.key` to the join.

### `rule_delivered`

One record per committed `rule_delivered` event: a `koto next` response
handed back a failed veto criterion's rule, as one entry of a blocking
condition's `rules` list. The event is appended after the response prints,
so the record comes after the gate's `check_evaluated` and its `check_rule`
records. A criterion that declares neither a `summary` nor a `rule_text`,
and a shadow criterion, deliver nothing and write no record.

A command gate's check reports its own deliveries. Each stdout line that
starts with `::koto-rule-delivered::` followed by one JSON object with
exactly the keys `rule_id`, `extent` (`summary` or `full`) and `delivery`
(`printed` or `refused`) is kept on the gate's `gate_evaluated` event, and
`koto next` appends one `rule_delivered` event per line after its response
prints, after any decider check's. A line with a missing or extra key, a
value outside those sets, or the marker anywhere but at the start of the
line is ordinary output and records nothing. At most 50 lines per
evaluation are kept. These records carry `koto.rule.via` and
`koto.rule.delivery`; a decider check's carry neither.

A directive reports the rules it carries through the state's
`directive_rules` (see the template format reference). Each time `koto next`
prints that state's directive, on an advance or a `--to` move, it appends one
`rule_delivered` event per entry whose `when_set` variable or capture holds
a non-empty value, after the response prints and after any check's deliveries. These
records carry `koto.rule.via` `directive` and `koto.rule.delivery` `printed`,
with the entry's declared `rule_id`, `check` and `extent`. An entry whose
value is empty writes no record, every print writes again (two ticks that
print the directive give two records), and `koto status` writes none. Like
every record here, they are telemetry: the response and routing are
unchanged.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state whose check failed, or whose directive carried the rule. |
| `koto.check.name` | string | always | The gate's name, under the same path as on `check_finding`, or a directive rule's declared `check`. The record has no `koto.check` of its own. |
| `koto.rule.id` | string | always | The failed criterion's id, the rule id the check printed, or a directive rule's declared id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.rule.extent` | string | always | How much of the rule was handed back. For a decider check, `summary` on the first failing evaluation of the check and rule in the visit, `full` on every later one; a criterion that declares only one text always gets that text's extent. For a command check, the `extent` it printed; for a directive rule, the `extent` it declares. Later koto versions may add values. |
| `koto.rule.unit_count` | integer | unit mode | How many failing units the rule was handed back for, at least 1: every unit that failed the criterion, by the decider's verdict or the agent's answer, including any the response's finding cap left out. Left out for a check without units and for every directive delivery. |
| `koto.rule.via` | string | command check, directive | `check_output`: the check printed a `::koto-rule-delivered::` line. `directive`: the printed directive carried a rule its state declares in `directive_rules`. |
| `koto.rule.delivery` | string | command check, directive | `printed` when the check printed the rule's text, `refused` when it declined to release it. Always `printed` for a directive delivery. |

The record never holds the rule's text, its summary, or a unit's key. One
response that fails a rule on twenty units writes one record with
`koto.rule.unit_count` 20, not twenty. To count how often a rule came back
at each extent, group by `koto.rule.id` and `koto.rule.extent`; to tie a
delivery to the evaluation that failed, join it to the `check_evaluated`
with the same `koto.session.id`, `koto.visit.seq` and `koto.state`, the
gate as `koto.check`, and the highest `koto.seq` below the delivery's.

### `check_overridden`

One record per committed `gate_override_recorded` event, for any gate type.
koto writes one per override, not one per criterion of a decider check.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the override was recorded in. |
| `koto.check` | string | always | The overridden gate's name. |

### Joining an override

The evaluation an override moved past is the `check_evaluated` with the
same `koto.session.id`, `koto.state`, `koto.check` and `koto.visit.seq` as
the override, and the highest `koto.seq` below the override's. Its
`check_rule` records name the rules the override moved past; for a decider
check, those are the criteria that failed.

koto ties an override to the gate's evaluations in the current visit only,
so an evaluation from an earlier visit never counts. When the visit holds
no such record, as for an override recorded before the gate was evaluated
in that visit, the override names no rule. One case this join can't see:
koto also stops counting earlier evaluations at a transition from the
state to itself, which stays in the visit. An override recorded after such
a transition, with no evaluation since, names no rule to koto, but the
join finds the evaluation before the transition.

### What writes no decider or override record

- A decider-check verdict koto reuses within a visit, which appends no
  `decider_checked`.
- Evidence the decider applied, evidence on a visit with no consultation or
  an `applied` one, and evidence holding none of the consulted fields,
  which write no `decider_answered`.
- A failed criterion that declares neither a `summary` nor a `rule_text`,
  and a shadow criterion, which hand nothing back and write no
  `rule_delivered`.
- Events committed by a koto that predates these records.

### Example

A criterion that fails and is handed back as its summary, and a later
override of the gate:

```json
{"kind":"decider_checked","v":1,"at":"2026-10-10T03:16:40.207Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":22,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.rule.id":"comment_reason","koto.decider.provider":"jev","koto.decider.model":"jev-1.2.3","koto.decider.mode":"veto","koto.decider.outcome":"fail","koto.decider.confidence_bp":9500,"koto.decider.blocked":true,"koto.decider.input_tokens":812,"koto.decider.output_tokens":3,"koto.decider.unread_usage_attempts":0,"koto.decider.attempts":1,"koto.decider.latency_ms":640}
{"kind":"check_evaluated","v":1,"at":"2026-10-10T03:16:40.209Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":23,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.check.outcome":"failed","koto.attempt":3,"koto.visit_attempt":3,"koto.check.rule_count":1,"koto.check.duration_ms":12,"koto.check.unanswered_count":0}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:16:40.209Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":23,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.attempt":3,"koto.visit_attempt":3,"koto.rule.id":"comment_reason","koto.rule.visit_count":1,"koto.rule.session_count":1}
{"kind":"rule_delivered","v":1,"at":"2026-10-10T03:16:40.214Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":24,"koto.visit.seq":9,"koto.state":"implement","koto.check.name":"comments","koto.rule.id":"comment_reason","koto.rule.extent":"summary"}
{"kind":"check_overridden","v":1,"at":"2026-10-10T03:18:02.731Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":25,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments"}
```

A unit-mode check that declares `unit_document: prd` and no
`unit_section`, grading one unit whose extraction printed a section:

```json
{"kind":"decider_checked","v":1,"at":"2026-10-10T03:20:11.502Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":31,"koto.visit.seq":28,"koto.state":"review","koto.check":"ac_binary","koto.rule.id":"ac_binary","koto.unit.key":"sha256:5f1d0c2a9e7b4c3d8a6f0e1b2c3d4e5f","koto.unit.document":"prd","koto.unit.section":"sha256:0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c5d","koto.decider.provider":"jev","koto.decider.model":"jev-1.2.3","koto.decider.mode":"veto","koto.decider.outcome":"pass","koto.decider.confidence_bp":9712,"koto.decider.blocked":false,"koto.decider.input_tokens":240,"koto.decider.output_tokens":3,"koto.decider.unread_usage_attempts":0,"koto.decider.attempts":1,"koto.decider.latency_ms":410,"koto.decider.input_bytes":61}
```

## Evidence fields

When the agent submits evidence with `koto next --with-data`, koto reads the
state's `accepts` block and keeps, on the `evidence_submitted` event, the
values telemetry may carry: its `exported_fields`. The journal relays one
`evidence_field` record per entry, after the event commits, with `koto.seq`
and `koto.visit.seq` as a check record has them.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the evidence was submitted to. |
| `koto.evidence.field` | string | always | The field's name, as `accepts` declares it, or its digest when outside the name shape. |
| `koto.evidence.value` | string | always | For an enum field, the submitted value when `accepts` declares it (as its digest when outside the name shape) and `other` when it doesn't; for a boolean field, `true` or `false`. |

Only enum and boolean fields the submission includes are relayed, in field
name order. String, number and task-list fields are never relayed, so a URL
or free text submitted as evidence stays in the session's log. Evidence koto
writes itself, such as an answer the decider applied, carries no
`exported_fields` and writes no `evidence_field`.

```json
{"kind":"evidence_field","v":1,"at":"2026-10-10T03:20:11.402Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":27,"koto.visit.seq":26,"koto.state":"pr_creation","koto.evidence.field":"pr_status","koto.evidence.value":"shared"}
```

## Review records

`koto review record` records what happened in a document review (a round of
reviewer seats over a brief, PRD, design, plan, vision, strategy or roadmap)
as telemetry: a round starting, each seat's verdict per rule and section,
the decision that follows the round, and publish-time document check
results. It appends one session event per record (see the
[session feed](session-feed.md)), and the journal relays each, after it
commits, as one record with `koto.seq` and `koto.visit.seq` as a check
record has them. A batch of verdicts or document checks is one event per
entry, so each record has its own `koto.seq`, and records of one batch
differ only in their own fields. None of these records changes an outcome,
a route or a state.

It also records what happens to a document while it is drafted:

- `doc_rule_checked`, once for each entry of a `rule-checks` call: one
  check-held rule's outcome at one moment.
- `doc_section_written`, once for each `section-written` call: a section
  written for the first time or rewritten.
- `doc_fix_applied`, once for each entry of a `fix` call: one rule and
  section a fix answers.

A `rule-checks` or `fix` call is one session event holding every entry, so
all the records of one call share its `koto.seq`. That sequence number is
the ordinal of a check or fix within the run.

Every value is one koto validated or mapped before the event was appended;
nothing the caller wrote is copied as written.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.doc.type` | string | `review_round`, `review_verdict`, `review_decision`, `doc_checked`; left out of the first three when `koto.doc.format` is `vision`, `strategy` or `roadmap` | `brief`, `prd`, `design` or `plan`. |
| `koto.doc.format` | string | `doc_rule_checked`, `doc_section_written`, `doc_fix_applied`; `review_round`, `review_verdict` and `review_decision` when the call gave `--doc-format` | `brief`, `prd`, `design`, `plan`, `vision`, `strategy` or `roadmap`. |
| `koto.state` | string | `doc_rule_checked`, `doc_section_written`, `doc_fix_applied` | The state the session was in when the call was made. |
| `koto.doc.rule_outcome` | string | `doc_rule_checked` | `pass`, `fail`, `not_ready` or `error`. |
| `koto.doc.check_moment` | string | `doc_rule_checked` | When the checks ran: `pre_jury`, `post_fix` or `post_create`. |
| `koto.doc.section` | string | `doc_section_written`, `doc_fix_applied` | The section, when the session's template declares it among the `sections` of its `review_vocabulary`; `other` otherwise, and always when the template declares none. |
| `koto.doc.section_write` | string | `doc_section_written` | `first` or `rewrite`. |
| `koto.doc.fix_answers` | string | `doc_fix_applied` | What the fix answers: `jury_fail` or `check_fail`. |
| `koto.doc.fix_round` | integer | `doc_fix_applied` answering a `jury_fail`, when the call gave `--round` | The jury round the fix answers, 1 to 50. |
| `koto.doc.fix_check_rule` | string | `doc_fix_applied` | The check rule that decides the answered rule, in the bounds of `koto.rule.id`, or `none`. |
| `koto.doc.schema` | string | always | The document's schema value, bucketed: `brief/v1`, `prd/v1`, `design/v1`, `plan/v1` or `plan/v2` as given, `absent` when there was none, `other` for anything else. |
| `koto.review.round` | integer | `review_round`, `review_verdict`, `review_decision` | The round's number, from 1. |
| `koto.review.seat_count` | integer | `review_round` | How many seats the round spawned. |
| `koto.review.fallback` | boolean | `review_round` | Whether the round is a fallback round. |
| `koto.review.seat` | string | `review_verdict` | The seat, when the session's template declares it among the `seats` of its `review_vocabulary`; `other` otherwise, and always when the template declares none. |
| `koto.review.section` | string | `review_verdict` | The section, under the same rule with the `sections` of its `review_vocabulary`. |
| `koto.rule.id` | string | `review_verdict`, `doc_checked`, `doc_rule_checked`, `doc_fix_applied` | The rule's id. koto refuses one outside the name shape, so it is never a digest here. |
| `koto.review.verdict` | string | `review_verdict` | `pass` or `fail`. |
| `koto.review.decision` | string | `review_decision` | What follows the round: `rerun`, `fix_and_proceed`, `loop_back`, `proceed` or `stop`. |
| `koto.doc.check_outcome` | string | `doc_checked` | `passed`, `failed` or `error`. |

A session holds at most 2,000 review entries (each round, verdict,
decision, document check and section write, and each entry of a
`rule-checks` or `fix` call), so one session can't flood telemetry with
distinct records.

```json
{"kind":"review_verdict","v":1,"at":"2026-10-10T03:22:05.118Z","session":"scope-export","koto.session.id":"7d1e2f30-4a5b-4c6d-8e9f-0a1b2c3d4e5f","koto.run.id":"7d1e2f30-4a5b-4c6d-8e9f-0a1b2c3d4e5f","koto.seq":42,"koto.visit.seq":38,"koto.doc.type":"prd","koto.doc.schema":"prd/v1","koto.review.round":1,"koto.review.seat":"clarity","koto.rule.id":"rs-012","koto.review.section":"goals","koto.review.verdict":"fail"}
{"kind":"doc_rule_checked","v":1,"at":"2026-10-10T03:21:40.512Z","session":"charter-export","koto.session.id":"3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f","koto.run.id":"3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f","koto.seq":17,"koto.visit.seq":15,"koto.state":"draft","koto.doc.format":"vision","koto.rule.id":"doc-required-sections","koto.doc.rule_outcome":"not_ready","koto.doc.check_moment":"pre_jury"}
```

## Response records

koto writes one `response` record each time `koto next` or `koto status`
prints a directive, right after printing it. It records how much
instruction text koto handed the agent, never the text itself.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.seq` | integer | always | The highest sequence number in the session's log when the record was written. A response relays no event, so this places it among the session's check, decider and override records: after every event committed before it printed. |
| `koto.visit.seq` | integer | always | The visit the session is in, as for check records. |
| `koto.state` | string | always | The state whose directive was printed. |
| `koto.response.source` | string | always | `next` or `status`: the command that printed it. |
| `koto.response.directive_bytes` | integer | always | The UTF-8 byte length of the printed `directive` string, after JSON decoding: escapes don't count, multi-byte characters count every byte, and text koto adds to the directive (the pointer to `koto status`, a notice) counts. |
| `koto.response.details_bytes` | integer | always | The same for the printed `details` string, or 0 when the response printed none, including when koto left the details out because this visit already received them. |

`koto next` writes its record after the response's `instructions_delivered`
event, when it has one, so `koto.seq` covers that event. A `koto next` that
chains through several states prints, and records, only the state it
stopped in. Summing a state's `koto.response.directive_bytes` and
`koto.response.details_bytes` per `koto.visit.seq` gives the instruction
bytes the agent received in each visit.

These write no `response` record:

- A `koto next` that lands on a terminal state, or a `koto status` of a
  session in one, since neither prints a directive.
- A command that prints an error, such as a `koto next` whose evidence is
  refused at validation, or a `koto status` of a session that doesn't
  exist.
- A response whose session another command moved to a different state
  between the print and the record. The record would place the response
  in a state it doesn't describe, so it's skipped.
- A `koto status` response that finds the journal locked by another
  writer for more than about 30 milliseconds. A writer that stopped while
  holding the lock must not hold up `koto status`, which an agent relies
  on to recover, so the record is skipped with the warning
  [Failures](#failures) describes. A `koto next` response waits for the
  lock as long as every other journal write, up to five seconds (see
  [Waiting for the lock](#waiting-for-the-lock)).

Two `response` records of one session written in the same millisecond with
no event committed between them are equal in every field. A reader that
removes exact duplicates (see [Write and ordering
rules](#write-and-ordering-rules)) counts them once; koto takes longer than
that to answer a command, so this is rare.

### Example

```json
{"kind":"response","v":1,"at":"2026-10-10T03:15:02.131Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":15,"koto.visit.seq":9,"koto.state":"implement","koto.response.source":"next","koto.response.directive_bytes":1342,"koto.response.details_bytes":0}
```

## Context reads

koto logs every read of a context key it makes on a session's behalf (a
context gate's check, `koto context get` and `koto context exists`, a
terminal's result map, a decider's context input) as an event in the
session's log. The journal never gets a line per read. When a state visit
ends, koto sums the visit's reads into `context_reads` records: one per
distinct key, reader and access, in the order each was first read. A visit's
first 50 distinct tuples get records of their own. The reads of any further
tuples are summed into one more record, marked `koto.context.overflow`, that
names no key, reader or access. So a visit writes at most 51 context lines.

A context-read record carries `koto.seq` and `koto.visit.seq` like a check
record, with two differences: `koto.seq` is the sequence number of the
session-log event that ended the visit (the highest in the log when the
record is written), and `koto.visit.seq` is the visit that ended, not the
one that event begins.

No record holds a context value as such, but the records do say what a
session read:

- `koto.context.key` is the key's name, verbatim, whenever it fits the
  name shape. Only a key outside the shape is written as its digest, and
  that digest is for shape, not secrecy (see [Rule ids](#rule-ids)).
- `koto.context.first_hash` and `koto.context.last_hash` are the plain,
  unsalted SHA-256 of the content. The same content gives the same hash on
  every host and in every session, so anyone holding a candidate value can
  confirm it, and a short or guessable value (a branch name, a yes or no, a
  small number) is easy to recover from its hash.

Context-read records hold key names and content hashes. koto never sends
journal lines off the host; a tool that does must leave out every
`context_reads` line.

### Reads from another agent session

A `koto context get` or `koto context exists` outside the session read's own
tick with no reader's anchor (`KOTO_ANCHOR` unset, or naming no anchor here),
or with an anchor that is the session read or holds it, goes to this host's
read spool beside the session read, not to its log. A visit's sum reads the
session's log and its spool together, merged by timestamp: a spooled read
made at or after the event the visit's window starts from counts in that
visit, in first-read order with the log's reads. The spool is never sent
anywhere and holds only reads made on this host.

A `koto context get` or `koto context exists` run from another agent
session (one whose `KOTO_ANCHOR` names an anchor that doesn't hold the
session read) is logged in the reader's anchor, not in the session it read
(see the [session-feed contract](session-feed.md#reads-from-another-agent-session)).
The session read's own visits never count it, since neither its log nor its
spool holds it. When the anchor commits the read, koto writes one
`context_reads` record for it, attributed to the session read:

- `session`, `koto.session.id` and `koto.run.id` are the session read's,
  taken from its header and its cached run id without writing anything of
  it. A session whose header can't be read here gets its name alone.
- `koto.context.read_by` is the anchor that recorded the read.
- `koto.state` is the session read's state at the read.
- `koto.context.read_count` is 1 and `koto.context.present_count` 1 or 0;
  the hashes are the read's, when it found the key.
- It carries no `koto.seq` or `koto.visit.seq`: those number the session
  read's log, which holds nothing for this read.

### When a visit's reads are written

The records go in the same write as the record of the event that ends the
visit, right before it:

- An arrival in another state, or a rewind: before its `state_entered`.
- `koto cancel` or a respawn fallback's cancellation: before `cancelled`.
- A terminal: the terminal state's own visit, whose reads are the result
  map's, ends with the result event koto's terminal tick appends to the
  session's log. Its context lines are written then, and the `terminal`
  record follows at once, in a write of its own, so a
  [rotation](#rotation) can fall between them.

A transition from a state to itself stays in the visit and writes none.
A visit's reads are written once.

The one exception to the rule below that reads after a `terminal` aren't
journaled: if the terminal tick couldn't append its result event to the
session's log, the terminal visit stays open. Its reads, and any read made
after the `terminal` record (a `koto context get` on the kept session, say),
are written at the visit's next end, such as a rewind out of the terminal,
before that rewind's `state_entered`.

### `context_reads`

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state of the visit, as its reads name it. |
| `koto.context.key` | string | except on the overflow record | The key's name, verbatim, when it has the name shape; otherwise (any key outside the name shape, such as one longer than 128 bytes or one with a space or a non-ASCII character) its digest, computed as for [Rule ids](#rule-ids). |
| `koto.context.reader` | string | except on the overflow record | Who read it: `gate`, `cli`, `result` or `decider`. A value outside the name shape is written as its digest. |
| `koto.context.access` | string | except on the overflow record | `content` for a read that fetched the content, `presence` for one that asked only whether the key exists. A value outside the name shape is written as its digest. |
| `koto.context.overflow` | boolean | on the overflow record only | `true`: the record sums the reads of every tuple past the visit's first 50. |
| `koto.context.read_count` | integer | always | Reads of the tuple in the visit, at least 1. |
| `koto.context.present_count` | integer | always | Those of them that found the key. |
| `koto.context.first_hash` | string | when `koto.context.present_count` is at least 1, except on the overflow record | The unsalted SHA-256 of the content the first read that found the key saw, in lowercase hex, as the session log records it. |
| `koto.context.last_hash` | string | as `koto.context.first_hash` | The same for the last such read. Equal to the first when the content didn't change. |
| `koto.context.read_by` | string | on a read a reader's anchor recorded only | The anchor that recorded the read of this session (see [Reads from another agent session](#reads-from-another-agent-session)). Such a record has no `koto.seq` or `koto.visit.seq`. |

### What writes no context-read record

- Reads koto doesn't log: `koto status`, and koto's own bookkeeping.
- Reads in a visit after its `terminal` or `cancelled` record was written,
  such as `koto context get` on a kept terminal session or on a cancelled
  one, except after a terminal whose result event couldn't be appended
  (see above).
- The reads in a log carried in by `koto session import`. An imported
  session's open visit counts only the reads made after the import.
- A visit still open when the session is removed without a `terminal` or
  `cancelled` record.
- Reads logged by a koto that predates context-read records, in a visit
  that ended before the upgrade.

### Example

```json
{"kind":"context_reads","v":1,"at":"2026-10-10T03:40:11.207Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":31,"koto.visit.seq":17,"koto.state":"ci","koto.context.key":"review/notes.md","koto.context.reader":"gate","koto.context.access":"content","koto.context.read_count":3,"koto.context.present_count":2,"koto.context.first_hash":"5f0c2a6b1d4e8f3a9c7b2e1d0f4a6c8b3e5d7f9a1c2b4d6e8f0a3c5b7d9e1f2a","koto.context.last_hash":"9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9c8b7a6f5e4d3c2b1a0f9e8d"}
{"kind":"state_entered","v":1,"at":"2026-10-10T03:40:11.207Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.state":"merge"}
```

## Run ids

A run is a root session and every session spawned under it, at any depth.
Its id is the root's `session_id`, so a root's `koto.run.id` equals its
`koto.session.id` and two roots never share a run id, even under one
driver. A child records its root's and parent's ids in its header when it's
created (`root_session_id` and `parent_session_id`, described in the
[session-feed contract](session-feed.md#header-record)), so its records
keep the run id after the root has been removed.

A child created by an older koto has neither header field. koto then walks
its `parent_workflow` names up to the root, reading only this host's
session store. When a link in that chain is gone or unreadable, the run id
is left out: a child's run id is never a non-root ancestor's id.

An imported session is a new run: its own id is its run id.

## Write and ordering rules

- Every record is written after the session's own log has committed the
  change it describes, never before. A failed commit writes no record.
- A command's records for one session appear in the order its commits
  happened. A `driver_seen` comes before that command's other records for
  the session.
- `session_started` comes before the session's other records, and a
  `state_entered` comes before the `terminal` for the same arrival.
- Writers take an advisory lock on the file for each commit's records,
  so records are never interleaved. Records from concurrent commands on
  different sessions interleave line by line in the order they were
  written. A writer that can't take the lock in time skips its records
  (see [Waiting for the lock](#waiting-for-the-lock)).
- `at` is the time the record was written, not the time of the event it
  describes, and it isn't guaranteed to increase down the file across
  processes. Use file order within a session.
- Order one session's check, decider, override and response records by
  `koto.seq`, then by file order. A `check_evaluated` comes before its own
  `check_rule` records and then its `check_finding` records, all in one
  write and sharing its `koto.seq`. A `response` comes
  after the records of the events committed before it printed.
- A visit's `context_reads` come after any `driver_seen` of the same
  command. A visit that ends in another state, a rewind or a cancel has
  them right before the `state_entered` or `cancelled` that ends it, in
  the same write, sharing that event's `koto.seq`. A terminal visit's are
  written with the commit of the result event the terminal tick appends
  to the session's log (see [When a visit's reads are
  written](#when-a-visits-reads-are-written)), and carry that event's
  `koto.seq`; the `terminal` record, which has no `koto.seq`, follows in
  a write of its own. Records of one visit differ in their key, reader
  and access.
- A copy of the journal that delivered some lines more than once (a
  sender that retries, for example) holds exact duplicates. Treat two
  check, decider, override, response or context-read records as duplicates
  only when every field, `at` included, is equal. Distinct check, decider
  and override records differ in `kind`, `at`, `koto.seq`,
  `koto.rule.id` or `koto.finding.level`, and distinct context-read records in `at`, `koto.seq` or
  their key, reader and access. Two `response` records can be equal without
  being a duplicate; see the note on responses printed in the same
  millisecond under [Response records](#response-records).
- `koto session import` writes the imported session's `session_started` and
  one `state_entered` for the state it's in, both timed at the import,
  exactly once per imported session. A retry or a rerun that adopts an
  already-imported session writes nothing, and the source's own records
  aren't copied.

## Drivers

The driver is the Claude Code session running koto, read from
`CLAUDE_CODE_SESSION_ID`. A value outside the id shape (empty, longer than
128 characters, or holding a character outside `A-Z a-z 0-9 . _ : -`, such
as a newline) counts as no driver.

- `session_started` carries the creating driver, when there is one.
- Whenever a command that appends to a session's log runs under a driver
  other than the last one recorded for that session, or the session has no
  driver recorded yet, koto writes `driver_seen` for it. This applies to
  every append: evidence, a gate result and a context write count, not only
  state changes. No driver is never a change and records nothing.
- A command that appends to a child's log (a child finishing appends
  `child_completed` to its parent's log) records the driver for each
  session it appended to, and only those.
- By default, gate commands and default actions don't receive
  `CLAUDE_CODE_SESSION_ID`, so a koto command run from one records no
  driver. Two exceptions pass it through, and then such a command records
  the outer session's driver like any other: a session created with
  `koto init --legacy-environment`, which hands commands the whole
  environment, and a template that lists the variable under `pass_env:`.
- The same driver can be recorded twice in a row: two commands racing under
  one new driver can each write a `driver_seen`, and so can a session whose
  sidecar was lost. A `driver_seen` whose write failed (see
  [Failures](#failures)) isn't written again.

`driver_seen` marks a change, not every command. A command run with no
driver writes nothing, so the records after a `driver_seen` aren't
guaranteed to come from that driver. The set of distinct driver values on a
session's records is the reliable signal; attributing each record to the
latest `driver_seen` before it is best-effort.

## Fixture rules

`koto.fixture` is `true` when the session's template came from a directory
that looks like a test fixture. The rules read the header's
`template_source_dir` as recorded when the session was created (for an
imported session, as the source recorded it). A session with no template
source directory (`--from-stdin`, `koto session start`) is never a fixture.
A session is a fixture when any rule matches:

| Rule | Matches |
|------|---------|
| Temporary directory | The directory is, or is under, `/tmp` or `/var/folders`, the same two under macOS's `/private` directory, or `TMPDIR` when `TMPDIR` is absolute (and not `/`). |
| mktemp directory | A path segment matches `^tmp\.[A-Za-z0-9]{6,}$`. |
| Ablation directory | A path segment starts with `shirabe-ablation.`. |
| Tool test directory | A `test` or `tests` segment follows an earlier `koto`, `niwa` or `shirabe` segment. |

The temporary-directory rule never touches the filesystem. It normalizes
the recorded path lexically, as POSIX `normpath` does (`.` and `..`
segments and repeated slashes resolved, exactly two leading slashes kept as
two), and resolves no symlink. A relative path is under no temporary
directory. So `/home/u/../../tmp/x` and `///tmp/x` match, while
`/tmp/../home/u/x`, `//tmp/x` and `tmp/x` don't, and a template reached
through a symlink into `/tmp` isn't a fixture unless the recorded path
itself is under a temporary root.

The rules are embedded in koto as data, in
`src/run_journal/fixture_rules.json`.

## Failures

Writing the journal is best-effort. When a record can't be written (a koto
home that can't be created, a read-only or full disk, a symlink at the
journal's path, a record over 4096 bytes), koto prints one warning line
naming the run journal on standard error, the first time in that process,
and carries on. The command's standard output, exit code, session state and
gate decisions don't change. A record over the cap is dropped, and the
command's other records are still written.

A write cut short by a crash or a full disk can leave a partial last line.
The next writer notices, under the lock, that the file doesn't end in a
newline and starts its record on a line of its own, so only the partial
line is lost.

### Waiting for the lock

Every write waits at most five seconds for the journal's lock, so ordinary
contention between koto commands doesn't lose records. A `koto status`
response waits at most 30 milliseconds. When the wait runs out, because
another process holds the lock (one that stopped or hung while writing,
say), the write's records are skipped with the warning above and the
command carries on. Its standard output and exit code are the ones it
would have had, and so is everything it does after the write: a terminal
tick that can't write its `terminal` record still notifies the session's
parent and removes the session, or keeps it, as it would have. The skipped
records aren't written later. The wait isn't configurable.

A process waits out a held lock once. After one write has waited the full
five seconds and given up, the process's later writes try the lock once,
without waiting, and are skipped too if it's still held (the warning isn't
repeated), so a command that writes several batches while the lock stays
held takes about five seconds, not five per batch. The first later write
that finds the lock free is written as usual and gives the writes after it
their full wait again, so a long-running command (a `koto next` polling a
gate) recovers once the holder lets go.

The wait covers taking the lock, and nothing else. A writer that holds the
lock finishes what it does under it however long that takes. A rotation
flushes the file it moves to disk and then runs retention, both under the
lock (see [Rotation](#rotation)); on a slow disk that can take longer than
the wait, and it isn't cut short. The writers waiting behind it are the
ones that give up, each after its own wait. The rotating writer then gets
a full wait of its own for the fresh file's lock, so a slow rotation
doesn't make it drop the records it rotated for. If it still can't take
that lock, its records are skipped, and the process's later writes keep
their full wait.

## Durability

Identity records are flushed to disk as they're written. Check, decider,
override and response records aren't flushed on their own, since a blocked
tick writes several and a flush takes milliseconds. They reach the
operating system as soon as they're written, so a koto process that
crashes or is killed loses none of them, and the next identity record's
flush (the session's next state entry, terminal or cancel) makes them
durable along with it. An operating system crash or a power loss can lose
check, decider, override and response records written since the journal's
last flush. Context-read records are written with the identity record that
ends their visit, or, at a terminal, just before the flushed `terminal`, so
they're made durable with it.

## Retention

No session-lifecycle command deletes or rewrites the journal:
`koto session cleanup`, `koto cancel --cleanup`, the removal of a session
at its terminal tick, `koto init --replace-terminal` (whether it removes
the old run and its children or moves them aside), the prunes and
`koto session import` all leave existing
lines byte-identical. The journal grows by a few hundred bytes per state
entered, per check evaluation, decider call, override, dispute and printed
directive, by up to 50 more lines for a check that reports many error
rules and up to 50 more for the distinct rule ids and levels of its
findings, and by up to 51 lines per visit for the context keys it read. A
check that reports no findings adds no `check_finding` line; a typical one
adds one line per rule it reports, at roughly 450 bytes each.

To remove it by hand, delete the file (or move it aside) while no koto
command is running. The next record creates a new file. Lines removed this
way can't be rebuilt once the sessions they describe are gone.

### Rotation

Rotation is off by default, and stays off by default until the journal's
consumers list rotated files. Turn it on with a size bound in user config,
for example 64 MiB:

```bash
koto config set run_journal.rotate_bytes 67108864 --user
```

The settings are read from user config only; `koto config set` refuses
them without `--user`, and a project's `.koto/config.toml` can't change them,
since the journal belongs to the user's store rather than to any one
repository. A bound of 0 turns rotation off again.

With a bound set, the write that finds the journal at or over the bound
first moves the journal to the next rotated name, then writes its records to
a fresh `_run_journal.jsonl`. The check runs once per write, under the
journal's lock and before anything is written. A write is one commit's
records (a check's `check_evaluated` and its `check_rule` records, say, or a
visit's `context_reads` and the `state_entered` that ends the visit), so
they always land together in one file; a rotation can fall between two
writes of the same command. The file being moved is flushed to disk first,
so the check, decider, override, response and context-read records it holds
that weren't flushed yet don't wait on a flush that will now go to the new
file. The journal is never truncated or rewritten in place:

- **Naming.** Rotated files sit beside the journal as
  `_run_journal.NNNNNNNNNN.jsonl`: ten decimal digits, zero-padded, starting
  at `_run_journal.0000000001.jsonl`. Each new one is numbered one past the
  highest that exists, so sorting the names gives rotation order, oldest
  first, and `_run_journal.jsonl` is always the newest file.
- **The move.** A rotation links the file to its new name, which fails rather
  than replace an existing file, and then removes the old name. The file keeps
  its device and inode number. A partial last line (a write cut short) gets
  its newline first, so no rotated file ends mid-line. A crash between the
  link and the removal leaves the file under both names; the next write
  finishes the move by removing the journal's name, so a file never gets two
  rotated names.
- **No late writes.** A command that opened the journal just before another
  one moved it notices, once it holds the lock, that the path now names a
  different file, and opens the path again. No record is written to a file
  after it was moved.
- **Waiting.** A write's wait for the lock (see
  [Waiting for the lock](#waiting-for-the-lock)) covers the whole write,
  including the lock of a fresh file it finds after another command's
  rotation, so turning rotation on doesn't lengthen it. The rotation itself
  isn't timed: its flush and its retention pass run under the lock to the
  end, and the writers behind them wait no longer for it. The writer that
  rotated then gets a full wait of its own for the fresh file's lock.
- **Failure.** A move that fails prints one warning on standard error, and
  the record goes to the unrotated journal. That command doesn't try to
  rotate again.

### Reading across a rotation

A reader that follows the journal by byte offset keeps three numbers for the
file it's reading: its device, its inode and the offset it has read up to.
On each pass:

1. Open `_run_journal.jsonl` and `fstat` the open file. (Stat-then-open could
   race a rotation.) If it doesn't exist, a rotation has moved it and no
   record has been written since. Do step 3 without the new file: read the
   rest of yours and every newer rotated file, keep the last one's device,
   inode and end offset, and look again on the next pass.
2. If the device and inode are the ones you kept, read from your offset.
3. Otherwise the file you were reading was rotated. Find the rotated file
   with your device and inode, read it from your offset to its end, then read
   every newer rotated file, in name order, from its first byte, then the
   file you opened in step 1 from its first byte. If that file shows up among
   the rotated ones (another rotation happened during the pass), skip it
   there, since you read it last. Keep its device and inode and the new
   offset.

A reader that doesn't follow by offset but copies the live journal by its
exact name every so often, keeping a shrunk or changed file as a new
version, needs one more step to stay lossless. The lines written after its
last copy and before a rotation exist only in the rotated file. So on every
pass, before copying `_run_journal.jsonl`, it lists the rotated files by the
naming rule and copies each one newer than the last it copied, in name
order.

### Retention

Rotated files are removed only during a rotation, and only when two things
hold: the file is older than the retention bound, measured from its rotation
(koto stamps the file's modification time when it rotates it), and no
recorded reader position still needs it. The newest rotated file is never
removed, so the numbering never restarts and name order stays rotation
order. The bound is 30 days by default:

```bash
koto config set run_journal.retain_days 30 --user
```

A reader that wants its unread files kept past the bound records its
position in a file of its own in `_run_journal.readers/` beside the journal,
named `<reader-id>.json`, where the id is 1 to 64 letters, digits, dots,
underscores or hyphens. koto creates the directory, mode 0700, at the first
rotation, and never writes a position itself. The file holds one JSON
object:

```text
{"v": 1, "dev": 16777231, "inode": 1234567, "offset": 52311,
 "file": "_run_journal.0000000007.jsonl"}
```

`dev` and `inode` name the journal file the reader is in, and `offset` is
how many bytes of it it has read. `file` is optional and only informational.
A reader that copies whole files records the device and inode of the last
file it copied and that file's length. Write the position to a temporary
file and rename it into place, and refresh it on every pass.

A position needs the file it names and every newer one, except that a
position at or past the end of a rotated file needs only the newer ones.
koto reads position files and the journal files' names and metadata, never a
journal record. A position file that is larger than 4 KiB, a symlink, or
doesn't parse protects nothing, and neither does one not modified within the
stale bound (90 days by default); each is named in a warning on standard
error. A position whose device and inode match no journal file (its file
was already removed) protects nothing either, and is reported at each
rotation until the reader updates or removes it. Change the stale bound
with:

```bash
koto config set run_journal.reader_stale_days 90 --user
```

With no positions recorded, the retention bound alone decides. A reader that
records no position and stays away longer than the bound misses the files
removed in the meantime.

## Reading the journal

- Read line by line. Skip any line that doesn't parse as JSON: it's a
  partial write, and the next line starts a fresh record.
- Skip records whose `kind` you don't know, and ignore fields you don't
  know. koto may add kinds and fields without changing `v`.
- `v` changes only when an existing field changes meaning or shape.
  Skip records with a `v` you don't support.
- Treat every field other than `kind`, `v`, `at` and `session` as
  optional.
- Integer fields are JSON numbers, never strings. Boolean fields are JSON
  `true` or `false`.
- Join a session's records on `koto.session.id`, not `session`: a name can
  be reused by a later session (`--replace-terminal`, or a name freed by
  cleanup), and its id can't. A record without `koto.session.id` comes
  from a session too old to have one; fall back to `session` for it.
