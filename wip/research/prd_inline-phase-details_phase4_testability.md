# Reviewer: testability

## Verdict
PASS

Every acceptance criterion is binary and I could construct a concrete verification for each; the seven flagged trouble spots from the previous round all check out against source, including two (the batch-lock and directed-transition machinery) that turned out to be more solid than the PRD's own cited comments suggested.

## Criterion-by-criterion verification plan

| Criterion (abbreviated) | Binary? | How to verify | Verdict |
|---|---|---|---|
| Gate-fail retick omits instructions on 2nd tick | Yes | Template with a `command` gate `command: "exit 1"` on a state with a `<!-- details -->` split; `koto init`, `koto next` (carries details), `koto next` again (gate still fails, no transition) — assert `details` absent on 2nd JSON | Verifiable |
| Loop-back re-carries instructions | Yes | Same template with a later transition back to that state; drive the loop, assert `details` present on re-arrival | Verifiable |
| Self-transition re-carries instructions | Yes | Confirmed: `template-format.md` "Self-loops" section — `transitions: [{target: proceed, when:...}, {target: same_state, when:...}]` compiles. Test: force the self-targeting branch, assert `details` present on the arrival response | Verifiable |
| Unconditional-transition arrival carries instructions | Yes | Confirmed: single `transitions` entry with no `when` is unconditional per template-format.md L74, L216. Straightforward fixture | Verifiable |
| Rewind arrival carries instructions | Yes | `koto rewind <name>` confirmed as a real subcommand (`--rationale` only). Advance past the phase, rewind, `koto next`, assert `details` present | Verifiable |
| Two consecutive directed transitions, 2nd omits | Yes | `koto next <name> --to <state>` confirmed real (`--to`, `--rationale`). Fire twice into same target, assert 1st has `details`, 2nd doesn't | Verifiable |
| Directed transition into never-occupied phase carries instructions | Yes | Same `--to` mechanism against a fresh state | Verifiable |
| `koto init` + first `next` carries initial instructions | Yes | Trivial fixture | Verifiable |
| Batch child's first `next` carries instructions | Yes | `koto init --parent <p> --template <child>`, then `next` on child | Verifiable |
| `--full` override returns instructions | Yes | Confirmed real flag: "Always include the details field... regardless of visit count" | Verifiable |
| Byte-identity for instructions-free template | Yes, contingent on baseline capture | No current test snapshots whole response bodies (confirmed — `tests/` has no such fixture today). PRD's own Decisions section already requires capturing a pre-change fixture as a prerequisite step, not something a later reader reconstructs. This makes the criterion satisfiable as scoped | Verifiable, decision correctly recorded |
| Retrieval returns id/directive/instructions by workflow name alone | Yes | Deferred to DESIGN for surface name; contract itself is checkable once implemented | Verifiable |
| Retrieval substitutes runtime variables | Yes | Template with `{{VAR}}` in details; compare retrieval output to what `next` would substitute | Verifiable |
| Retrieval returns instructions even when rule suppresses | Yes | Drive to 2nd-tick-suppressed state, call retrieval, assert non-empty instructions | Verifiable |
| Retrieval doesn't affect next `koto next` | Yes | Diff two 3-call sequences, with/without a retrieval spliced in between | Verifiable |
| State file byte-identical before/after retrieval | Yes | `md5sum`/byte-diff the `.state.jsonl` file pre/post | Verifiable |
| Retrieval doesn't execute a shell-command gate | Yes | Gate command touches a sentinel file; call retrieval; assert sentinel absent | Verifiable |
| Retrieval doesn't execute default_action | Yes | Same sentinel-file pattern against `default_action.command` (confirmed `ActionDecl.command` is an arbitrary shell string) | Verifiable |
| Retrieval at terminal doesn't clean up session | Yes | Drive to terminal, call retrieval, assert session dir still exists and response is success-shaped | Verifiable |
| Retrieval against batch-scoped parent succeeds under held lock | Yes | **Now genuinely constructible end-to-end.** `state_is_batch_scoped` (src/cli/mod.rs:2211) calls `state_has_materialize_children`, which is a real, tested detector (not the "always returns false" stub `batch_lock_test.rs`'s header comment still describes — that comment predates Issue #7 landing). Build a template with a `materialize_children` hook on a state, add a slow `command` gate (`sleep N`, arbitrary shell string, confirmed) on the same state so the holding tick has a window, run it as a real backgrounded process, and call the retrieval concurrently. `batch_lock_test.rs`'s own header even anticipates this exact upgrade path ("When real batch plumbing lands, this file can be replaced... with a cross-process `koto next` test against a template that carries a `materialize_children` hook") | Verifiable — feasibility upgraded vs. what the codebase's own stale test comment implies |
| Retrieval against non-batch session succeeds during a slow tick | Yes | Confirmed constructible: both `Gate.command` (arbitrary shell, configurable `timeout`) and `ActionDecl.command` (arbitrary shell) support a deliberately slow command (e.g. `sleep 5`). Build a template with such a gate or default_action, launch a real `koto next` subprocess against it, and race a retrieval call against it mid-sleep, asserting the retrieval returns promptly | Verifiable — buildable via existing grammar, no gap |
| Unknown-workflow retrieval errors structurally | Yes | Call retrieval against a name with no session dir | Verifiable |
| No-instructions phase reports absence, not error | Yes | Template state with no `<!-- details -->` marker | Verifiable |
| Retrieval returns expected-evidence schema when declared | Yes | State with an `accepts` block; compare to `derive_expects` output shape | Verifiable |
| Discoverability pointer on every instructions-carrying variant | Yes | **Confirmed exact against source.** `NextResponse` (src/cli/next_types.rs:63-127) has exactly 7 variants: `EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`, `Terminal`, `ActionRequiresConfirmation`, `Error`. The 5 named in the AC are exactly those carrying a `details: Option<String>` field; `Terminal` and `Error` are exactly the 2 that don't. The enumeration is complete and correctly excludes the right two | Verifiable, and independently confirmed correct |
| No-pointer on no-instructions phase | Yes | Response inspection | Verifiable |
| Directive text present/unaltered alongside pointer | Yes | String comparison | Verifiable |
| No new session-dir file, schema version unchanged | Yes | `ls` the session dir before/after; check `schema_version` in header | Verifiable |
| `koto next` opens no extra files (strace diff) | Binary in principle; heavy in practice | Realistically constructible but not trivial: this is a git repo, so a developer builds the pre-change commit and the post-change branch, runs both under `strace -f -e trace=open,openat` against the same template/session/call sequence, and diffs the filtered opens. Feasible on the CI's ubuntu-latest runner (ptrace is unrestricted there) but not something a unit test asserts — it's a manual, one-time developer verification, consistent with how R18 frames it ("How that is achieved is the DESIGN's to decide"). One gap: the AC doesn't name a *concrete* workflow/call sequence the way the byte-identity criterion does ("the same template and the same sequence of calls, on every path above") — it just says "the same workflow and the same call." Recommend tightening this to point at one concrete fixture so two developers don't strace different scenarios | Verifiable, with a wording tightening suggested (non-blocking) |
| `koto-stability-tests` passes unmodified | Yes | `cargo test -p koto-stability-tests`. Confirmed this crate pins Rust-level types (`StateFileHeader`, `Event`, `EventPayload`, `SessionBackend` trait methods) — it does **not** touch `NextResponse`/JSON shape at all, so this instruction-delivery change has no plausible path to breaking it | Verifiable, and low-risk as scoped |
| `koto template compile` succeeds on every template under `plugins/` | Yes | **Ran it live.** `./target/release/koto template compile plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md` exits 0. Confirmed the CI glob (`.github/workflows/validate-plugins.yml:39`) is `find plugins/koto-skills/skills/ -path '*/koto-templates/*.md'`, and such templates genuinely exist and compile today | Verifiable, confirmed working |
| Tests exercise response construction / delivery rule directly | Yes | New `#[test]`s in `tests/` against `handle_next`/`NextResponse` construction, not just the counting primitive | Verifiable |
| `cargo fmt --check`, `cargo clippy -D warnings`, full suite pass | Yes | Direct commands | Verifiable |
| `koto-user` docs describe shipped rule + retrieval | Yes | Diff review against R20 | Verifiable |
| `koto-author` docs describe author-facing contract | Yes | Diff review against R21 | Verifiable |
| `cli-usage.md` / Cursor rules match shipped behavior | Yes | Diff review against R22 | Verifiable |
| Every skill under `plugins/*/skills/*/` retains ≥1 eval | Yes | Confirmed real: `evals.json` exists under `koto-user/evals/`, `koto-author/evals/`, `koto-adhoc/evals/` today. `ls`/count post-change | Verifiable |
| `CHANGELOG.md` records the change | Yes | Diff review | Verifiable |
| `wip/` empty, no committed `wip/` reference | Yes | `grep -r wip/` per the workspace's standing wip-hygiene rule | Verifiable |

## Findings

**1. Binary-ness.** Every criterion resolves to a clear pass/fail with no subjective judgment required. None asks a reviewer to assess quality, adequacy, or "good enough" — each is a concrete state comparison, string/field presence check, or command exit code.

**2. Verification constructibility.** I could name a concrete procedure for all 41 acceptance criteria; none required assuming machinery that doesn't exist. The two items the review brief called out as previously-problematic (batch lock, respawn race) both turned out to be *more* solidly buildable than the codebase's own stale comments suggest, because `state_is_batch_scoped` already does real detection (Issue #7 landed) and both gate commands and default actions accept arbitrary shell strings including deliberately slow ones.

**3. Edge-case coverage — one real gap.** R3 lists six ways an occupancy can begin: conditional transition, unconditional transition, directed transition, self-transition, rewind, and init. The acceptance criteria test self-transition only via the natural-advancement path (a `when`-gated transition targeting the same state) and directed transitions only into a *different* or *never-occupied* state. Nothing tests a **directed transition whose `--to` target is the state already occupied** (`koto next <name> --to <current-state>`). Per the Occupancy definition ("ends when the next state-entry event names any phase, including the same one"), this should behave like a self-transition and re-deliver — but that's an inference, not something any AC pins down. This is a legitimate coverage gap worth adding a criterion for, though not one that undermines the PRD's overall testability.

**4. Duplication.** No acceptance criterion merely restates a requirement in imperative form — each adds either a concrete scenario (delivery-rule criteria) or a concrete command/tool (constraints-and-downstream criteria). No changes needed here.

**5. Feasibility.** All criteria are achievable within scope. The heaviest one — the strace file-open comparison — is a manual, one-time developer verification rather than a CI-automated assertion, which is appropriate for an NFR whose DESIGN is explicitly left open ("How that is achieved is the DESIGN's to decide"), but it would benefit from naming one concrete template/call-sequence the way the byte-identity criterion does, so two different verifiers don't strace two different scenarios and reach different conclusions about what counts as "the same call."

**6. Terminology note (non-blocking).** Several acceptance criteria say "no instructions field" / "returns... instructions" as if `instructions` were a literal JSON key. The current wire shape's equivalent is the `details: Option<String>` field on `NextResponse` (confirmed in `next_types.rs`), which already omits itself from serialization when `None`. The PRD never explicitly states that "instructions" (its conceptual term) maps onto the existing `details` field rather than requiring a newly-named key. This doesn't break binary-ness — once a verifier knows which key to check, the criterion is unambiguous — but a developer coming in cold could read "instructions field" as requiring an actual field renamed or added called `instructions`, which has R6/R17 implications the PRD doesn't discuss. Worth a one-line clarification, not a blocker.

## Required changes

No required changes — verdict is PASS. The two items above (directed-self-transition coverage gap, "instructions" vs. "details" terminology) are worth a light pass before DESIGN but don't rise to acceptance-criteria defects that block finalization.
