# Lead: What is the full blast radius, in tests, fixtures, evals and documentation, of making a self-transition suppress phase details?

All paths relative to the koto repo root
(`/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_90_self_loop-73478d7e/public/koto/.claude/worktrees/koto-90-self-loop`).

## Findings

### 1. `tests/instructions_delivery_test.rs` (592 lines, `#![cfg(unix)]`)

#### Harness

- `koto_cmd` (L18-24): `Command::cargo_bin("koto")`, `current_dir(dir)`, `KOTO_SESSIONS_BASE` and `HOME` both pointed at a tempdir.
- `run_koto` (L34-46): runs, asserts success, parses the **last non-blank stdout line** as JSON.
- `details_of` (L48-50), `assert_carries` (L52-60), `assert_omits` (L62-69). The two assertion helpers are the only thing every test flips between.

#### `DELIVERY_TEMPLATE` (L81-145) — verbatim

Doc comment above it (L75-80):

```
/// `gather` and `implement` both declare instructions. `implement` declares a
/// self-transition (`loop_again: yes`), a loop-back into `gather`
/// (`loop_again: redo`), and a terminal exit (`loop_again: no`), and is also a
/// valid `--to` target from `gather`, so the same template covers conditional,
/// unconditional (via `relay`), self, loop-back, directed, rewind, and
/// override arrivals.
```

```rust
const DELIVERY_TEMPLATE: &str = r#"---
name: delivery
version: "1.0"
initial_state: gather
states:
  gather:
    accepts:
      route:
        type: enum
        required: true
        values: [direct, indirect]
    transitions:
      - target: implement
        when:
          route: direct
      - target: relay
        when:
          route: indirect
  relay:
    transitions:
      - target: implement
  implement:
    accepts:
      loop_again:
        type: enum
        required: true
        values: [yes, no, redo]
    transitions:
      - target: implement
        when:
          loop_again: yes
      - target: gather
        when:
          loop_again: redo
      - target: done
        when:
          loop_again: no
  done:
    terminal: true
---

## gather

Collect the inputs.

<!-- details -->

Gather instructions.

## relay

Hand off to the implementer.

## implement

Make the change.

<!-- details -->

Implement instructions.

## done

All done.
"#;
```

**The `implement` phase's self-transition** is L108-111:

```yaml
    transitions:
      - target: implement
        when:
          loop_again: yes
```

So `koto next wf --with-data '{"loop_again":"yes"}'` while at `implement` is the
conditional self-transition, and `koto next wf --to implement` while at
`implement` is the directed self-transition. Both are exercised by tests below.

Two other templates: `GATE_BLOCKED_TEMPLATE` (L149-176, single `guarded` state
with a `context-exists` gate on `approval_note`, declares details "Guarded
instructions.") and `PARENT_TEMPLATE`/`CHILD_TEMPLATE` (L178-251, batch fan-out;
the child's `work` state declares "Child work instructions.").

#### Every test case

| # | Test (line) | Sequence | Asserts | Verdict under new rule |
|---|---|---|---|---|
| 1 | `init_then_first_tick_carries_details_for_initial_phase` (L264-276) | `init` → `next` | carries "Gather instructions." | unchanged |
| 2 | `batch_spawned_child_first_tick_carries_details_for_its_own_initial_phase` (L278-304) | parent `init` → parent `next --with-data tasks` → `next par.a` | child carries "Child work instructions." | unchanged |
| 3 | `conditional_transition_arrival_carries_details` (L310-330) | `init` → `next` → `next --with-data '{"route":"direct"}'` | carries "Implement instructions." | unchanged (gather → implement is a cross-phase arrival) |
| 4 | `unconditional_transition_arrival_carries_details_separately_from_conditional` (L332-356) | `init` → `next` → `next --with-data '{"route":"indirect"}'` (chains gather→relay→implement) | `state == "implement"` and carries "Implement instructions." | unchanged |
| 5 | **`self_transition_arrival_carries_details_again` (L358-381)** | `init` → `next` → `next '{"route":"direct"}'` → `next '{"loop_again":"yes"}'` | `state == "implement"` and **`assert_carries(&resp, "Implement instructions.", "self-transition arrival")`** | **MUST FLIP to `assert_omits`.** This is the single most direct encoding of the old rule. Its comment (L373-374) — "First occupancy of `implement` has already delivered. A self-transition starts a fresh occupancy, so delivery happens again." — must be rewritten, and the test name is now wrong (`..._carries_details_again`). |
| 6 | `loop_back_arrival_at_previously_occupied_phase_carries_details_again` (L383-408) | `init` → `next` → `'{"route":"direct"}'` → `'{"loop_again":"redo"}'` (implement → gather) | `state == "gather"`, carries "Gather instructions." | unchanged. Its comment explicitly says this is "the case a visit-count predicate gets backwards" — still true, and this test is the guard that the fix does not over-suppress loop-backs. |
| 7 | `rewind_arrival_carries_details` (L410-429) | `init` → `next` → `'{"route":"direct"}'` → `rewind` → `next` | `state == "gather"`, carries "Gather instructions." | unchanged |
| 8 | `gate_blocked_first_tick_carries_and_repeat_omits` (L435-455) | gate template: `init` → `next` (carries) → `next` (omits) | `action == "gate_blocked"` both times | unchanged (non-advancing re-tick) |
| 9 | `directed_transition_carries_then_nonadvancing_tick_omits` (L461-484) | `init` → `next` → `next --to implement` (carries) → `next` (omits) | as named | unchanged — the `--to` here is gather → implement, a cross-phase directed arrival |
| 10 | **`two_consecutive_directed_transitions_into_same_phase_both_carry` (L486-514)** | `init` → `next` → `next --to implement` (carries) → `next --to implement` (**carries**) | second call `assert_carries(..., "second directed transition (self-transition)")` | **MUST FLIP: the second `--to implement` is a directed self-transition and must now omit.** Test name and the comment at L505-507 ("a second directed transition into it is valid and is a fresh occupancy") both need rewriting. |
| 11 | `full_override_returns_details_on_a_response_that_would_otherwise_be_suppressed` (L520-547) | `init` → `next` → `'{"route":"direct"}'` → `next` (asserts omits) → `next --full` (asserts carries) | as named | unchanged |
| 12 | `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions` (L555-592) | `init` → `next` → `next '{"route":"direct"}' --full` (carries) → `next` (omits) | as named | unchanged |

**Precisely which assertions must flip: exactly two, both in this file.**
`tests/instructions_delivery_test.rs:380` (`assert_carries` → `assert_omits`,
plus drop the now-meaningless `expected` argument) and
`tests/instructions_delivery_test.rs:508-513` (same). Both tests also need
renaming and their comments rewritten. The module doc comment (L1-10) and the
`DELIVERY_TEMPLATE` doc comment (L75-80) both enumerate "self" as an arrival that
"the design's predicate must handle uniformly" — prose that survives but should
be reworded, because "self" moves from the deliver list to the suppress list.

Nothing else in the file needs to change: the template already contains every
shape needed for a *new* test asserting that a conditional self-transition
suppresses while a loop-back through `gather` and back into `implement`
re-delivers.

### 2. Unit tests in `src/engine/persistence.rs`

#### Production code that defines the rule

- `occupancy_slice` (L1028-1046), private, with a 31-line doc comment (L997-1027)
  whose second sentence is the rule being changed:

  > An occupancy begins when a state-entry event names the phase as its target
  > and ends when the next state-entry event names any phase, **including the same
  > one. So a self-transition ends one occupancy and begins another**, and so does
  > a rewind into the phase: both append an entry event naming it. State-entry
  > events are `Transitioned`, `DirectedTransition`, and `Rewound`.

- `latest_epoch_gate_failed` (L1058-1070) — **shares `occupancy_slice`**, by
  explicit design ("Shared rather than copied so the predicates built on it ...
  cannot come to disagree about where an occupancy starts", L1017-1019). Its
  consumers are `src/workflows_surface/project.rs:183` and
  `src/cli/dashboard_data.rs:458`. Changing `occupancy_slice` itself therefore
  also changes blocked-classification epochs across a self-transition, for the
  dashboard and the `/workflows` projection. No unit test in `persistence.rs`
  covers `latest_epoch_gate_failed` directly, so that shift would land silently.
- `instructions_delivered_this_occupancy` (L1099-1106), doc comment L1072-1098,
  which says the occupancy is `occupancy_slice`'s "so self-transition, rewind, and
  arrival at the initial state all fall out of one definition shared with
  `latest_epoch_gate_failed`" (L1082-1085).

#### The `#[cfg(test)] mod tests` block (opens L1109)

Helpers for this section, L2503-2544: `transitioned(seq, from, to)`,
`rewound(seq, from, to)`, `directed(seq, from, to)`, `delivered(seq, state)`.

| Test (line) | Event sequence built | Assertion | Verdict |
|---|---|---|---|
| `instructions_delivered_false_when_nothing_was_delivered` (L2546-2550) | `T(1,None→gather)` | `!delivered("gather")` | unchanged |
| `instructions_delivered_true_within_the_current_occupancy` (L2552-2567) | `T(1,None→gather)`, `D(2,gather)`, `EvidenceSubmitted(3,gather)` | `delivered("gather")` | unchanged |
| `instructions_delivered_false_when_the_record_predates_the_entry_event` (L2569-2580) | `T(1,None→gather)`, `D(2,gather)`, `T(3,gather→implement)`, `T(4,implement→gather)` | `!delivered("gather")` | unchanged — this is a loop-back, not a self-transition |
| `instructions_delivered_resets_on_arrival_by_rewind` (L2582-2592) | `T(1,None→gather)`, `D(2,gather)`, `T(3,gather→implement)`, `D(4,implement)`, `R(5,implement→gather)` | `!delivered("gather")` | unchanged |
| **`instructions_delivered_resets_on_a_self_transition` (L2594-2609)** | `T(1,None→review)`, `D(2,review)`, `T(3,review→review)`; then push `D(4,review)` | `!delivered("review")`, then `delivered("review")` | **MUST FLIP.** First assertion becomes `assert!(instructions_delivered_this_occupancy(&events, "review"))`. The second half becomes vacuous (a delivery at seq 4 answers trivially) — the test should be renamed (`..._survives_a_self_transition`) and its second half rewritten or replaced, e.g. by asserting that a delivery *before* the self-transition still answers after it. The comment at L2596-2597 — "A self-transition ends one occupancy and begins another, so the delivery recorded before it does not answer for the new one" — is the exact sentence to invert. |
| `instructions_delivered_ignores_an_intermediate_phases_record` (L2611-2632) | `T(1,None→gather)`, `D(2,gather)`, `T(3,gather→implement)`, `D(4,implement)`, `T(5,implement→verify)`; then push `D(6,implement)` | `!delivered("verify")` twice | unchanged. **Important guard**: it proves the name check on `InstructionsDelivered { state }` is load-bearing, and any fix that widens the slice past a self-entry must keep that check meaningful. |
| **`instructions_delivered_resets_on_arrival_by_directed_transition` (L2634-2649)** | `T(1,None→gather)`, `Dir(2,gather→implement)`, `D(3,implement)` → asserts `delivered`; then push `Dir(4,implement→implement)` → asserts `!delivered` | second assertion | **MUST FLIP.** `Dir(4, "implement" → "implement")` is a directed self-transition; the assertion becomes `assert!(...)`. The first half (cross-phase directed arrival) stays. Comment at L2636-2638 — "Two consecutive directed transitions into the same phase are two occupancies, exactly as two self-transitions are" — inverts. |
| `instructions_delivered_reads_the_whole_log_when_no_entry_event_names_the_state` (L2651-2658) | `D(1,gather)` only | `delivered("gather")`, `!delivered("implement")` | unchanged |

So: **two of eight unit tests flip**, plus the `occupancy_slice` doc comment
(L1000-1004), the `instructions_delivered_this_occupancy` doc comment
(L1082-1085), and whatever the fix does to the shared-helper contract.

Note the "safe direction" clause already in the doc (L1092-1095): a mismatched
record reads as "not delivered" so koto re-delivers. Suppressing on a self-loop
moves the failure direction for that one case from "delivers twice" to "never
delivers", which is worth calling out in the design.

### 3. `tests/next_response_baseline.rs` + `tests/fixtures/next-response-baseline/instruction-free.json`

**What it pins.** `capture()` (L445-526) runs 13 named `Sequence`s (L313-441),
each in a fresh `TempDir` with five templates written out, recording the **raw
stdout bytes** of every `record(...)` step (setup steps run but are not
recorded). The result is a pretty-printed JSON document
`{"notes": NOTES, "sequences": [{label, description, responses:[{argv, stdout}]}]}`
with a trailing newline. Test 1,
`instruction_free_responses_are_byte_identical_to_the_baseline` (L532-570),
compares that document to the fixture as **whole-string equality** — deliberately
not `serde_json::Value` equality, because key ordering differs between the
natural-advancement path (sorted `serde_json::Map`) and the directed path
(struct field order), and both orderings are part of what's pinned (L18-23).

Test 2, `baseline_fixture_covers_every_required_sequence_and_stays_instruction_free`
(L576-667), independently asserts: all 13 labels present (L587-601); the set of
`action` values across all recorded bodies is exactly `["confirm","done",
"evidence_required","gate_blocked","integration_unavailable"]` (L629-639); every
sequence records at least one response; and **no recorded body carries a
`details` key** (L658-664).

Test 3, `regenerate_baseline_fixture` (L681-689), is `#[ignore]`d.

**Can the change affect it?** All five templates are instruction-free by
construction — no `<!-- details -->` marker anywhere (L46-53 says so explicitly),
so `TemplateState::details` is empty for every phase and no code path can put a
`details` key in any recorded body regardless of the predicate's answer. The
`self-transition-arrival` sequence (L360-369) and the two-call
`directed-transition` sequence (L370-379) both exercise self-transitions and both
record bodies with no `details`; I confirmed against the fixture that their
recorded stdout is byte-identical to the corresponding non-self arrivals. So the
predicate change cannot move a byte here, and **`instruction-free.json` must stay
green unmodified.** If it goes red, the change leaked into the instruction-free
path — exactly the R6 violation the harness exists to catch.

**The one real trap.** The fixture embeds the test file's `NOTES` array
(L261-270) and each sequence's `description` string. Two of those strings state
the old rule:

- `SEQUENCES[5].description` (L362, fixture line ~84): `"`implement` transitions to itself, ending one occupancy and beginning another."`
- `SEQUENCES[6].description` (L372): `"Two consecutive `--to` transitions into `implement`. The second is reachable only because `implement` declares itself as a target. Note the key order differs from the natural-advancement path."` (this one survives)
- `NOTES[7]` (L269): "...the conditional and unconditional and self-transition arrivals..." are identical bodies — still true, since none carry details.

Editing the L362 description in the `.rs` file **fails the byte comparison**
unless the fixture's `description` field is edited to match exactly. The test's
own panic message calls this out: "the document also embeds this file's `NOTES`
and the per-sequence `description` strings, so editing that prose trips this test
too. A diff confined to those lines is the harmless case." Recommendation: leave
both alone. If the team insists on fixing the stale sentence, both files must be
edited in lockstep, character for character, and the diff must be confined to
that one line.

### 4. `plugins/koto-skills/skills/koto-user/evals/evals.json`

#### Schema

Top level: `{"skill_name": "koto-user", "evals": [...]}`. Each eval object, in the
order the file uses them:

```json
{
  "id": 12,                       // int, sequential; next free id is 13
  "name": "details-redelivered-after-rewind",   // kebab-case; becomes the eval's directory name
  "prompt": "...",                // the user turn, with fenced JSON blocks escaped inline
  "expected_output": "...",       // prose description of the correct answer
  "files": [],                    // present on every eval, empty on all 12
  "assertions": ["...", "..."]    // 3-4 prose assertions, graded by a model
}
```

`scripts/run-evals.sh` also honours an optional `"fixture_dir"` key (relative to
`evals/`, copied into the eval's `inputs/`); no koto-user eval uses it. Note
`files` is read by nothing in `run-evals.sh` — the prep step consumes only `id`,
`name`, `prompt`, `assertions`, and `fixture_dir`.

#### Eval 11 (L132-144) — verbatim reproduction

- `"id": 11`, `"name": "details-omitted-on-repeat-tick-same-occupancy"`.
- Prompt: workflow `ship-it`; first `koto next ship-it` returned a `gate_blocked`
  body at `state: "preflight"`, `directive: "CI must pass before merge."`, **with**
  `"details":"Run the full preflight checklist: lint, unit tests, and the smoke
  suite against staging. Fix any failures and re-run this command."`, plus a failed
  `ci_check` command gate (`category: "corrective"`, `agent_actionable: true`).
  User fixed nothing and re-ran; second body is identical **minus** `details`.
  Question: "Why is `details` missing this time, and is this a bug I should
  report?"
- `expected_output`: "Agent explains this is expected, not a bug: `details` is
  delivered once per occupancy of a state, and re-ticking the same failing gate
  without leaving and re-entering `preflight` stays in the same occupancy, so
  `details` is correctly omitted on the second call. It should NOT tell the user
  to report a bug or treat this as an error. It may mention `--full` to force
  `details` back, or `koto status ship-it` to retrieve it unconditionally."
- Assertions (4): (a) missing `details` is expected, not a bug; (b) "Response
  explains the delivery-per-occupancy rule: `details` is delivered once per
  occupancy of a state and omitted on further ticks that don't leave and re-enter
  it"; (c) does not tell the user to file a bug; (d) mentions `--full` or
  `koto status ship-it`.

**Survives.** The scenario is a gate-blocked non-advancing re-tick, whose
behaviour is unchanged. Only the *wording* of `expected_output` and assertion (b)
is affected: "don't leave and re-enter it" now describes an incorrect boundary
(leaving and re-entering the same phase would no longer re-deliver). Rewording
those two strings is a legitimate edit; deleting assertion (b) is not.

#### Eval 12 (L145-156) — verbatim reproduction

- `"id": 12`, `"name": "details-redelivered-after-rewind"`.
- Prompt: workflow `audit-pass`; the agent already saw `details` for the
  `remediate` state and advanced past it; a reviewer asked for a redo, so the
  agent ran `koto rewind audit-pass` and landed back on `remediate`. Question:
  "If I call `koto next audit-pass` now, will I see the remediation checklist
  again, or do I need to pass `--full` to get it back?"
- `expected_output`: "Agent explains that `koto rewind` landing back on
  `remediate` starts a new occupancy of that state, so the next plain `koto next
  audit-pass` call will deliver `details` again automatically -- `--full` is not
  required for this case (though it would also work)."
- Assertions (4): (a) `koto next audit-pass` shows `details` again without
  `--full`; (b) rewind starts a new occupancy, hence re-delivery; (c) does NOT
  claim `--full` is required; (d) "Response distinguishes this from a plain repeat
  tick that stays in the same occupancy, where `details` would stay omitted".

**Survives entirely unchanged.** The rewind in the prompt is from a *later* phase
back into `remediate`, which stays a delivering arrival.

#### Shape for a new eval 13

Same six keys, `"id": 13`, kebab-case name (e.g.
`details-omitted-on-self-loop-retry`), a prompt containing two fenced JSON
`koto next` bodies where the state is unchanged and the second lacks `details`
but with `"advanced": true` (the distinguishing signal from eval 11's
`"advanced": false`), `"files": []`, and 3-4 prose assertions. To be
discriminating against eval 11 it should force the agent to explain that the
*self-transition advanced the workflow* and still suppressed, and that a
loop-back from a different phase would not.

#### How evals are run and validated

- Runner: `scripts/run-evals.sh` (`<skill>` | `--all` | `--list` | `--validate
  <skill>` | `--prep-only <skill>`). Prereqs: the `claude` CLI, `python3`, and the
  skill-creator plugin (L20, checks at L30-31). It preps
  `plugins/<plugin>/skills/<name>/evals/workspace/iteration-<N>/<eval_name>/` with
  `with_skill/outputs/`, `without_skill/outputs/`, and an `eval_metadata.json`
  holding `{eval_id, eval_name, prompt, assertions}` (L133-190), then executes and
  grades. Exit codes: 0 all pass, 1 assertion failure, 2 no results, 3 missing
  prereqs.
- CI does **not** run the evals. `.github/workflows/eval-plugins.yml` job
  `eval-coverage` runs only `bash scripts/check-evals-exist.sh`, which asserts
  every `plugins/*/skills/*/` has an `evals/evals.json` with `len(evals) >= 1`
  (script L39-51). A second job `no-hooks` checks no `hooks.json` sits in a skill
  directory. So adding eval 13 is CI-free; validating it is a manual
  `scripts/run-evals.sh koto-user`.

### 5. Documentation surfaces

#### `plugins/koto-skills/skills/koto-user/references/response-shapes.md` — 5 passages

**L37-47** (the normative paragraph; the whole thing must be rewritten):

> `details` follows a delivery rule, not a visit-count: a phase's `details` are
> delivered once per **occupancy** of that phase, then omitted on any further tick
> that doesn't leave and re-enter the phase, unless `--full` is passed. An occupancy
> begins whenever the workflow enters a phase — including a rewind back into it, a
> self-transition, or a directed (`--to`) transition — and ends when the workflow
> next enters any phase (including the same one again). Concretely: a gate-blocked
> loop that keeps re-evaluating the same failing gate without transitioning stays in
> one occupancy, so `details` shows once and is omitted on every retry after that;
> a `koto rewind` that lands back on the phase starts a new occupancy, so `details`
> is delivered again on the next `koto next`. It is always absent on `done`
> regardless.

**L16** (field-presence table row, unaffected but worth a glance):
`| `details` | conditional | conditional | conditional | conditional | **absent** | conditional |`

**L74** (scenario (a) example body): `"details": "Extended guidance shown once per occupancy of this state.",`

**L107-108** (scenario (a) decision point):

> - `details` is omitted once it's already been delivered for the current occupancy of
>   this phase, unless `--full` is passed.

**L168-171** (scenario (b) decision point — the sentence that names self-transition as a re-delivering arrival):

> - `details` is absent here because it was already delivered earlier in this occupancy
>   of the state (for example, an earlier tick against the same failing gate). It would
>   appear again if the workflow left and re-entered this state — via rewind, a
>   self-transition, or a directed transition — starting a new occupancy.

**L549-552** (the "Checking for absent fields" list):

> - Check whether `details` is present before reading it; it may be omitted on any action
>   type once it's already been delivered for the phase's current occupancy. Use
>   `koto status <name>` (see `command-reference.md`) to retrieve it unconditionally
>   without depending on delivery state.

(L420, `- `details` is **absent** — the terminal variant has no `details` field.`,
is unaffected.)

#### `plugins/koto-skills/skills/koto-user/references/command-reference.md` — 1 passage

**L96** (the `--full` flag row of the `koto next` table):

> | `--full` | Always include the `details` field, even if it was already delivered earlier in this occupancy of the state. By default `details` is omitted once delivered, until the workflow leaves and re-enters the state. |

The trailing clause "until the workflow leaves and re-enters the state" becomes
wrong for a self-loop. L297-326 (the `koto status` retrieval section) describes
unconditional retrieval and is unaffected.

#### `plugins/koto-skills/skills/koto-author/SKILL.md` — 1 passage

**L67** (step 3 of the runtime loop):

> 3. Read the `directive` for instructions. A `details` field may contain extended guidance -- it's delivered once per occupancy of a state (each time the workflow enters it, **including a rewind or a self-transition back into it**) and omitted on further ticks that don't leave and re-enter the state (pass `--full` to force it through anyway). `koto status <session-name>` retrieves the current state's `directive`/`details`/`expects` unconditionally, without depending on delivery state -- useful for recovering guidance you've lost track of

(L154 mentions the skill's own template has a "self-loop" as an example — that's
a topology reference, not a delivery-rule statement.)

#### `plugins/koto-skills/skills/koto-author/references/template-format.md` — 3 passages

**L118**:

> Content before the marker is the **directive** -- always returned by `koto next`. Content after is the **details** -- delivered once per **occupancy** of the state, or whenever the caller passes `--full`.

**L120** (the author-facing guarantee, the most consequential doc change in the repo):

> An occupancy begins when the workflow enters a state -- including a rewind back into it, a self-transition, or a directed (`--to`) transition -- and ends when the workflow next enters any state (including the same one again). This is what an author can rely on: a gate-blocked self-loop that keeps re-evaluating the same failing gate without transitioning stays in one occupancy, so the agent sees `details` once and not again on every retry -- don't write directive text that assumes it repeats. A rewind back into the state, on the other hand, starts a new occupancy and re-delivers `details` -- useful when a phase is meant to be redone with its full instructions in view again.

**L122**:

> Use details for multi-paragraph instructions, step-by-step procedures, or reference material that clutters the directive on repeat ticks within the same occupancy. Keep the directive itself short: a one- or two-line summary of what the state expects, since it's what the agent sees on every tick regardless of delivery state.

Nearby, unaffected but adjacent: L92-128 is the whole `<!-- details -->` marker
section (L124: "States without the marker behave exactly as before"; L126: the
`koto status` recovery note; L128: first-marker-wins). L657-666 is the "Self-loops"
Layer-3 section (`- target: await_doc  # self-loop: re-evaluate until the key
appears`) and L602 the `skip_if` polling idiom — both topology docs that now
acquire a delivery consequence worth a cross-reference.

#### `plugins/koto-skills/.cursor/rules/koto.mdc` — 1 passage

**L168-185** ("## The details Field"):

> The `details` field carries extended instructions. It's delivered once per
> **occupancy** of a state -- each time the workflow enters the state, including a
> rewind, a self-transition, or a directed (`--to`) transition back into it -- and
> omitted on any further tick that stays in the same occupancy, to save context.
> Use `--full` to force inclusion regardless:
>
> ```bash
> koto next <session> --full
> ```
>
> If you've lost track of a state's instructions and don't want to force a
> delivery, `koto status <session>` returns the current state's `directive`,
> `details`, and `expects` unconditionally, without depending on delivery state or
> changing anything. Whenever the current state declares instructions, `directive`
> also carries a short pointer to this command -- whether or not `details` came
> along on that particular response.

(L73 is an example body carrying a `details` string; unaffected.)

#### `docs/guides/cli-usage.md` — 2 passages

**L82** (the `--full` flag bullet under `koto next`):

> - `--full` -- Include the `details` field in the response regardless of delivery state. By default, `details` is delivered once per occupancy of a state (each time the workflow enters it -- including a rewind, a self-transition, or a directed transition back into it) and omitted on any further tick that stays in the same occupancy. This flag forces inclusion every time.

**L117** (the legend under the field-presence table at L108):

> "yes" = always present. "--" = absent from the JSON (not `null`, just missing). "object or `null`" = present as an object when the state has an `accepts` block, `null` otherwise. "optional" = present once per occupancy of the state (or when `--full` is passed), absent on any further tick within the same occupancy and when the state has no details content. Use `koto status <name>` to retrieve `directive`/`details`/`expects` unconditionally regardless of delivery state -- see the `status` command below.

(L227, L302, L304 concern terminal absence and the `koto status` retrieval;
unaffected. L171 and L242-245 use "details" in unrelated senses — gate output and
the error envelope's `details` array.)

#### `README.md`

**Nothing.** `grep -i 'details\|occupancy' README.md` returns no matches. No edit
needed.

#### `CHANGELOG.md` — 1 block

**L11-32**, the `## [Unreleased] / ### Fixed` entry "**`details` suppression now
keys on delivery, not visit count.**", 20 lines, unreleased. It states the
occupancy predicate but — read closely — it never actually says a self-transition
re-delivers; it only names rewind and the non-advancing tick as the two cases
visit-counting got wrong. Both are still true after the change. The block can be
amended in place (it is unreleased, so no released note is being rewritten): the
cleanest edit is one added sentence saying a self-transition stays inside the same
delivery window. L34-59 (`### Added`, the `koto status` retrieval and the
directive pointer) is unaffected.

### 6. Anything the list above misses

Grepped `occupancy`, `details`, `InstructionsDelivered`, `instructions_delivered`,
`self-transition`, `self-loop`, `redeliver`/`re-deliver`, `--full` across the
whole tree (`src/`, `tests/`, `test/`, `koto-stability-tests/`, `benches/`,
`scripts/`, `plugins/`, `docs/`, `.github/`).

**A second failing test outside `instructions_delivery_test.rs` — the one real
find.** `tests/status_phase_retrieval_test.rs`, test
`status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected`
(L491-526). Setup: `init_workflow_with_vars` (L92-101) runs only `koto init` — no
tick. Then:

```rust
    // First occupancy of `implement`: carries details.
    run_koto(root, &["next", "wf", "--with-data", r#"{"route":"go"}"#]);
    let first_implement = run_koto(root, &["next", "wf", "--to", "implement"]);
    assert!(
        first_implement.get("details").is_some(),
        "first arrival at implement should carry details: {first_implement}"
    );
```

The first call sits at `gather`, submits `route: go`, and lands on `implement` —
which declares details (`PHASES_TEMPLATE` L160-164, "Implement instructions for
{{ORG}}.") and so delivers there. The second call is `--to implement` **while
already at `implement`**: a directed self-transition. Under the new rule it
suppresses, and `first_implement.get("details").is_some()` **fails**. The fix is
local and harmless — arrive at `implement` via the `--to` from `gather` instead of
via `--with-data`, or just drop the redundant `--to` hop — but it will not be
found by grepping for `occupancy` (the word appears only in a comment at L497),
and it lives in a file whose subject is `koto status`, not delivery. The
template's doc comment at L107-109 also says "`implement` declares instructions
and self-loops".

**Other places the rule is written down, beyond the eight files in sub-question 5:**

- `docs/prds/PRD-inline-phase-details.md` — status Done. **L140-142** is the
  normative Definitions entry: "A phase's occupancy begins when a state-entry event
  names that phase ... including the same one. A self-transition therefore ends one
  occupancy and begins another..." Plus **L161** (R2's delivery-not-count framing),
  **L166-167** (R3: "The first response of a phase's occupancy carries that phase's
  instructions, however the occupancy began: a conditional transition, an ..."),
  **L262** ("above and the occupancy definition"), **L275** ("both carry the
  instructions, because each begins a new occupancy"). This is the document the
  user's ruling contradicts head-on, and it is the artifact question for
  lead-artifacts.
- `docs/designs/current/DESIGN-inline-phase-details.md` — status Current. **L221**,
  **L247-253** (an explicit decision paragraph reconciling AC 3 against the PRD
  definition: "...self-transition begin a new occupancy — so instructions must be
  delivered — while ... occupancy by the PRD's own definition. The Definitions are
  normative and R3 is..."), **L264** (component table row for the predicate),
  **L280**, **L357-358** (the unit-test list this change edits), **L404** ("The
  added write is bounded by occupancy count rather...").
- `docs/reference/session-feed.md` **L679-692**, the `instructions_delivered` event
  entry. Two problems: it repeats the occupancy definition ("A phase's occupancy
  runs from the state-entry event naming it to the next state-entry event naming
  any phase"), **and it is already stale** — it says "**Not emitted yet.** ... no
  koto build appends one: instruction suppression is still keyed on visit count",
  which the shipped `InstructionsDelivered` append contradicts. Pre-existing drift,
  not caused by this change, but the same edit should fix it.
- `src/cli/mod.rs` — long explanatory comments at **L3366-3392** (directed path:
  "Whether this occupancy has already delivered...", including the reasoning that
  the synthetic post-append event "always starts a fresh, undelivered occupancy")
  and **L4283-4300** (natural-advancement path). Both encode the old semantics in
  prose and both call sites pass `already_delivered` into the shared combinator
  (L3417-3419, L4298-4300).
- `src/cli/next_types.rs` **L374-392** (`with_details_suppressed_unless_full` doc:
  "delivered during the current occupancy and the caller did not...") and
  **L487-488** (`carries_details` doc). Its unit tests at L1346-1440 exercise the
  combinator with explicit `(already_delivered, full)` booleans and are
  **predicate-agnostic** — they take `already_delivered` as an input, so none of
  them flip.
- `src/engine/types.rs` **L808** (`InstructionsDelivered { state }` variant),
  **L1040/L1364-1367/L1456** (serde), **L3420-3480** (round-trip and
  forward-compat tests). No rule text; nothing flips.
- `docs/briefs/BRIEF-inline-phase-details.md` L68-72, L207 and
  `docs/designs/current/DESIGN-koto-next-output-contract.md` L147-162, L214-215,
  L238, L297 and `docs/prds/PRD-koto-next-output-contract.md` L131, L146, L178,
  L212 describe the *superseded* visit-count rule. They are historical and should
  stay as they are.
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md` — the
  shipped author template. It has exactly one self-loop, `compile_validation`
  (`- target: compile_validation when compile_result: fail`, frontmatter
  L62-67), and that state's body section (L215-231) declares **no**
  `<!-- details -->`. The two details markers (L155, L185) sit in `state_design`
  and `template_drafting`, neither of which self-loops. **So no shipped template
  changes behaviour** — nice, because it means no `koto template compile` output
  or `.mermaid.md` artifact moves.
- `plugins/koto-skills/skills/koto-adhoc/SKILL.md` L48, L228 mention self-loops as
  a topology pattern only. `koto-adhoc/evals/evals.json` and
  `koto-author/evals/evals.json` have no delivery-rule evals.
- `test/`, `koto-stability-tests/`, `benches/` — **zero hits** on every search
  term. Nothing there to change.

### 7. What CI actually runs

`.github/workflows/validate.yml` — triggers on push to `main` and on PRs to
`main` (`opened, synchronize, reopened, ready_for_review, converted_to_draft`).
Jobs and their verbatim commands:

| Job | Command |
|---|---|
| `check-artifacts` (L11-27) | shell loop `for dir in wip; do ...` — fails with `::error::$dir/ contains intermediate work artifacts that should not be merged into main.` if `wip/` exists and is non-empty. Skipped on draft PRs (`if: ${{ github.event.pull_request.draft != true }}`). |
| `unit-tests` (L29-40) | `cargo test -- --test-threads=1` |
| `stability-tests` (L42-62) | `cargo test -p koto-stability-tests -- --test-threads=1` |
| `fmt` (L64-77) | `cargo fmt --check` |
| `clippy` (L79-92) | `cargo clippy -- -D warnings` |
| `audit` (L94-108) | `cargo install cargo-audit --locked` then `cargo audit` |
| `coverage` (L110-132) | `cargo llvm-cov --all-features --lcov --output-path lcov.info -- --test-threads=1`, uploaded to Codecov with `fail_ci_if_error: false` |
| `tsuku-distributed-install` (L134-161) | on PR: `tsuku recipe validate .tsuku-recipes/koto.toml \|\| echo "Recipe validation not available, skipping"`; on push: `tsuku install tsukumogami/koto -y` then `koto version` |
| `cloud-integration` (L163-185) | skips when `KOTO_TEST_S3_ENDPOINT` is unset, else `cargo test --features cloud-integration-tests --test cloud_integration_test -- --test-threads=1` |
| `validate` (L187-205) | aggregator; `needs: [check-artifacts, unit-tests, stability-tests, fmt, clippy, audit, tsuku-distributed-install, cloud-integration]`, `if: always()`, fails if any of those reported `failure` |

**There is no `koto template compile` step in `validate.yml`.** That lives in a
different workflow: `.github/workflows/validate-plugins.yml`, job
`template-compilation` (L12-45), which triggers only on PRs touching
`plugins/**` or `.claude-plugin/**` and runs `cargo build --release` then, for
each `find plugins/koto-skills/skills/ -path '*/koto-templates/*.md' -type f`
(skipping `*.mermaid.md`), `./target/release/koto template compile "$template"`.
That same workflow also has `hook-smoke-test` and `schema-validation` jobs, and
an aggregator `validate-plugins`.

Also relevant to a PR touching plugins and docs:
`.github/workflows/eval-plugins.yml` (`bash scripts/check-evals-exist.sh` +
no-hooks check, on `plugins/**`) and `.github/workflows/validate-docs.yml` (calls
`tsukumogami/shirabe/.github/workflows/validate-docs.yml@main` on `docs/**`).
`.github/workflows/check-template-freshness.yml` is `workflow_call`-only and is
not invoked from within this repo's own PR flow.

The practical consequence: a PR that touches the predicate, tests, docs, plugin
references and evals fires `validate`, `validate-plugins`, `eval-plugins` and
`validate-docs` — and the `wip/` check means **this research file and everything
else under `wip/` must be deleted before the PR can leave draft.**

## Implications

The code-level blast radius is small and precise: **four assertion flips across
three test files** (`instructions_delivery_test.rs:380` and `:508-513`;
`persistence.rs:2603` and `:2648`; `status_phase_retrieval_test.rs:497-503`), plus
two test renames and one test whose second half becomes vacuous. The
documentation blast radius is larger: **eight passages across six files** state
the current rule in a form that explicitly names self-transition as a
re-delivering arrival, and the phrase "leave and re-enter the state" appears as a
stock formula in five of them — it has to go everywhere, because after the change
leaving and re-entering *the same* state is exactly the case that does not
re-deliver.

The structural decision the fix has to make is what happens to `occupancy_slice`.
It is deliberately shared with `latest_epoch_gate_failed`, and its doc comment
says so ("cannot come to disagree about where an occupancy starts"). Changing the
slice itself silently changes gate-epoch classification for the dashboard
(`src/cli/dashboard_data.rs:458`) and the `/workflows` projection
(`src/workflows_surface/project.rs:183`) across a self-transition, and there is no
unit test on `latest_epoch_gate_failed` to catch it. Splitting the two predicates
so only the delivery one treats a self-entry as non-boundary is the safer shape,
but it costs the design's stated no-drift property and the comment has to be
rewritten to explain why the two now legitimately differ.

The evals constraint is satisfiable without deleting anything. Evals 11 and 12
both keep their scenarios and their verdicts; only eval 11's `expected_output` and
one assertion string need rewording, because they explain a correct answer using
the boundary definition that's changing. A new eval 13 in the identical six-key
shape asserts the self-loop case, and CI costs nothing since
`.github/workflows/eval-plugins.yml` only counts evals rather than running them.

`instruction-free.json` needs no edit and should get none: the fixture's templates
carry no `<!-- details -->` marker at all, so no code path can move a byte there.
The only way to break it is to edit the `SEQUENCES[5].description` prose in the
`.rs` file, which is embedded in the fixture verbatim — a tempting-looking cleanup
that would fail the byte comparison unless both files are edited in lockstep.

## Surprises

`tests/status_phase_retrieval_test.rs` fails and nobody would predict it from the
issue text. Its `--to implement` while already at `implement` is a directed
self-transition buried in the setup of a test about `koto status` not writing to
the log; the word "occupancy" appears in it exactly once, in a comment.

`docs/reference/session-feed.md:681-687` is already wrong on main: it says the
`instructions_delivered` event is "**Not emitted yet** ... instruction suppression
is still keyed on visit count", which the shipped `InstructionsDelivered` append
and the unreleased CHANGELOG entry both contradict. Pre-existing drift the same
edit pass should sweep up.

`README.md` says nothing about `details` at all — one fewer surface than the lead
assumed.

The shipped `koto-author` template dodges the change entirely: its one self-loop
(`compile_validation`) has no `<!-- details -->` section, and the two states that
do have one never loop to themselves. No template compilation output or Mermaid
artifact moves.

The CHANGELOG's unreleased "Fixed" entry, read carefully, never actually claims a
self-transition re-delivers — it only indicts visit-counting for the rewind and
non-advancing-tick cases, both of which survive. It needs an addition, not a
rewrite.

`tests/instructions_delivery_test.rs` needs no new template: `DELIVERY_TEMPLATE`
already declares a conditional self-transition, a loop-back, a directed
self-transition target and a rewind path on a phase that carries details, so every
new assertion the change needs is expressible against the template as it stands.

## Open Questions

Does `occupancy_slice` get changed in place, or does the delivery predicate get
its own slicing that treats a self-entry as non-boundary while
`latest_epoch_gate_failed` keeps the current one? This is the only decision with
consequences outside the delivery path, and it is the one the design's own
no-drift comment argues against — someone has to overrule that comment
deliberately and rewrite it.

What happens to the Done PRD's normative Occupancy definition
(`docs/prds/PRD-inline-phase-details.md:140-142`) and the Current DESIGN's
reconciliation paragraph (`docs/designs/current/DESIGN-inline-phase-details.md:247-253`),
which the user's ruling directly contradicts? That's lead-artifacts' question, but
it gates whether the doc edits here can cite a coherent upstream definition.

Should the "loop-back to the same phase from a later phase" case get a dedicated
test? It is the boundary the fix can most easily overshoot — a naive "ignore any
entry event whose `from == to`" is safe, but a "walk back past consecutive entries
naming the phase" implementation could swallow a genuine
`implement → gather → implement` round trip. Test 6 in
`instructions_delivery_test.rs` covers the `gather` direction; nothing covers
returning to `implement` after one hop away.

Does the `koto status` pointer spliced into `directive` change meaning now that a
self-loop suppresses? The pointer is described as appearing "precisely when it's
needed, on the very responses that suppressed the details"
(`response-shapes.md:56-57`) — that claim gets stronger, not weaker, but the
sentence should be re-read against the new rule.

## Summary

Only four assertions in three test files actually flip — `instructions_delivery_test.rs:380` and `:508-513`, `persistence.rs:2603` and `:2648`, plus a fifth failure nobody would predict at `status_phase_retrieval_test.rs:497-503`, where a `--to implement` while already at `implement` is buried in the setup of a `koto status` test; the byte-for-byte `instruction-free.json` fixture is provably unaffected because none of its templates declare a `<!-- details -->` marker, and evals 11 and 12 both keep their scenarios with only two wording edits to eval 11.

The documentation blast radius is the larger half: eight passages across `response-shapes.md`, `command-reference.md`, `koto-author/SKILL.md`, `template-format.md`, `koto.mdc`, `cli-usage.md` and `CHANGELOG.md` (README says nothing) all name a self-transition as a re-delivering arrival and repeat the stock phrase "leave and re-enter the state", which becomes exactly backwards; `docs/reference/session-feed.md:681` is separately already stale.

The biggest open question is whether `occupancy_slice` changes in place — it is deliberately shared with `latest_epoch_gate_failed`, whose dashboard and `/workflows` consumers have no unit-test coverage, so changing the shared helper shifts blocked-classification epochs silently and overrules the design's own explicit no-drift rationale.
