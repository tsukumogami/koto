# Changelog

All notable changes to the koto crate are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project's pre-1.0 versioning treats MINOR as MAJOR per the Cargo
0.x semver convention (`0.10.0` is breaking-change-eligible relative
to `0.9.x`).

## [Unreleased]

### Compatibility

- **The first release after v0.24.0 is the
  first release that honours pins in full.** Every host that shares a bucket
  needs it. koto v0.24.0 has pins
  on the local backend only, with the commands hidden: under the cloud
  backend it refuses to pin, and its removals read only their own host's
  copy, so it doesn't keep a session pinned in the bucket. koto v0.23.0
  reads a pin as a malformed link,
  which keeps the session's hierarchy, so its `koto session prune`,
  `koto workspace prune`, terminal tick and same-name replace keep a session
  pinned in a local store; its `koto session cleanup`, `koto cancel --cleanup`
  and `koto session import` don't honour pins and remove a pinned session or
  drop its pin. Under the cloud backend a pin is a bucket object older
  releases never read. Releases before v0.23.0 ignore pins entirely.
- **Pin and unpin write into a session another agent session or host owns.**
  They are a named exception to the rule that a koto command writes only into
  the session it opens, ticks, links or imports, and write only the pin and
  its release record, with no log event. They are listed with koto's other
  cross-session writers in
  [koto#445](https://github.com/tsukumogami/koto/issues/445).
- **`koto session import` keeps its source as a read-only predecessor.
  v0.32.0 is the first release that honours the relation it leaves.** The
  copy gets `koto/links/continues/<source>` and the source a back-reference,
  both written before the copy is pushed, and the migration marker follows
  the push. In the source's own workspace, `koto next`, `koto context add` and
  `remove`, `rewind`, `cancel`, `decisions record`, `overrides record`,
  `session link`, `session rebind`, `resolve`, `update`, `pin` and `unpin`
  still exit 2 with `session_migrated`; `koto status`, `koto context get`,
  `exists` and `list`, `koto decisions list` and `koto overrides list` now
  exit 0 and answer from its last pushed bucket copy, changing nothing. A
  re-run on a copy that holds a relation naming another session exits 2 with
  `relation_conflict`. `koto session prune --migrated-from` removes the
  marker and source only once the copy and any later member have no standing
  (reason `released`), so `session.cloud.marker_retain_days` and
  `--source-retired` are accepted but ignored; a run that uses either lists it
  under `ignored`. Older releases fall short in three ways: their local prunes
  over-keep a chain; under the cloud backend they can delete a predecessor
  whose successor they never pulled; and their import still retires its
  source.

- **Anchorless context reads are no longer written to the read session's
  log.** `koto context get` and `koto context exists` run with no reader
  anchor record their `context_read` in the host-local read spool,
  `context-reads.jsonl` beside the session's local copy, which never leaves
  the host. A reader that names another agent session's anchor records it in
  that anchor's log instead. A tool that counted `context_read` events in a
  session's own log should read the run journal's `context_reads` records,
  which fold in spooled reads, or the spool through
  `koto::session::read_spool::read`.
- **The koto-skills plugin now registers a SessionStart hook.**
  `hooks/session-start.sh` runs at every agent session start, resume,
  compaction and `/clear`. With a koto older than the anchor verbs it opens
  no anchor, and its block in `CLAUDE_ENV_FILE` unsets `KOTO_ANCHOR` and
  `KOTO_DISPATCHER_ANCHOR`. Without koto or `jq` it does nothing. It always
  exits 0. It runs koto with the `KOTO_SESSION_*` variables removed, so a
  cloud backend chosen only through those variables, and not in a config
  file, gets no anchor its shells can see; set the backend in a config file.

### Added

- **Rules a directive carries, recorded as telemetry.** A state's new
  `directive_rules` list names the rules its directive hands the agent
  (`rule_id`, `check`, `extent` and `when_set`). Each time `koto next`
  prints that directive, it appends one `rule_delivered` event per entry
  whose `when_set` variable or capture is non-empty, after the response
  prints, with the new `via` value `directive` and `delivery: printed`.
  `koto status` records nothing, and the response is unchanged. The
  compiler checks each entry (`E-DIRECTIVE-RULE-*`).
- **`capture_allow_empty` on a `default_action`.** Beside
  `capture_stdout_as`, it delivers an empty output as the empty string
  instead of failing the tick, so a directive that interpolates it renders
  nothing. Without it an empty output fails as before. A template that uses
  neither new field compiles to the same bytes and hash; one that uses
  either needs this koto.
- **Check and evidence telemetry in the run journal.** `check_evaluated`
  gains `koto.check.reason` (why a check didn't pass: `rule_failed`,
  `undecided`, `no_finding`, `timed_out`, `spawn_failed` or `error`),
  `koto.check.not_ready` and `koto.check.pre_directive`, and
  `check_finding` gains `koto.finding.source` (`check` or `fallback`). A
  check says "not ready" by printing a line that starts with
  `::koto-not-ready::`, or, on a command gate, by exiting with a code the
  new optional `not_ready_exit_codes` field lists; it is telemetry only and
  changes no outcome, route or fallback finding. A new `evidence_field`
  record carries each enum or boolean field of the agent's evidence (a
  declared value, its digest, `other`, or `true`/`false`), never string or
  number values. In the session log, `instructions_delivered` is now
  appended for every printed directive with an additive
  `details_included`, which details suppression reads, and
  `evidence_submitted` gains an additive `exported_fields` map.
- **Command checks report rule deliveries.** A command gate's check that
  prints a line starting with `::koto-rule-delivered::` and holding a JSON
  object with exactly `rule_id`, `extent` (`summary` or `full`) and
  `delivery` (`printed` or `refused`) has it kept on `gate_evaluated` as
  `rules_delivered`, and `koto next` appends a `rule_delivered` event per
  line after its response prints. The event and its run-journal record gain
  `via` (`check_output`) and `delivery`, as `koto.rule.via` and
  `koto.rule.delivery`; a decider check's delivery carries neither.
  Malformed lines are ordinary output, and nothing routes on the record.
- **`koto review record` records document reviews as telemetry.** Four
  subcommands, `round-started`, `verdicts --file`, `decision` and
  `doc-checks --file`, append `review_round_started`,
  `review_verdict_recorded` (one per entry), `review_decision_recorded` and
  `doc_check_recorded` (one per entry) to a non-terminal session, and the
  run journal relays them as `review_round`, `review_verdict`,
  `review_decision` and `doc_checked`. Every value is checked first and
  stored in koto's words: closed sets for document type, verdict, decision
  and check outcome, a schema bucket, validated rule ids, positive counts,
  and seats and sections the template declares in the new optional
  `review_vocabulary` field (anything else is `other`). A batch holds at
  most 200 entries and a session 2,000 review events; one invalid entry
  refuses the whole batch, and errors never repeat a value the caller gave.
  Nothing routes on the events.
- **`koto review record` records a document's drafting as telemetry.**
  Three more subcommands: `rule-checks --doc-format --moment --file` (each
  check-held rule's outcome, `pass`, `fail`, `not_ready` or `error`, at
  `pre_jury`, `post_fix` or `post_create`), `section-written --doc-format
  --section --write first|rewrite`, and `fix --doc-format --answers
  jury_fail|check_fail [--round 1-50] --file` (entries of rule, section and
  the check rule that decides it, or `none`). Each call appends one event,
  `doc_rule_checks_recorded`, `doc_section_recorded` or `doc_fix_recorded`,
  and the run journal relays one `doc_rule_checked`, `doc_section_written`
  or `doc_fix_applied` record per entry, every record of a call under its
  `koto.seq` and carrying the session's state and `koto.doc.format`.
  `round-started`, `verdicts` and `decision` take an optional
  `--doc-format`, which adds `koto.doc.format` and accepts `vision`,
  `strategy` and `roadmap`, for which `koto.doc.type` is left out. The
  session cap of 2,000 now counts review entries, one per entry of a batch
  call. Checks and refusals work as for the four review events.
- **`koto review record code-round` records a code review round as its
  own kinds (koto#537).** `code-round --round <n> --head <head> --file`
  takes a JSON array of seats, each `{"seat", "model", "verdict"}` with
  `verdict` `pass` or `fail`, at most 200, and appends one
  `code_review_round_recorded` event. The run journal relays one
  `code_review_round` record (`koto.code_review.round`,
  `koto.code_review.seat_count`, `koto.code_review.head`) and one
  `code_review_verdict` per seat (`koto.code_review.seat`,
  `koto.code_review.model`, `koto.code_review.verdict`), all under the
  call's `koto.seq` and carrying `koto.state`, so a pull request review
  panel is never counted among the document review kinds. The head is
  stored only as its digest, never as the commit id given. A seat the
  template's `review_vocabulary` doesn't declare is `other`, so is a model
  outside the journal's name shape (an id with a `[1m]` suffix, say), and
  a malformed batch (a missing seat or model, an unknown verdict) is
  refused whole with nothing written. The round and each seat count against the
  session's 2,000 review entries. The document review subcommands and
  their records are unchanged.
- **`KOTO_HOME` moves the whole koto root (koto#459).** Sessions, the
  request store, `config.toml`, the terminal index, coordinator cursors,
  wake files, the decider ledger, the run journal and the root
  `koto session prune` walks all resolve from one place: `KOTO_HOME` when
  it's set, `~/.koto` otherwise, so with it unset nothing moves. Under
  `KOTO_HOME` the compiled-template cache moves to `$KOTO_HOME/cache`
  too, and a root created by a session or a cache write gets mode 0700. A test or
  trial can isolate koto with that one variable instead of overriding
  `HOME`. `koto session dir` with no name prints the resolved root without
  reading config or writing anything, and gates and default actions get
  `KOTO_HOME` when the ticking koto has it set, so a nested koto uses the
  same root. `KOTO_SESSIONS_BASE` keeps its meaning. `clippy.toml` now
  disallows `dirs::home_dir` outside `src/koto_home.rs`, so a new path built
  from the home directory fails the lint check.
- **Pin a finished session so no removal takes it (koto#441).**
  `koto session pin <name> --reason <text>` keeps a session from the terminal
  tick, the parent sweep, the same-name replace, both prunes,
  `koto session cleanup` and `koto cancel --cleanup` until
  `koto session unpin <name>`. A pin keeps its own session only, and every
  removal that keeps one names it (`kept_by_pin`, `pin` with its `source`, or
  `pin_unreadable`). A call by hand may unpin; an agent session may unpin only
  when its start was handed `KOTO_AUTHORITY=release-pin`, which the
  koto-skills start hook records in its anchor. `koto session list --pinned`
  lists every pin, and `koto session hierarchy` shows a session's pin and its
  latest release record. Under the cloud backend the pin is the bucket's own
  object: every host sharing the bucket keeps the session, a removal that
  can't check the bucket keeps it and reports `bucket_unchecked`, and pin and
  unpin write nothing else. `koto session import` carries the pin and its
  release record. The [session retention guide](docs/guides/session-retention.md)
  has the details.
- **Read a session in another workspace without importing it (koto#455).**
  `koto --workspace <id|path> status <name>`, and the same option before
  `context get`, `context exists` and `context list`, read a session under
  another workspace's prefix in the cloud bucket, in place. The read writes
  nothing to the bucket or to this host's session store and logs no
  `context_read`. Every other verb refuses the option with
  `workspace_read_only`. A missing session or workspace is reported as
  `session_not_found` or `workspace_not_found` (exit 2), and a bucket that
  can't be read as `workspace_unreadable` (exit 3).
- **A `reserved_actions` retry carries its evidence as a field (koto#405).**
  Each entry has `evidence`, the `{"retry_failed": {...}}` object its
  `invocation` submits. An agent writes it to a file and submits it with
  `koto next <parent> --with-data @<file>`, with no shell string to take
  apart; that matters behind a token-stripping prefix, where a quoted
  argument is refused. `invocation` is unchanged. koto-skills' batch
  references now point at `evidence`, and `scripts/check-inline-evidence.sh`
  no longer lets an inline `"invocation":` line through.
- **Each agent session with the koto-skills plugin opens its own anchor.**
  The plugin's start hook runs `koto session anchor open` for
  `anchor-<session id>`, linked to the anchor it inherited, and exports
  `KOTO_ANCHOR` and `KOTO_DISPATCHER_ANCHOR`, both naming that anchor, into
  the session's shells through its block of `CLAUDE_ENV_FILE`. It falls back
  to the `KOTO_ANCHOR` export line as `additionalContext` when that file
  can't be written safely. A detached watcher records niwa's reap key as the
  observed marker when it appears in the session's env files, and copies a
  `/clear` predecessor's marker. So the runs an agent session starts,
  dispatches or reads stay readable while it lives, without the agent naming
  an anchor. A test bounds the hook's synchronous part (the open and the env
  file write) at a 95th percentile under 300 ms.

- **The run journal carries what a routing decider answered, and what the
  agent answered for the same field.** `decider_consulted` now records the
  field (`koto.decider.field`), the decider's answer (`koto.decider.answer`:
  the winning declared value, or `below_threshold`, `escape` or
  `no_answer`), the declared mode for it (`koto.decider.mode`), whether it
  met its threshold (`koto.decider.threshold_met`) and the field count. A
  consultation of several fields writes one new `decider_field` record per
  field instead. When the agent answers a visit the decider didn't settle, a
  new `decider_answered` record carries its value
  (`koto.decider.agent_answer`) and the consultation's `koto.seq`
  (`koto.decider.consulted_seq`), so agreement, coverage and the
  below-threshold rate can be read from the journal. `decider_check_answered`
  is now relayed too, with the check, the criterion, the agent's answer and
  the reason the criterion fell back. Every answer is a value the template
  declares or a word koto defines: an agent's value outside the declared
  values is written `other`, and no probability is copied.
- **An `input_unavailable` consultation says why.** The `decider_consulted`
  event, the ledger's `consulted` line and the journal record now carry the
  reason (`unset`, `over_budget` or `not_text`), the declared label of the
  input that stopped it, and its size in bytes. `koto decider report` counts
  each question's `input_unavailable` consultations by reason; a ledger
  written before the reason existed reports as it did.
- **The run journal records every finding a check logs, passing or
  failing.** Each check evaluation now adds one `check_finding` line per
  distinct rule id and level among the findings its event logged, with how
  many findings had that pair, the state, the check (as `koto.check.name`,
  the path a tool that nests dotted names gives `check_evaluated`'s
  `koto.check`) and the attempt numbers.
  Error pairs come first, then warnings, then info, at most 50 lines per
  evaluation, in the same write as the evaluation's `check_evaluated`.
  `check_evaluated` gains `koto.check.finding_count`, always written (0
  included), and `koto.check.findings_truncated` when the check reported
  more findings than its event kept. `check_rule` is unchanged: it still
  counts the error rules of failing checks only. A dispute adds a
  `check_disputed` line with the state, the gate as `koto.check.name`, the
  disputed evaluation's attempt numbers (so it joins to exactly that
  evaluation), the rule id and who disputed it, never the reason. The line format stays at version 1, and tools that send
  journal lines off the host see the new kinds without a format change.

- **A passing check's warnings reach the agent as `advisories`.** When a
  `koto next` call evaluates a gate or default action that passes while
  reporting `warning` findings, the response carries a top-level
  `advisories` list, one entry per such check with its `state`, its `gate`
  (`__action__` for a default action) and those findings in the usual
  finding shape, at most 100 findings in all, with
  `advisories_truncated: true` beside it when that cap left warnings out. A
  response with none carries no `advisories` key and is otherwise unchanged. `koto next --to` and
  `koto status` never carry it, and a warning never blocks.

- **`koto disputes record` records that a finding is wrong without obeying
  it.** `koto disputes record <session> --gate <gate> --rule <rule-id>
  --reason <text> [--state <state>] [--by agent|person]` appends one
  `finding_disputed` event naming the state (the current one by default),
  gate, rule id, reason and source, and the disputed evaluation's attempt
  numbers, and prints `{"status":"recorded"}`. The
  gate must have a settled evaluation logged in that state (a polling gate
  that has only answered pending doesn't count); the rule needn't have a
  logged finding. The reason goes through koto's known-credential redaction
  before it's stored. A dispute changes no gate outcome, route or state, and
  the next `koto next` responds as it would have without it. A missing
  session exits 1; a gate not evaluated in the state (`gate_not_evaluated`),
  a rule id outside 1 to 128 bytes (`invalid_rule`), a reason outside 1 to
  1,000 bytes (`invalid_reason`) or a `--by` other than `agent` or `person`
  exits 2 and appends nothing. The event is additive: koto v0.14.1 reads a
  log holding it and reports the same current state.

- **The run journal records check evaluations.** Each gate evaluation and
  default-action run a session commits now adds a `check_evaluated` line to
  `~/.koto/_run_journal.jsonl`, with the check's outcome, attempt numbers,
  rule count and run time, followed by one `check_rule` line per error rule
  with the rule's visit and session counts. A polling gate that is still
  pending adds a `check_pending` line instead. Every check line carries
  `koto.seq`, the relayed event's sequence number in the session's log, and
  `koto.visit.seq`, the sequence number of the entry that opened its visit.
  A rule id outside the journal's name shape is written as `sha256:` and 32
  hex characters of its digest. The lines carry no check output, finding
  text, path or command. Unlike the identity records, check lines aren't
  flushed to disk on their own, so an operating-system crash can lose the
  ones written since the session's last state entry; a process crash loses
  none. `docs/reference/run-journal.md` documents the three kinds.

- **The run journal records decider calls and overrides.** Each decider
  consultation a session commits adds a `decider_consulted` line (provider,
  model, outcome, token counts, latency), each graded decider-check
  criterion a `decider_checked` line (the gate, the criterion's id, mode,
  outcome, reason, whether it blocked, tokens, attempts, latency), and each
  `koto overrides record` a `check_overridden` line naming the state and
  gate. They carry `koto.seq` and `koto.visit.seq` like check lines, so an
  override can be joined to the evaluation it moved past and decider tokens
  summed per model from the journal alone. A verdict koto reuses writes
  nothing. No line holds a probability, hash, threshold, endpoint, rule
  reference, rationale or override value.

- **The run journal records the context keys each state visit read.** When
  a visit ends (an arrival elsewhere, a rewind, a cancel, or a terminal's
  result), the journal gets one `context_reads` line per distinct key,
  reader and access the visit read, in first-read order, with the read
  count, how many reads found the key, and the first and last content
  hashes. A visit writes at most 51 such lines: past 50 tuples, one
  overflow line sums the rest. The lines come right before the
  `state_entered`, `cancelled` or `terminal` that ends the visit, carry
  `koto.seq` and `koto.visit.seq`, and never hold a context value, though
  the key's name and the content's unsalted SHA-256 can reveal what a
  session read, so the lines are host-local data to forward with care. A
  key, reader or access outside the journal's name shape is written as its
  digest. Reads made after a visit's terminal or cancel, and reads carried
  in by `koto session import`, aren't journaled.

- **The run journal records the size of each printed directive.** When
  `koto next` or `koto status` prints a non-terminal state's directive, koto
  now adds a `response` line with the state, the command (`next` or
  `status`), and the UTF-8 byte lengths of the printed `directive` and
  `details` (0 when the details were left out), never the text. Like check
  lines, it carries `koto.seq`, here the highest sequence number in the
  session's log when it was written, and `koto.visit.seq`, so a reader can
  sum the instruction bytes an agent received per visit. A chained tick
  writes one line, for the state it stopped in; terminal ticks, errors and
  refusals write none. The `instructions_delivered` event is unchanged.
  Writing the line never holds up `koto status`: it waits at most a few
  tens of milliseconds for the journal's lock and otherwise skips the line
  with the usual one warning, and it writes nothing else.

- **A session waiting on a request leg is woken when the leg changes.** Every
  write that records a leg's result (a promoted worker result, `koto request
  resolve`, or a refusal `koto init --koto-leg` records), abandons a leg or a
  whole request, or closes a request now appends one line to
  `~/.koto/wakes/<session>` for the request's coordinator of record and its
  requester, before the command that made the change returns. The wake carries
  no state, only "look again", so a lost or duplicate wake is harmless. A
  harness subscribes by watching that file, or with the new `koto request
  watch --session <id> --timeout-secs <n> [--since <cursor>]`, which exits 0
  with `woke` and a `cursor` within 1 second of the changing command
  returning, or at its timeout.
  The request group's `cli_contract` moves to 1.2. `koto request wait
  --timeout-secs` remains the way to wait with no subscriber. Wakes are local
  to one machine.

- **A `request-leg` gate routes on what another session reported.** A gate of
  type `request-leg` names a `request` and a `leg` (both may use `{{VAR}}`) and
  reads that leg from the request store: its `disposition` (`open`,
  `resolved`, `abandoned`, `missing`), whether a session is `bound`, the
  result's `source`, `status`, and `payload`, the promoted result's
  `final_state` and `template`, the payload's `outcome`, `step`, and `reason`,
  and a `valid` flag checked against an optional `expect` map. An open leg is a
  temporal block, so a coordinator waits on it without writing an arm for the
  wait, and a resolved or abandoned leg passes. A `when` clause can route on a
  key inside the payload (`gates.<gate>.payload.<key>`), the one place a
  `gates.*` path may run past three segments, and `context_assignments` can
  copy payload values into context. The gate never writes to the request log.
  A promoted leg result now records the terminal state it came from, shown as
  `result_final_state` in `koto request get`.

- **The strict reachability check exempts non-overridable gates.** Strict
  compilation requires that some gate-only transition fires when every gate
  takes its override value, so that an override can always move a stuck
  state. No override can apply to an `overridable: false` gate, so transitions
  referencing one are now left out of that check, and a state whose gate-only
  transitions all reference one is exempt. Before this, a non-overridable gate
  routed on values its default can't produce (a `request-leg` gate's
  `payload.outcome`, or a `context-matches` gate routed only on
  `matches: false`) could not compile strictly at all, since the
  `override_default` that would satisfy the check is an error on such a gate.
  Transitions on overridable gates are checked as before.

- **Transition `context_assignments` now run (koto#204).** A transition can
  write context keys when it fires: literals, `{{VAR}}`,
  `${evidence.<field>}`, and `${gates.<gate>.<path>}` (a dot path into any
  gate's structured output), alone or inside a string literal. The resolved
  values are recorded on the `transitioned` event, so they can't be separated
  from the transition, and a store write that fails afterwards is restored on
  the next `koto context get`, `koto context exists`, or context gate. An
  absent evidence field or gate path resolves to the empty string; resolved
  values are never expanded twice. Until now the compiler dropped these blocks
  without a word. **Breaking for templates that relied on that:** any
  transition key other than `target`, `when`, and `context_assignments` now
  fails compilation, and assignment references are checked against the
  state's `accepts` fields, gates, and declared variables. The W5 lint no
  longer warns for a `failure: true` terminal whose every incoming transition
  assigns `failure_reason`.

- **A gate can refuse overrides with `overridable: false`.** Until now any
  gate could be forced with `koto overrides record`, so one override on a gate
  that decides a merge or a report could drive any arm it routes, including
  arms nothing re-checks. A gate declared `overridable: false` (any gate type)
  now refuses the override with exit 2 and the typed code
  `gate_not_overridable`, with or without `--with-data`, and appends nothing;
  it reports `agent_actionable: false`; and `koto next` ignores any override
  for it already in the log and evaluates the gate for real. The field
  defaults to `true` and is omitted from the compiled JSON when `true`, so
  existing templates compile byte-identical and existing sessions' template
  hashes stay valid. Declaring `override_default` on a non-overridable gate is
  a compile error, as is an `overridable` value that isn't a boolean.
  Compatibility note: a gate declaration now rejects unknown keys, naming the
  state, gate, and key, so a misspelled `overrideable: false` can't compile
  and leave the gate overridable. A template that carried a stray key on a
  gate compiled before and fails now.

- **A terminal state can declare the result it reports.** A workflow's result
  used to carry whatever evidence was submitted on its terminal state, so the
  outcome a caller routed on was text an agent composed. A terminal state may
  now declare `result:`, a map of up to 32 keys whose values mix literal text,
  `{{VAR}}` and `${context.<key>}`. koto resolves it once, on the tick that
  lands in the terminal, and it becomes the result's `payload`; a key whose
  context reference does not resolve comes through empty and is listed in
  `payload.missing`. The result now also rides the terminal `koto next`
  response and `koto status` on a terminal session, as a new `result` field,
  alongside the places it already went: the child's log, a bound request leg,
  and the parent's `ChildCompleted`. A terminal without a map reports exactly
  what it did before.

- **`koto request attach` lets a root session answer a request leg.** Only a
  dispatched child could bind a leg, because the dispatch epoch was the only
  thing fencing it, so a workflow that runs another workflow as a stable root
  session had no way to have that session's result recorded against its
  request. `koto request attach <request> <leg> --session <name>` admits a
  root session in place of the epoch with checks that all run before any
  write, the ones that read the request inside its lock: the session isn't
  terminal or cancelled (`session_terminal`), it was built from a template
  file the leg names (`template_mismatch`), its non-`rebind` variables equal
  the leg's `inputs` (`input_mismatch`), the leg is open and unbound or
  already bound to it (a no-op), and it isn't answering another live leg
  (it moves only when its old leg was abandoned or its old request closed).
  The bind event records `attach: self` and the session's template identity,
  shown on the leg as `attach` and `bound_template`. On a self-attached leg
  `progress`, `resolve` and leg-scoped `abandon` are refused with
  `self_attached_leg` whatever `--dispatch-epoch` says; the result arrives by
  promotion from the session's terminal tick, including under
  `--no-cleanup`. A leg's `template` may now be a list of up to eight names,
  the leg view gains the `refused` result source (written only by koto's own
  refusal write, never by `resolve`), and `koto init` records the source
  template's file name on the session header as `template_source_file`. The
  request contract moves to 1.1; a dispatched child passed to `attach`
  binds exactly as `bind` binds it, and `bind` is unchanged.

- **`koto init` can attach, replace, and answer a request leg in one call.**
  A skill that reopened the same named session on every invocation had to
  read it first and pick between creating, resuming and cleaning up, and a
  finished session left behind by an earlier run blocked the name outright.
  Four new flags make that one step with one of four outcomes, reported as
  `outcome` in the output: `created`, `attached`, `replaced`, or a typed
  refusal. `--vars-file <path>` takes the variables as a JSON list of
  `["KEY", "VALUE"]` pairs, so a repeated key is refused as `duplicate_var`,
  and validates them before the name is looked up. `--replace-terminal`
  replaces a finished session and returns its `replaced_result`, refusing a
  running one (`session_live`). `--attach-live` joins a running session when
  it was built from a template file of the same name (`template_mismatch`),
  its new origin record matches the caller's worktree and session store
  (`origin_mismatch`), and every explicitly passed non-`rebind` variable
  equals the recorded one (`var_mismatch`); it then re-applies the
  `rebind: true` variables. `--koto-leg <request>:<leg>` binds the session to
  a request leg with every `koto request attach` check. All checks run
  before any write, and writes follow in a fixed order (create or replace,
  bind, rebind), so a refused or stale invocation changes nothing. Under
  `--koto-leg` every refusal is also recorded on an open, unbound leg as
  `result_source: refused` with a `{outcome, reason, var, recorded,
  requested}` payload, without changing the command's output. Every new
  session's header now carries `origin`; sessions created by earlier
  releases have none and are refused by `--attach-live` until they finish or
  are removed. Plain `koto init` without these flags behaves as before.

- **Template variables can declare `values:`, `pattern:`, and `rebind: true`.**
  A variable used to accept anything the character allowlist allowed, so a
  skill that needed `--intent` to be `continue` or `stop` had to check it in
  prose. Now `values:` names a closed set and `pattern:` a regular expression
  matched against the whole value, and both are enforced at `koto init` and on
  batch child spawns. A refused value exits 2 with no session and a typed
  `code` in the error body: `invalid_var` (with `var`, `value`, and
  `constraint`), `duplicate_var`, or `unknown_var`; the existing `error` text
  is unchanged. The compiler checks the declaration itself: a default must
  satisfy the constraint, an optional variable with no default needs a
  constraint that accepts the empty value, `values:` and `pattern:` are
  exclusive, and an unknown key in a variable declaration (a misspelled
  `valuez:`) is now a compile error instead of being dropped. `rebind: true`
  marks a per-invocation setting that a later attach re-applies on a live
  session; the engine records that as a new additive `variables_rebound` event,
  which the variable fold reads in order. The only command that rebinds a
  variable is an accepted `koto init --attach-live`. Templates that declare
  none of the new keys compile to byte-identical output, so existing sessions'
  template hashes still match.

- **`koto session rebind` moves a session whose checkout moved.** Execution
  anchoring shipped in 0.12.0 with the enforcement but not the repair: both
  refusals told the user to run `koto session rebind <session> --to <dir>`,
  and no such subcommand existed, so a developer whose checkout genuinely
  moved was refused and then sent to a command that did nothing. The verb now
  exists. `koto session rebind <name> [--to <dir>]` canonicalizes the target,
  defaulting to the directory it runs in, writes it to the session header,
  and appends an `execution_anchor_rebound` event carrying the directory the
  session left and the one it landed on, so a move is audited rather than
  silent. It is the only verb that changes an anchor, it works on a session
  created by another session exactly as on any other, and rebinding a child
  does not move its parent. Rebinding to the directory a session is already
  bound to appends nothing and reports `"rebound": false`; a target that does
  not resolve, names a file, or belongs to an unknown session is refused with
  the anchor left alone.

- **`koto session recover` brings back sessions the layout migration had to
  set aside.** The migration from the old per-repository layout flattens
  `sessions/<repo-id>/<name>/` into `sessions/<name>/`. A name reused across
  repositories has several sources and one destination, so only one of them
  can keep the name; the rest are moved into
  `sessions/.migration-conflicts/<repo-id>/<name>/`, where they are preserved
  and unreachable — not listed, not resumable, not nameable. One install
  reported a thousand sessions in that state. `koto session recover` reports
  what is in the quarantine, and `--apply` moves it back into the session
  list in one run; `--session <name>` narrows it to one workflow. A session
  returns as `r<repo-id>-<name>`, which is unique per originating repository
  and, because a session's parent is the dotted prefix of its own name, keeps
  a parent and its children pointing at each other. Recovery moves and never
  deletes, never writes over an existing session, and is safe to re-run. The
  migration now closes with one line naming the command.

### Changed

- **A cloud `koto next` pushes its session once, before it returns.** The
  tick's own event appends, header rewrites and the evidence it copies into
  context now wait in the session's batch with its commands' writes, and the
  flush on the way out pushes the keys, the manifest, the state file and
  `version.json` once. A tick used to push the whole state file after almost
  every event it appended, so its upload grew with the number of events
  times the state file's size. Every tick that writes now pays the batch's
  fixed cost (the batch-open marker's PUT and DELETE, and the flush's version
  and migration checks), and a tick killed before its flush leaves the batch
  open: `koto session import` refuses the session until the next koto command
  on that host pushes it. The local backend is unchanged.
- **End-of-run removal no longer removes a session with a link or
  lineage.** The tick that lands a session in a terminal state now keeps
  it, with or without `--no-cleanup`, when it has a link of its own, a
  `parent_workflow`, a `chain/parent`, or another session naming it
  (`"retention": {"retained": true, "reason": "hierarchy"}`), and `koto
  session prune` reclaims it once its hierarchy is released past
  `retention.keep`. A lone session is still removed at its success
  terminal, and the check reads no retention setting, so no tick fails on
  one. A parent kept this way keeps its terminal children too: the sweep
  that removed them removes nothing, and the response lists them in
  `kept_children`, each with the links and lineage that keep it.
  `koto init --replace-terminal` on a finished session whose hierarchy the
  rule keeps moves it and its koto subtree aside to `<name>_r<N>` instead
  of deleting them, and its answer gains `moved_to`; a released one is
  replaced as before. A live descendant refuses the replace with the new
  `replace_held_live`, and a move that fails partway is undone and refused
  with `replace_failed`.
- **Both prunes apply the hierarchy retention rule.** `koto session prune`
  judges every session with the engine behind `koto session hierarchy`:
  rule `a` is now "alive" (a running or unreadable session, or an open
  anchor), and rules `b` and `c` are replaced by rule `h`, which keeps a
  session whose hierarchy has a live member, is inside `retention.keep`
  since its release instant, or holds something unreadable; its entry
  carries the hierarchy's JSON. A finished session is no longer removed the
  moment it stops: its hierarchy goes whole, once the retention has passed.
  Filters narrow the candidates only, `--older-than` measures from the stop
  instant instead of the last event, apply re-judges each hierarchy under
  its members' locks, and `--apply --plan` refuses only when it would
  remove something the plan didn't name (what it now keeps is skipped and
  listed). `koto workspace prune --root` removes only the subtree members
  the rule releases and lists the rest with reasons; `--force` passes the
  terminal-state gate but never the rule. Both prunes list stale observed
  anchors with their close commands, refuse a malformed retention setting
  (exit 2) and remove nothing when the store can't be read (exit 3).
  `koto session cleanup` and `koto cancel --cleanup` still remove, and now
  name the other live members of the hierarchy in `live_members`, with a
  warning on stderr.
- **`koto status` answers for a session koto removed.** koto removes a
  session at its success terminal (unless the tick passed `--no-cleanup`),
  and a parent removes its kept terminal children. Before either removal
  koto now writes a finished record to `.finished/<name>.json` under the
  sessions directory, and `koto status` on that name exits 0 with
  `removed: true`, `current_state`, `is_terminal: true`, `result`,
  `finished_at`, `session_id` and `execution_dir`, where it used to exit 2
  with `not found`. A caller that read exit 2 as "the run is gone" should
  read `removed: true` instead. `not found` now means no session and no
  record: the name never existed, a person removed it with
  `koto session cleanup` or `koto cancel --cleanup`, or `koto session
  prune` removed its record. Creating a session under the name, or a rewind
  moving a child session to it, deletes the record.

- **Gates and default actions run in an environment fixed when the session
  is created (koto#261).** Until now every command gate and default action
  ran as `sh -c` with the whole environment of whatever process called `koto
  next`, so a shim first on one shell's `PATH`, an exported shell function, or
  a `HOME` whose `.gitconfig` defines an alias could change a gate's verdict,
  even on a gate marked `overridable: false`. Now every command runs as
  `/bin/sh -c`, with standard input at end of file, in a cleared environment
  holding only: the live value, on each tick, of each name on koto's default
  list and each name the template declares in `pass_env:`; then `PATH`, `HOME`
  and `XDG_CONFIG_HOME` as recorded when the session was created (an unset
  `PATH` becomes `/usr/bin:/bin`); then `KOTO_TICK_SESSION` and
  `KOTO_SESSIONS_BASE`, set by koto. The default list covers the user and
  locale names, `TZ`, `TMPDIR`, `TERM`, `CI`, the XDG directories, TLS
  certificate paths, `SSH_AUTH_SOCK`, the `gh` and GitHub token and host
  names, and the proxy variables; the exact list is published in
  [What a command's environment is](docs/guides/default-action-authoring.md#what-a-commands-environment-is).
  `koto init` (every top-level form) makes the record, and a child, including
  one from `koto session start`, copies its parent's. `PATH` loses its empty and relative
  entries, a recorded value that contains a token's or proxy URL's value is recorded unset,
  and the `koto init` response reports both in a new `environment` object. The
  record can't be changed; the remedy for a wrong one is a new session (`koto
  cancel --cleanup <name>`, then `koto init`). A session created by an earlier
  koto adopts a record on its first `koto next`, from the ticking process,
  with an `environment_adopted` event and a one-time notice on the directive.

  A template declares extra names in a new top-level `pass_env:` list. Their
  values are read live and never recorded. `BASH_ENV`, `ENV`,
  `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GH_CONFIG_DIR`, and names starting
  `BASH_FUNC_` or `GIT_CONFIG` can never be passed: declaring one is a compile
  error, and koto filters them again at run time. Declaring a name koto sets
  itself compiles with warning W7.

  When a gate or action fails and a recorded value that existed at creation
  no longer exists, or the command wasn't found (exit 127, or the shell's
  `not found` message), the
  `koto next` response's directive opens with a note naming the gate or
  action, the missing values or the recorded `PATH`, and the remedy. Stale
  values with no failure get a notice, and a refused `--to` carries the note in
  its message. Gate evidence and action output are unchanged. `koto init
  --attach-live` from a shell whose `PATH`, `HOME` or `XDG_CONFIG_HOME`
  differs from the record still attaches, prints one stderr warning, and adds
  `environment_drift` to its output, naming the variables and never their
  values.

  **Breaking for templates whose commands read other variables.** A gate or
  action that reads a variable outside the default list now finds it unset.
  Declare the name in the template's `pass_env:`, or, for this release only,
  create the session with `koto init --legacy-environment`, which records that
  the session's commands run with the caller's whole environment as before.
  The flag is set only at creation, is refused with `--parent`, and **will be
  removed in the next release**, once shirabe's test harnesses have migrated
  off it. A gate that relied on `.` or another relative `PATH` entry must spell
  the path.

- **A session that reaches a failure terminal is kept, and keeping a session
  no longer withholds its result.** A tick that lands a session in a state
  declared `failure: true` no longer removes it, root or child, with or without
  `--no-cleanup`, so a parent's `retry_failed`, `koto rewind`, `koto status`
  and `koto context get` (a `failure_reason`, a skill's running record) still
  work after the failure. Every arrival at a terminal now records the result on
  the session's own log, writes the terminal-index entry and appends
  `ChildCompleted` to the parent, once, whether or not the session is kept, so
  `--no-cleanup` means only "keep the session": a child that passes it still
  delivers its result, and the parent's gate reports `results_in: true` for any
  parent shape. A child parked by an earlier koto without a recorded result
  delivers on its next tick. The `koto next` terminal response gains a
  `retention` field: `{"retained": false}`, `{"retained": true, "reason":
  "failure_terminal"}` or `{"retained": true, "reason": "no_cleanup"}`. When
  koto removes a parent at its terminal, or `koto init --attach-live
  --replace-terminal` replaces one, it first removes the parent's terminal
  descendants, never one that is still running, has anything running under it,
  can't be read, or has a request leg still waiting for its result. Kept
  sessions otherwise stay until `koto workspace prune` or `koto session
  cleanup` removes them.

- **`RequesterWoken` is now delivered.** The wake-candidates pass in `koto
  next` still records `RequesterWoken` with the same fields and the same
  `(child, epoch)` deduplication, but instead of printing a "not yet wired"
  line it rings the requester's wake file, through a new `SignalWaker`.
  `LoggingWaker` is removed.

### Fixed

- **A gate-blocked session's `/workflows` entry reads as blocked on Claude
  Code 2.1.314, and a finished session's last phase reads done
  ([koto#553](https://github.com/tsukumogami/koto/issues/553),
  [koto#554](https://github.com/tsukumogami/koto/issues/554)).** koto wrote
  a gate-blocked session's top-level `status` as `blocked`, a word 2.1.314
  doesn't know, so the entry was counted as completed and its active step
  read Stopped with the gate's FAIL hidden; and a completed session's
  terminal phase read `0/1` with its step Stopped. The entry file is now
  contract version 5. Its top-level `status` is `running`, `completed` or
  `failed`, and a gate-blocked session is written `running`, with its
  active step as an `error` step carrying `blocked: true` and
  `error: "gate <name>: FAIL"`, which 2.1.314 draws as Blocked with that
  Outcome, and a summary ending `· blocked on gate <name>`. koto's own
  classification, `blocked` included, moves to the new `koto.status`. A
  completed session writes every step `done`; a cancelled or failed one
  writes its last step as an `error` reading `cancelled at <state>` or
  `ended at <state>`. Every template-derived string the screen draws is
  escaped. A reader that took the top-level `status` as koto's
  classification should read `koto.status` instead.

- **The koto-skills plugin's hook file moved to where Claude Code reads
  it, and registers no hook.** The plugin shipped its hooks in a root
  `hooks.json` with each command directly under its event, a place and a
  shape Claude Code doesn't load, so neither hook ever ran. The file is
  now `plugins/koto-skills/hooks/hooks.json`, in the matcher-group shape
  (each event maps to groups that carry a `hooks` list), and it registers
  no event: the /workflows `SessionStart` script and the `Stop` reminder
  stay off, since running them at every session start and every turn end
  would cost every plugin user a command each time. Both stay on disk,
  unregistered, as `hooks/session-start-workflows.sh` and
  `hooks/stop-workflows.sh` (the reminder's command text is unchanged),
  and CI's plugin smoke test now runs the reminder script directly.
  `tests/plugin_hooks_shape.rs` fails if the file goes missing, stops
  parsing, returns to the old shape or registers either script.

- **A held run-journal lock can no longer hang a koto command (koto#354).**
  Run-journal writes waited for the journal's lock with no limit, so a
  process that stopped while holding it left every later `koto next`
  stuck after printing its directive, never reaching its exit. Every
  journal write now waits at most five seconds for the lock (`koto status`
  keeps its 30 milliseconds), then skips that write's records with the
  usual one warning and carries on: the command's output, exit code and
  everything it does afterwards, a terminal tick's cleanup included, are
  unchanged. Once one write has given up, later writes try the lock once
  without waiting until one takes it again, so a held lock costs a
  command one wait, not one per write, and a long-running command
  recovers once the holder lets go. The wait isn't configurable and
  covers only taking the lock: a rotation that is slow while holding it
  still finishes, the writers behind it are the ones that give up, and the
  rotating writer gets a full wait of its own for the fresh file. A
  write's records still go to one file whole. `docs/reference/run-journal.md` describes the wait.

- **A failed batch offers its retry after the parent leaves the batching
  state (koto#277).** `reserved_actions` was built only from the batch
  scheduler, which runs only in a state that declares
  `materialize_children`. A coordinator that routes out of that state on the
  tick its batch completes -- the koto-author example routes to
  `analyze_failures` on `all_complete` and `needs_attention` -- stopped where
  the scheduler never runs, so the response had no `reserved_actions`, and the
  example's instruction to copy `reserved_actions[0].invocation` could not be
  followed. Now a non-terminal state that accepts evidence and has a transition guarded on
  `evidence.retry_failed` back to the batching state carries
  `reserved_actions` for the most recently recorded batch's failed, skipped
  and spawn-failed children that have a session, on the response that
  arrives there (by advance or by `koto next --to`) and on every later tick,
  until a `retry_failed` or a rewind starts the batch over. A dependent
  skipped because its upstream failed is left out when it has no skip marker
  yet; retrying the upstream brings it back. The response shape is
  unchanged; it appears in more places. A state without that route, and a
  terminal state, still carries none.

- **A failed child's `reason` is the `failure_reason` it wrote, on every
  batch surface (koto#278).** The koto-user batch reference said a failed
  child in the `children-complete` output has a `reason` string, but only
  `koto status` had one; the gate output and the `batch_final_view` frozen
  from it had `failure_mode` and `reason_source` only. And the reason was
  never the one the worker gave: nothing read a child's `failure_reason`
  context key, so `reason_source: "failure_reason"` was never emitted and
  `koto status` showed the state name. Failed children now carry `reason` in
  the gate output, `batch_final_view` and `koto status`. It is the child's
  `failure_reason` when the child wrote one during its current run, with
  `reason_source: "failure_reason"`, and the state name otherwise, with
  `reason_source: "state_name"`. koto resolves it on the child's terminal
  tick, with the child's result, and records it beside the result: a new
  optional `failure_reason` field on the child's `request_store.result`
  event and on the parent's `child_completed` event. So a child that has
  been cleaned up keeps its reason, and a write after the child failed
  changes nothing. A reason from before a `retry_failed` or `koto rewind`
  restarted the child doesn't count. The text is folded onto one line and
  cut to at most 500 characters. `failure_mode` is still the state name, so
  a consumer reading it sees no change. A view frozen by an older koto gains
  `reason` (the state name) when the terminal response or `koto status`
  reads it; the `batch_final_view` context key it wrote is left as written.
  The koto-author batch pages recommended declaring `failure_reason` in the
  failure terminal's `accepts`, which never stores it: evidence writes no
  context, and koto refuses evidence for a session already in a terminal
  state. They now show a `context_assignments` entry on the edge that takes
  the evidence, and warn that `${evidence.<field>}` on an auto-advancing
  state's edge resolves to an empty value that overwrites the reason. The W5
  compile warning no longer counts `failure_reason` in a failure terminal's
  `accepts` as storing the key, and its remedy names the assignment form
  instead, so a template that relied on that declaration now sees W5.

- **A completed batch is recorded even when the completing tick leaves the
  batching state (koto#263).** The `BatchFinalized` event and the
  `batch_final_view` context key were written only when the tick that saw the
  batch complete also stopped in the state that declares
  `materialize_children`. A parent that routes out of that state on
  `gates.<gate>.all_complete: true`, the usual way to reach a summary state,
  left in the same tick and got neither: `koto context get <parent>
  batch_final_view` exited 3 and the event log had no `BatchFinalized`. Both
  are now written once per completed batch, decided against the batching
  state, whether the tick stays there, advances out of it (even if it then
  stops on an error in a later state), or leaves with `koto next --to`. A
  tick that completes the batch and advances onto a terminal state now also
  carries `batch_final_view` in its response (a `--to` response still
  doesn't). A consumer that read the
  key after such a batch and went on without it when it was missing now gets
  the view.

- **Each batch in a session is recorded as its own (koto#275).** Whether a
  batch was already recorded was decided across the whole log, so once one
  batch had a `BatchFinalized`, a later batch (a second `materialize_children`
  state, or the same state re-entered through an ordinary transition) never
  got one, and `batch_final_view` kept the earlier batch's view. A batching
  state reached before its task list was submitted could also be recorded
  at once, with the earlier batch's children. The decision is now made per
  batching state: a state's batch exists once its task list is submitted,
  and it is recorded when it completes, again after a retry of one of its
  own children (including one submitted in another state that routes back)
  or a rewind, and again whenever its per-child outcomes differ from the last
  record (a new task list on a later visit); a visit that finds the same
  children and outcomes, with no retry of them or rewind since, records
  nothing. With the fix above alone, a parent with two batches in sequence
  would have read the first batch's view after the second.

- **A variable value may contain `+` (koto#266).** The value allowlist now
  accepts letters, digits, `.`, `_`, `-`, `/`, `:`, `@`, `+` and spaces
  (`^[a-zA-Z0-9._/:@+ \-]*$`). A path such as a directory named
  `workspace+instance`, or a version like `1.2.0+build`, used to be refused
  with `invalid_var` at `koto init --var` and `--vars-file`. `+` is safe to
  admit because no POSIX shell gives it a meaning inside a `sh -c` word, and
  where a value lands in a `context-matches` pattern it is escaped to match
  itself. Every other character stays refused, shell metacharacters included,
  and a variable's own `values:` or `pattern:` still decides what it accepts.
  The one allowlist is shared by every place a value is checked, so a rebind
  through `--attach-live`, `--replace-terminal`, and a captured command output
  accept the same values. A context key still may not contain `+`. A session
  whose variables hold a `+` can't be advanced by an older koto, which
  re-checks every recorded value on each tick: its `koto next` exits with
  `persistence_error` and `variable re-validation failed`. A skill that passes
  such a value should require this release.

- **The `children-complete` gate no longer shows a retried child's old result.**
  It reads only the result a child recorded for its current arrival, and it no
  longer answers a child it can read from its own log with the parent's
  `ChildCompleted` copy, which could belong to an earlier arrival.

- **A cleaned-up `<parent>.<task>` child of a parent without a batch hook is
  no longer outstanding forever.** The gate listed it as `<task>`, so it never
  matched the result the parent held under its full name.

- **`koto workspace prune` removes a tree deepest first.** It used to remove a
  parent before its children and stop at the first failure, which could leave a
  grandchild whose parent was already gone, out of reach of the next prune. A
  `parent_workflow` cycle no longer hangs its walk.

- **`koto next --to` no longer walks past a failing `overridable: false`
  gate (koto#251).** A directed transition skipped gate evaluation entirely,
  so one `--to` could bypass a gate that `koto overrides record` refuses to
  force. `--to` now evaluates the non-overridable gates the edge to its target
  depends on, the way `koto next` would route it, and when their result doesn't
  satisfy the edge it exits 1 with `gate_blocked`, names the gate, and appends
  nothing. An edge that routes on the gate failing stays reachable while it
  fails, and an edge that needs an output the gate isn't producing is refused
  even while the gate passes. Overridable gates are still skipped, so `--to`
  remains the recovery path for a stuck session. The rule is in
  `docs/guides/cli-usage.md`, under "Directed transitions and non-overridable
  gates".

- **The decider ledger records `koto next --to` exits from consulted visits
  (koto#254).** The ledger paired a consultation only with an `answered`
  record, which only `--with-data` writes, so a visit the agent left with
  `--to` was counted as a consultation and then dropped from every paired
  metric. Those are the visits where the agent most likely overrode the
  decider. `--to` now appends a `directed_exit` record, naming the state left,
  its `visit_seq`, and the target, when the visit being left holds a
  consultation that wasn't applied and the agent hasn't answered it (a visit
  answered first is already a paired observation). It's written after the
  `directed_transition` event is recorded, so a `--to` the non-overridable
  gate check refuses (koto#251) writes no record. `koto decider report` counts
  directed exits per question and per value, lists them with the disagreeing
  visits, and adds a promotion condition, `ledger_directed_exits`: a value
  isn't eligible while any visit where the decider chose it confidently was
  left with `--to`. Directed exits aren't paired observations, so recall and
  the confusion matrix don't change. An older koto reading the ledger counts
  the new lines as an unknown kind and skips them, so its report doesn't apply
  the new condition: judge promotion with this version or later.

- **`koto context add` and `koto context remove` refuse a session that has no
  state log, instead of creating a log koto can't read.** Both verbs append an
  event to the session's log, and neither checked that the log existed. The
  context store created the session directory and the append created the log,
  so a write to a batch child that was still `blocked`, to a session that had
  finished and been cleaned up, or to a child a parent's `koto rewind` had moved
  left a log whose first line was an event. From then on every command on that
  session failed with `failed to parse header: missing field workflow`, the
  session dropped out of `koto session list`, and a stranded batch child kept
  its parent's batch from ever settling (koto#236). The same path produces the
  one-line headerless log in koto#200.

  Both verbs now exit 2 with `workflow '<name>' not found`, the same as
  `koto status`, before anything is stored. That is a behavior change: a script
  that added context before `koto init` used to get exit 0 and a corrupted
  session, and now gets exit 2. Underneath the verbs, every append to a state
  log refuses, and writes nothing, unless the log exists and its first line is a
  header. So an empty log, or one that lost its header some other way, can't
  gain a headerless event either, and the next event can no longer reuse
  `seq: 1`. A log that already has no header reports `state log has no header`
  instead of a parse error, and the error-code reference says how to clear one.

  Eight integration tests, all of which fail against the previous release, and
  six unit tests.
  Closes koto#236 and koto#200.

- **A gate field that reads an undelivered capture is now refused instead of
  handing the raw token to whatever the field feeds.** A `{{KEY}}` naming
  another state's `capture_stdout_as` compiles inside a gate, and it should: a
  state that has already run may well have produced the value the gate wants to
  test. Nothing checked at run time that it had. When it had not, substitution
  left the token in place and the literal `{{KEY}}` reached `sh -c`, the context
  store or the regex engine. A gate that grepped for the value tested a string
  nobody wrote and could pass; one that consumed it ran a command with a nonsense
  argument and often exited 0; a `name_filter` became a prefix no child name
  starts with, so the gate counted zero children and blocked forever with no
  diagnostic at all. In every case the run carried on and nothing said the
  reference was why.

  Every field `Gate::substitutable_fields` enumerates is now checked before any
  gate is substituted, so nothing is spawned, no context key is read and no
  regex is compiled first. The run stops with `capture_unset` (exit 3), naming
  the state, the gate, the field, the capture and the state that would have
  delivered it. A gate on a state whose own polling action delivers the name gets
  its own wording, because that value cannot exist while the command is still
  running and the general sentence would report a state the run is standing in as
  one it has not entered.

  It is deliberately not a gate error, which is the shape a `pattern` or a
  `name_filter` that resolves to empty gets. Those fire on a value that resolved
  and turned out unusable, and a default fixes them. An undelivered capture
  resolves to nothing at all and no run-time input fixes it -- the template's
  state ordering is wrong. Reporting it as a gate outcome would put it in the
  channel templates route on: gate output is injected into the evidence map
  whatever the outcome, and a recorded override can force a gate to pass, so a
  template could have carried the run past the defect with the value still unset.
  That is the argument koto#221 recorded for the action case, and it is stronger
  for a gate, where the conditions are the routing surface.

  A delivered capture resolves exactly as before, whether it arrived on an
  earlier tick or from an earlier state on the same one, and a gate reading the
  capture its own state's non-polling action delivers still resolves -- the
  action runs first. The empty-`pattern` and empty-`name_filter` refusals keep
  their existing messages. Seven integration tests, four of which fail against
  the previous release.
  Closes koto#225.

- **A `children-complete` gate's `name_filter` resolves `{{KEY}}` references,
  and a `default_action`'s `fallback` no longer accepts one in silence.**
  `name_filter` scopes a gate to one fan-out among several, and the natural way
  to write that scope is against the parent's own name -- children spawned as
  `{{SESSION_NAME}}.research.1`, a gate reading
  `name_filter: "{{SESSION_NAME}}.research."`. The reference reached the gate
  verbatim, no child name started with it, the gate counted zero children and
  blocked, and the compile-time warning that catches a missing trailing dot
  said nothing because the trailing dot was there. It now substitutes, in the
  plain form -- a name prefix is neither a shell word nor a regex -- and an
  undeclared reference is refused at compile time naming the gate and the
  field. A `name_filter` that resolves to an empty string is refused at the
  gate with a reason: an empty prefix does not narrow a gate, it removes the
  filter, so a gate written to watch one fan-out would silently watch every
  child of the parent. Declaring no `name_filter` still means "watch every
  child" and is unaffected.

  `fallback` is the opposite decision, deliberately. It is spliced onto a
  failure response's directive after substitution has run, so the prose reaches
  the agent as written; that is intended and documented on the field, and it
  does not change. What changes is that an author who writes a `{{KEY}}` there
  used to compile clean and find out from an agent that could not follow a
  pointer to `{{SESSION_DIR}}`. The compiler now refuses the reference, names
  the state and the field, and points at the directive, which does resolve one.
  Any reference is refused, not only an undeclared one: the field is never
  expanded, so a declared reference is exactly as undelivered.

  Underneath both is the shape rather than the two fields. `Gate` already
  enumerated its substitutable fields once, next to the struct, with the
  compiler and the tick both reading that list. `ActionDecl` now has the same
  thing, split in two because the promises differ: `substitutable_fields` for
  `command` and `working_dir`, whose references resolve, and `literal_fields`
  for `fallback`, whose references are refused. Every accessor destructures its
  struct exhaustively, so a field added later cannot compile until someone says
  which list it belongs to -- which is what turns four consecutive fixes of the
  same drift into a compile error rather than a fifth. Seven integration tests,
  five of which fail against the previous release.

- **A `default_action` that reads an undelivered capture name is now
  refused instead of handing the raw token to `sh -c`.** A `{{KEY}}`
  naming another state's `capture_stdout_as` compiles inside
  `default_action.command`, so an author was told the reference was
  valid; at run time, if the producing state had not run yet, the
  literal `{{KEY}}` reached the shell and the command exited 0. A
  command that echoed the token failed loudly on the value allowlist,
  which is what made this look narrower than it was — one that consumed
  it, the `koto context add {{TOKEN}} key` shape, did the wrong thing
  quietly. Both `command` and `working_dir` are now checked before
  either is substituted, so nothing is spawned and no
  `default_action_executed` event is written; the run stops with
  `capture_unset` (exit 3), naming the state, which of the two fields
  read the name, the name itself, and the state that would have
  delivered it. This is the same typed stop a directive reading the
  same name already got. It is deliberately not an action failure: the
  author's `fallback` prose describes a command that ran and failed,
  and an `__action__` condition is routable, so a template could have
  carried the run past the defect with the value still unset. A
  delivered capture resolves exactly as before, whether it was
  delivered earlier in the same tick or on an earlier one, and a
  `working_dir` naming an undelivered capture — which used to fail as a
  bare spawn failure that never mentioned the capture — now reports the
  same refusal as the command. One further behaviour changes: an action
  whose command reads the very name that action delivers is refused on
  entry, where it used to hand the raw token to the shell. Its message
  says so in its own terms rather than reporting a state the run has not
  entered, because the run is standing in that state and the fix is not
  a routing one.
  Closes koto#221.

- **A context gate's `key` and `pattern` now resolve `{{KEY}}`
  references.** Substitution rewrote a gate's `command` and nothing else,
  so a state whose action stored `{{SESSION_NAME}}-note` and whose gate
  read `key: "{{SESSION_NAME}}-note"` disagreed about what that reference
  meant: the store held `kprobe1-note` and the gate asked for a key
  spelled with the raw token, reporting the real one absent. Nothing said
  so — the compiler validated references in directives, gate commands and
  a `default_action`, but never in these two fields, so an author got no
  signal either way and the symptom was a gate that would not pass.
  Scoping a context key by session name works now, and both fields join
  the compiler's validation, so an undeclared reference in either is a
  compile error naming the gate and the field rather than a token handed
  to the context store.

  A value substituted into `pattern` is escaped and matches itself. Of
  the characters the value allowlist admits, only `.` is
  regex-significant at top level; `-` and `:` become significant inside a
  character class the author opened, and whitespace under `(?x)`. So
  escaping costs almost nothing an author could have reached for, and a
  dot is a version number or a session name far more often than it is a
  wildcard. `regex::escape` leaves `:` and whitespace alone, so both are
  escaped explicitly — the second matters because free-spacing mode
  would otherwise make a value stop matching itself and start matching
  something wider. One behaviour
  changes for an existing template: a pattern containing a `{{KEY}}`
  reference used to be rejected at compile time as an invalid regex,
  because a raw `{{KEY}}` is not one. It now compiles, with references
  stood in for while the pattern is checked — an authored regex that is
  genuinely malformed is still refused.

  Because the value does not exist at compile time, two run-time cases
  are reported as a gate error with the reason rather than as a plain
  mismatch: a `key` that resolves to something the context store will not
  accept, and a `pattern` that resolves to empty. The second matters most
  — an empty regex matches every input, so without the guard a gate whose
  pattern was a single reference to an unset variable would pass on
  content it was written to reject. That is the opposite of the symptom
  this issue reports, and the more dangerous direction.

  A `children-complete` gate's `name_filter` remains unsubstituted; that
  is koto#224. Whether the value allowlist and the context-key rules
  should converge is koto#227.
  Closes koto#222.

- **`{{SESSION_NAME}}` and `{{SESSION_DIR}}` now resolve inside a
  `default_action` command.** Both reached `sh -c` as literal text while
  the same references in a gate command resolved, and the template
  compiler accepts either name in `default_action.command` and
  `default_action.working_dir` — so an author was told the reference was
  valid in a position where it did not resolve. The failure was silent:
  `koto context add {{SESSION_NAME}} key` wrote into a session directory
  named with the literal token and exited 0, and the state's own
  `context-matches` gate then reported the real key as absent, so it
  looked like a gate that would not pass rather than a command that did
  the wrong thing. The runtime-name pass now runs on the action command,
  the action's `working_dir`, and the gate commands a polling action
  re-evaluates from inside its own loop. One behaviour changes for an
  existing template: `working_dir: "{{SESSION_DIR}}"` used to stay
  literal and fail with a bare "No such file or directory"; it now
  resolves to an absolute path and is refused with a message saying a
  `working_dir` must be relative. Separately, the command reported on an
  `action_requires_confirmation` response is now the string the action
  ran, carried out of the advancement loop rather than substituted a
  second time against an overlay that has since gained the state's own
  capture.
  Closes koto#220.

- **A nested `koto next` no longer leaves the outer tick reporting a
  state the session has left.** A `koto next` run from inside a command
  koto itself was executing — a state's `default_action` or a command
  gate — performed a real transition, in the reported case advancing the
  session all the way to its terminal state. The tick that spawned it
  then finished against the snapshot it started with and answered
  `advanced: false` on the original state, so the caller was told the
  workflow was still waiting on a session that had already ended. The
  answer was wrong rather than missing, and nothing surfaced an error.
  A tick now exports `KOTO_TICK_SESSION` naming the session it is
  advancing before it runs anything, and a `koto next` that finds the
  marker set refuses with the new `nested_invocation` error code (exit
  2), naming the tick in flight and the call to remove. The refusal is
  scoped to the process tree rather than to a session name, so a tick on
  a second workflow from inside a command is refused too — a chain that
  ticks back into the outer session through another one lands on the
  same defect. It covers `koto next` only: `koto context` reads and
  writes from inside a command are a supported pattern and keep working.
  The marker is inherited and carries no liveness — a command that
  detaches survives the process-group kill at timeout and keeps it — so
  the refusal message names the escape hatch, `KOTO_TICK_SESSION= koto
  next <name>`, for a process that outlived its tick.
  Closes koto#208.

- **`details` suppression now keys on delivery, not visit count.** koto
  previously decided whether a `koto next` response carried a phase's
  long-form `details` by counting entries into the phase's state, which
  came apart from the thing that actually mattered in both directions: a
  non-advancing tick (e.g. re-evaluating the same failing gate) re-sent
  `details` forever, while a `koto rewind` back into a phase the agent
  was told to redo withheld them because rewind counts as a re-entry, not
  a fresh visit. The directed-transition path (`koto next --to`) applied
  no suppression rule at all, so it could disagree with the
  natural-advancement path on the same phase. The new rule records the
  fact of delivery directly: an `InstructionsDelivered` event is
  appended whenever a response carries a phase's `details`, and the
  suppression predicate
  (`instructions_delivered_this_window`) asks whether a delivery has
  happened since the most recent arrival at the phase. A shared
  combinator (`NextResponse::with_details_suppressed_unless_full`) now
  applies that one rule at both response-construction sites, so the
  directed path and the natural-advancement path can no longer disagree.
  `--full` still forces `details` through, and now also records a
  delivery, so the next plain tick doesn't re-deliver.

- **A self-loop no longer re-sends a phase's `details`.** A phase
  transitioning to itself, and a `koto next --to <phase>` issued while
  the workflow already occupies that phase, are laps around a loop the
  agent is already in rather than arrivals at it — the agent still holds
  the procedure, so it is not re-sent. Only arrival from a different
  phase delivers, along with any `koto rewind` into the phase, which
  means redo this rather than continue. This is koto#90's acceptance
  criterion 3, which the previous rule overrode without citing it. On a
  long loop the per-lap cost is now the directive alone; an agent that
  loses the procedure mid-loop recovers it with `koto status <name>`,
  which every response for a phase that declares instructions names in its
  `directive`, whether or not that response carried them.
  The boundary that decides delivery is now separate from the epoch the
  dashboard's blocked classification reads, so that badge is unchanged.

### Added

- `koto status <name>` now returns the current phase's `directive`,
  `details`, and `expects` when the workflow is not at a terminal state —
  a read-only retrieval, substituted through the same pipeline `koto
  next` uses, that returns the phase's full instructions regardless of
  the delivery rule above and appends nothing (no delivery record, no
  lock). This is the recovery path for an agent that has lost track of a
  phase's instructions, or that never received them because they were
  suppressed. `directive`, `details`, and `expects` are absent together
  at a terminal phase; `details` is additionally absent when the phase
  declares none.
- `koto status <name>` gains a `template_hash_mismatch` key
  (`{"recorded", "actual"}`) when the compiled template read from disk no
  longer matches the hash recorded in the session header. Unlike `koto
  next`, which fails closed on the same mismatch, `koto status` reports
  it instead of failing, since this command is often the only recovery
  path an agent has left.
- Every `koto next` response whose current phase declares instructions
  now carries a short koto-authored pointer to `koto status` in its
  `directive`, so an agent that has lost everything else still learns
  the retrieval exists. The pointer appears whether or not `details` was
  actually included on that particular response — that's the case it
  matters most for — and is spliced in before any leg-abandonment
  notice.

## [0.10.0] - 2026-05-24

### Request-store substrate + first stability lockdown

This release ships the request-store dispatch protocol — a
coordinated set of engine modules that let a coordinator session
discover unassigned child sessions, atomically claim them, dispatch
them to a substrate, wake their requester when they reach terminal
state, and respawn requesters whose substrate transcripts have
expired. It also freezes the first crate surface that external
substrates may import.

The changes span the request-store header fields on
`StateFileHeader`, the discovery scan with per-coordinator cursor,
the claim sidecar, the terminal-index, the audit-event family,
idempotency hashing with three-point fsync discipline, the
wake-candidates pass, F1 cold-restart re-priming, recursion caps,
and the public-surface lockdown.

See `docs/STABILITY.md` (added in this release) for the bump
protocol, the four frozen `SessionBackend` methods, and the
additive-evolution rules that apply to every public type re-exported
under `koto::engine::types::*`.

#### Operator-facing behavior change — auto-cleanup removed (load-bearing)

**koto v0.9.x auto-cleans terminal sessions; this release removes
that default.** Operators must invoke `koto workspace prune` to
reclaim disk space from completed or abandoned workflows. Without
periodic prunes, `~/.koto/sessions/` grows unbounded.

The intentional behavior change is required by the dispatch
protocol itself: the discovery scan and the terminal-index reader
both depend on terminal sessions remaining on disk long enough for
the per-coordinator cursor to advance correctly. The 7-day TTL on
coordinator cursors (`request_store.coord_cursor_ttl_days`) bounds the
horizon during which a terminal session needs to remain visible.

koto's dashboard surfaces stale-tree indicators so operators see at
a glance when prune is needed.

**Operators upgrading from 0.9.x should add `koto workspace prune` to
their periodic-maintenance script.** See the verb's documentation in
`docs/guides/cli-usage.md` for the full flag set.

#### Downstream consumer contract

The crate-surface lockdown in this release establishes the first
durable contract for downstream consumers that import from
`koto::engine::types`. The eight types frozen here — plus the four
`SessionBackend` methods marked `# Stability: Stage 1 — Frozen` —
form the import contract documented in `docs/STABILITY.md`.

External-consumer compile verification ships in this release as the
`koto-stability-tests` crate (workspace-internal, not published).
CI runs `cargo test -p koto-stability-tests` on every PR to catch
accidental breaking changes before release.

### Added

- `koto-stability-tests/` external-consumer fixture crate. Imports
  every promised export from the frozen surface and exercises the
  four frozen `SessionBackend` methods via a trait-object smoke
  test.
- `docs/STABILITY.md` — public stability contract, bump protocol,
  and additive-evolution rules.
- `docs/workspace-layout.md` — workspace dir/file layout
  reference.
- `koto::engine::types::*` re-exports for the eight frozen types:
  `StateFileHeader`, `Event`, `EventPayload`, `SpawnEntrySnapshot`,
  `ChildSnapshot`, `AssignmentClaim`, `derive_state_from_log`,
  `CURRENT_SCHEMA_VERSION`.
- `koto::error::Error` — re-exported `EngineError` alias.
- `StateFileHeader` request-store fields: `needs_agent`, `role`,
  `inputs`, `coordinator_of_record`, `requested_by`,
  `assignment_claim`, `dispatch_epoch`, `respawn_generation`, plus
  four forward-compat reserved fields.
- `Event.idempotency_hash` for retry-safe append discipline.
- `koto workspace prune --root <session> [--dry-run] [--yes] [--force]`
  CLI verb. Reclaims terminal workflow trees with a symlink-refusal
  safety gate and an interactive confirmation prompt.
- `koto next` directive return: `unassigned_children` array
  populated by the per-tick discovery scan.
- `koto next` directive return: every variant (including Terminal
  and Error) carries `unassigned_children` for uniform
  coordinator-side consumer branching.
- Discovery scan with mtime-cursor + tied-boundary seen-set rule
  + 7-day cursor TTL + cursor GC.
- Terminal-index JSONL writer + skip-malformed reader +
  compaction-lease O_EXCL sidecar with stale-recovery.
- Claim sidecar: O_EXCL + happy-path dispatch orchestration +
  four-case drift recovery.
- Audit-event family with reserved `kind` discriminator on
  `EvidenceSubmitted`: `ChildDispatched`, `ChildRedelegated`,
  `RequesterWoken`, `RequesterRespawn`. The `request_store.` prefix is
  reserved for future audit kinds.
- Idempotency-hash short-circuit + 3-point fsync discipline before
  substrate wake-delivery.
- Wake-candidates pass + age-and-activity recovery.
- F1 cold-restart re-priming + F3 fallback +
  `respawn_generation_cap`. The resume-context prompt is a
  fixed-form committed template.
- Epoch-fence validation on child-log writes.
- Recursion-cap enforcement + recursion_caps bench harness.
- Discovery scan bench harness with soft-by-default reporting at
  100/1k/10k/26k workspace sizes.
- `RequestStoreConfig` 5-level precedence cascade + reserved
  `[request_store.recursion]` warn. Eight operator-tunable dimensions:
  `stale_claim_timeout_seconds`,
  `stale_dispatch_timeout_seconds`, `redelegation_cap`,
  `coord_cursor_ttl_days`, `terminal_index_compact_lines`,
  `compact_lock_timeout_seconds`, `directive_batch_size`,
  `respawn_generation_cap`.
- `ValidatedSessionId` / `ValidatedCoordId` newtypes for security
  hardening at every public boundary.
- New typed errors: `EpochFenceViolation`,
  `RedelegationCapExceeded`, `ConcurrentSubmissionConflict`,
  `RecursionCapExceeded`, `ReservedKindCollision`,
  `InvalidSessionId`, `InvalidCoordId`.

### Changed

- **Removed auto-cleanup default** (see "Operator-facing behavior
  change" above). This is the load-bearing operator-facing change
  in this release.
- `StateFileHeader` extended with the request-store fields listed
  above (additive — pre-existing state files round-trip unchanged).
- `Event` extended with `idempotency_hash: Option<String>`
  (additive).
- `NextResponse::Terminal` and `NextResponse::Error` now carry
  `unassigned_children: Vec<UnassignedChild>`. Adds a new key in
  the JSON output; consumers that ignore unknown keys continue to
  work.
- `koto next` startup runs cursor GC, terminal-index compaction
  threshold check, and wake-candidates pass before the per-tick
  advance loop.
- `cargo workspace` layout: koto crate sits alongside
  `koto-stability-tests/` as workspace members.
- Crate version bumped 0.9.1-dev → 0.10.0 per the pre-1.0 semver
  discipline. Breaking changes to the locked surface require a
  6-week deprecation window per `docs/STABILITY.md`; additive
  evolution is permitted in minor releases.

### Stability

- **Stage 1 freeze.** Eight types under `koto::engine::types::*`
  plus `koto::error::Error` and four `SessionBackend` methods
  (`create`, `list`, `read_events`, `init_state_file`) are
  documented as the load-bearing public surface. The
  `# Stability: Stage 1 — Frozen` doc-comment marker identifies
  each. Renames, removals, and signature changes follow the
  deprecation protocol in `docs/STABILITY.md`. Adding new fields
  (to structs), new variants (to enums whose serde uses
  `#[serde(other)]` or accepts unknown keys), and new error
  variants is permitted in minor releases.
