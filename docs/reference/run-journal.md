# Run Journal Reference

The run journal is a local record of session identity and progress for
tooling outside koto. koto appends one JSON object per line as sessions
start, enter states, reach a terminal, are cancelled, or are driven by a new
Claude Code session, each time koto evaluates one of their checks or
consults the decider, and each time an override is recorded. The journal
is append-only: koto never rewrites or reads back its records, so a
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
`koto.template.hash`) must match `^[A-Za-z0-9._:-]{1,128}$`. Names
(`koto.template.name`, `koto.state`, `koto.terminal`, `koto.check`, and
the string values of the check, decider and override kinds) must match
`^[A-Za-z0-9._:/@-]{1,128}$` and not start with `/` or `~`. A value outside
its shape is left out and the record is still written. `koto.rule.id` is
never left out: a rule id outside the name shape, a decider criterion's id
included, is written as its digest (see [Rule ids](#rule-ids)).

### Record kinds

| Kind | Its own fields | Written when |
|------|----------------|--------------|
| `session_started` | `koto.parent.session.id`, `koto.driver.session.id`, `koto.template.name`, `koto.template.hash`, `koto.fixture`, `koto.imported_from.session.id` | A session is created. |
| `state_entered` | `koto.state` | The session enters a state. |
| `terminal` | `koto.terminal` | The session arrives at a terminal state. |
| `cancelled` | none | The session is cancelled. |
| `driver_seen` | `koto.driver.session.id` | A command runs under a driving session other than the last one recorded for it. |
| `check_evaluated` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.check.outcome`, `koto.attempt`, `koto.visit_attempt`, `koto.check.rule_count`, `koto.check.rule_counts_truncated`, `koto.check.duration_ms`, `koto.poll.status`, `koto.check.unanswered_count`, `koto.check.error` | koto evaluates a gate and the result isn't pending, or runs a state's default action. |
| `check_rule` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.attempt`, `koto.visit_attempt`, `koto.rule.id`, `koto.rule.visit_count`, `koto.rule.session_count` | Once for each error rule of a check evaluation, after its `check_evaluated`. |
| `check_pending` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.poll.status`, `koto.poll.evaluations`, `koto.poll.elapsed_secs` | A polling gate's evaluation is still pending. |
| `decider_consulted` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.decider.provider`, `koto.decider.model`, `koto.decider.outcome`, `koto.decider.error_class`, `koto.decider.input_tokens`, `koto.decider.output_tokens`, `koto.decider.unread_usage_attempts`, `koto.decider.latency_ms`, `koto.decider.directive_bytes` | koto consults the decider about a state's evidence. |
| `decider_checked` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check`, `koto.rule.id`, `koto.decider.provider`, `koto.decider.model`, `koto.decider.mode`, `koto.decider.outcome`, `koto.decider.reason`, `koto.decider.blocked`, `koto.decider.error_class`, `koto.decider.input_tokens`, `koto.decider.output_tokens`, `koto.decider.unread_usage_attempts`, `koto.decider.attempts`, `koto.decider.latency_ms` | koto grades one criterion of a decider check. |
| `check_overridden` | `koto.seq`, `koto.visit.seq`, `koto.state`, `koto.check` | An override is recorded for a gate. |

The first five are the identity kinds. The next three are the check kinds,
described in [Check records](#check-records), and the last three are the
decider and override kinds, described in
[Decider and override records](#decider-and-override-records).

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
| `koto.check.duration_ms` | integer | when logged | The check command's run time in milliseconds. Command gates, decider checks and default actions log it; context gates don't. |
| `koto.poll.status` | string | polling gates | Where the poll settled: `done`, `failed` or `timed_out`. |
| `koto.check.unanswered_count` | integer | decider checks | How many veto criteria got no verdict. |
| `koto.check.error` | string | decider checks, when set | Why no criterion could be graded, such as `log_unreadable` or `missing_spec`. |

### `check_rule`

One record per error rule of a check evaluation, in the order koto first
saw each rule reported, right after the evaluation's `check_evaluated`.
koto counts error rules only on a check that didn't pass: the rule ids it
reported at level `error`, or koto's own fallback finding's id when none of
its findings was an error. A passing check writes no `check_rule`, whatever
it printed, and an evaluation writes at most 50.

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

### `check_pending`

One record per evaluation of a polling gate that is still pending. A
pending evaluation judged nothing, so it carries no outcome, attempt or rule
field and never a `check_evaluated`. The evaluation where the poll settles
is a check evaluation.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the gate belongs to. |
| `koto.check` | string | always | The gate's name. |
| `koto.poll.status` | string | always | `pending`. |
| `koto.poll.evaluations` | integer | always | The gate's runs since its polling window opened, at least 1. |
| `koto.poll.elapsed_secs` | integer | always | Seconds since the polling window opened. |

A polling gate re-run several times inside one evaluation (a `hold_secs`
wait) writes one record for that evaluation, and `koto.poll.evaluations`
counts the runs.

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
{"kind":"check_evaluated","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.check.outcome":"failed","koto.attempt":2,"koto.visit_attempt":2,"koto.check.rule_count":2,"koto.check.duration_ms":840}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"E501","koto.rule.visit_count":2,"koto.rule.session_count":2}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:15:02.114Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":14,"koto.visit.seq":9,"koto.state":"implement","koto.check":"lint","koto.attempt":2,"koto.visit_attempt":2,"koto.rule.id":"no-unused-vars","koto.rule.visit_count":1,"koto.rule.session_count":1}
{"kind":"check_pending","v":1,"at":"2026-10-10T03:21:40.502Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":19,"koto.visit.seq":17,"koto.state":"ci","koto.check":"ci","koto.poll.status":"pending","koto.poll.evaluations":2,"koto.poll.elapsed_secs":31}
```

## Decider and override records

koto relays each decider consultation and each recorded override from the
event the session's log commits for it, after the commit, as it relays
checks. Like check records, they carry `koto.seq` and `koto.visit.seq` after
`koto.run.id`. `koto.visit.seq` is koto's visit, by the rule in
[Check records](#check-records). A decider event carries a `visit_seq` of
its own, which isn't copied.

These records copy names, counts and short vocabulary values field by
field. They never hold a probability, an input or declaration hash, a
threshold, the endpoint or where it came from, a criterion's rule
reference, question or descriptions, the slice a decider check sent, a
routing consultation's per-field detail, an override's rationale, its
values or the output it moved past.

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
| `koto.decider.directive_bytes` | integer | always | Byte length of the directive and details the agent would have received for the state. |

### `decider_checked`

One record per criterion koto grades on a decider check: a committed
`decider_checked` event. A decider check's records come before its gate's
`check_evaluated`, each with a lower `koto.seq`. A verdict koto reuses
within a visit appends no event, so it writes no record.

| Field | Type | Present | Description |
|-------|------|---------|-------------|
| `koto.state` | string | always | The state the check belongs to. |
| `koto.check` | string | always | The decider check's gate name. |
| `koto.rule.id` | string | always | The criterion's id, or its digest (see [Rule ids](#rule-ids)). |
| `koto.decider.provider` | string | always | The provider's name. |
| `koto.decider.model` | string | always | The model the provider reported, or `unknown`. |
| `koto.decider.mode` | string | always | The criterion's effective mode: `veto` or `shadow`. |
| `koto.decider.outcome` | string | always | `pass`, `fail`, `escape`, `unanswered` or `not_graded`. |
| `koto.decider.reason` | string | when logged | Why a criterion got no verdict: `provider_error`, `unreadable_response`, `over_budget`, `extraction_failed`, `cap_spent` or `busy`. |
| `koto.decider.blocked` | boolean | always | Whether this verdict blocked the state. |
| `koto.decider.error_class` | string | when logged | Why the last provider call failed. |
| `koto.decider.input_tokens` | integer | when logged | Input tokens, summed over the attempts that reported usage. |
| `koto.decider.output_tokens` | integer | when logged | Output tokens, likewise. |
| `koto.decider.unread_usage_attempts` | integer | always | Billed attempts whose token usage couldn't be read. |
| `koto.decider.attempts` | integer | always | Provider calls made for this criterion, 0 when none was sent. |
| `koto.decider.latency_ms` | integer | always | Wall time of those calls, in milliseconds. |

A failing veto criterion is also one of its gate's error rules, so the
gate's `check_evaluated` that follows has a `check_rule` for it under the
same `koto.rule.id`.

To total decider tokens, sum `koto.decider.input_tokens` and
`koto.decider.output_tokens` over both decider kinds, by
`koto.decider.model` and `koto.run.id`, and keep `unknown` as a model of
its own. Sum `koto.decider.unread_usage_attempts` the same way: a nonzero
count means some billed answers' usage couldn't be read, so that model's
totals are a lower bound.

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
- Events committed by a koto that predates these records.

### Example

```json
{"kind":"decider_checked","v":1,"at":"2026-10-10T03:16:40.207Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":22,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.rule.id":"comment_reason","koto.decider.provider":"jev","koto.decider.model":"jev-1.2.3","koto.decider.mode":"veto","koto.decider.outcome":"fail","koto.decider.blocked":true,"koto.decider.input_tokens":812,"koto.decider.output_tokens":3,"koto.decider.unread_usage_attempts":0,"koto.decider.attempts":1,"koto.decider.latency_ms":640}
{"kind":"check_evaluated","v":1,"at":"2026-10-10T03:16:40.209Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":23,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.check.outcome":"failed","koto.attempt":3,"koto.visit_attempt":3,"koto.check.rule_count":1,"koto.check.duration_ms":12,"koto.check.unanswered_count":0}
{"kind":"check_rule","v":1,"at":"2026-10-10T03:16:40.209Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":23,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments","koto.attempt":3,"koto.visit_attempt":3,"koto.rule.id":"comment_reason","koto.rule.visit_count":1,"koto.rule.session_count":1}
{"kind":"check_overridden","v":1,"at":"2026-10-10T03:18:02.731Z","session":"issue_42","koto.session.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.run.id":"b43af83c-5948-4cf2-8424-563d8436f02e","koto.seq":25,"koto.visit.seq":9,"koto.state":"implement","koto.check":"comments"}
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
  written.
- `at` is the time the record was written, not the time of the event it
  describes, and it isn't guaranteed to increase down the file across
  processes. Use file order within a session.
- Order one session's check, decider and override records by
  `koto.seq`, then by file order. A `check_evaluated` comes before its own
  `check_rule` records, which share its `koto.seq`.
- A copy of the journal that delivered some lines more than once (a
  sender that retries, for example) holds exact duplicates. Treat two
  check, decider or override records as duplicates only when every field,
  `at` included, is equal. Distinct records of these kinds differ in
  `kind`, `at`, `koto.seq` or `koto.rule.id`.
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

## Durability

Identity records are flushed to disk as they're written. Check, decider
and override records aren't flushed on their own, since a blocked tick
writes several and a flush takes milliseconds. They reach the operating
system as soon as they're written, so a koto process that crashes or is
killed loses none of them, and the next identity record's flush (the
session's next state entry, terminal or cancel) makes them durable along
with it. An operating system crash or a power loss can lose check,
decider and override records written since the journal's last flush.

## Retention

No session-lifecycle command deletes or rewrites the journal:
`koto session cleanup`, `koto cancel --cleanup`, the removal of a session
at its terminal tick, a parent's sweep of its children,
`koto init --replace-terminal` and `koto session import` all leave existing
lines byte-identical. The journal grows by a few hundred bytes per state
entered, per check evaluation, decider call and override, and by up to 50
more lines for a check that reports many error rules.

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
records (a check's `check_evaluated` and its `check_rule` records, say), so
they always land together in one file; a rotation can fall between two
writes of the same command. The file being moved is flushed to disk first,
so the check, decider and override records it holds that weren't flushed
yet don't wait on a flush that will now go to the new file. The journal is
never truncated or rewritten in place:

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
