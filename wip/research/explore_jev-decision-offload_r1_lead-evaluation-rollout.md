# Lead: How would we know classifier decisions made inside koto are good enough, and how would we roll them out safely?

## Findings

### What koto records about each agent decision today

A koto session is one append-only JSONL file: a header line, then events with `seq`,
`timestamp` (millisecond RFC 3339), `type`, and `payload`
(`src/engine/types.rs`, `Event` around line 1155; `EventPayload` from line 480). The events
that matter for decision evaluation:

- `evidence_submitted { state, fields, submitter_cwd }`. This is the agent's answer. In
  shirabe templates almost every branch is an `accepts` field of `type: enum`, so `fields`
  holds a closed-set label such as `review_outcome: passed` or `verdict: absorb`. That label
  is structurally the same thing a Jev `choice` question returns.
- `transitioned { from, to, condition_type, skip_if_matched }`. It records which transition
  fired. In practice `condition_type` is nearly always `auto` (evidence plus gate conditions
  resolved by the advance loop) or `skip_if`.
- `gate_evaluated { state, gate, output, outcome, timestamp }`. This is the full structured
  gate output at evaluation time. It's the most complete record of "facts koto knew at
  decision time", and it's the biggest event class in real logs.
- `default_action_executed { command, exit_code, stdout, stderr, truncated }` and
  `variable_captured { key, value }`. These hold command output that koto itself produced.
- `context_added { key, hash, size }`. It carries only the digest and size, not the
  content. Content lives in the mutable `ctx/` sidecar (`src/session/context.rs`), where
  `add` overwrites a key and nothing keeps the history
  (`docs/prds/PRD-session-persistence-storage.md`, Known limitations: "koto doesn't track
  content history").
- `instructions_delivered { state }`. It marks that the phase directive reached the agent.
- `decision_recorded { state, decision }` (`koto decisions record`,
  `docs/designs/current/DESIGN-mid-state-decision-capture.md`). These are free-form
  mid-state choice/rationale records. They're advisory and prose-shaped, not typed labels.
- `gate_override_recorded { gate, rationale, override_applied, actual_output }`,
  `directed_transition { rationale }`, `rewound { rationale }`. These are the places where an
  agent overrode or undid something, with a reason.
- The header carries `template_hash` (SHA-256 of the compiled template). The template path
  and `--var` values sit on `workflow_initialized`. Compiled templates are cached by hash in
  `~/.cache/koto` (`src/cache.rs`), or in the session dir for inline templates.

The schema is designed to grow: new `EventPayload` variants and optional fields are
additive and don't bump `CURRENT_SCHEMA_VERSION` (`docs/STABILITY.md`; comments on
`ContextRemoved`, `VariableCaptured`, `InstructionsDelivered`). An older build reads an
unknown event as `Unknown` and carries on. So a classifier-verdict event can land without a
format break. It would need registering in `docs/reference/session-feed.md` (the
`koto template validate-feed` contract).

### The retention problem: most decision logs are deleted

`finish_terminal_tick` in `src/cli/mod.rs` (around line 2696) calls `backend.cleanup(name)`
on every terminal tick unless `--no-cleanup` is passed. For the local backend that is
`fs::remove_dir_all` of the session dir (`src/session/local.rs:85`). The cloud backend
deletes the local copy and the S3 copy too (`src/session/cloud.rs:694`). What survives is one
line per session in `~/.koto/_terminal_index.jsonl` (`src/engine/terminal_index.rs`): a
session id, a timestamp, and an mtime. It holds no decisions. For children, a
`child_completed` event on the parent's log carries only the terminal outcome and a
`WorkflowResult` summary.

shirabe's `/work-on` deliberately passes `--no-cleanup` on root runs, because a blocked run
otherwise loses its record (`shirabe/skills/work-on/scripts/terminal-retention_test.sh`,
`SKILL.md` around line 256). Children must not pass it, or the parent's `children-complete`
gate never converges. So the only logs that survive by default are root `/work-on` runs,
sessions still in flight, and sessions abandoned mid-run.

On this machine, `~/.koto/sessions` holds 34 session dirs, against 47 terminal-index
entries. Across the surviving logs the event mix is roughly 404 `gate_evaluated`,
237 `transitioned`, 191 `evidence_submitted`, 51 `context_added`, 19
`default_action_executed`, 1 `rewound`, and 1 `gate_override_recorded`. That's enough to
prototype a replay extractor, not enough to measure agreement.

### Can past runs become a labeled dataset?

Partly. The label side is there. For every `evidence_submitted` whose field is a template
`enum`, the pair (session, state, field, value) is a clean closed-set label. The option set
comes from the compiled template found via `template_hash`.

The input side, "state at decision time", is not reliably there:

1. The agent decides from the rendered directive plus whatever it read: files, `gh` output,
   sub-agent reports, conversation. koto sees none of that except what came through gates,
   default actions, and context keys.
2. Context content is overwrite-only. The log's `context_added.hash` lets a replay tool
   *detect* that the `ctx/` file on disk is not the version the agent saw (a hash mismatch).
   It can't recover the old version.
3. Most logs are deleted, as described above.

So what replay can honestly reconstruct is the koto-visible state: template variables,
directive text for the state, the latest `gate_evaluated` outputs, captured variables,
default-action output, and context content whose current hash still matches the logged
one. That is exactly the pruned, fact-shaped state Jev wants (the Jev doc warns that
cluttered context degrades accuracy). But it means replay measures "can the classifier
match the agent from the facts koto holds", not "can it match the agent from everything the
agent saw". For decisions where the agent's call really depends on unlogged reading (review
verdicts, analysis outcomes), a low replay score is ambiguous. The classifier may be weak,
or the state may just be missing inputs.

### The labels are heavily skewed toward the happy path

Value distribution across surviving logs (counts per state/field):
`review_outcome` passed 4/4, `scrutiny_outcome` passed 5/5, `qa_outcome` passed 4/4,
`verification_outcome` passed 5/5, every `scope` hop outcome `landed` 6/6,
`plan_validation.verdict` proceed 7/7, `analysis.plan_outcome` plan_ready 7/7. The only
fields with more than one observed class were `fold.verdict` (keep 15, absorb 3),
`worktree_discipline_check.impact` (none 5, informational 2), `analysis.issue_type`
(code 6, task 1), `pre_pr_evidence.design_diagram`, and `orchestrator_setup.status`.

A classifier that always answers `passed` would reach 100% agreement on the review gates.
Raw concordance is meaningless for these decisions. The number that matters is recall on
the minority class (did it catch the `blocking_retry` the agent caught?), and the logs hold
almost no minority examples. That points to a hand-built fixture set, not mined logs.

### The agent's label is not ground truth

For verdict states (scrutiny, review, qa, verification), the same agent that did the work
submits the verdict, usually after reading sub-agent output stored as a context key
(`scrutiny_results.json`, `review_results.json`). Agreement with the agent measures
imitation, not correctness. The downstream signals that could serve as weak correctness
labels are all things koto logs: a later `rewound` to or past the state, a
`blocking_retry` loop that re-enters `implementation`, a `done_blocked` terminal with
`failure_reason`, a `gate_override_recorded`, or (outside koto) CI failure after merge.
Only the koto-internal ones can be joined to a session without new plumbing.

### Existing eval scaffolding in shirabe

Each shirabe skill has `evals/evals.json`, and several have `fixtures/` with scripted
`koto next` responses (`shirabe/skills/work-on/evals/fixtures/scenarios/*`,
`fixtures/bin/koto`). Those evals grade the agent's plan in prose against expectations.
They're not decision-level, but the scenario directories (`gate-failure`, `already-complete`,
`docs-routing`, `blocking-label`, the `e2e-*` ones) already name the minority-class cases a
decision golden set needs. That's the natural home for a per-decision fixture set: a state
payload plus an expected label, run against the classifier offline.

### Where "wrong" is cheap and where it isn't

The templates already encode some of the answer:

- **Guarded verdicts.** `scrutiny`, `review`, and `qa_validation` in `work-on.md` only take
  the `passed` edge when `gates.<x>_results.exists: true` also holds. A classifier that
  wrongly answers `passed` still can't skip the gate unless the review artifact exists. But
  a wrong `passed` on a real finding silently ships a defect, and nothing downstream in koto
  catches it. False-pass cost is high. False-block cost (`blocking_retry`) is one extra
  implementation loop, which is cheap and self-correcting.
- **Routing and classification.** `analysis.issue_type` (code/docs/task),
  `worktree_discipline_check.impact`, `fold.verdict` (keep/absorb), and `entry.mode` are
  cheap to get wrong in one direction and recoverable with `koto rewind`. These are the
  natural first candidates for auto.
- **Irreversible exits.** `scope` `bail.*` (cancel/force_materialize),
  `exit_abandonment`, `ci_monitor.ci_outcome`, `pr_finalization.pause_decision`, and any
  `override` value that bypasses a gate (`setup_*`, `staleness_check`, `orchestrator_setup`
  in execute) end work or skip a check. These should stay agent- or human-decided no matter
  how high the agreement.
- **`/explore` routing** isn't a koto state at all. The skill has no template, so a misrouted
  exploration leaves no decision record. It's cheap to redo, but it can't be measured until
  it becomes a declared state.

So the cost of an error is asymmetric per *edge*, not just per decision. A single
confidence threshold per question is too coarse. The threshold belongs on the answer that
would be auto-taken.

## Implications

### Design sketches grounded in the log

**1. A new additive event: `classifier_evaluated`.** One event per question per tick,
appended before the evidence it would pre-empt:

```
classifier_evaluated {
  state, question_id, primitive: choice|score|noul,
  answer, probabilities, confidence,
  mode: shadow|auto|off, threshold, acted: bool,
  state_digest,            // sha256 of the exact state payload sent
  state_sources: [ {kind: gate|context|var|action, key, hash} ],
  provider, model, latency_ms, error?
}
```

Storing the digest plus a manifest of sources, rather than the payload itself, keeps the
log small and keeps agent-written text from being duplicated into it. But with overwrite
semantics on `ctx/`, the digest alone can't reproduce the payload later. For evaluation
runs, an opt-in `--record-classifier-state` should write the exact payload as a context key
(`classifier/<state>/<question>.json`), so replay has a byte-exact input. The Jev doc stresses
continuous evaluation against production data, and this is the only way koto gets it.

**2. Shadow mode.** In `mode: shadow`, `koto next` evaluates the question when the state is
entered (or when the agent submits evidence), logs `classifier_evaluated` with
`acted: false`, and returns the normal directive. The agent's `evidence_submitted` then
supplies the paired label. Shadow needs no change to the response contract and no change to
shirabe prose. It can ship first and run on every template, and the agent never sees the
classifier's answer. It does depend on sessions surviving, though (see below).

**3. Per-decision thresholds in the template.** Declare on the accepts field, per value:

```yaml
review_outcome:
  type: enum
  values: [passed, blocking_retry, blocking_escalate]
  classifier:
    question: review_verdict
    mode: shadow            # default; template can never declare auto for a guarded edge without a floor
    auto_min:
      blocking_retry: 0.70  # cheap to be wrong: one extra loop
      passed: 0.97          # expensive: a missed finding ships
      blocking_escalate: never
```

`never` compiles to "this value is always left to the agent". The compiler should reject
`auto` on any value whose transition bypasses a gate (`override`) or reaches a terminal the
template marks irreversible. That encodes the cost asymmetry above in validation, not
convention. It matches the Jev doc's advice to pair semantic checks with programmatic rules.

**4. Concordance report.** A `koto classifier report [--template T] [--state S]
[--since D]` command that walks sessions and pairs each `classifier_evaluated` with the next
`evidence_submitted` in the same state and epoch. Per question it would report agreement,
a per-class confusion matrix, recall on each non-default class, agreement by confidence
band, and abstention and error rates. Because terminal sessions are deleted, the report
can't just read `~/.koto/sessions`. There are two options: (a) require `--no-cleanup`-style
retention while any question is in shadow, which is unsafe for children as shown above, or
(b) have the terminal tick append a compact concordance line (question, answer, agent
label, confidence, template_hash) to a workspace-level `_classifier_ledger.jsonl` next to
`_terminal_index.jsonl`, using the same O_APPEND, 4 KiB atomic-line discipline. Option (b)
survives cleanup, stays small, and holds no agent text. I recommend it.

**5. Promotion from shadow to auto.** Make promotion a recorded, reviewable change, not an
automatic flip. The report computes eligibility per (template_hash, question, value):
at least N paired observations (say 50), at least M minority-class observations (say 10),
recall on the value being auto-taken above the declared bar, and the calibration check that
agreement inside the auto band is at least the band's lower bound. A human then changes
`mode: auto` in the template. Eligibility should key on `template_hash`, because a directive
or option-set edit changes what the question means, so stats from an old hash shouldn't
count. The skew finding means review-gate `passed` will essentially never become eligible
from organic traffic. That's the right outcome.

**6. Kill switch.** Three levels, from coarse to fine: an env var
(`KOTO_CLASSIFIER=off`, matching the existing `KOTO_*` override pattern, e.g.
`KOTO_REQUEST_STORE_*`); a config key (`classifier.mode = off|shadow|auto`) through
`src/config/validate.rs`, which today only whitelists `session.*` keys; and the per-question
template `mode`. The effective mode is the most restrictive of the three, and it's recorded
on every `classifier_evaluated` so reports can drop kill-switched periods. A missing key or
a provider error should degrade to the agent path, never block a tick.

**7. Downgrade on disagreement in auto.** Auto decisions produce no agent label, so
concordance goes blind once a question is promoted. Keep a sampled shadow slice (for
example 1 in 10 auto-eligible ticks still go to the agent, with the classifier answer
logged) so drift is visible. Treat `rewound` past an auto-decided state, or a
`blocking_retry` loop right after an auto `passed`, as a negative signal the report
surfaces.

## Surprises

- koto deletes the whole session, including the cloud copy, on every terminal tick by
  default. The "past runs as a dataset" idea mostly finds nothing: what survives is
  abandoned or in-flight sessions and `/work-on` root runs. Even the `session_id` and
  rationale fields added for auditability (`PRD-session-schema-hygiene.md`) only live as long
  as the file.
- Context content isn't versioned, but the log keeps a hash per add. That's enough to
  *prove* a replay input is stale, not enough to rebuild it.
- The observed labels are almost all the happy-path value, including every review,
  scrutiny, and QA verdict. Headline agreement numbers would look excellent and mean
  nothing.
- The review-gate transitions already AND the agent's verdict with a context-existence gate,
  so the template language can already express "classifier may decide, but only when a
  programmatic fact also holds". That's the Jev doc's own recommended pattern.
- `/explore`, where a misroute is cheapest, has no koto template, so it has no decision
  record to evaluate at all.

## Open Questions

- Should shadow mode run at state entry (the classifier sees only what koto knew before the
  agent worked) or at evidence submission (it sees context the agent just wrote)? Entry
  isolates the classifier from agent framing, which also shrinks the injection surface.
  Submission gives it the review artifact, which verdict states need.
- Is agreement with the agent even the right target for verdict states, or should the bar
  be agreement with a stronger judge on a curated fixture set, with agent agreement only
  as a sanity signal?
- Where does the ledger live for multi-machine or cloud-synced workspaces? The terminal
  index is explicitly local-only (NFS out of scope in `terminal_index.rs`).
- How many minority-class examples are realistic per question per month? If it's a
  handful, promotion bars have to come from fixtures, and production shadow only confirms
  there's no regression.
- Should `classifier_evaluated` be Tier 1 (required display) in the session-feed contract so
  the dashboard shows "decided by classifier" next to the transition? Operators arguably
  need that to trust auto mode.
- Does a template edit that only touches directive prose invalidate promotion stats? Keying
  on `template_hash` says yes. A per-question hash (option set plus criteria text plus state
  sources) would be more precise.

## Summary

koto logs each agent decision as a typed enum in `evidence_submitted`, alongside
`gate_evaluated` outputs and context hashes, so labels exist. But terminal sessions are
deleted by default (cloud copy included), context content isn't versioned, and surviving
labels are almost all the happy-path value, so past runs can't serve as a meaningful
dataset. The safe path is to add an additive `classifier_evaluated` event and a
cleanup-surviving concordance ledger, run shadow mode first, and declare per-value
thresholds in templates, with `never` enforced by the compiler on override and irreversible
edges. Promotion to auto should be a human edit gated on minority-class recall keyed to
`template_hash`, with an env/config/template kill switch whose most restrictive setting
wins.
