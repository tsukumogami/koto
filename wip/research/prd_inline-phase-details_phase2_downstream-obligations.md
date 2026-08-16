# Lead: mandatory downstream obligations

## Findings

### 1. koto's CLAUDE.md contract (verbatim)

From `/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_90-f3dfa61e/public/koto/.claude/worktrees/docs+inline-phase-details/CLAUDE.md:58-64`:

> **After completing any source change in `src/` or `cmd/`, assess both skills before closing the work:**
>
> 1. **Broken contracts** -- read the diff and each skill, then ask: does anything the skill currently documents no longer match the code? Look for changed flag names, renamed fields, removed subcommands, altered response shapes, or behavior that works differently than described.
>
> 2. **New surface** -- ask: does this change add CLI flags, subcommands, response fields, gate types, or behavior that neither skill mentions? New surface that agents will encounter belongs in the relevant skill.
>
> If either question surfaces gaps, update the skill in the same PR. A separate skill-update PR is acceptable only when the scope is large enough to warrant it -- document the gap in the PR description so it isn't lost.

Source-area-to-skill mapping table (`CLAUDE.md:66-73`):

| Area | Relevant skill |
|------|---------------|
| `src/cli/` -- subcommands, flags, JSON output types | both |
| `src/engine/` -- advance loop, action values, response schema | koto-user |
| `src/gate/` -- gate types, structured output fields | both |
| `src/template/` -- frontmatter fields, compiler errors/warnings | koto-author |

The incoming change touches `src/cli/` and `src/engine/` -- both rows apply, so both `koto-user` and `koto-author` are named by the mapping table (koto-user via both listed rows, koto-author via the `src/cli/` row).

Eval-results-in-PR-description requirement (`CLAUDE.md:95-105`):

> **Include eval results in the PR description** when submitting skill changes. Use this format:
>
> ```
> ## Eval Results
>
> | Skill | Assertions | with_skill | without_skill | Delta |
> |-------|-----------|------------|---------------|-------|
> | koto-user | 18/18 (100%) | 100% | 60% | +40pp |
> ```
>
> CI enforces that every skill has at least one eval (`check-evals-exist.sh`). Running the evals themselves is manual -- they require an Anthropic API key and spawn Claude sessions.

That last sentence is the load-bearing distinction for this whole lead: CI enforces evals *exist*; it does not enforce that they *pass*, and it does not run them.

### 2. The two skills -- current `details` / `--full` / visit-count contract

**koto-user** (guides agents *running* koto workflows):

- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:16` -- field-presence table row: `details` is "conditional" on `evidence_required`/`gate_blocked`/`integration`/`integration_unavailable`/`confirm`, "absent" on `done`.
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:37-38` -- "`details` follows visit-count logic: present on the first visit to a state, absent on subsequent visits unless `--full` is passed. It is always absent on `done` regardless."
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:88` -- Scenario (a) decision point: "`details` is omitted on subsequent visits unless `--full` is passed."
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:148-149` -- Scenario (b) decision point: "`details` is absent here because this is a repeat visit. It would appear on the first visit to this state."
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:398` -- Scenario (h) `done`: "`details` is **absent** -- the terminal variant has no `details` field."
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:527-528` -- "Checking for absent fields" reminder: "Check whether `details` is present before reading it; it may be omitted on any action type depending on visit count."
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:96` -- flag table entry for `koto next`: "`--full` | Always include the `details` field, even on repeat visits to a state. By default `details` is omitted after the first visit."
- `plugins/koto-skills/skills/koto-user/SKILL.md` has **no direct mention** of `details`/`--full` in the main body -- it only points to `response-shapes.md` and `command-reference.md` as references to consult on demand (SKILL.md:467-474). So the entire user-facing contract for this behavior lives in the two reference files above, not in SKILL.md itself.

If the suppression rule changes (e.g., different threshold, different flag semantics, or a change to what counts as a "visit"): every one of the response-shapes.md passages above needs rewording, plus command-reference.md:96's flag description. This is concentrated in exactly two files.

If a new read-only retrieval command/flag is added: `command-reference.md`'s subcommand table (lines 8-38) needs a new row categorized by audience ("Runner -- primary" is the pattern used for read-only commands like `koto status`, `koto context get`); a new `## koto <command>` section needs to be added following the existing pattern (see `koto status` at command-reference.md:271-309 as the closest analog -- read-only, no side effects, no gate evaluation); and response-shapes.md's "Checking for absent fields" section likely needs a cross-reference if the new command surfaces `details` outside the visit-count-gated path. SKILL.md itself may need a one-line pointer if the new command becomes part of the common path rather than an edge case (SKILL.md's own convention is "reference material... consult only when you hit the specific situation" — SKILL.md:469).

**koto-author** (guides agents *writing* koto templates):

- `plugins/koto-skills/skills/koto-author/SKILL.md:67` -- "Read the `directive` for instructions. On first visit to a state, a `details` field may contain extended guidance (pass `--full` to force it on repeat visits)" -- this is the one mention in SKILL.md itself, in the context of explaining the compiled-template verification loop.
- `plugins/koto-skills/skills/koto-author/references/template-format.md:92-124` -- the full "`<!-- details -->` marker" section, which documents template *authoring* syntax (not the runtime read contract): how to split a state's markdown body into directive+details with the HTML comment marker, the authoring guidance ("Use details for multi-paragraph instructions..."), the no-marker fallback behavior, and the multiple-marker tie-break rule (first marker wins). Specifically:
  - `template-format.md:94-116` -- the marker syntax and worked example.
  - `template-format.md:118` -- "Content before the marker is the **directive** -- always returned by `koto next`. Content after is the **details** -- returned only on first visit to the state, or when the caller passes `--full`." This is the same suppression rule restated from the authoring side.
  - `template-format.md:120` -- authoring guidance on when to use details vs. keep everything in the directive.
  - `template-format.md:122` -- "States without the marker behave exactly as before -- everything is the directive, and `details` is empty."
  - `template-format.md:124` -- multiple-marker tie-break rule.

If the suppression rule changes: `SKILL.md:67` and `template-format.md:118` both restate the visit-count/`--full` rule and both need the update. The authoring guidance at `template-format.md:120` may also need revision if the new rule changes what content authors should put in `details` (e.g., if retrieval becomes on-demand rather than visit-gated, "multi-paragraph instructions that clutter the directive on repeat visits" framing may no longer be the right authoring heuristic).

If a new read-only retrieval command is added: koto-author's contract is about *producing* `details` content via the template marker, not about *retrieving* it at runtime, so the marker syntax section itself likely doesn't change. But the authoring guidance (`template-format.md:120`) may need a note if the new retrieval path changes how authors should think about details length/content (e.g., if details becomes retrievable on demand regardless of visit count, authors might be encouraged to be more liberal about what goes there). SKILL.md:67's one-line mention should get the same cross-reference update as koto-user's if a new command changes how details is read back during the author's own verification loop.

### 3. Evals

**Format:** Both `evals.json` files use the schema consumed by `scripts/run-evals.sh`'s Python prep step: a top-level `{"evals": [...]}` array, each item having `id`, `name`, `prompt`, `assertions` (list), and optionally `tier` (1 = plan_only, 2 = execute) and `fixture_dir`. Tier defaults to 1 if absent (`run-evals.sh:213`, `ev.get("tier", 1)`).

**Counts:**
- `plugins/koto-skills/skills/koto-user/evals/evals.json`: 10 evals -- `session-init-first-cycle`, `evidence-submission-enum-field`, `gate-blocked-non-actionable-escalate`, `negative-template-authoring-redirect`, `hierarchy-temporal-blocking`, `negative-general-state-machine-redirect`, `hierarchy-init-child-workflow`, `converge-read-child-result-inline`, `request-leg-result-read-precedence`, `bound-leg-resolves-by-promotion`.
- `plugins/koto-skills/skills/koto-author/evals/evals.json`: 7 evals -- `new-skill-init-command`, `convert-existing-skill`, `compile-error-handling`, `template-format-layers`, `negative-workflow-execution-redirect`, `children-complete-gate-template`, `negative-general-yaml-redirect`.

**None assert on `details`/`--full` behavior today.** A grep for `details` and `--full` inside both `evals.json` files returned zero matches. This is a real gap: the suppression rule and the `--full` override are documented contract (per section 2 above) but have no eval coverage today. A PRD introducing a new retrieval command or changing the suppression rule has no existing eval to update -- it would need to *add* a new eval (or extend an existing one, e.g. `session-init-first-cycle`) to cover the changed/new behavior, not just "keep evals passing."

**`check-evals-exist.sh` enforcement:** it walks `plugins/*/skills/*/`, and for every `SKILL.md` found (skipping `disable-model-invocation: true` skills as exempt), requires `evals/evals.json` to exist AND contain at least 1 eval (counted via `python3 -c "import json; print(len(...))"`). It does **not** check content, does not check that any eval covers a specific behavior, and does not run anything. It's purely "does this skill have >=1 eval scenario recorded." Exit 1 on any missing/empty file, listed under "MISSING EVALS" (`check-evals-exist.sh:41-53`).

**Running the evals is manual, not a CI gate.** `scripts/run-evals.sh` requires the `claude` CLI and spawns `claude -p` sessions running `/skill-creator` to execute with-skill/without-skill agent pairs per eval and grade against assertions -- this needs an Anthropic API key and takes real wall-clock time ("this may take several minutes", `run-evals.sh:230`). Nothing in `.github/workflows/` invokes `run-evals.sh`. The CLAUDE.md line quoted in section 1 says this explicitly.

### 4. CI -- every check that runs on a PR

Ten workflow files under `.github/workflows/`. Enumerated by trigger relevance to a `src/cli/` + `src/engine/` change (which will also very likely touch `plugins/koto-skills/**` per the mandatory-assessment rule):

| Workflow file | Job(s) | Trigger | What would fail it for this change |
|---|---|---|---|
| `validate.yml` | `check-artifacts` | every PR | Leftover files under `wip/` not cleaned up before merge (wip-hygiene). |
| `validate.yml` | `unit-tests` | every PR | `cargo test -- --test-threads=1` -- any broken unit/integration test, including the `derive_visit_counts` tests and `NextResponse` serialization tests in `src/cli/next_types.rs` and `src/engine/persistence.rs` if the change alters those without updating tests. |
| `validate.yml` | `stability-tests` | every PR | `cargo test -p koto-stability-tests` -- only fails if the change touches the frozen Stage-1 public surface (`koto::engine::types::*`, the 4 frozen `SessionBackend` methods). A `details`/retrieval change is unlikely to touch this unless it changes a frozen exported type. |
| `validate.yml` | `fmt` | every PR | `cargo fmt --check` -- any unformatted Rust. |
| `validate.yml` | `clippy` | every PR | `cargo clippy -- -D warnings` -- any new clippy warning (warnings are errors here). |
| `validate.yml` | `audit` | every PR | `cargo audit` -- only fails on a newly-flagged dependency vulnerability; unrelated to this feature unless it touches `Cargo.toml`. |
| `validate.yml` | `coverage` | every PR | Does **not** fail CI on its own: `fail_ci_if_error: false` on the Codecov upload step (`validate.yml:132`). `codecov.yml` sets a 60% project target / 1% threshold and 50% patch target, but these are Codecov *status checks* surfaced on the PR by the Codecov GitHub App/bot, not a job in this workflow that the `validate` aggregator job (`validate.yml:187-205`) waits on or fails from. Whether an under-target patch actually blocks merge depends on GitHub branch-protection required-status-check configuration, which is not visible in this repo's files -- **not a gate enforced by anything in `.github/workflows/`**, only a possible external status check.
| `validate.yml` | `tsuku-distributed-install` | every PR | Only fails on `.tsuku-recipes/koto.toml` syntax issues (PR path) or actual koto binary breakage (push path); essentially unrelated to this change. |
| `validate.yml` | `cloud-integration` | every PR (draft excluded) | Skips entirely if R2 secrets aren't configured (`if [ -z "$KOTO_TEST_S3_ENDPOINT" ]; then ... exit 0`); when configured, runs `cargo test --features cloud-integration-tests`. Unrelated to this feature unless it touches the cloud session backend. |
| `eval-plugins.yml` | `eval-coverage` | PR touching `plugins/**` | **This is the merge gate for evals.** Runs `bash scripts/check-evals-exist.sh` -- fails only if a skill under `plugins/*/skills/*/` lacks `evals/evals.json` or has zero evals in it. Since this change is very likely to touch `plugins/koto-skills/**` (the mandatory skill assessment), this workflow triggers. It would NOT fail merely because the new evals don't yet cover `details`/`--full` behavior -- it only checks *existence/count*, not content relevance. |
| `eval-plugins.yml` | `no-hooks` | PR touching `plugins/**` | Fails only if a `hooks.json` is placed inside an individual skill directory instead of the plugin root; unrelated to this change. |
| `validate-plugins.yml` | `template-compilation` | PR touching `plugins/**` or `.claude-plugin/**` | Builds koto release binary, then runs `koto template compile` against every `.md` under `*/koto-templates/*` (skipping `.mermaid.md`). If the change alters template compilation (e.g., changes to the `<!-- details -->` marker parsing in `src/template/compile.rs`) in a way that breaks the existing `koto-author.md` / any shipped template, this fails. |
| `validate-plugins.yml` | `hook-smoke-test` | same trigger | Exercises the `Stop` hook against a mock session state file; unrelated to `details`/retrieval unless the hook's output format changes. |
| `validate-plugins.yml` | `schema-validation` | same trigger | Validates `plugin.json` and `marketplace.json` required fields; unrelated unless those manifests change. |
| `check-template-freshness.yml` | `check-freshness` | `workflow_call` only (reusable, not directly triggered by `pull_request` in this repo -- no top-level `on:` beyond `workflow_call`) | Not a direct PR gate in this repo's own workflows; it's a reusable workflow other repos/workflows can call. Not applicable unless something else in this repo calls it (a grep for callers found none in `.github/workflows/*.yml` other than itself). |
| `validate-docs.yml` | `validate` (calls shirabe's reusable `validate-docs.yml@main`) | PR touching `docs/**` | Per-file doc format validation (frontmatter, sections, issue-table rows) for docs carrying a `schema:` field. Relevant if the PRD/DESIGN docs for this feature get added under `docs/`. |
| `validate-pr-body.yml` | `pr-body` (calls shirabe's reusable `pr-body.yml@main`) | every PR (opened/edited/reopened/synchronize/ready_for_review) | Enforces Conventional Commits PR title, one `---` separator with non-empty Part 1, no AI-attribution footer. Applies to this PR regardless of what files it touches. |
| `lifecycle.yml` | `lifecycle` (calls shirabe's reusable `lifecycle.yml@main`) | every PR (opened/synchronize/reopened/ready_for_review/converted_to_draft) | Enforces DRAFT-vs-READY doc lifecycle discipline (mid-chain states pass in draft; single-pr chains must be at terminal state -- PLAN deleted, BRIEF/PRD Done, DESIGN Current -- when marked ready). Applies if this PRD/PLAN work is tracked via the shirabe doc chain in this repo. |
| `benches.yml`, `prepare-release.yml`, `release.yml`, `finalize.yml` | various | not PR-triggered (release/benchmark automation) | Not applicable to a feature PR. |

**Summary MERGE GATE vs CONVENTION table:**

| Requirement | Type | Enforcing file / mechanism |
|---|---|---|
| Skill has >=1 eval scenario | MERGE GATE | `eval-plugins.yml` -> `scripts/check-evals-exist.sh` |
| Evals actually pass / cover the new behavior | CONVENTION only | Nothing runs `run-evals.sh` in CI; manual, documented in `CLAUDE.md:105` |
| Eval results table in PR description | CONVENTION only | No workflow parses/validates PR body content for this table; `validate-pr-body.yml` only checks mechanical structure (title format, `---` separator, no AI-attribution footer), not this table's presence |
| Assess koto-user/koto-author for skill drift after `src/`/`cmd/` change | CONVENTION only | Documented in `CLAUDE.md:58-64`; no workflow checks that a skill file was touched alongside a `src/cli/` or `src/engine/` diff |
| wip/ empty before merge | MERGE GATE | `validate.yml` -> `check-artifacts` job |
| `cargo fmt` clean | MERGE GATE | `validate.yml` -> `fmt` job |
| `cargo clippy -D warnings` clean | MERGE GATE | `validate.yml` -> `clippy` job |
| Unit/integration tests pass | MERGE GATE | `validate.yml` -> `unit-tests` job |
| Stability-tests pass (frozen surface) | MERGE GATE | `validate.yml` -> `stability-tests` job |
| Coverage threshold met | NOT a workflow gate | `codecov.yml` targets exist but `fail_ci_if_error: false`; enforcement (if any) is an external GitHub-branch-protection Codecov status check not visible in-repo |
| Template compilation succeeds for shipped templates | MERGE GATE (if `plugins/**` touched) | `validate-plugins.yml` -> `template-compilation` |
| PR body format (Conventional Commits title, `---` separator, no AI footer) | MERGE GATE | `validate-pr-body.yml` -> shirabe reusable `pr-body.yml` |
| Doc lifecycle discipline (DRAFT/READY states) | MERGE GATE (if doc chain used) | `lifecycle.yml` -> shirabe reusable `lifecycle.yml` |
| Doc format (frontmatter/sections) for `docs/**` | MERGE GATE (if `docs/**` touched) | `validate-docs.yml` -> shirabe reusable `validate-docs.yml` |
| Updating `AGENTS.md` / Cursor rules | N/A -- see Surprises | No `AGENTS.md` currently exists in this repo (see section 5) |

### 5. Documentation surfaces PR #109 touched (precedent)

Verified via `git show --stat 517ee83` (commit `517ee8301c8c4d8fef95e40f60d0ec173eee84eb`, "feat(cli): redesign koto next output contract (#109)", merged 2026-03-30). Full changed-file list, 23 files, 2132 insertions / 303 deletions. Non-source files (excluding `.rs` and `.feature` test files):

- `docs/designs/current/DESIGN-koto-next-output-contract.md` (new, 346 lines)
- `docs/plans/done/PLAN-koto-next-output-contract.md` (new, 195 lines)
- `docs/prds/PRD-koto-next-output-contract.md` (new, 230 lines)
- `docs/guides/cli-usage.md` (131 lines changed)
- `plugins/koto-skills/.cursor/rules/koto.mdc` (140 lines changed)
- `plugins/koto-skills/AGENTS.md` (195 lines changed)
- `plugins/koto-skills/skills/koto-author/SKILL.md` (11 lines changed)
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md` (4 lines changed)
- `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md` (4 lines changed)
- `plugins/koto-skills/skills/koto-author/references/template-format.md` (48 lines changed -- this is exactly the `<!-- details -->` marker section documented in section 2 above)
- `Cargo.lock` (dependency lockfile, 2 lines -- incidental, not documentation)

This list is the precedent for what a follow-up touching the same `details`/`--full` contract should touch: a DESIGN/PLAN/PRD doc trio under `docs/`, `docs/guides/cli-usage.md`, and the koto-author skill's SKILL.md + template-format.md. Notably PR #109 did **not** touch `koto-user`'s SKILL.md or references -- because at that time `koto-user` did not yet exist as a skill (it was added later in PR #126, "feat(koto-skills): add koto-user skill and update koto-author"). Since koto-user now exists and documents the same contract (per section 2), a follow-up to this feature has a strictly larger documentation surface than PR #109's precedent: it must also touch `koto-user`'s `response-shapes.md` and `command-reference.md`.

**Verification of the earlier agent's two claims, both confirmed:**

1. **No `AGENTS.md` exists in this repo today.** `find . -iname "AGENTS.md" -not -path "*/target/*"` returns nothing, and `ls plugins/koto-skills/` shows only `hooks/`, `hooks.json`, `skills/` -- no `AGENTS.md`. Git history shows `plugins/koto-skills/AGENTS.md` was touched by four commits (`34a4ec4`/`35a4ec4` add, `c28f4e4`, `517ee83`, `a15ed58`) and `git log --all --diff-filter=D -- plugins/koto-skills/AGENTS.md` shows it was deleted in `a15ed58` ("feat(koto-skills): add koto-user skill and update koto-author (#126)") -- i.e., the file was retired when koto-user superseded it. So PR #109's `AGENTS.md` update is dead precedent; a follow-up must NOT try to update `plugins/koto-skills/AGENTS.md` since it no longer exists.
2. **The Cursor rules file lives at `plugins/koto-skills/.cursor/rules/koto.mdc`, not at repo root.** Confirmed: `find` for `.mdc` files (excluding `target/`) returns exactly `plugins/koto-skills/.cursor/rules/koto.mdc`, and this file still exists today (it was not deleted alongside `AGENTS.md`). This file is a live documentation surface that PR #109 touched (140 lines) and that a follow-up should check for drift, even though it isn't named anywhere in koto's CLAUDE.md skill-maintenance rule -- that rule only names the two `plugins/koto-skills/skills/*` skills, not `.cursor/rules/koto.mdc`. This is a gap in the CLAUDE.md contract worth flagging (see Surprises).

Also worth noting: `docs/guides/cli-usage.md` is referenced repeatedly from koto-user's SKILL.md as the fallback "for topics not covered here" (e.g., SKILL.md:439, command-reference.md:786, response-shapes.md:542) -- it's a fourth documentation surface (beyond the two skills) that PR #109 updated and that isn't mentioned anywhere in koto's CLAUDE.md skill-maintenance rule either.

### 6. Tests -- existing coverage for details/visit-count behavior

**Unit tests (`derive_visit_counts`):** `src/engine/persistence.rs:2224-2430` (approximately; the actual test module block runs from `derive_visit_counts_empty_events` through `derive_visit_counts_ignores_non_entry_events`, six tests total):
- `derive_visit_counts_empty_events` -- empty event list -> empty map.
- `derive_visit_counts_single_transitioned` -- one `Transitioned` event -> count 1 for that state.
- `derive_visit_counts_multiple_visits_same_state` -- `Transitioned` -> `Transitioned` -> `Rewound` back to the first state; asserts `gather` count is 2, `analyze` count is 1 (rewind counts as a re-entry into the target).
- `derive_visit_counts_directed_transition` -- a `DirectedTransition` (i.e. `koto next --to <state>`) counts as an entry too.
- `derive_visit_counts_rewound` -- a bare `Rewound` event alone counts as an entry into its target.
- `derive_visit_counts_mixed_event_types` -- mixes `WorkflowInitialized`, `Transitioned`, `EvidenceSubmitted`, `DirectedTransition`, `DecisionRecorded`, `Rewound`; asserts only `Transitioned`/`DirectedTransition`/`Rewound` count as entries (2 keys total, `gather`=2, `analyze`=1).
- `derive_visit_counts_ignores_non_entry_events` -- confirms `EvidenceSubmitted`, `DecisionRecorded`, `IntegrationInvoked`, `DefaultActionExecuted`, `WorkflowCancelled` never register as a "visit."

The actual suppression decision (`if full || count <= 1 { Some(details) } else { None }`) lives in `src/cli/mod.rs:4001-4015`, inside `handle_next`'s success path, immediately after re-reading events post-advancement-loop. This is the exact code the incoming change would modify. I found **no dedicated unit test for this specific `if full || count <= 1` branch** in `src/cli/mod.rs` itself -- the `derive_visit_counts` tests validate the counting primitive but not the suppression decision that consumes it.

**Serialization-level tests:** `src/cli/next_types.rs` has unit tests asserting `details` presence/absence in the serialized JSON, but they construct `NextResponse` variants directly with `details: None` or `details: Some(...)` already decided (e.g. `serialize_evidence_required_no_options` at line ~897, `serialize_evidence_required_...` at line ~830) -- i.e. they test that the field serializes correctly (omitted when `None`, present when `Some`) but not the visit-count/`--full` decision logic that produces the `Option`.

**Integration/functional tests:** A grep across `tests/integration_test.rs`, `tests/request_cli.rs`, `tests/error_envelope_schema_test.rs` for `"details"`, `--full`, `first_visit`, `visit_count` found matches only for the unrelated `error.details` (per-field validation-error array, a completely different `details` key nested under `error`, not the top-level response `details` field). **No integration test exercises the top-level `details` field's first-visit/`--full` behavior end-to-end** (i.e., no test does: init workflow -> `koto next` (expect details present) -> `koto next` again (expect details absent) -> `koto next --full` (expect details present again)).

`test/functional/features/*.feature` (Gherkin) -- searched but the `find | xargs grep` invocation errored (exit 123, likely `find` returning nothing before piping); worth a direct look, but based on PR #109's diff stat, only `gate-with-evidence-fallback.feature` and `workflow-lifecycle.feature` were touched by that PR (2 lines each), suggesting no dedicated Gherkin scenario for the details-suppression contract exists there either -- it was a minor edit, not scenario coverage.

**`koto-stability-tests/`** -- not searched in depth, but per the CI job's own comment (`validate.yml:43-51`), this crate only compile-checks the frozen `koto::engine::types::*` surface and the 4 frozen `SessionBackend` methods; `details`/`--full` behavior is not part of that frozen surface (it's plumbed through `handle_next` in `src/cli/mod.rs`, not through `engine::types`), so this test suite is not a relevant coverage surface for this change.

**Conclusion for the PRD:** the suppression decision itself (`if full || count <= 1`) has zero direct test coverage today -- only its input primitive (`derive_visit_counts`) and its output serialization (`Option<String>` -> present/absent JSON key) are separately tested. If the PRD requires "cover the new behavior at the same level as today," the honest baseline to match is: unit-test the counting primitive, unit-test the serialization shape, and there is no existing integration/functional test of the end-to-end suppression sequence to point to as precedent -- a new integration test would be new coverage, not parity.

### 7. CHANGELOG.md convention

Head of `CHANGELOG.md` (lines 1-60 read): follows Keep a Changelog format, with the project's pre-1.0 versioning treating MINOR as MAJOR per Cargo 0.x semver convention. The most recent entry, `## [0.10.0] - 2026-05-24`, uses a structure of: a `###`-level subsection per major theme (e.g., "### Request-store substrate + first stability lockdown"), prose paragraphs (not terse bullet-only Keep-a-Changelog style) explaining what shipped and why, and a distinctly called-out `#### Operator-facing behavior change -- auto-cleanup removed (load-bearing)` subsection for a breaking behavior change, explicitly bolded as "(load-bearing)" and spelling out the required operator action ("Operators upgrading from 0.9.x should add `koto workspace prune`..."). It also has a `#### Downstream consumer contract` subsection for API-surface-freeze announcements.

This establishes two conventions relevant to a PRD for this feature: (a) CLI additions get a normal prose paragraph under the version heading, not a terse "Added:" bullet; (b) a **behavior change** (e.g., altering when `details` is suppressed, which is exactly what's proposed) gets its own explicitly-flagged subsection if it's the kind of thing that would surprise an existing consumer -- the "(load-bearing)" convention signals "read this before upgrading." Whether the inline-phase-details change rises to that level depends on whether it changes the *default* suppression behavior (load-bearing precedent applies) versus purely adds a new opt-in retrieval command (ordinary "Added" treatment, more like a normal feature entry).

## Implications

- The PRD must explicitly separate "must happen for the PR to be mergeable" from "must happen because it's the right thing to do" -- CI genuinely only gates: fmt, clippy, unit/integration tests passing, stability-tests (only if the frozen surface is touched, unlikely here), wip/ hygiene, eval-*existence* (not content or passing), PR body mechanics, and (conditionally) template compilation and doc-lifecycle/doc-format if `docs/**` is touched. The skill-content-drift assessment, the eval-results-in-PR-description table, running the evals at all, and CHANGELOG.md conventions are all CI-invisible and rely entirely on author/reviewer discipline.
- Because no eval today asserts on `details`/`--full` behavior, and `check-evals-exist.sh` only counts evals (not relevance), a PR that changes this contract *and adds zero new eval scenarios* would still pass `eval-plugins.yml` cleanly. The PRD should make explicit that a new/extended eval covering the changed behavior is expected -- CI will not catch its absence.
- Since the suppression decision itself (`src/cli/mod.rs:4001-4015`) has no direct unit or integration test today, "match existing test coverage" is a low bar; the PRD should decide explicitly whether to require new coverage (recommended, since this is precisely the logic being changed) rather than assume parity with an untested baseline.
- The documentation surface for this change is now larger than PR #109's own precedent, because koto-user didn't exist yet when PR #109 shipped. A follow-up must touch: koto-author's `SKILL.md:67` + `template-format.md:92-124`, koto-user's `response-shapes.md` (5+ locations) + `command-reference.md:96` + the subcommand table if a new command is added, `docs/guides/cli-usage.md`, and likely `plugins/koto-skills/.cursor/rules/koto.mdc` (live file, touched by #109, not tracked by CLAUDE.md's own skill-maintenance rule -- a coverage gap in the contract itself, see Surprises).
- `plugins/koto-skills/AGENTS.md` no longer exists (deleted in #126) -- any plan or checklist that mechanically repeats PR #109's file list as a template must drop that line item or it will silently reference a dead path.
- CHANGELOG.md needs an entry; whether it's a plain "Added" style entry or a flagged "(load-bearing)" behavior-change subsection depends on whether the PRD's chosen design changes default `koto next` output shape for existing consumers (load-bearing) versus is purely additive (a new opt-in command/flag, ordinary entry).

## Surprises

- **`.cursor/rules/koto.mdc` is a real, currently-existing documentation surface that CLAUDE.md's own skill-maintenance rule doesn't name.** The rule at `CLAUDE.md:49-105` only talks about the two `plugins/koto-skills/skills/*` skill directories. But PR #109 updated `koto.mdc` (140 lines) in the same commit as the skills, and the file is still live today. There's no CI check ensuring `.mdc` stays in sync with source changes the way `check-evals-exist.sh` checks skill eval presence. This is a real gap between "what the contract says is mandatory" and "what actually needs to stay in sync" -- the PRD should probably treat `koto.mdc` as in-scope even though CLAUDE.md doesn't say so.
- **`docs/guides/cli-usage.md` is in the same boat** -- referenced from all three koto-user reference files as the canonical fallback for anything not covered, updated by PR #109, but not named in CLAUDE.md's skill-maintenance table either.
- **Codecov's patch/project targets are not a workflow-level gate in this repo at all** -- `fail_ci_if_error: false` means even a failed upload doesn't fail the `coverage` job, and nothing in `validate.yml`'s aggregator job (`validate:` at the bottom, lines 187-205) references the coverage job's result. If coverage is a merge gate in practice, it's entirely via GitHub branch-protection required-status-checks pointing at Codecov's own PR check, which lives outside this repo's files and outside this research's visibility.
- **`AGENTS.md` at `plugins/koto-skills/AGENTS.md` was deleted, not just superseded conceptually** -- confirmed via `git log --diff-filter=D`. It's fully gone, not merely stale.
- **`check-template-freshness.yml` isn't actually wired up as a direct PR gate in this repo** -- it only has a `workflow_call:` trigger (reusable-workflow entrypoint), and no other workflow file in this repo calls it. It exists for *consumers* of koto (other repos using koto to check their own template-derived Mermaid/HTML freshness), not for koto's own PRs.

## Open Questions

- Does the PRD's chosen design change the *default* behavior of existing `koto next` calls (load-bearing CHANGELOG treatment, likely needs the explicit "(load-bearing)" callout precedent) or is it purely additive (new command/flag, ordinary changelog entry)? This can't be answered from the repo alone -- it depends on the design this PRD lands on.
- Should the PRD formally extend CLAUDE.md's skill-maintenance table to also name `.cursor/rules/koto.mdc` and `docs/guides/cli-usage.md` as mandatory-assessment targets, given they're demonstrably live, referenced, and were touched by the precedent PR but aren't currently covered by the written rule? That's a process fix arguably worth flagging back to the team even if out of scope for this specific feature.
- Is a Gherkin functional-test scenario expected for the new retrieval command, given `test/functional/features/` exists as a distinct coverage tier from `tests/integration_test.rs` and PR #109 touched two `.feature` files trivially (2 lines each) rather than adding a dedicated scenario?

## Summary

koto's CLAUDE.md makes skill-drift assessment and the eval-results PR table *conventions*, not CI-enforced gates -- the only eval-related merge gate is `check-evals-exist.sh` via `eval-plugins.yml`, which checks that each skill has >=1 eval, not that any eval covers `details`/`--full`, and no workflow runs the evals or validates the PR-description table. The real merge gates for this change are fmt, clippy, unit/integration tests, wip/-hygiene, PR-body mechanics, and (conditionally) template-compilation and doc-lifecycle/format checks if `plugins/**` or `docs/**` are touched; Codecov's coverage targets are not enforced by any workflow file in this repo. Both `koto-user` (response-shapes.md, command-reference.md) and `koto-author` (SKILL.md, template-format.md) currently document the exact `details`/visit-count/`--full` contract this change will alter, PR #109's own precedent for documentation surfaces is now incomplete because `koto-user` didn't exist when it shipped and `plugins/koto-skills/AGENTS.md` has since been deleted, and the suppression logic itself (`src/cli/mod.rs:4001-4015`) has no direct test coverage today beyond its `derive_visit_counts` primitive and JSON-serialization shape.
