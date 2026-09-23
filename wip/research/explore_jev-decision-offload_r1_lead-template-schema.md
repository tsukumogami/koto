# Lead: How would a koto template declare a classifier decision, and how would its state payload be assembled?

## Findings

### 1. What koto's template dialect already offers for a "decision"

A decision in koto today is an `accepts` field plus conditional `transitions`. The
source schema lives in `src/template/compile.rs` (`SourceState`, `SourceFieldSchema`,
`SourceGate`, `SourceActionDecl`) and the compiled form in `src/template/types.rs`
(`TemplateState`, `FieldSchema`, `Transition`).

- `FieldSchema` carries `field_type` (one of `enum`, `string`, `number`, `boolean`,
  `tasks` -- `VALID_FIELD_TYPES`, types.rs:680), `required`, `values: Vec<String>`, and
  a single field-level `description`. There is no per-value description. The meaning
  of each enum value lives in the state's markdown directive body instead. Real
  example: shirabe `skills/execute/koto-templates/execute.md`, state
  `worktree_discipline_check`, which declares `impact: [none, informational,
  intent-changing]` in YAML and explains each value in a bulleted list in the
  `## worktree_discipline_check` body.
- `when` clauses route on flat evidence keys, `gates.<gate>.<field>`,
  `evidence.<field>: present`, and `vars.<NAME>: {is_set: bool}`
  (`validate_evidence_routing`, types.rs:1809-2060).
- Compile-time guarantees already relevant to a classifier: Rule 2 (a `when` value
  on an enum field must be in `values`), Rule 4 (pairwise mutual exclusivity of
  conditional transitions), D3 (a `gates.*` key must name a declared gate and a field
  in that gate type's static schema, `gate_type_schema`, types.rs ~540), and the
  `skip_if` rules E-SKIP-TERMINAL / E-SKIP-NO-TRANSITIONS / E-SKIP-AMBIGUOUS.
- `koto next` renders the agent-facing prompt from this same declaration:
  `derive_expects` in `src/cli/next_types.rs` (~982) turns `FieldSchema` into
  `ExpectsFieldSchema { type, required, values, item_schema }` plus
  `options: [{target, when}]`. Note that `ExpectsFieldSchema` drops the field's
  `description` today -- the agent sees only the enum keys and the directive prose.

### 2. How the runtime resolves a decision (and where a fallback already exists)

`advance_until_stop` (`src/engine/advance.rs`, 545-1150) runs: action -> gates ->
evidence assembly -> `skip_if` -> `resolve_transition`. Evidence assembly merges
current-epoch agent evidence (`merge_epoch_evidence`, last-write-wins) with gate
output under `gates`. `resolve_transition` (advance.rs:1235) returns
`NeedsEvidence` whenever no conditional transition matches and either a gate failed
or the state was reached by auto-advance without fresh evidence. For a state with an
`accepts` block, `NeedsEvidence` becomes `StopReason::EvidenceRequired`, i.e. the
ordinary `evidence_required` response to the agent.

This is the key structural fact for this lead: **"fall back to the agent" already
is the default outcome** of a state whose required evidence is absent. A classifier
that writes evidence when confident and writes nothing otherwise gets fallback for
free, with no new response shape.

### 3. Sources available for assembling a Jev `state` payload

| Source | Exists today? | Where | Fit for Jev state |
|---|---|---|---|
| Template variables (`--var`) | yes | `bindings_from_events`, substitute.rs | Single-line, allowlisted `^[a-zA-Z0-9._/:@ \-]*$`; good for small labels, not content |
| `capture_stdout_as` | yes | `ActionDecl`, types.rs ~300 | Capped at 4096 bytes, same allowlist, no newlines -- too narrow for diffs/logs |
| `default_action` stdout | yes (log only) | `default_action_executed` event, 64KB/stream | Content exists in the event log but isn't addressable from a template |
| Gate output | yes | `gates.<name>` in merged evidence | Structured JSON but tiny: command gates emit only `exit_code`/`error` and discard stdout (execute.md's own prose complains about this) |
| Context store keys | yes | `ContextStore::get`, `src/session/context.rs`; gates already read it (`evaluate_context_exists_gate`, gate.rs:126) | Arbitrary bytes, per-session, already written by agents and by `default_action` commands via `koto context add` (supported per template-format.md) |
| Prior-state evidence | no | only current epoch is merged | Would need a new resolver over the event log (e.g. `<state>.<field>`) |
| Files on disk | no (only via command gates) | execution anchor | Better routed through context: a `default_action` does `koto context add <sess> <key> --from-file <path>` |

The context store is the natural carrier for bulky, multi-line state. It's already
the seam gates read, it's per-session and synced, and a `default_action` can fill it
deterministically (`koto context add` from an action is a documented, supported
pattern; only a nested `koto next` is refused).

### 4. Where a classifier declaration could sit -- three shapes

**(a) New gate type `classify`.** Fits the `evaluate_gates` closure the advance loop
already injects, emits a `GateEvaluated` event, and routes via `gates.<name>.choice`.
Problems: `gate_type_schema` is static per type, so D3 can check the field name
`choice` but not the value domain (Rule 2 only exists for accepts enums); gates carry
pass/fail semantics that don't mean anything for a three-way choice; and a failing
gate plus an accepts block produces an `evidence_required` whose `expects` has no
link to the question the classifier was asked, so the fallback prompt and the
classifier question are two separate declarations.

**(b) A classifier attached to an `accepts` field (recommended).** The accepts field
*is* the question: its `description` becomes Jev `instructions`, per-value
descriptions become `criteria`, and when confident the engine writes an
`evidence_submitted` event (with a source marker) exactly as if the agent had. Every
existing rule then applies unchanged: Rule 2, mutual exclusivity, compound `when`
with `gates.*`, `skip_if`. On low confidence nothing is written and the unchanged
`evidence_required` response goes to the agent, rendered from the same field. One
declaration, two consumers.

**(c) A separate state-level `decide:` block that names questions independently of
`accepts`.** Most flexible, but it duplicates the enum domain in two places and
needs its own routing namespace (`decide.<q>.choice`), losing the "same declaration
is the fallback prompt" property.

Recommendation: (b), plus a small state-level block for the parts that are shared
across all questions in one Jev request (inputs, default threshold), since Jev
evaluates several questions against one shared `state`.

### 5. Proposed syntax (in koto's actual dialect)

State level (new key on `SourceState`):

```yaml
classify:
  inputs:                 # ordered; each becomes one labelled entry in Jev `state`
    - context: <key>      # context-store key; {{KEY}} refs allowed, validated like gate `key`
      as: <label>
      max_bytes: 8000     # prune budget; over budget -> fall back, never truncate silently
    - var: <NAME>         # declared variable or capture name
    - gate: <gate_name>   # this state's gate output JSON
    - evidence: <state>.<field>   # prior-state evidence (needs a new resolver)
  min_confidence: 0.85    # default for every classified field in this state
```

Field level (new keys on `SourceFieldSchema`):

```yaml
accepts:
  <field>:
    type: enum | boolean   # score maps onto enum via bands; noul onto boolean
    description: <question text>      # -> Jev instructions AND agent prompt
    criteria: {<value>: <description>, ...}   # enum: -> Jev choice criteria; also rendered in expects
    classify:
      primitive: choice | score | noul
      escape: {<key>: <description>}   # choice/score: mandatory, see section 6
      levels: [<anchor0>, <anchor1>, ...]   # score only, 2-10
      bands: {<value>: [lo, hi], ...}       # score only: expected-value ranges -> enum value
      min_confidence: 0.9                    # override
      true_above: 0.9                        # noul only
      false_below: 0.1                       # noul only
```

Mapping onto Jev's primitives:

- **choice**: `values` + `criteria` + `escape` -> Jev `criteria` map (2-255 keys,
  escape included). Winner written to `<field>` if `confidence >= min_confidence`
  and winner is not the escape key.
- **score**: `levels` -> Jev ordered `criteria` list (2-10). koto (not Jev) turns
  the returned expected value into an enum value through `bands`, which keeps the
  arithmetic in deterministic code as the Jev doc demands. A score that lands in a
  gap between bands falls back. The routed field stays an ordinary `enum`, so `when`
  clauses and the agent prompt don't change.
- **noul**: `description` is the proposition. Written as `true` when
  `P >= true_above`, `false` when `P <= false_below`, fallback in between. Noul has
  no separate confidence (the doc says P itself is the confidence), so
  `min_confidence` is rejected on noul fields.

### 6. Compile-time enforcement

New rules, in the existing E/W vocabulary style:

- **E-CLASSIFY-ESCAPE**: every `choice` field must declare exactly one `escape`
  key, and a `score` should declare one too (Jev is closed-world; out-of-domain
  input is otherwise forced onto the nearest wrong category with high confidence).
  The escape key must not collide with a `values` entry.
- **E-CLASSIFY-ESCAPE-ROUTED**: the escape key must not appear in any `when`
  clause. It is a sentinel meaning "hand this to the agent", not a destination.
  (Optional relaxation: allow routing it to an explicit `needs_human` state.)
- **E-CLASSIFY-CRITERIA**: `criteria` keys must equal `values` exactly; every
  description non-empty; field `description` non-empty (Jev never sees the key
  names, so an undocumented key is an unlabelled class).
- **E-CLASSIFY-ARITY**: choice 2-255 keys including escape; score 2-10 levels;
  `bands` cover only declared enum values, ranges ordered, non-overlapping, within
  `[0, levels-1]`.
- **E-CLASSIFY-THRESHOLD**: thresholds in `[0.5, 1.0]`; `false_below < true_above`;
  `min_confidence` refused on noul.
- **E-CLASSIFY-INPUTS**: `context` keys pass `unusable_context_key_reason` when
  literal; `{{KEY}}` refs validated through the same union (variables + captures +
  runtime names) the compiler already uses; `gate:` names a gate on this state;
  `var:` names a declared variable or capture; a state with classified fields and
  no inputs is an error (Jev would classify an empty state).
- **E-CLASSIFY-REQUIRED**: every `required: true` field on a classified state must
  itself be classified, or the classifier can't produce a complete submission.
  (This bites shirabe's pattern of a required `rationale` string next to a verdict.)
- **W-CLASSIFY-UNROUTED**: a non-escape value that no `when` clause references
  (a confident classification that leads to `NeedsEvidence` anyway).
- **W-CLASSIFY-AGENT-TEXT**: an input sourced from agent-authored evidence or an
  agent-written context key (injection exposure); not an error, since that's often
  the only source.
- Weak lint, maybe: criteria text containing digits/comparatives/dates ("more
  than 3", "before", "within") hints at arithmetic or temporal reasoning Jev can't do.

Runtime-only checks (can't be done at compile time): assembled state over 32k
tokens or over a `max_bytes` budget -> fall back; missing context key -> fall back
with a recorded reason; no API key -> skip the classifier entirely.

### 7. Outcome -> transitions, and the fallback path

With shape (b), mapping is trivial: the classifier outcome is evidence, so existing
`when` clauses route it. The engine should treat all classified fields on a state as
one unit -- if any required classified field falls back, write none of them and
return the normal `evidence_required`. Partial writes would leave the agent
submitting half a record against half a machine-written one, and last-write-wins
merging would blur the audit trail.

The fallback prompt comes from the same declaration with two differences:

1. The escape key is stripped from the agent-facing `values` (the agent is the
   fallback; it can't punt to itself).
2. `ExpectsFieldSchema` would need to start carrying `description` and `criteria`
   (it drops `description` today). That also retires the duplicated per-value prose
   in directive bodies like `## worktree_discipline_check`.

Whether to show the agent the classifier's distribution on fallback ("leaned
`informational`, 0.62") is open; it helps on near-misses and anchors on bad ones.

### 8. Example 1 -- choice, from shirabe `/execute`'s `worktree_discipline_check`

Today the state asks the agent to classify upstream drift, write
`wip/work-on_{{PLAN_SLUG}}_impact.json`, and submit `impact`; a command gate checks
the file exists. Classifier-shaped version: the preceding state (`worktree_sync`)
deterministically writes a drift summary into the context store, and this state
becomes pure judgment.

```yaml
  worktree_sync:
    default_action:
      # deterministic: counts and file lists computed in code, not by Jev
      command: >-
        git log --stat --format='%h %s' "$(git merge-base HEAD origin/main)"..origin/main
        | koto context add {{SESSION_NAME}} upstream_drift.txt
    # ... existing gates/transitions ...

  worktree_discipline_check:
    classify:
      inputs:
        - context: upstream_drift.txt
          as: upstream_changes
          max_bytes: 12000
        - context: plan_scope.md          # PLAN's referenced files + intent, written at setup
          as: plan_scope
      min_confidence: 0.85
    accepts:
      impact:
        type: enum
        required: true
        values: [none, informational, intent-changing]
        description: >
          How do the commits main gained since this branch forked affect the
          intent of the PLAN described in plan_scope?
        criteria:
          none: Main has not advanced, or its commits touch no file, contract, or concept plan_scope names.
          informational: Main advanced in ways plan_scope does not rely on, such as docs, unrelated tests, or unrelated modules.
          intent-changing: Main deleted, renamed, or changed a file, interface, or behaviour plan_scope depends on.
        classify:
          primitive: choice
          escape:
            unclear: The change summary is empty, truncated, or the overlap with plan_scope cannot be judged from the summary alone.
      rationale:
        type: string
        required: false
        description: Rationale when impact is intent-changing
    transitions:
      - target: spawn_and_await
        when:
          impact: none
      - target: spawn_and_await
        when:
          impact: informational
      - target: escalate_upstream_drift
        when:
          impact: intent-changing
```

Notes: the `impact_classified` command gate goes away (the classifier path has no
file to write); if a file record is still wanted, it's derived from the logged
decision. `unclear` appears in no `when`, so escape and low confidence both produce
the ordinary `evidence_required` with `values: [none, informational,
intent-changing]` and the criteria as the agent's instructions.

### 9. Example 2 -- score + noul, on `/work-on`'s `staleness_check`

Today: a command gate pipes `check-staleness.sh` JSON through `jq`, and the agent
submits `staleness_signal` from `[fresh, stale_requires_introspection, override,
blocked]`. The numbers stay deterministic; the judgment moves to Jev.

```yaml
  staleness_check:
    default_action:
      command: 'check-staleness.sh --issue {{ISSUE_NUMBER}} | koto context add {{SESSION_NAME}} staleness.json'
    classify:
      inputs:
        - context: staleness.json        # precomputed: commits since filed, files touched, days elapsed as a number
          as: staleness_facts
        - context: context.md            # issue body, written by context_injection
          as: issue
          max_bytes: 8000
    accepts:
      staleness_signal:
        type: enum
        required: true
        values: [fresh, stale_requires_introspection, override, blocked]
        description: How far has the codebase moved away from what this issue assumes?
        classify:
          primitive: score
          levels:
            - Nothing the issue mentions has changed since it was filed.
            - Nearby code changed, but every file and API the issue names is intact.
            - Some file or API the issue names was modified; the approach probably still holds.
            - A file, API, or behaviour the issue depends on was removed, renamed, or redesigned.
          escape:
            cannot_judge: The staleness facts or the issue body are missing or unreadable.
          bands:
            fresh: [0.0, 1.2]
            stale_requires_introspection: [2.2, 3.0]
          min_confidence: 0.8
      issue_superseded:
        type: boolean
        description: Does the issue body or the recent changes show this issue was already fixed or superseded by other work?
        classify:
          primitive: noul
          true_above: 0.9
          false_below: 0.15
    transitions:
      - target: introspection
        when:
          staleness_signal: stale_requires_introspection
      - target: analysis
        when:
          staleness_signal: fresh
          issue_superseded: false
      - target: introspection
        when:
          staleness_signal: fresh
          issue_superseded: true
      - target: analysis
        when:
          staleness_signal: override
      - target: done_blocked
        when:
          staleness_signal: blocked
```

`override` and `blocked` are values the classifier never emits (no band maps to
them); they remain agent-only escape valves. That suggests one more rule: bands
need not cover every enum value, and uncovered values stay available on the
fallback path. An expected score of 1.6 falls between bands and goes to the agent.
(The example also shows why E-CLASSIFY-REQUIRED matters: `issue_superseded` isn't
required, but if it fell back while `staleness_signal` was confident, the unit rule
in section 7 would send both to the agent.)

## Implications

- The cheapest correct design reuses the evidence path: a classifier is an
  automatic evidence submitter for declared accepts fields. Routing, validation,
  mutual exclusivity, `skip_if`, and the fallback response all stay as they are,
  and the agent-facing prompt is the same declaration minus the escape key.
- Two schema additions do most of the work: per-value `criteria` on enum fields
  (useful even without Jev, since it moves semantics out of directive prose into
  `expects`), and a `classify` block. `ExpectsFieldSchema` has to start carrying
  `description`/`criteria`.
- State assembly should go through the context store. It's already the seam gates
  read, it holds multi-line content, and `default_action` + `koto context add`
  fills it deterministically. That fits Jev's own guidance: compute counts and
  time deltas in code, pass pruned facts.
- Shirabe states have to split into gather -> decide -> act before they're
  classifier-eligible. Most current decision states mix "do the work, write a wip
  file" with "report a verdict", and gate on the file.
- Score routing needs koto-side banding, which is itself arithmetic the engine must
  own and log.

## Surprises

- `ExpectsFieldSchema` drops the field `description`, so the agent never sees
  the per-field text the template author wrote. Value semantics survive only
  because shirabe authors restate them in directive prose.
- Shirabe templates use `context_assignments` on transitions
  (`work-on.md`, `execute.md`) with `${evidence.detail}` interpolation, but koto's
  `SourceTransition` has no such field and isn't `deny_unknown_fields`, so koto
  silently drops it. A `classify` key placed somewhere without strict parsing would
  fail the same quiet way on a typo.
- The strictness differs by nesting level, and that's a versioning lever:
  `SourceState` is `deny_unknown_fields` (an older koto *rejects* a state-level
  `classify:`), while `SourceFieldSchema` is not (an older koto *ignores*
  field-level `criteria`/`classify` and just asks the agent). Putting everything at
  field level buys graceful degradation on old binaries; putting inputs at state
  level buys a loud failure. Worth a deliberate choice.
- There's no way today to reference a prior state's evidence from a template; the
  merged evidence map is current-epoch only. "State from prior evidence" needs a new
  resolver, or earlier states must write what later classifiers need into context.
- `koto decisions record/list` (`DecisionSummary {choice, rationale,
  alternatives_considered}`, next_types.rs:49) already exists as a decision log, a
  plausible home for the classifier's distribution and confidence.

## Open Questions

- Field-level vs state-level placement of `inputs` (graceful degradation vs loud
  failure on older koto). Could `inputs` live per field and be de-duplicated at
  request time?
- Should the escape key be allowed to route to an explicit state, or always mean
  "fall back to agent"?
- On fallback, should `expects` include the classifier's distribution? Helps on
  near-misses, anchors on errors, and exposes the decision the lead says the agent
  "never sees".
- How do classified states coexist with gates whose `when` clauses combine agent
  evidence and `gates.*` (e.g. `status: completed` + `gates.baseline_exists.exists:
  true`)? If the classifier is confident but the gate fails, the agent gets
  `evidence_required` with a pre-written value in the epoch it can't see.
- String fields next to a verdict (`rationale`, `detail`): leave empty, fill with a
  machine summary of the distribution, or forbid them as required on classified
  states (E-CLASSIFY-REQUIRED as proposed)?
- Replay: should the assembled `state` payload (or its hash plus context-key
  revisions) be logged so a decision can be re-scored or audited later? Owned by the
  operational-constraints lead, but the schema has to make inputs enumerable for it.
- Is a template-level default threshold (frontmatter `classify.min_confidence`)
  plus a user/config override for shadow mode needed, or is per-field enough?

## Summary

A classifier decision fits koto best as an automatic evidence submitter for an existing `accepts` field: the field's description and new per-value `criteria` become Jev's instructions and criteria, a `classify` block picks choice/score/noul with thresholds and a mandatory escape key, and confident answers are written as ordinary evidence, so `when` routing, Rule 2, and mutual exclusivity work unchanged. Low confidence, an escape pick, or no API key just writes nothing, and the existing `evidence_required` response, rendered from the same declaration minus the escape key, is the fallback, but `ExpectsFieldSchema` must start carrying descriptions/criteria since it drops them today. State payloads should be assembled from labelled context-store keys (filled deterministically by `default_action` + `koto context add`), variables, and gate output, with new compile rules for escape hatches, arity, thresholds, inputs, and required fields; there's no template path to prior-state evidence yet, and shirabe states must be split into gather/decide/act before they qualify.
