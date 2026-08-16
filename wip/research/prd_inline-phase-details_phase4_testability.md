# Reviewer: testability

## Verdict

FAIL

Most criteria are binary and constructible against the real codebase, but the concurrency criterion names a scenario that is impossible to build for the majority of sessions and the PRD's own headline scenario (a respawned agent racing its still-running predecessor on an ordinary, non-batch workflow) has no criterion that actually exercises it.

## Criterion-by-criterion verification plan

| Criterion (abbrev) | Binary? | How to verify | Verdict |
|---|---|---|---|
| Gate-blocked repeat: 1st carries instructions, 2nd (still blocked) omits | Yes | Template with a failing command gate; run `koto next` twice against same session, diff JSON, check `details` key absence on 2nd via `jq 'has("details")'` | OK |
| Loop back to same phase after gate passes: instructions re-arrive | Yes | Same template, flip gate to pass, transition away and route back, assert `details` present on arrival tick | OK |
| Advance-then-rewind: next response carries instructions | Yes | `koto next` past the phase, then a rewind-triggering event/call, assert `details` present | OK |
| Two consecutive directed transitions into same phase: 1st carries, 2nd doesn't | Yes | `koto next --state <s>` twice in a row, diff `details` presence | OK |
| Directed transition into never-occupied phase carries instructions | Yes | Fresh session, single directed transition, assert `details` present | OK |
| `koto init` + first `koto next` carries initial phase instructions | Yes | Trivial; existing fixture pattern | OK |
| Batch child's first `koto next` carries its initial instructions | Yes | Existing batch-spawn test fixtures cover this shape already | OK |
| `--full` override returns instructions when rule would omit | Yes | `--full` flag confirmed present at `src/cli/mod.rs:148,2899,4517`; run it on a suppressed tick, assert `details` present | OK |
| No-details template: no `details` field on every path above | Yes | Confirmed today's serialization already omits the key entirely (`skip_serializing_if = "Option::is_none"`, tested at `next_types.rs:861`); same `jq` check | OK |
| Retrieval (name only) returns id, directive, instructions | Yes | Invoke the new command/flag with just the workflow name, inspect JSON | OK |
| Substituted variable in directive/instructions matches `koto next`'s substitution | Yes | Run `koto next` and the retrieval against the same session, diff the substituted string | OK |
| Retrieval returns instructions on a rule-suppressed phase | Yes | Combine with the gate-blocked-repeat fixture, call retrieval on the suppressed tick | OK |
| Retrieval doesn't change next `koto next`'s output (R10) | Yes | `koto next` → retrieval → `koto next`, diff 2nd response against a control run with no retrieval in between | OK |
| **Session state file byte-identical before/after retrieval** | Yes | State file path is deterministic and discoverable: `<sessions_root>/<id>/koto-<id>.state.jsonl` (`persistence.rs:441`, `session/mod.rs:170`). `sha256sum` before/after the retrieval call. No mtime concern — nothing about "byte-identical" implies metadata, and a read-only open never touches file content. | OK, with one caveat below |
| **Gate command with observable side effect not executed by retrieval** | Yes | Gate grammar supports exactly `command` (shell), `context-exists`, `context-matches` (`gate.rs:1`); a `command` gate can run e.g. `touch marker`. Retrieval must leave `marker` absent while a normal `koto next` on the same fixture creates it. | OK |
| Default action not executed by retrieval | Yes | Same side-effect pattern, using a `default_action` shell command instead of a gate | OK |
| Terminal-phase retrieval doesn't clean up session | Yes | Drive to terminal, call retrieval, `test -d <session_dir>` | OK |
| **Retrieval succeeds while a second process holds the session, without blocking** | Yes, but under-specified | See Findings — only buildable against a batch-scoped parent state while it holds the advisory `flock`; the phrase "holds the session" doesn't say this, and as written the scenario is unconstructible against >90% of templates (any non-batch session, which is the PRD's own respawn scenario). | **Needs rewording** |
| Unknown workflow name: structured error, non-zero exit | Yes | Existing error-convention pattern (`NextError`/exit codes already tested elsewhere) | OK |
| No-instructions phase: succeeds, reports absence, not an error | Yes | Retrieval on a plain template, assert exit 0 and absent/empty instructions field | OK |
| Retrieval returns expects schema when declared | Yes | `expects: ExpectsSchema` already exists on `EvidenceRequired` and as `Option<ExpectsSchema>` elsewhere (`next_types.rs:69,86,...`); reuse the same type on the retrieval's declared phase | OK |
| Every non-terminal response carries the pointer | Yes, enumerable | `NextResponse` is a closed 7-variant enum: `EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable, Terminal, ActionRequiresConfirmation, Error` (`next_types.rs:63-127`). "Non-terminal" = all but `Terminal` (6 variants). A non-exhaustive `match` won't compile once the field is added to the shared type, so the compiler forces coverage; tests then need one fixture per variant. | OK, but the PRD/AC should say "every `NextResponse` variant except `Terminal`" rather than leave the enumeration implicit |
| Directive text unaltered when pointer also present | Yes | Diff directive string with/without the new field present | OK |
| No new file under session dir; schema version unchanged | Yes | `ls <session_dir>` before/after across the whole suite; grep the version constant | OK |
| `koto-stability-tests` passes unmodified | Yes | `cargo test -p koto-stability-tests` | OK |
| Direct tests of response construction/delivery rule (repeat, rewind, both directed cases) | Yes | Unit-level, same pattern as existing `advance.rs` tests | OK |
| `cargo fmt --check`, `cargo clippy -D warnings`, full suite pass | Yes | Direct commands | OK |
| **`koto next` responses byte-identical to those koto produces today** (R6 in prose, echoed by the no-details AC) | Yes, but no baseline exists yet | No golden JSON fixture for full response bodies exists (`koto-stability-tests` pins type/schema shape, not response content — checked `lib.rs`, all tests are `resolves_and_constructs`/schema-version tests, none snapshot a `koto next` JSON body). Verifier must freeze today's output as a fixture *before* the change lands (or diff a pre-change build against post-change on an identical frozen session), then diff. Response bodies carry no wall-clock/uuid fields in the checked structs, so identical inputs should produce identical outputs once a state file is held fixed. | OK, but flag: this needs the verifier to manufacture the "today" baseline; nothing pre-existing does it for them |
| Downstream docs/evals/CHANGELOG criteria (last block) | Yes | File existence + grep for described behavior; standard | OK |
| `wip/` empty, no committed `wip/` reference | Yes | `grep -r wip/` over tracked files, standard per workspace convention | OK |

## Findings

**1. Binary pass/fail.** Every criterion resolves to a yes/no check; none ask a developer to judge quality, style, or "reasonableness." No subjective-language criteria found (no "appropriately," "correctly handles," etc.).

**2. Verifiability — grounded, with one real gap and one implicit dependency.**

- Session state file path, gate types, `--full` flag, the closed `NextResponse` enum, and the no-details serialization behavior are all confirmed to exist and behave as the criteria assume — checked directly against `src/session/mod.rs:170`, `src/engine/persistence.rs:441`, `src/gate.rs`, `src/cli/mod.rs`, and `src/cli/next_types.rs:63-127,861`.
- **The concurrency criterion is the one place grounding changes the verdict.** The advisory `flock` on a session's state file is acquired **only for batch-scoped parent states**, and only "for the rest of the tick" (`src/cli/mod.rs:3746-3789`, comment: *"Non-batch workflows intentionally skip the lock"*; `src/engine/leg_pointer.rs:13`: *"a non-batch session holds no lock during `koto next`"*). For any ordinary (non-batch) session — which is every session in the PRD's own motivating narrative about a respawned agent — there is **no lock to hold**, so "a second process holds the session" cannot be constructed at all in that setting. The only way to build this test is: a batch-scoped parent state, whose gate or default action sleeps long enough to hold the lock open, run in the background, with the retrieval racing in concurrently and a third `koto next` call used to confirm the lock is genuinely held (it should fail with `ConcurrentTick`). That is buildable — the existing `lock_state_file_cross_process_contention` test at `src/session/local.rs:1781` is precedent for exactly this pattern — but the AC's own vocabulary ("holds the session") doesn't say any of this, and a developer following it literally against a plain template would find the scenario simply can't be made true, or would build a vacuous version that never touches the actual lock. This also means R12's second sentence — *"A respawned agent must be able to retrieve instructions while its predecessor is still running"* — is, for the common non-batch case, trivially true by construction (nothing blocks) and untested by anything in the criteria set as written.

**3. Coverage of edge cases.** The set is broad and mostly matches the failure modes named in the Problem Statement and Decisions section (gate-fail loop, rewind, both directed-transition cases, batch child spawn exemption implicitly covered by "not in defect set" needing no criterion). One gap stands out: **the PRD's headline scenario — a freshly respawned agent whose predecessor is still alive on an ordinary, non-batch session — has no dedicated criterion.** The closest one (concurrency/lock) resolves, per the grounding above, to the batch-parent case specifically. A criterion exercising "process A is mid-tick on a non-batch session (e.g., blocked in a slow gate command); process B calls the retrieval concurrently and succeeds immediately" would directly verify the scenario the Problem Statement leads with and currently goes unverified. Context-compaction itself is correctly *not* given a dedicated criterion — it's unobservable to koto by definition, and the Known Limitations section says so; the criteria instead correctly test the mitigation (retrieval + pointer), which is the right substitution.

**4. Duplication.** No criterion is a bare restatement of a requirement without an attached observable check; each names a concrete setup and a concrete assertion.

**5. Feasibility.** Nothing found requires a materially larger change than the PRD's own scope. Notably, R16 ("no new state file, no schema bump") looked risky at first glance — a gate-blocked, non-advancing tick appears in the Problem Statement to "enter nothing" — but `src/engine/advance.rs:382-389` shows a `GateEvaluated` event is already appended on every gate evaluation, blocked or not, so a delivery rule can plausibly be derived from the existing log the same way `derive_visit_counts` already is. For gate-less phases that only await evidence (`EvidenceRequired` with no gate), no event currently appears to be logged on a non-advancing repeat tick either way; if the design needs a new lightweight event kind to cover that case, the codebase's own precedent (`skip_if_matched`, `submitter_cwd`, `spawn_entry` — all additive, `skip_serializing_if`-guarded fields with no version bump) shows that's compatible with R16 as written. Not a blocker, just worth the DESIGN's attention.

## Required changes

1. **Reword the concurrency criterion** ("A retrieval succeeds while a second process holds the session, without blocking") to name the actual mechanism: the advisory `flock` on a batch-scoped parent's state file, held for the duration of a tick. As written it's either unconstructible (non-batch) or silently vacuous depending on which template a developer reaches for.

2. **Add a criterion for the non-batch respawn race**, the scenario the PRD's Problem Statement and Goals actually lead with: a first process mid-tick on an ordinary session (e.g., blocked inside a slow gate/default-action command) while a second process calls the retrieval and gets an immediate, correct response. Without this, the criteria set verifies the rare batch-lock case and never verifies the common one.

3. **Enumerate the non-terminal variant set** in the discoverability criterion (or point at `NextResponse`'s variants explicitly) rather than leaving "every non-terminal response" for the verifier to derive from the enum definition themselves — cheap to add, removes the one place a developer has to go spelunking to know what "all such responses" means.

4. **Flag, in the AC or a Decisions note, that the "byte-identical to today" baseline doesn't exist yet** and must be captured before the change lands (frozen fixture or pre-change binary diff) — not a blocker, but leaving it unstated risks an implementer treating it as self-evidently checkable when no current test does anything like it.
