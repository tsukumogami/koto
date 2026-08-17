# Verdict: PASS

## Re-review

### Round 1 findings

| # | Finding | Status after round 2 |
|---|---|---|
| B1 | `--full` criterion cannot fail | Resolved (via N2) |
| B2 | Unpredicted inversion in the status test | Resolved |
| B3 | R11 has no criterion | Resolved |
| B4 | "diff touches only two tests" not binary | Resolved (via N4) |
| M5 | Predicate criteria don't discriminate | Resolved |
| M6 | `git diff --exit-code` vacuous | Resolved |
| M7 | R18 vs. the fixture | Resolved (via N6) |
| M8 | Reachability of self-rewind and round trip | Resolved |
| M9 | Unenumerated no-diff set for R15 | Resolved (via N1) |
| M10 | Omitted gates | Resolved |
| M11 | Orphan requirements | Resolved (via N3, N5, N7) |
| m12 | "the six unit tests" | Resolved |
| m13 | Built-binary criterion has no artifact | Resolved |

### Round 2 findings

| # | Finding | Status | Verification |
|---|---|---|---|
| N1 | R15 unenforceable — shared helper unpinned, `latest_epoch_gate_failed` untested | **Resolved** | Traced the new criterion's synthetic log through both functions; it discriminates. Detail below |
| N2 | `--full` criterion's justification false | **Resolved** | The replacement states the true reason and points R8's recording clause at the one test that decides it |
| N3 | R13 vacuous — override unblocks, next response terminal | **Resolved** | `koto decisions record` verified present and buildable; it is a better instrument than the PRD claims. Detail below |
| N4 | Editing a shared template constant uncovered | **Resolved** | Third clause added, with the right precondition |
| N5 | R12 criterion vacuous by construction | **Resolved** | Restated as by-construction, citing the three real unit tests; new test dropped |
| N6 | Baseline and surfaces criteria disagreed on fixture strings | **Resolved** | Both now permit `notes` and `description`; the load-bearing clause is the `"stdout"` one |
| N7 | R16, R17 orphaned | **Resolved for R16; resolved-with-a-wording-error for R17** | See finding 1 |
| N8 | Window fallback unstated; grep pattern too narrow | **Resolved** | Fallback stated; `occupancy` added with the reason |

**On N1, since it decided the last verdict.** I traced the new criterion's log — entry into
`implement` from `gather`, `delivered(implement)`, a failed `GateEvaluated`, then
`implement → implement` and nothing after — through both functions as they stand.
`latest_epoch_gate_failed` (`src/engine/persistence.rs:1058`) slices from the self-entry,
finds no `GateEvaluated` in the new epoch, and returns `false` via its `unwrap_or`. The
delivery decision slices from the `gather → implement` transition and finds the record, so
it returns `true`. Opposite answers off one log, exactly as the criterion says. And it bites
in the right direction: widen `occupancy_slice` (`:1028`) in place and the gate function
starts slicing from the `gather → implement` transition too, finds the failed gate, returns
`true`, and the criterion's first assertion fails. That is the check R15 was missing, and it
is the only one in the document that can see the in-place edit. It is also constructible
with existing helpers — `GateEvaluated { state, gate, output, outcome, timestamp }`
(`src/engine/types.rs:555-561`) built through `make_event` (`:1141`), with precedent at
`src/engine/persistence.rs:2000`.

**On N3.** `koto decisions record <name> --with-data '{"choice":…,"rationale":…}'` is real
(`src/cli/mod.rs:238,1484-1489`, handler at `:4666`), appends `DecisionRecorded` without
running the advancement loop, and is not in the state-changing set. The criterion is
buildable as written. It is worth more than the PRD claims: `DecisionRecorded` carries a
`state` field (`src/engine/types.rs:551-553`) and `InstructionsDelivered` is the
last-declared variant of an `#[serde(untagged)]` enum requiring only `state` (`:800-808`) —
so a decision event is exactly the payload that would be misread as a delivery if the manual
type-string dispatch at `:1231`/`:1364` ever regressed. This criterion is the only
end-to-end guard on that pairing. Worth a sentence in the criterion so nobody later
"simplifies" it back to a gate override.

## Findings

### 1. MINOR — the R17 criterion misdescribes the natural path

> "The two response-construction sites keep their current read behavior: the directed path
> still builds its event list in memory and the natural path still reuses the tick's own
> re-read."

The directed half is right (`src/cli/mod.rs:3406-3416` chains `events` with a synthetic
event; the comment there says why). The natural half is not: `:4294-4298` calls
`backend.read_events(&name)` itself, and the comment immediately above says it does so
*rather than* reusing an earlier read, because the advancement loop may have appended since.
There is no "tick's own re-read" being reused — the site performs its own.

The check the criterion prescribes — a one-line diff review at each site — still decides
R17, so this is wording, not a hole. Fix: *the natural path still performs exactly one
`read_events` after the advancement loop, gated on the phase declaring instructions
(`src/cli/mod.rs:4291-4299`).*

### 2. MINOR — R7, R10 and R14 are the only requirements without a tag

Every other requirement now carries an explicit `(Rn)`. These three are covered in substance
but untagged, which makes the mapping look incomplete on a skim:

- R7 is decided by `gate_blocked_first_tick_carries_and_repeat_omits` and
  `directed_transition_carries_then_nonadvancing_tick_omits`, both inside the
  what-must-not-have-moved criterion, and by the status test's trailing repeat.
- R10 is decided by having the same behaviors asserted on both paths.
- R14 is decided by the baseline criterion.

Adding the three tags costs nothing and closes the audit.

### 3. MINOR — one seam of R10 that no criterion touches

`--full` is never exercised on the directed path. Both override tests
(`tests/instructions_delivery_test.rs:521,556`) drive the natural path, and no criterion
pairs the flag with `--to`. The risk is low — both sites call the same
`with_details_suppressed_unless_full` combinator (`src/cli/mod.rs:3419` and `:4300`) — but
it is the one combination where the override and the suppression meet on the directed path,
and it is one line to assert: `koto next wf --to implement --full`, issued while already at
`implement`, carries the instructions.

## Harness inventory

Everything the revised criteria lean on exists with a compatible signature. Entries new to
this round are marked.

| Helper | Location | Signature | Fits |
|---|---|---|---|
| `run_koto` | `tests/instructions_delivery_test.rs:34`, `tests/status_phase_retrieval_test.rs:50` | `fn(&Path, &[&str]) -> Value` | yes |
| `assert_carries` / `assert_omits` | `tests/instructions_delivery_test.rs:52,62` | `fn(&Value, &str, &str)` / `fn(&Value, &str)` | yes |
| `session_state_path` | `tests/status_phase_retrieval_test.rs:42` | `fn(&Path, &str) -> PathBuf` | yes |
| `transitioned` | `src/engine/persistence.rs:2503` | `fn(u64, Option<&str>, &str) -> Event` | yes; `condition_type` hardcoded `"auto"` — R1's `skip_if` clause is not expressible as a unit case, and is not claimed as one |
| `rewound` / `directed` / `delivered` | `:2515,2526,2537` | `fn(u64, &str, &str)` / same / `fn(u64, &str)` | yes |
| `make_event` | `:1141` | `fn(u64, EventPayload) -> Event` | yes — **new**: carries the `GateEvaluated` half of the R15 criterion |
| `GateEvaluated` payload | `src/engine/types.rs:555-561` | `{ state, gate, output, outcome, timestamp }` | yes; a `gate_evaluated(seq, state, outcome)` helper would be tidy but is not required |
| `latest_epoch_gate_failed` | `src/engine/persistence.rs:1058` | `pub fn(&[Event], &str) -> bool` | yes, in-module via `use super::*` — **and this is its first test** |
| `occupancy_slice` | `:1028` | `fn(&[Event], &str) -> &[Event]` | the shared boundary; now pinned by behavior rather than by diff |
| `derive_evidence` | `:722` | `pub fn(&[Event]) -> Vec<&Event>` | independent inline scan, as the supplementary criterion now says |
| `CURRENT_SCHEMA_VERSION` | `src/engine/types.rs:199` | `pub const u32 = 1` | yes — **new**: R16's criterion targets the right file |
| `koto decisions record` | `src/cli/mod.rs:238,1484`, handler `:4666` | `<name> --with-data <json>`; `choice` and `rationale` required | yes — **new** |
| `NextResponse::Terminal` combinators + tests | `src/cli/next_types.rs:243,367,479`; tests `:993,1409,1453` | — | yes; R12 now credits them |
| `RECOVERY_POINTER` splice | `src/cli/mod.rs:3426-3430`, `4310-4314` | gated on `details.is_empty()`, terminal passes through | matches R11 and R12 as written |
| `DELIVERY_TEMPLATE` | `tests/instructions_delivery_test.rs:81` | `const &str` | no round-trip path and no terminal-with-instructions phase; the PRD now says both |
| `PHASES_TEMPLATE` | `tests/status_phase_retrieval_test.rs:112` | `const &str` | declares `implement -> implement`; the status-test fix routes around it correctly |

## Summary

Every round-2 finding is closed, and I verified the two that carried real weight rather than
taking them on report: the new R15 criterion produces genuinely opposite answers off one
synthetic log and fails loudly on an in-place widening of the shared helper, and
`koto decisions record` exists, is buildable, and is a sharper instrument for R13 than the
PRD claims — it is the only end-to-end check that a `DecisionRecorded` payload is not
misread as a delivery through the untagged enum.

What remains is three minor items, none of which leaves a criterion undecidable: the R17
criterion describes the natural path's read as reusing an earlier one when the code
deliberately performs its own, R7/R10/R14 are the only requirements left untagged, and
`--full` is never paired with `--to`.

Every acceptance criterion is now decidable by a developer who did not write the document,
every requirement has coverage, all twelve tests in the delivery file and all eight unit
cases are accounted for as unchanged or predicted-to-invert, and the named gate commands
match CI verbatim.
