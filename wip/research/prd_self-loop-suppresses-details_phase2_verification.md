# Lead: harnesses and what a testable criterion looks like

Scope note: this lead was asked what harness can prove each behavior. The
single most consequential finding is not about harnesses at all — four
existing assertions (two integration, two unit) currently assert the
*opposite* of three of the ten behaviors in the brief. This change is an
inversion of committed tests, not an addition to them. That fact shapes
every criterion below, so it is stated first and repeated in Implications.

## Findings

### 1. The three harness families

#### 1a. `tests/instructions_delivery_test.rs` (592 lines, integration)

Drives a real `koto` binary via `assert_cmd`. File-gated `#![cfg(unix)]`
(`tests/instructions_delivery_test.rs:12`), so nothing here runs on Windows.

Helpers, verbatim signatures:

```rust
fn koto_cmd(dir: &Path) -> Command                                   // :18
fn sessions_base(dir: &Path) -> PathBuf                              // :26
fn run_koto(dir: &Path, args: &[&str]) -> serde_json::Value          // :34
fn details_of(resp: &serde_json::Value) -> Option<&str>              // :48
fn assert_carries(resp: &serde_json::Value, expected: &str, context: &str)  // :52
fn assert_omits(resp: &serde_json::Value, context: &str)             // :62
fn write_delivery_template(dir: &Path) -> PathBuf                    // :254
```

`assert_carries` / `assert_omits` are the entire assertion vocabulary a new
test needs — every behavior in the brief is expressible as one of the two
against a `run_koto(...)` value. `run_koto` asserts exit success itself and
parses the **last non-blank stdout line** as JSON, returning
`Value::Null` on a parse failure rather than panicking
(`:44-45`) — a silent-pass hazard if a future response stops being
single-line JSON, since `details_of(Null)` is `None` and would satisfy
`assert_omits`. Any new omission criterion should therefore also assert
`resp["state"]` or `resp["action"]`, as the existing tests do at `:350`,
`:380`, `:406`, `:427`, `:447`.

Isolation, and this is uniform across all three integration files: a fresh
`assert_fs::TempDir` per test, `cmd.current_dir(dir)`,
`cmd.env("KOTO_SESSIONS_BASE", <dir>/sessions)`, and `cmd.env("HOME", dir)`
(`:18-30`). The `HOME` override is what keeps the real `~/.koto/` untouched;
`KOTO_SESSIONS_BASE` is what redirects the session store. Both are needed.

Templates are written into the tempdir as literal `&str` consts and passed by
absolute path to `koto init --template`. `DELIVERY_TEMPLATE` (`:81-145`) is
already built to cover this feature's whole arrival matrix in one file:
`gather` and `implement` both declare `<!-- details -->`; `implement` has a
self-transition (`loop_again: yes`), a loop-back to `gather`
(`loop_again: redo`), and a terminal exit (`loop_again: no`), and is a valid
`--to` target from `gather`. No new template is needed for any of the ten
behaviors.

**What it can prove:** presence or absence of the `details` key in a real
end-to-end response, for any arrival path expressible in the template
grammar, including `--full` and `--to`. It is the only harness that proves
the CLI-observable contract.

**What it cannot prove:** it cannot see the event log (it never reads the
state file), so it cannot directly prove that a delivery was *recorded*.
Test `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`
(`:556`) works around this with a two-step observation: `--full` on the first
tick of an occupancy, then a plain tick that must omit. That indirection is
the established pattern for "recorded" criteria and is what the `--full`
criterion below reuses. It also cannot prove cost or size — only key
presence.

**Four existing tests assert what this change inverts:**

| Test | Line | Asserts today | Must assert after |
|---|---|---|---|
| `self_transition_arrival_carries_details_again` | `:359` | self-transition **carries** | omits |
| `two_consecutive_directed_transitions_into_same_phase_both_carry` | `:487` | second `--to implement` **carries** | omits |

Both names encode the old semantics and must be renamed, not just re-asserted.

#### 1b. Unit tests in `src/engine/persistence.rs` (2712 lines)

The predicate under test:

```rust
pub fn instructions_delivered_this_occupancy(events: &[Event], current_state: &str) -> bool  // :1099
fn occupancy_slice<'a>(events: &'a [Event], current_state: &str) -> &'a [Event]              // :1028
```

`occupancy_slice` finds the last event whose `to` names `current_state` among
`Transitioned`, `DirectedTransition`, `Rewound` (`:1030-1035`) and returns
everything after it; with no such event it returns the whole log (`:1044`).
`instructions_delivered_this_occupancy` then looks for an
`InstructionsDelivered { state }` in that slice matching by name (`:1100-1105`).
Self-transition suppression is therefore a change to which entry events open a
new slice — the three-variant match at `:1030` is the exact line the feature
lands on.

The doc comment at `:1020-1027` records a contract a new test may rely on:
only `.payload` and position matter; `.seq`, `.timestamp`, `.event_type`,
`.idempotency_hash` never factor in. That is why the directed path can build
its post-append list in memory.

Test-module constructors, verbatim signatures (`mod tests`, `:1109`):

```rust
fn transitioned(seq: u64, from: Option<&str>, to: &str) -> Event   // :2503
fn rewound(seq: u64, from: &str, to: &str) -> Event                // :2515
fn directed(seq: u64, from: &str, to: &str) -> Event               // :2526
fn delivered(seq: u64, state: &str) -> Event                       // :2537
fn make_event(seq: u64, payload: EventPayload) -> Event            // :1141
fn make_header() -> StateFileHeader                                // :1115
```

A synthetic-event test is three lines: build a `Vec<Event>`, call the
predicate, `assert!`. No filesystem, no binary, no `--test-threads=1`
concern. `rewound(seq, from, to)` takes `from` and `to` independently, so the
self-rewind case (`rewound(n, "gather", "gather")`) is directly expressible —
see the gap noted below.

**What it can prove:** the predicate's verdict over any event sequence,
including sequences no CLI path can currently produce. It is the only harness
that can exercise the whole-log fallback (`:2652`) and an intermediate
phase's record landing inside the current occupancy (`:2612`).

**What it cannot prove:** that the CLI calls it, that the CLI appends the
event, or that the response actually omits `details`. A predicate-only change
with no call-site wiring passes every test here.

**Two existing unit tests assert what this change inverts:**

| Test | Line | Asserts today |
|---|---|---|
| `instructions_delivered_resets_on_a_self_transition` | `:2595` | `Transitioned{from:"review",to:"review"}` **resets** the slice |
| `instructions_delivered_resets_on_arrival_by_directed_transition` | `:2635` | `directed(4,"implement","implement")` **resets** the slice |

Both carry comments stating the old rationale ("A self-transition ends one
occupancy and begins another", `:2596`) that must be rewritten, not just
inverted.

**Gap:** there is no test for `Rewound { from: X, to: X }`. `rewound` at
`:2583` only covers `implement -> gather`. A self-rewind is the single case
that discriminates a correct implementation from the cheapest wrong one — see
the criteria table.

#### 1c. `tests/next_response_baseline.rs` + `tests/fixtures/next-response-baseline/instruction-free.json`

Two active tests plus one `#[ignore]`d regenerator:

- `instruction_free_responses_are_byte_identical_to_the_baseline` (`:533`)
- `baseline_fixture_covers_every_required_sequence_and_stays_instruction_free` (`:577`)
- `regenerate_baseline_fixture` (`:683`, `#[ignore = "regeneration helper; rewrites the baseline fixture"]`)

The document is rebuilt in-process by `fn capture() -> String` (`:445`) which
replays 13 `Sequence`s (`SEQUENCES`, `:313-441`), each in its own `TempDir`
with the same `HOME` + `KOTO_SESSIONS_BASE` isolation (`:279-287`), and
compares the resulting pretty-printed JSON **as raw strings** against the
fixture. The `Step` type has a `record: bool`; `setup(argv)` (`:296`) runs
without recording, `record(argv)` (`:303`) captures raw stdout including the
trailing newline (`:499-509`). Machine-specific template paths are replaced by
tokens (`<TEMPLATE>` etc., `:39-43`) so the fixture is portable.

Comparison is on strings, not `serde_json::Value`, deliberately: the two
construction sites serialize through different orderings (a key-sorted
`Map` on the natural-advancement path, declared field order on the directed
path), and a `Value` comparison would pass through that drift (`:19-23`).

The second test is the one that keeps the first honest: it asserts the 13
required labels are present (`:587-601`), that the union of `action` values
across every recorded body is exactly
`["confirm","done","evidence_required","gate_blocked","integration_unavailable"]`
(`:629-639`), and — most relevant here — that **no recorded body carries a
`details` key** (`:658-664`).

**What it can prove:** that a template declaring no `<!-- details -->` anywhere
produces byte-for-byte the stdout a pre-change binary produced, across all
five response shapes and all 13 sequences (which include
`self-transition-arrival` at `:360` and `directed-transition` at `:370`).

**What it cannot prove:** anything about the delivery rule. Its own header says
so (`:4-7`): a phase with no `<!-- details -->` marker never carries `details`
regardless of history. It also compares stdout only, never the state file — so
a change in *whether an `InstructionsDelivered` event is appended* is invisible
to it. Both facts matter: the self-loop change must leave this fixture
untouched, and that is a real, cheap, binary criterion.

The failure message (`:546-568`) explicitly forbids regeneration as a fix
while this feature is in flight. Worth quoting in the PRD so a reviewer does
not accept "regenerated the baseline" as a resolution.

### 2. `tests/status_phase_retrieval_test.rs` (766 lines)

Same harness shape, self-described as mirroring the delivery test (`:25`).
Adds `session_state_path(dir, name) -> PathBuf` (`:42`),
`run_koto_raw(dir, args) -> (bool, Value, String)` (`:64`),
`parse_last_json(stdout: &[u8]) -> Value` (`:74`, tries the whole trimmed
stdout first, falls back to the last non-blank line),
`init_workflow(dir, name, template_content)` (`:83`),
`init_workflow_with_vars(dir, name, template_content, vars: &[&str])` (`:92`),
and `session_dir_str(root, name) -> String` (`:385`). It imports
`koto::session::SessionBackend`, so it can touch the library surface directly.

`PHASES_TEMPLATE` (`:112-175`) declares: variable `ORG` with default `"acme"`;
`gather` (accepts `route: enum[go]`, `<!-- details -->` containing both
`{{ORG}}` and `{{SESSION_DIR}}`), `implement` (accepts
`loop_again: enum[yes,no]`, **self-transition on `yes`** at `:138`, plus
`-> bare` on `no`, and its own `<!-- details -->`), `bare` (no details,
unconditional `-> done`), `done` (terminal). So the status harness already
ships a details-declaring self-looping phase — no template edit needed to add
a status-side self-loop test.

What the file proves about `koto status`, by test:

- `status_directive_details_expects_match_what_next_would_return` (`:333`) —
  `directive`, `details`, `expects` come back with runtime (`{{SESSION_DIR}}`)
  and template (`{{ORG}}`) vars substituted, **before any `next` call**, and
  `expects.event_type == "evidence_submitted"` with the `accepts` schema in
  `expects.fields`.
- `status_omits_all_three_keys_when_terminal` (`:393`).
- `status_details_and_expects_absent_when_phase_declares_neither` (`:425`).
- `status_does_not_execute_gate_or_default_action` (`:459`).
- **`status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected`
  (`:492`)** — the load-bearing one. It reads the state file bytes before and
  after (`std::fs::read(session_state_path(...))`), asserts
  `assert_eq!(before, after, "the session state file must be byte-identical
  before and after a retrieval")` (`:513`), asserts `status` returns `details`
  regardless of delivery history, and then asserts a following plain
  `koto next` still omits. That triple is exactly the brief's "returns the
  instructions and appends nothing" behavior, already written.
- `status_unknown_workflow_returns_structured_error` (`:534`),
  `status_reports_template_hash_mismatch_without_failing` (`:552`).
- Four lock-behavior tests (`:607`, `:633`, `:670`, `:719`) proving status
  returns promptly under a held state-file lock, a batch parent's lock, and a
  concurrent slow-gate `koto next`, and attempts no lock syscall at all on a
  non-batch session.

The status test at `:492` sets up its "first occupancy of implement" with
`next wf --to implement` (`:499`). That is a directed transition from
`gather`, not into an already-occupied phase, so this change does **not**
invert it. It stays green as written.

### 3. The eval suite

`plugins/koto-skills/skills/koto-user/evals/evals.json`. Schema, exactly:

```json
{ "skill_name": <string>, "evals": [ { "id": <int>, "name": <string>,
  "prompt": <string>, "expected_output": <string>, "files": [],
  "assertions": [<string>, ...] } ] }
```

All six keys appear on all 12 evals; `files` is `[]` on every one. Three
skills ship evals: `koto-user` (12), `koto-author`, `koto-adhoc`.

Two koto-user evals already cover this feature:

- id 11, `details-omitted-on-repeat-tick-same-occupancy` — a user pastes two
  `gate_blocked` bodies, the second missing `details`, and asks if it's a bug.
  Assertions require the agent to call it expected, to state the
  delivery-per-occupancy rule, not to advise filing a bug, and to name at
  least one of `--full` / `koto status`.
- id 12, `details-redelivered-after-rewind` — asserts the agent says a rewind
  redelivers *without* `--full`, and distinguishes it from a same-occupancy
  repeat tick.

Neither inverts under this change. There is no eval for a self-loop, which is
the gap the feature creates.

`scripts/run-evals.sh` (483 lines) consumes the file: it counts
`json.load(...)['evals']` (`:71`, `:124`), writes a per-eval
`eval_metadata.json` carrying `prompt` and `assertions` (`:133-155`), then
shells out to `claude -p` with `/skill-creator` to run each eval twice
(with-skill and without-skill baseline) and grade the with-skill run against
the assertions into a `grading.json` (`:227-268`). Results land in
`plugins/<plugin>/skills/<name>/evals/workspace/iteration-<N>/`. It exits 0
when all assertions pass, 1 on any failure, 2 when no results were produced,
3 on missing prerequisites — and it hard-requires the `claude` CLI and
`python3` on PATH (`:31-32`).

**CI does not run it.** `.github/workflows/eval-plugins.yml` has exactly two
substantive jobs: `eval-coverage`, which runs `bash scripts/check-evals-exist.sh`,
and `no-hooks`, which greps for a stray `hooks.json` in skill directories.
`check-evals-exist.sh` only asserts that every non-exempt skill has an
`evals/evals.json` with `len(evals) > 0` — a count, never a run. Both jobs are
`if: ${{ github.event.pull_request.draft != true }}` and the workflow is
`paths: ['plugins/**']`-filtered.

**Consequence for the PRD:** an eval is not a gating criterion. An
acceptance criterion phrased "eval N passes" is LLM-graded, non-deterministic,
and never executed by CI. The only mechanically checkable eval criterion is a
structural one — that a named eval exists with a stated assertion — which
`check-evals-exist.sh` partially backs (it checks the count, not the name).

### 4. `.github/workflows/validate.yml` — verbatim gating commands

| Job | Command, verbatim | Draft-skipped? | In the `validate` gate? |
|---|---|---|---|
| `check-artifacts` | inline shell: fails if `wip/` is non-empty (`:18-27`) | **yes** (`:13`) | yes |
| `unit-tests` | `cargo test -- --test-threads=1` (`:40`) | no | yes |
| `stability-tests` | `cargo test -p koto-stability-tests -- --test-threads=1` (`:62`) | no | yes |
| `fmt` | `cargo fmt --check` (`:77`) | no | yes |
| `clippy` | `cargo clippy -- -D warnings` (`:92`) | no | yes |
| `audit` | `cargo audit` (`:108`) | no | yes |
| `coverage` | `cargo llvm-cov --all-features --lcov --output-path lcov.info -- --test-threads=1` (`:126`) | no | **no** |
| `tsuku-distributed-install` | `tsuku recipe validate .tsuku-recipes/koto.toml \|\| echo ...` on PRs (`:151`) | no | yes |
| `cloud-integration` | `cargo test --features cloud-integration-tests --test cloud_integration_test -- --test-threads=1`, guarded by `[ -z "$KOTO_TEST_S3_ENDPOINT" ] && exit 0` (`:180-185`) | **yes** (`:165`) | yes |

Two things the PRD should name precisely:

- **Clippy runs without `--all-targets`** (`:92`). It lints the default targets
  (lib + bins) only. Nothing under `tests/` or `benches/`, and no `#[cfg(test)]`
  module, is clippy-gated. New test code is still *compiled* by `cargo test`,
  so a compile error fails `unit-tests` — but a clippy lint in a new
  integration test will not fail CI.
- **`coverage` is not in the `validate` aggregator's `needs` list** (`:189`)
  and is not checked in its result script (`:195-203`). A coverage drop cannot
  block a merge. `codecov-action` is additionally configured
  `fail_ci_if_error: false` (`:132`).

Draft PRs skip `check-artifacts` and `cloud-integration` here, plus every job
in `validate-plugins.yml` and `eval-plugins.yml`. `unit-tests`, `fmt`,
`clippy`, `audit`, and `stability-tests` run on drafts.

### 5. Performance / token-cost harness

There is none that fits.

`benches/` holds exactly two: `discovery_scan.rs` and `recursion_caps.rs`.
`.github/workflows/benches.yml` runs `cargo bench --bench discovery_scan --
--save-baseline ci` and `cargo bench --bench recursion_caps -- --save-baseline ci`.
Per that file's own header comment, they were **moved off the per-PR path**:
they run nightly (`cron: '0 7 * * *'`) and on push to `main`, and explicitly
"no longer run on pull requests". They are also reporting-only —
`KOTO_BENCH_STRICT` is unset, so a threshold breach prints `BREACH` to stderr
and still exits 0. Neither measures response size, token count, or anything
about `koto next`.

`koto-stability-tests/` is not a perf harness at all: it is an
external-consumer compile-check fixture that imports the frozen public surface
(`use koto::engine::types::*`, `koto::error::Error`) and exercises four
`SessionBackend` methods through a trait object (`validate.yml:43-51`). It
pins `CURRENT_SCHEMA_VERSION == 1` (`koto-stability-tests/src/lib.rs:171`).
Relevantly, `instructions_delivered_this_occupancy` is **not** in that frozen
surface — only `derive_state_from_log` is re-exported there
(`koto-stability-tests/src/lib.rs:159-167`, mirrored at
`tests/lib_reexports.rs:125`). Changing the predicate's semantics breaks no
stability contract.

**Therefore:** a criterion about "the cost of a long loop" cannot be stated
against a harness. It must be restated as a response-shape criterion — count
the responses carrying `details` across an N-iteration loop, which
`instructions_delivery_test.rs` can assert exactly and deterministically. That
is strictly better than a token measurement anyway: it is binary, has no
threshold to tune, and no runner-variance.

### 6. `koto template compile` and the shipped templates

`koto template compile <source>` compiles a YAML template source to
FormatVersion=1 JSON (`src/cli/mod.rs:617`, dispatched at `:1370`) and then
runs `validate_compiled_template` on the output (`:1390`), so a compile is
also a validation. Sibling subcommands: `template validate <path.json>`
(`:630`), `template validate-feed <log_file>` (`:637`), `template export`
(`:642`).

**The invocation that validates every shipped template** is the "Compile all
templates" step of `.github/workflows/validate-plugins.yml`:

```bash
while IFS= read -r template; do
  case "$template" in *.mermaid.md) continue ;; esac
  ./target/release/koto template compile "$template"
done < <(find plugins/koto-skills/skills/ -path '*/koto-templates/*.md' -type f)
```

preceded by `cargo build --release`. The workflow is `paths`-filtered to
`plugins/**` and `.claude-plugin/**` and every job is draft-skipped.

That `find` currently matches **exactly one** file:
`plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md`.

**No shipped template has a self-transition on a phase declaring
`<!-- details -->`.** Checked exhaustively:

- `koto-author.md` — its only self-transition is `compile_validation ->
  compile_validation` on `compile_result: fail` (`:68-71`). `compile_validation`
  declares no details block; the file's two `<!-- details -->` markers are in
  `state_design` (`:155`) and `template_drafting` (`:185`), neither of which
  self-loops.
- `references/examples/complex-workflow.md` — self-loops on `preflight`
  (`:25`) and `build` (`:37`); its sole `<!-- details -->` is in `test`
  (`:88`), which has no self-transition. This is a documentation example and
  is **not** matched by the CI `find` (it is not under `koto-templates/`).
- `references/examples/evidence-routing-workflow.md` — no self-transitions,
  no details blocks.

So compiled output is unchanged, and would be even if an overlap existed:
`details` is a compile-time `String` field on the template state
(`src/template/types.rs:57`) and delivery is a runtime decision made in
`src/cli/mod.rs`. Compilation cannot observe the rule.

The three committed `.mermaid.md` artifacts (`koto-author.mermaid.md`,
`complex-workflow.mermaid.md`, `evidence-routing-workflow.mermaid.md`) are
likewise unaffected, and are in any case **not freshness-checked in this
repo**: `.github/workflows/check-template-freshness.yml` is `workflow_call`-only
and no workflow under `.github/workflows/` invokes it. It is published for
downstream consumers.

## Proposed acceptance criteria

Every criterion below is binary — it either holds or the named command
fails — and every one names a harness that already exists. "Delivery test"
means `tests/instructions_delivery_test.rs`; "predicate unit" means the
`mod tests` in `src/engine/persistence.rs`.

| # | Behavior | Harness | Criterion (binary) |
|---|---|---|---|
| 1 | A conditional self-transition omits | Delivery test | With `DELIVERY_TEMPLATE`, after reaching `implement` via `{"route":"direct"}`, a `next wf --with-data {"loop_again":"yes"}` returns a body with `state == "implement"` and **no** `details` key. Asserted by `assert_omits`. This replaces `self_transition_arrival_carries_details_again` (`:359`), which asserts the opposite and must be renamed and inverted. |
| 2 | Two consecutive self-transitions both omit | Delivery test | From the same setup, **two** successive `--with-data {"loop_again":"yes"}` ticks each return `state == "implement"` and no `details`. Both calls asserted, not just the second — a one-shot suppression bug passes if only the second is checked. |
| 3 | A directed transition into the already-occupied phase omits | Delivery test | After `next wf --to implement` carries `"Implement instructions."`, a second `next wf --to implement` returns `state == "implement"` and no `details`. Replaces `two_consecutive_directed_transitions_into_same_phase_both_carry` (`:487`). |
| 3b | (predicate half of 1 and 3) | Predicate unit | `instructions_delivered_this_occupancy(&[transitioned(1,None,"review"), delivered(2,"review"), transitioned(3,Some("review"),"review")], "review")` is `true`; same with `directed(3,"implement","implement")` in place of the self-transition is `true`. Inverts `instructions_delivered_resets_on_a_self_transition` (`:2595`) and the second half of `instructions_delivered_resets_on_arrival_by_directed_transition` (`:2648`), both of which assert `false` today. |
| 4 | Arrival from a different phase delivers | Delivery test | `conditional_transition_arrival_carries_details` (`:311`) and `unconditional_transition_arrival_carries_details_separately_from_conditional` (`:333`) pass **unmodified**. Stated as a no-diff criterion: neither test's body may change in this PR. |
| 5 | A loop-back from a later phase delivers | Delivery test | `loop_back_arrival_at_previously_occupied_phase_carries_details_again` (`:384`) passes unmodified: `implement --with-data {"loop_again":"redo"}` returns `state == "gather"` **with** `details == "Gather instructions."`. Also `instructions_delivered_false_when_the_record_predates_the_entry_event` (`:2570`) unmodified. This is the criterion that fails a naive "ever delivered for this phase" implementation. |
| 6 | A rewind delivers | Delivery test | `rewind_arrival_carries_details` (`:411`) passes unmodified. |
| 6b | **A rewind landing on the phase it started from delivers** | Predicate unit (new) + delivery test (new) | Unit: `instructions_delivered_this_occupancy(&[transitioned(1,None,"gather"), delivered(2,"gather"), rewound(3,"gather","gather")], "gather")` is `false`. Integration: a `koto rewind` whose `from` and `to` are the same phase is followed by a `koto next` carrying that phase's `details`. **No test covers this today** — `instructions_delivered_resets_on_arrival_by_rewind` (`:2583`) only rewinds `implement -> gather`. This is the single case that separates a correct implementation from one keyed on `to == from`, which would wrongly suppress here. If `koto rewind` cannot produce a same-phase `Rewound` from the CLI, the unit half stands alone and the PRD should say so rather than dropping the criterion. |
| 7 | A non-advancing re-tick omits (unchanged) | Delivery test | `gate_blocked_first_tick_carries_and_repeat_omits` (`:436`) and `directed_transition_carries_then_nonadvancing_tick_omits` (`:462`) pass unmodified. |
| 8 | `--full` delivers regardless **and records the delivery** | Delivery test | Both halves required. (a) `full_override_returns_details_on_a_response_that_would_otherwise_be_suppressed` (`:521`) passes unmodified. (b) `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions` (`:556`) passes unmodified. (c) **New:** `--full` on a self-transition tick returns `details`, and the immediately following plain `--with-data {"loop_again":"yes"}` tick — a second self-transition — omits. (c) is the recording half specialised to the new suppression path; without it an implementation that skips the append on the self-loop branch passes everything else. |
| 9 | `koto status` returns the instructions and appends nothing | Status test | `status_appends_nothing_and_leaves_the_next_delivery_decision_unaffected` (`:492`) passes unmodified, **plus** a new case using `PHASES_TEMPLATE`'s self-looping `implement`: after a self-transition tick that omits `details`, `koto status wf` returns a non-null `details`, `std::fs::read(session_state_path(root,"wf"))` is byte-identical before and after the status call, and the following plain `koto next` still omits. Byte-identity of the state file is the "appends nothing" half and is already the established assertion form (`:513`). |
| 10 | A template with no instructions is byte-identical to the pre-change binary | Baseline test | `cargo test --test next_response_baseline` passes and `git diff --exit-code tests/fixtures/next-response-baseline/instruction-free.json` is empty. Both clauses are needed: the first alone is satisfiable by regenerating the fixture, which the test's own failure message forbids (`:546-568`). The fixture's `self-transition-arrival` (`:360`) and `directed-transition` (`:370`) sequences are the ones this feature could disturb, and both are instruction-free by construction, so the expected result is exact equality. |
| 11 | (gate) The whole suite passes as CI runs it | validate.yml | `cargo test -- --test-threads=1`, `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test -p koto-stability-tests -- --test-threads=1` all exit 0. Name these four verbatim; they are the gating set. Note clippy has no `--all-targets`, so new `tests/*.rs` lints are not gated — do not write a criterion that assumes they are. |
| 12 | (docs) The stated rule matches the code | grep, manual | Every prose statement that a self-transition starts a new occupancy is updated: `plugins/koto-skills/skills/koto-user/references/response-shapes.md:38-45`, `:107`, `:168-171`, `:550`; `plugins/koto-skills/skills/koto-user/references/command-reference.md:96`; `plugins/koto-skills/.cursor/rules/koto.mdc:171-173`; `docs/guides/cli-usage.md:82`, `:117`; `docs/reference/session-feed.md:683-686`. Binary form: `grep -rn "self-transition" plugins/ docs/` returns no line asserting a self-transition redelivers. |
| 13 | (eval) A self-loop scenario is covered | evals.json | A new eval in `plugins/koto-skills/skills/koto-user/evals/evals.json` (id 13) whose assertions state that a self-loop tick omits `details` and that this is expected. Structural criterion only: `python3 -c "import json; d=json.load(open(...)); assert any(e['name']=='details-omitted-on-self-loop' for e in d['evals'])"`. **Do not write a criterion that the eval passes** — CI never runs `scripts/run-evals.sh`, and its grading is LLM-judged. Existing evals 11 and 12 do not invert and need no change. |

## Implications

The single largest one: **this change inverts four committed assertions.** Two
integration tests (`instructions_delivery_test.rs:359`, `:487`) and two unit
tests (`persistence.rs:2595`, `:2635`) currently assert that a self-transition
and a repeat directed transition *redeliver*. Their names, their doc comments,
and the rationale prose inside them all encode the old semantics. A PRD whose
acceptance criteria only *add* tests will produce a PR that cannot go green,
and a reviewer who sees those four tests edited without the PRD having
predicted it will reasonably read the diff as a regression being papered over.
The criteria table above therefore names each inverted test explicitly, and
pairs each with an unmodified-test criterion (rows 4, 5, 6, 7, 8a/b) so the
PR's test diff is fully predicted by the document.

The blast radius is smaller than that makes it sound. `occupancy_slice`
(`persistence.rs:1028`) is shared with `latest_epoch_gate_failed` (`:1058`),
which the dashboard read seam and the `/workflows` projection writer both call
— so if the implementation changes `occupancy_slice` itself rather than
introducing a delivery-specific variant, blocked-session classification moves
with it. The PRD should state which of the two it wants; a criterion that
dashboard blocked-classification is unchanged (i.e. every existing
`latest_epoch_gate_failed` test passes unmodified) makes that choice checkable
either way.

Nothing in the frozen public surface moves.
`instructions_delivered_this_occupancy` is not re-exported through
`koto::engine::types` and is not exercised by `koto-stability-tests`, so
`cargo test -p koto-stability-tests` cannot fail on this change and no
`docs/STABILITY.md` bump is implicated.

No template artifact moves either. One shipped template is compiled by CI, its
only self-loop is on a details-free phase, and `details` is a compile-time
string that the delivery rule never touches. The PRD can state flatly that
`validate-plugins.yml`'s "Compile all templates" step and the three committed
`.mermaid.md` files are out of scope, and back it with row 10's byte-identity
criterion plus a `git diff --exit-code plugins/` check if it wants belt and
braces.

A criterion about the token cost of a long loop cannot be gated. Restate it as
row 2's counting form — across N self-loop iterations, exactly one response
carries `details` — which the delivery test proves exactly and which no bench
could prove better.

Two process notes. First, drafts skip `check-artifacts`, `cloud-integration`,
and all of `validate-plugins.yml` and `eval-plugins.yml`; a green draft is not
a green PR, so any "CI passes" criterion should be qualified as "on a
ready-for-review PR". Second, `check-artifacts` fails on a non-empty `wip/`,
which is where this research file lives — it must be deleted before the PR
leaves draft, along with any committed reference to its path.

## Surprises

`docs/reference/session-feed.md:688-692` still says the
`instructions_delivered` event is "**Not emitted yet.** The event type is
reserved and its shape is fixed, but no koto build appends one: instruction
suppression is still keyed on visit count." That is false as of this branch —
`src/cli/mod.rs:3459` and `:4604` both append it, and the integration tests
depend on it. The doc is stale independently of this feature. Correcting it is
cheap and belongs in this PR's doc sweep (row 12), but the PRD should note it
as a pre-existing defect rather than one this change introduces.

`run_koto` returning `Value::Null` on a JSON parse failure
(`instructions_delivery_test.rs:45`) means `assert_omits` passes vacuously if
a response ever stops being parseable single-line JSON. Every omission
criterion above pairs with a `state` or `action` assertion for that reason.

Clippy without `--all-targets` was the one CI detail most likely to be
guessed wrong. A PRD that writes "clippy passes on the new tests" would be
naming a gate that does not exist.

The eval suite looks like a gate and is not one. `eval-plugins.yml` runs a
count check and a `hooks.json` grep, nothing more. `scripts/run-evals.sh`
requires the `claude` CLI, spawns `claude -p` per eval, and is manual-only.

The baseline fixture's second test
(`baseline_fixture_covers_every_required_sequence_and_stays_instruction_free`,
`:577`) is a better anti-regeneration guard than the byte comparison it
protects, because it fails on a fixture that was regenerated into something
weaker — a dropped sequence, or a recorded body that acquired a `details` key.
Row 10 relies on it, and it needs no changes.

## Open Questions

1. Does `koto rewind` ever emit `Rewound { from: X, to: X }` from the CLI?
   Criterion 6b's integration half depends on it. The unit half stands
   regardless, and is the more important of the two, but the PRD should say
   which halves it is claiming.
2. Does the implementation change `occupancy_slice` in place or introduce a
   delivery-specific slice? The former moves `latest_epoch_gate_failed` and
   the dashboard's blocked classification with it. This is a design question,
   but it decides whether the PRD needs a "dashboard classification unchanged"
   criterion, so it needs an answer before the criteria are frozen.
3. Should a *directed* (`--to`) transition into the already-occupied phase
   suppress, given it is an explicit operator action rather than an automatic
   loop? The brief says yes (row 3), and treating both entry kinds alike keeps
   one rule. But `--to` into the phase you are already in is plausibly a
   deliberate "show me this again" gesture, and `--full` is the only escape.
   Worth one sentence in the PRD either way, because it is the row most likely
   to be re-litigated in review.
4. Is a same-phase `DirectedTransition` distinguishable in intent from a
   template-declared self-transition at the point where the rule is applied? If
   the implementation collapses them, question 3 is settled by construction.

## Summary

Three harness families already exist and cover eight of the ten behaviors; the
gaps are a self-rewind (`Rewound{from:X,to:X}`, untested in either harness) and
`--full`'s recording on a suppressed self-loop path. The decisive finding is
that four committed assertions — `instructions_delivery_test.rs:359` and
`:487`, `persistence.rs:2595` and `:2635` — currently assert the opposite of
the self-transition and repeat-directed behaviors, so the PRD must name them as
inversions or the PR reads as a regression. CI's real gates are
`cargo test -- --test-threads=1`, `cargo fmt --check`,
`cargo clippy -- -D warnings` (no `--all-targets`, so test-file lints are
ungated), and `cargo test -p koto-stability-tests`; coverage is non-gating,
evals are never run by CI, no perf harness can support a token-cost criterion,
and no shipped template self-loops on a details-declaring phase, so compiled
output and the byte-identity baseline are both untouched.
