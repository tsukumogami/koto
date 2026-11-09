# Workflow Signal Contract Reference

**Status: draft, contract version 0.1.0.** Names and record kinds may still
change before 1.0.0. The draft becomes 1.0.0, with its schema file, once the
records have been checked live on two backends; emitters built before then
state `0.1.0`.

This contract says what koto tells OpenTelemetry about a workflow run, and
what niwa adds to the agent runtime's own records when it launches a session.
koto emits each fact it already records as an OpenTelemetry log event named
`koto.<kind>`, and joins every record to the runtime's records by the
runtime's own `session.id` and trace context. Names stay in the `koto.` and
`niwa.` namespaces, except where OpenTelemetry's semantic conventions already
have a word for the concept. How koto reaches a receiver is a separate piece
of work; this document fixes what arrives.

```
runtime session (Claude Code)                    team's receiver
  records: session.id, prompt.id, ...  ───────────►  (local collector or
  resource: niwa.* (dispatched workers),              hosted backend)
            niwa.contract.version                           ▲
        │                                                   │
        │ CLAUDE_CODE_SESSION_ID, TRACEPARENT (headless)    │
        ▼                                                   │
koto process (one per command)                              │
  scope "koto", schema_url = koto schema file               │
  resource: service.name=koto, service.version,             │
            koto.contract.version                           │
  events koto.<kind>: session.id, koto.run.id,  ────────────┘
            koto.session.id, koto.seq, ...
```

The local [run journal](run-journal.md) is a separate record and doesn't
change for this contract. Where a journal field name differs from the
contract's, the name table below says so.

## Nothing is sent by default

No koto or niwa signal leaves the machine unless the team turns the signals
on. Turning them on is an explicit, koto-specific switch; an
`OTEL_EXPORTER_OTLP_*` endpoint that happens to be set in a shell is not
enough on its own. Once on, the signals go to the receiver the team's
OpenTelemetry settings name, and nowhere else. Nothing is sent to the koto
project. With the signals off, koto records locally as it does today and
sends nothing.

## Record envelope

Every koto record is an OpenTelemetry log record with:

- event name `koto.<kind>` (one of the kinds below) in the record's
  `EventName`, or in an `event.name` attribute where an SDK predates the
  field; body empty, severity INFO, timestamp the time of the fact;
- trace context (trace id, span id, flags) copied from `TRACEPARENT` when the
  runtime passed one to koto's process, and absent otherwise;
- instrumentation scope `koto`, with `schema_url` naming the contract's
  schema file (see [Versioning](#versioning));
- resource attributes `service.name` = `koto`, `service.version` = koto's
  release version, `koto.contract.version`, and no others (SDK resource
  detectors that would add process, host or command-line attributes stay
  off), with the resource `schema_url` naming semantic conventions 1.37.0;
- the common attributes: `session.id` (the driving runtime session, when one
  exists), `koto.run.id` and `koto.session.id` (each left out when koto
  can't resolve it, as for a session created before koto recorded session
  ids), and `koto.seq`.

`koto.seq` is the sequence number, in the koto session's own log, of the
event the record relays, so records derived from one event share it: a
failing evaluation's `koto.check_rule` and `koto.check_finding` records carry
the evaluation's, and a visit's `koto.state_exited` carries the same number
as the `koto.state_entered` the transition opened. A record that relays no
single event (`koto.response`, `koto.terminal`) carries the log's highest
number when it was written. Readers order one session's records by
`koto.seq`, then by kind in the order the kind table lists them.

A **visit-scoped** kind also carries `koto.state` and `koto.visit.seq`.

## Record kinds

Groups used in the name table: "check kinds" are `check_evaluated`,
`check_rule`, `check_finding`, `check_pending` and `check_disputed`;
"visit-scoped kinds" are the kinds marked yes below; "all" means every kind.
Kind names in the name table drop the `koto.` prefix.

| Kind | Meaning | Visit-scoped | Own attributes (besides common) |
|---|---|---|---|
| `koto.session_started` | A koto session was created; on a root session, a run began. Carries the configuration version. | no | 12 |
| `koto.state_entered` | The session entered a state (a visit began). | yes | 0 |
| `koto.state_exited` | A visit ended, by transition, rewind or cancel. A terminal state's visit has none. | yes | 2 |
| `koto.terminal` | The session reached a terminal state. | yes | 2 |
| `koto.cancelled` | The session was cancelled. | no | 1 |
| `koto.driver_seen` | A different runtime session started driving the koto session. | no | 0 |
| `koto.check_evaluated` | A gate or default action reached a conclusion. | yes | 14 |
| `koto.check_rule` | One rule a failing evaluation reported at error. | yes | 6 |
| `koto.check_finding` | Findings of one rule and level an evaluation logged. | yes | 6 |
| `koto.check_pending` | A polling gate or decider check is still pending; not an attempt. | yes | 6 |
| `koto.check_disputed` | Someone disputed a rule's finding at a gate. | yes | 5 |
| `koto.decider_consulted` | koto asked its decider model about a state's evidence. | yes | 17 |
| `koto.decider_field` | One field of a multi-field consultation. | yes | 5 |
| `koto.decider_answered` | The agent answered a field the decider didn't decide. | yes | 3 |
| `koto.decider_checked` | The decider graded one criterion of a decider check. | yes | 15 |
| `koto.decider_fallback` | A criterion with no verdict went to the agent. | yes | 5 |
| `koto.response` | `koto next` or `koto status` returned a directive. | yes | 3 |
| `koto.decision_recorded` | A decision was recorded (see [Decisions](#decisions)). | yes | 12 |
| `koto.change_linked` | The run linked a change (pull request and head commit) it produced. | no | 3 |

The common attributes add four more (six on visit-scoped kinds, with
`koto.state` and `koto.visit.seq`), so the largest kind,
`koto.decider_consulted`, holds 23 record attributes beside koto's three
resource attributes. No kind exceeds 64, half the OpenTelemetry SDKs'
default limit of 128, leaving room for the resource attributes a runtime and
a launcher add.

`koto.state_exited` carries `koto.visit.duration_ms` and `koto.visit.exit`
(`transition`, `rewind`, `cancel`); a terminal state's visit ends with the
run and has no exit record. `koto.terminal` carries `koto.terminal` and
`koto.run.result`; `koto.cancelled` carries `koto.run.result` =
`cancelled`. `koto.run.result` takes the class the template declares for
that terminal (`success`, `blocked`, `refused`, `error`), or `unclassified`
when the template declares none.

**Signal kinds.** Every kind is a log event. niwa emits no records of its
own in this contract; its attributes ride the runtime's records as resource
attributes, the only channel the runtime gives a launcher (it builds
resource attributes from `OTEL_RESOURCE_ATTRIBUTES` alone), which is why
niwa's identifiers are the one case of identifiers on a resource.

## Names

Every attribute the contract defines. "Class" is the cardinality class: a
dimension stays under 500 distinct values in any 28 days and may be grouped
by; an identifier never is. Owner `runtime-key` marks the runtime's own join
key, which the contract carries but doesn't own or set.

| Name | Owner | Type | Requiredness | Stability | Class | Record kinds | Meaning |
|---|---|---|---|---|---|---|---|
| `session.id` | runtime-key | string | conditional | development | identifier | all (when a runtime session exists) | The driving runtime session's id, as the runtime reports it (join key; the run journal calls it koto.driver.session.id) |
| `service.name` | koto | string | required | stable | dimension | resource | `koto`: the emitting tool |
| `service.version` | koto | string | required | stable | dimension | resource | koto's release version (the engine version) |
| `koto.contract.version` | koto | string | required | stable | dimension | resource | Contract version the record was emitted under |
| `koto.run.id` | koto | string | conditional | stable | identifier | all | koto root session id of the run; left out when koto can't resolve it (a session whose header predates session ids) |
| `koto.session.id` | koto | string | conditional | stable | identifier | all | koto session id (root or child); left out when koto can't resolve it (a session whose header predates session ids) |
| `koto.parent.session.id` | koto | string | conditional | stable | identifier | session_started | Parent koto session id of a child session |
| `koto.imported_from.session.id` | koto | string | conditional | stable | identifier | session_started | koto session id an imported session was copied from |
| `koto.template.name` | koto | string | required | stable | dimension | session_started | Template (workflow) name |
| `koto.template.hash` | koto | string | required | stable | dimension | session_started | Compiled template content hash |
| `koto.template.plugin.name` | koto | string | conditional | development | dimension | session_started | Plugin that shipped the template |
| `koto.template.plugin.version` | koto | string | conditional | development | dimension | session_started | That plugin's version |
| `koto.template.skill` | koto | string | conditional | development | dimension | session_started | Skill that ships the template, plugin:name |
| `koto.config.id` | koto | string | required | development | dimension | session_started | Digest of the configuration tuple |
| `koto.fixture` | koto | boolean | required | stable | dimension | session_started | True when the session is a test fixture |
| `koto.run.variant` | koto | string | optional | development | dimension | session_started | Value of the one template variable the template marks as its variant |
| `koto.state` | koto | string | required | stable | dimension | visit-scoped kinds | State name |
| `koto.seq` | koto | int | required | stable | identifier | all | Sequence number of the koto event the record relays (see Record envelope) |
| `koto.visit.seq` | koto | int | required | stable | identifier | visit-scoped kinds | Sequence number of the event that opened the visit |
| `koto.visit.duration_ms` | koto | int | required | development | identifier | state_exited | Milliseconds from the visit's start to its end |
| `koto.visit.exit` | koto | string | required | development | dimension | state_exited | How the visit ended: transition, rewind or cancel |
| `koto.terminal` | koto | string | required | stable | dimension | terminal | Terminal state name |
| `koto.run.result` | koto | string | required | development | dimension | terminal,cancelled | success, blocked, refused, error, unclassified, cancelled |
| `koto.check.name` | koto | string | required | stable | dimension | check kinds, decider_checked, decider_fallback, decision_recorded | Gate name, or `__action__` (the run journal calls it koto.check) |
| `koto.check.outcome` | koto | string | required | stable | dimension | check_evaluated | passed, failed, timed_out, error |
| `koto.attempt` | koto | int | required | stable | identifier | check_evaluated, check_rule, check_finding, check_disputed | Attempt number of a state's checks, counted over every visit to that state in the session |
| `koto.visit_attempt` | koto | int | required | stable | identifier | check_evaluated, check_rule, check_finding, check_disputed | Attempt number within the current visit; back to 1 when a new visit starts |
| `koto.check.rule_count` | koto | int | required | stable | identifier | check_evaluated | Distinct rule ids at error (capped at 50) |
| `koto.check.rule_counts_truncated` | koto | boolean | optional | stable | identifier | check_evaluated | True when more than 50 rule ids |
| `koto.check.finding_count` | koto | int | required | stable | identifier | check_evaluated | Findings logged, every level |
| `koto.check.findings_truncated` | koto | boolean | optional | stable | identifier | check_evaluated | True when more than 50 findings |
| `koto.check.duration_ms` | koto | int | conditional | stable | identifier | check_evaluated | Wall-clock ms of the check's command |
| `koto.check.unanswered_count` | koto | int | conditional | stable | identifier | check_evaluated | Decider-check veto criteria with no verdict |
| `error.type` | koto | string | conditional | stable | dimension | check_evaluated | Error code of a decider check's output.error (the run journal calls it koto.check.error) |
| `koto.rule.id` | koto | string | conditional | stable | dimension | check_rule, check_finding, check_disputed, decider_checked, decider_fallback, decision_recorded | Rule id or its digest; required on every kind it appears on except decision_recorded, where only a check_answer carries it |
| `koto.rule.visit_count` | koto | int | required | stable | identifier | check_rule | Times the rule failed in this visit |
| `koto.rule.session_count` | koto | int | required | stable | identifier | check_rule | Times the rule failed in the session |
| `koto.finding.level` | koto | string | required | stable | dimension | check_finding | error, warning, info |
| `koto.finding.count` | koto | int | required | stable | identifier | check_finding | Findings with that rule id and level |
| `koto.dispute.source` | koto | string | required | stable | dimension | check_disputed | agent or person |
| `koto.poll.status` | koto | string | conditional | stable | dimension | check_evaluated, check_pending | pending, done, failed, timed_out; always on check_pending, and on check_evaluated only for a polling gate |
| `koto.poll.evaluations` | koto | int | required | stable | identifier | check_pending | Command runs since the poll window opened |
| `koto.poll.elapsed_secs` | koto | int | required | stable | identifier | check_pending | Seconds since the poll window opened |
| `koto.decider.awaiting_count` | koto | int | conditional | stable | identifier | check_evaluated, check_pending | Criteria waiting on the agent's answer |
| `koto.decider.agent_answered_count` | koto | int | conditional | stable | identifier | check_evaluated, check_pending | Criteria decided from an agent answer |
| `gen_ai.provider.name` | koto | string | required | development | dimension | decider_consulted, decider_checked | Decider provider (the run journal calls it koto.decider.provider) |
| `gen_ai.response.model` | koto | string | required | development | dimension | decider_consulted, decider_checked | Model build the provider reported (the run journal calls it koto.decider.model) |
| `gen_ai.usage.input_tokens` | koto | int | conditional | development | identifier | decider_consulted, decider_checked | Decider input tokens (the run journal calls it koto.decider.input_tokens) |
| `gen_ai.usage.output_tokens` | koto | int | conditional | development | identifier | decider_consulted, decider_checked | Decider output tokens (the run journal calls it koto.decider.output_tokens) |
| `koto.decider.outcome` | koto | string | required | stable | dimension | decider_consulted, decider_checked | Outcome of a consultation or criterion |
| `koto.decider.mode` | koto | string | conditional | stable | dimension | decider_consulted, decider_field, decider_checked, decider_fallback | veto, shadow |
| `koto.decider.reason` | koto | string | conditional | stable | dimension | decider_consulted, decider_checked, decider_fallback, decision_recorded | Why a criterion went unanswered |
| `koto.decider.error_class` | koto | string | conditional | stable | dimension | decider_consulted, decider_checked | How a decider call failed |
| `koto.decider.blocked` | koto | boolean | conditional | stable | identifier | decider_checked | True when the criterion blocked the state |
| `koto.decider.unread_usage_attempts` | koto | int | conditional | stable | identifier | decider_consulted, decider_checked | Billed answers whose usage couldn't be read |
| `koto.decider.attempts` | koto | int | conditional | stable | identifier | decider_checked | Provider attempts |
| `koto.decider.latency_ms` | koto | int | conditional | stable | identifier | decider_consulted, decider_checked | Milliseconds in provider calls |
| `koto.decider.directive_bytes` | koto | int | conditional | stable | identifier | decider_consulted | Bytes the agent was spared by an applied consultation |
| `koto.decider.input_bytes` | koto | int | conditional | stable | identifier | decider_consulted, decider_checked, decider_fallback | Bytes of the graded slice |
| `koto.decider.field_count` | koto | int | conditional | stable | identifier | decider_consulted | Fields a routing consultation asked about |
| `koto.decider.field` | koto | string | conditional | stable | dimension | decider_consulted, decider_field, decider_answered | Field name or digest |
| `koto.decider.answer` | koto | string | conditional | stable | dimension | decider_consulted, decider_field, decision_recorded | What the decider (or agent) answered |
| `koto.decider.threshold_met` | koto | boolean | conditional | stable | identifier | decider_consulted, decider_field | Winning value met its threshold |
| `koto.decider.input_label` | koto | string | conditional | stable | dimension | decider_consulted | Label of the unavailable input |
| `koto.decider.consulted_seq` | koto | int | conditional | stable | identifier | decider_field, decider_answered | koto.seq of the routing consultation |
| `koto.decider.agent_answer` | koto | string | conditional | stable | dimension | decider_answered | Agent's value for an undecided field |
| `koto.decider.fallback_seq` | koto | int | conditional | stable | identifier | decision_recorded | koto.seq of the fallback record answered |
| `koto.response.source` | koto | string | required | stable | dimension | response | next or status |
| `koto.response.directive_bytes` | koto | int | required | stable | identifier | response | UTF-8 bytes of the directive |
| `koto.response.details_bytes` | koto | int | required | stable | identifier | response | UTF-8 bytes of the details, 0 when none |
| `koto.decision.id` | koto | string | required | development | identifier | decision_recorded | Decision identity: the source event's koto.session.id and koto.seq joined by ':' |
| `koto.decision.kind` | koto | string | required | development | dimension | decision_recorded | Kind of decision, an open vocabulary the decision record owns |
| `koto.decision.actor.kind` | koto | string | required | development | dimension | decision_recorded | agent, coordinator, person |
| `koto.decision.actor.session.id` | koto | string | conditional | development | identifier | decision_recorded | Runtime session id of the deciding session |
| `koto.decision.authority` | koto | string | conditional | development | dimension | decision_recorded | Authority level the decider acted under; absent until the decision record supplies it |
| `koto.decision.defaulted` | koto | boolean | required | development | dimension | decision_recorded | True when no decider was named and the observed session was recorded |
| `koto.decision.reverses` | koto | string | conditional | development | identifier | decision_recorded | koto.decision.id this decision reverses; absent when it reverses nothing |
| `vcs.change.id` | koto | string | required | development | identifier | change_linked | Pull request number |
| `vcs.repository.url.full` | koto | string | required | development | identifier | change_linked | Repository URL, no credentials |
| `vcs.ref.head.revision` | koto | string | required | development | identifier | change_linked | Commit the run linked |
| `niwa.dispatch.id` | niwa | string | required | stable | identifier | resource (runtime records) | Dispatch identity, the run identity of a dispatched run |
| `niwa.dispatch.slug` | niwa | string | required | stable | identifier | resource (runtime records) | Dispatch slug |
| `niwa.parent.session.id` | niwa | string | required | stable | identifier | resource (runtime records) | Runtime session id of the dispatching coordinator |
| `niwa.requested.skill` | niwa | string | conditional | stable | dimension | resource (runtime records) | Skill the brief asks the worker to run |
| `niwa.brief.id` | niwa | string | conditional | stable | identifier | resource (runtime records) | Digest of the brief |
| `niwa.workspace` | niwa | string | required | stable | dimension | resource (runtime records) | Workspace configuration name |
| `niwa.authority.level` | niwa | string | conditional | development | dimension | resource (runtime records) | Authority level the worker was dispatched with |
| `niwa.authority.grant` | niwa | string | conditional | development | dimension | resource (runtime records) | Grant the worker was dispatched with |
| `niwa.contract.version` | niwa | string | required | stable | dimension | resource | Contract version niwa's attributes follow |
| `koto.leg.request.id` | koto | string | conditional | development | identifier | session_started | koto request id the session reports its result to |
| `koto.leg.name` | koto | string | conditional | development | dimension | session_started | Leg name within that request |

## Value rules

Ids are strings matching `^[A-Za-z0-9._:-]{1,128}$`. Integers are
non-negative and below 2^53. Version strings (`service.version`,
`koto.template.plugin.version`, the contract versions) are semantic
versions. Names a template author or a plugin declares (`koto.state`,
`koto.terminal`, `koto.check.name`, `koto.decider.field`,
`koto.decider.input_label`, `koto.leg.name`, `koto.template.name`,
`koto.template.plugin.name`, `koto.template.skill`), and values from a
vocabulary the contract or the decision record fixes (`koto.decision.kind`,
`koto.decision.authority`, `koto.run.result`, `koto.visit.exit`, and
`error.type`, which takes koto's error codes), match
`^[A-Za-z0-9._:/-]{1,128}$`, not starting with `/` or `~`, or travel as
`sha256:` plus 32 hex characters (the contract's shape drops the `@` the run
journal allows, so an email address can't pass). Values an agent or a model
supplies never pass through as text: `koto.decider.answer` and
`koto.decider.agent_answer` are one of the field's declared values, its
digest, or `other`, and `koto.run.variant` is one of the variable's declared
values or `other`. `koto.rule.id` comes from check output: in the shape it
passes unchanged, outside it, or when it matches a credential pattern koto's
redactor knows, it travels as its digest. `vcs.repository.url.full` is
`https://host/path` with no user information, query or fragment. niwa's
values follow the same rules, and niwa refuses to compose a resource value
whose own entries contain `,` or `=`. No value holds a secret, prompt or
file content, a command line, an absolute path, or a person's account
identity, to the extent the shape and redaction checks above can tell (see
[Privacy and residual risks](#privacy-and-residual-risks) for what they
can't).

## Runs and identity

A run's identity is its dispatch id when niwa dispatched it and its koto
root session id otherwise. koto records carry `koto.run.id` (the root
session); a reader finds the dispatch id on the runtime's records with the
same `session.id`. Edge cases:

| Case | What a backend holds |
|---|---|
| koto started from a plain shell, no runtime session | koto records with no `session.id`; nothing joins them to a runtime session, and nothing is guessed. |
| Runtime session not dispatched by niwa | runtime records with no `niwa.*`; the run has no parent. |
| Run cancelled | `koto.state_exited` with `koto.visit.exit` = `cancel`, then `koto.cancelled` with `koto.run.result` = `cancelled`. |
| Run with no `koto.terminal` and no `koto.cancelled` | the run is open, or was abandoned mid-run; a reader can't tell the two apart from these records alone. |
| Plugin or skill version unknown | `koto.template.plugin.version` (or `.name`, or `koto.template.skill`) absent; `koto.config.id` computed with that value empty. |
| Template declares no terminal classes | `koto.run.result` = `unclassified`. |

## Gates and attempts

A conclusion (`passed`, `failed`, `timed_out`, `error`) is a
`koto.check_evaluated` with `koto.attempt` and `koto.visit_attempt`. A poll
that hasn't settled is a `koto.check_pending` with no attempt counter. A
poll, poll, fail, pass sequence is two pending records and two evaluations,
attempts 1 and 2. Rules violated are the `koto.check_rule` records after a
failing evaluation.

## Decisions

`koto.decision_recorded` carries `koto.decision.id`, `koto.decision.kind`,
`koto.decision.actor.kind` (`agent`, `coordinator`, `person`),
`koto.decision.actor.session.id`, `koto.decision.authority`,
`koto.decision.defaulted` and `koto.decision.reverses`. Kinds are an open
vocabulary that koto's decision record owns (see the
[decision record design](../designs/DESIGN-decision-record.md)). This
version maps two kinds, from facts koto already records:

| Source fact | `koto.decision.kind` | Actor kind | Actor session | Defaulted | Also carries |
|---|---|---|---|---|---|
| A gate override (the journal's `check_overridden`) | `gate_override` | `agent` | the driving session | `true` (overrides name no actor today) | `koto.check.name` |
| An agent's answer to a decider-check criterion (the journal's `decider_check_answered`) | `check_answer` | `agent` | the driving session | `false` | `koto.check.name`, `koto.rule.id`, `koto.decider.answer`, `koto.decider.reason`, `koto.decider.fallback_seq` |

For both, `koto.decision.id` is the source event's `koto.session.id` and
`koto.seq` joined by `:`, `koto.decision.reverses` is absent (neither
reverses anything), and `koto.decision.authority` is absent: koto can't read
the authority a session was dispatched with, so a reader takes
`niwa.authority.level` from the runtime records of the actor's session until
the decision record supplies it.

A dispute stays `koto.check_disputed`: it contests a finding and decides
nothing; the decision that upholds or rejects it is a later
`koto.decision_recorded`. When the decision record records other kinds
(accepting a document, merging, passing a gate, escalating or declining to,
a judgment call, a reversal) as session keys, koto emits one record per key
write with that record's values.

## Changes and configuration

`koto.change_linked` carries `vcs.change.id`, `vcs.repository.url.full` and
`vcs.ref.head.revision`, written each time the run records that it opened or
pushed a change.

A run's configuration is the tuple (koto engine version, template name,
template hash, template plugin name, template plugin version, template
skill). The engine version rides as `service.version` on koto's resource,
the rest as `koto.template.*` on the run's `koto.session_started`.
`koto.config.id` is `sha256:` plus the first 32 hex characters of the
SHA-256 of the tuple's values joined by `\n` in that order, with absent
values as empty strings. The tuple is normative and the digest is a
convenience for grouping. A component koto can't establish (a template run
from a developer's checkout, with no plugin version) is left out rather than
guessed, and the digest still covers the rest.

A document is traced to its configuration in three hops: the document is in
a change, the change's `koto.change_linked` names the run, and the run's
`koto.session_started` holds the tuple, with the engine version on its
resource.

## Versioning

The contract's machine-readable history is an OpenTelemetry telemetry-schema
file in `schemas/signals/`, one file per version named by the version, in
file format 1.1.0, whose `versions`
section lists each version's `rename_attributes` so standard tooling (such
as the collector's schema processor) can apply a rename. This draft's file
is [`schemas/signals/0.1.0`](../../schemas/signals/0.1.0): one version, no
renames.

The files carry no extension because the
[OpenTelemetry schema specification](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/schemas/README.md)
makes the last segment of a schema URL the schema's version, and the URL
must retrieve the file. OpenTelemetry's own semantic-conventions schema
files are stored the same way.

Each koto record's instrumentation scope is `koto`, with `schema_url` set to
the file's raw URL. This draft's URL is
`https://raw.githubusercontent.com/tsukumogami/koto/main/schemas/signals/0.1.0`;
an emitter stamps the same path at its own release tag
(`https://raw.githubusercontent.com/tsukumogami/koto/<tag>/schemas/signals/<version>`).
Not every backend surfaces `schema_url`, so the version also rides as
`koto.contract.version` on koto's resource and `niwa.contract.version` on
niwa's composed resource value. The semantic conventions version the core
words are pinned to (1.37.0) is stated here and on koto's resource
`schema_url`, which covers the `service.*` words that live there.

Adding a name is a minor version. Renaming or removing a `stable` name is a
major version, and the emitter then sends both names for at least one minor
release. A `development` name may change in a minor version, recorded in the
schema file's history. A name adopted upstream into the OpenTelemetry
conventions lands here first as a minor-version addition and, after the
release that sends both names, as a major-version rename.

## What the runtime already reports

The agent runtime that drives a session (Claude Code today) already reports,
on its own records, the session (`session.id`), each prompt (`prompt.id`),
subagents (`agent_id`, `parent_agent_id`), skill and plugin use, tool calls
and whether each was accepted, hook runs, the model, token counts, cost and
request durations, and account and environment attributes. The contract
names none of these. Every koto record carries the runtime's `session.id`
when a runtime session drives it, and trace context when the runtime passes
one, so a backend joins the two; the cost, tokens and time of a run, a phase
or a gate come from the runtime's records over that span, not from a copy in
koto's. The contract also stays out of the attribute families the runtime
claims for itself: `workflow.`, `user.`, `app.`, `organization.`, `event.`
and `claude_code.`.

## Conventions used

Pinned to semantic conventions 1.37.0 for the core words. GenAI words are at
Development stability and are tracked in their own repository.

| Contract concept | Convention exists | Name used | Note |
|---|---|---|---|
| Driving session | yes | `session.id` | The runtime's own key; carried as the join, not repeated. |
| Emitting tool and engine version | yes | `service.name`, `service.version` | |
| Contract version history | yes | scope `schema_url` | Plus `koto.contract.version`, because not every backend surfaces `schema_url`. |
| Run | partly | `koto.run.id` | `cicd.pipeline.run.id` names a CI run; `gen_ai.conversation.id` a message thread. Neither is a workflow run. |
| Workflow name | partly | `koto.template.name` | GenAI's `invoke_workflow` operation names a workflow, at Development stability; its workflow-name attribute is adopted in a minor version, as an addition, once settled. |
| Configuration version | partly | `koto.template.*`, `koto.config.id` | `gen_ai.agent.version` exists in newer GenAI text for an agent, not a template; no word for a template hash. |
| Phase (visit) | no | `koto.state`, `koto.visit.*` | The GenAI conventions' durable-runtime discussion names none yet. |
| Gate conclusion | partly | `koto.check.outcome` | `test.case.result.status` is `pass`/`fail` for unit tests; `cicd.pipeline.result` is a CI result with different values. Neither has timed_out and error as conclusions. |
| Attempt | no | `koto.attempt`, `koto.visit_attempt` | |
| Rule violated | no | `koto.rule.id` | |
| Decision and actor | no | `koto.decision.*` | The runtime's `tool_decision` covers tool-call acceptance only. |
| Dispatching coordinator | no | `niwa.parent.session.id`, `niwa.dispatch.id` | `session.previous_id` means a prior session of the same user, not a parent. |
| Run end | partly | `koto.terminal`, `koto.run.result` | `cicd.pipeline.result` values (`success`, `failure`, `error`, `timeout`, `cancellation`, `skip`) don't hold `blocked` or `refused`. |
| Decider model call | yes | `gen_ai.provider.name`, `gen_ai.response.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens` | koto's own model calls; the runtime doesn't report them. |
| Decider-check error code | yes | `error.type` | Stable. |
| Change produced | yes | `vcs.change.id`, `vcs.repository.url.full`, `vcs.ref.head.revision` | Used in the convention's meaning. The branch name (`vcs.ref.head.name`) is left out: it can carry a person's or customer's name. The runtime may set `vcs.` attributes of its own about a session's repository; these three sit only on koto's `koto.change_linked` records and describe the change a run produced, so no record has two writers for one key. |

## Privacy and residual risks

**What a record can hold.** Every attribute is an allowlisted copy of a
content-free value: names in a fixed shape, integers, booleans, digests, and
a repository URL with no user information. Prompts, file content, command
lines, check output, rationales, paths and context keys never enter a
record. A field koto adds to an internal event later can't leak, because
each record kind copies named fields.

**Shape checks are not secrecy checks.** A shape check proves a value is a
name, not that it's harmless: a token can look like a name. So the contract
lets no agent- or model-supplied text through at all (answers and variants
are declared values or `other`), drops `@` from its name shape, and leaves
out the branch name, which can carry a person's handle or a customer's name.

**Rule ids.** A rule id comes from a check's output. In the name shape it
passes unchanged; a value matching a credential pattern koto knows is
digested. Two risks remain: a check that prints an unknown secret in the
name shape would still send it, and a short value's digest can be reversed
by guessing.

**Person identity.** The runtime puts account identifiers and an email on
every one of its records. The contract adds none: a person who decides
appears as `koto.decision.actor.kind` = `person` and the session they acted
through. But `session.id` joins koto's records to the runtime's, so in a
backend that holds both, a decision or a dispute can be tied to a person's
account. That's the point of the join for a team reading its own data. A
team forwarding the records to a third party should drop or hash
`session.id` and the runtime's `user.*` attributes in its collector first.

**Repository names.** `vcs.repository.url.full` names a repository, which
for a private repository is itself sensitive. It goes only to the team's
own receiver; a team forwarding to a hosted service decides whether that
service may hold it.

**Opt-in export.** Export needs koto's own switch, not just an ambient
`OTEL_EXPORTER_OTLP_*` variable, so an endpoint left in a developer's shell
for another tool receives nothing.

**Resource and composition.** koto's resource holds exactly its three
attributes, with SDK resource detectors off, so no process owner, command
line or path rides along. niwa refuses to compose an
`OTEL_RESOURCE_ATTRIBUTES` value from an entry containing `,` or `=`, which
would otherwise let a slug or workspace name inject `service.name` or
`user.*`.

**Forged records.** Any process running as the user can emit records that
look like koto's. Records are measurement, not evidence: nothing in koto or
niwa reads them back to decide anything.

**Schema URL.** A collector that applies the schema file fetches the
`schema_url` a record names. A team should allow only koto's published
schema location in its collector.

## Worked example

A coordinator dispatches a worker to run `/work-on` on one issue. The
worker's runtime session is `s-w`, the coordinator's `s-c`, the dispatch
`d-1`, the koto root session `k-1`. The `pr_body_valid` gate fails twice
before passing (two retries), and `ci_green` polls, polls, fails, then
passes. Values are illustrative and times are omitted; every required
attribute is shown, and conditional ones where they apply
(`koto.poll.status` only on the polling gate).

Runtime records for `s-w` (from Claude Code, not koto) carry, on every
record, `session.id` = `s-w` and the resource attributes `niwa.dispatch.id` =
`d-1`, `niwa.dispatch.slug` = `fix-issue-42`, `niwa.parent.session.id` =
`s-c`, `niwa.requested.skill` = `shirabe:work-on`, `niwa.workspace` =
`acme`, `niwa.contract.version` = `0.1.0`. Every koto record below carries
the resource `service.name` = `koto`, `service.version` = `0.25.0`,
`koto.contract.version` = `0.1.0`, and the common `session.id` = `s-w`,
`koto.run.id` = `k-1`, `koto.session.id` = `k-1`, plus the `koto.seq` in
its row. Visit-scoped records also carry the `koto.state` and
`koto.visit.seq` in their row.

| # | Event | `koto.seq` | `koto.state` / `koto.visit.seq` | Other attributes |
|---|---|---|---|---|
| 1 | `koto.session_started` | 1 | | `koto.template.name`=`work-on`, `koto.template.hash`=`h-7`, `koto.template.plugin.name`=`shirabe`, `koto.template.plugin.version`=`0.35.0`, `koto.template.skill`=`shirabe:work-on`, `koto.config.id`=`sha256:3b1f...`, `koto.fixture`=`false` |
| 2 | `koto.state_entered` | 2 | `analysis` / 2 | |
| 3 | `koto.response` | 2 | `analysis` / 2 | `koto.response.source`=`next`, `koto.response.directive_bytes`=1840, `koto.response.details_bytes`=6120 |
| 4 | `koto.state_exited` | 3 | `analysis` / 2 | `koto.visit.duration_ms`=412000, `koto.visit.exit`=`transition` |
| 5 | `koto.state_entered` | 3 | `pr_creation` / 3 | |
| 6 | `koto.check_evaluated` | 4 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.check.outcome`=`failed`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.check.rule_count`=1, `koto.check.finding_count`=1, `koto.check.duration_ms`=820 |
| 7 | `koto.check_rule` | 4 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.rule.id`=`pr-body/no-heading-in-part1`, `koto.rule.visit_count`=1, `koto.rule.session_count`=1 |
| 8 | `koto.check_finding` | 4 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.rule.id`=`pr-body/no-heading-in-part1`, `koto.finding.level`=`error`, `koto.finding.count`=1 |
| 9 | `koto.check_evaluated` | 5 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.check.outcome`=`failed`, `koto.attempt`=2, `koto.visit_attempt`=2, `koto.check.rule_count`=1, `koto.check.finding_count`=1, `koto.check.duration_ms`=790 |
| 10 | `koto.check_rule` | 5 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.attempt`=2, `koto.visit_attempt`=2, `koto.rule.id`=`pr-body/no-heading-in-part1`, `koto.rule.visit_count`=2, `koto.rule.session_count`=2 |
| 11 | `koto.check_finding` | 5 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.attempt`=2, `koto.visit_attempt`=2, `koto.rule.id`=`pr-body/no-heading-in-part1`, `koto.finding.level`=`error`, `koto.finding.count`=1 |
| 12 | `koto.check_evaluated` | 6 | `pr_creation` / 3 | `koto.check.name`=`pr_body_valid`, `koto.check.outcome`=`passed`, `koto.attempt`=3, `koto.visit_attempt`=3, `koto.check.rule_count`=0, `koto.check.finding_count`=0, `koto.check.duration_ms`=805 |
| 13 | `koto.change_linked` | 7 | | `vcs.change.id`=`1234`, `vcs.repository.url.full`=`https://github.com/acme/app`, `vcs.ref.head.revision`=`9f2c41d...` |
| 14 | `koto.state_exited` | 8 | `pr_creation` / 3 | `koto.visit.duration_ms`=655000, `koto.visit.exit`=`transition` |
| 15 | `koto.state_entered` | 8 | `ci_monitor` / 8 | |
| 16 | `koto.check_pending` | 9 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.poll.status`=`pending`, `koto.poll.evaluations`=1, `koto.poll.elapsed_secs`=30 |
| 17 | `koto.check_pending` | 10 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.poll.status`=`pending`, `koto.poll.evaluations`=2, `koto.poll.elapsed_secs`=90 |
| 18 | `koto.check_evaluated` | 11 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.check.outcome`=`failed`, `koto.poll.status`=`failed`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.check.rule_count`=1, `koto.check.finding_count`=1, `koto.check.duration_ms`=2100 |
| 19 | `koto.check_rule` | 11 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.rule.id`=`ci_green`, `koto.rule.visit_count`=1, `koto.rule.session_count`=1 |
| 20 | `koto.check_finding` | 11 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.attempt`=1, `koto.visit_attempt`=1, `koto.rule.id`=`ci_green`, `koto.finding.level`=`error`, `koto.finding.count`=1 |
| 21 | `koto.check_evaluated` | 12 | `ci_monitor` / 8 | `koto.check.name`=`ci_green`, `koto.check.outcome`=`passed`, `koto.poll.status`=`done`, `koto.attempt`=2, `koto.visit_attempt`=2, `koto.check.rule_count`=0, `koto.check.finding_count`=0, `koto.check.duration_ms`=1900 |
| 22 | `koto.state_exited` | 13 | `ci_monitor` / 8 | `koto.visit.duration_ms`=1260000, `koto.visit.exit`=`transition` |
| 23 | `koto.state_entered` | 13 | `done` / 13 | |
| 24 | `koto.terminal` | 13 | `done` / 13 | `koto.terminal`=`done`, `koto.run.result`=`success` |

The CI failure carries the gate's own name as its rule id, koto's fallback
finding when a check reports none. `koto.attempt` counts a state's attempts
across the session, so `ci_green` in `ci_monitor` starts again at 1. The
`done` visit has no `koto.state_exited`: a terminal state's visit ends with
the run, and `koto.terminal` marks it.

From these records a reader can say that the run ran `work-on` at template
`h-7` from shirabe 0.35.0 on koto 0.25.0, that `pr_body_valid` took three
attempts (two retries), each failure violating
`pr-body/no-heading-in-part1`, that `ci_green` took two attempts after two
polls, and that the run ended `success`.

**From a document to its configuration.** A document committed in pull
request 1234 of `acme/app`:

1. The change is `vcs.change.id` = `1234` with `vcs.repository.url.full` =
   `https://github.com/acme/app` on row 13, a `koto.change_linked` record.
2. Row 13's `koto.run.id` = `k-1` and `session.id` = `s-w` name the run; the
   runtime records with `session.id` = `s-w` carry `niwa.dispatch.id` =
   `d-1` (the run identity) and `niwa.parent.session.id` = `s-c` (the
   coordinator).
3. The record of that run with event name `koto.session_started` (row 1)
   gives `koto.template.name` = `work-on`, `koto.template.hash` = `h-7`,
   `koto.template.plugin.name` = `shirabe`, `koto.template.plugin.version`
   = `0.35.0` and `koto.template.skill` = `shirabe:work-on`.
4. Its resource `service.version` = `0.25.0` gives the koto engine version.
