---
schema: plan/v1
status: Active
execution_mode: single-pr
upstream: docs/designs/DESIGN-doc-code-drift.md
milestone: "Name resolution for koto's documentation and error strings"
issue_count: 7
tracking_level: none
---

# PLAN: Name resolution for koto's documentation and error strings

## Status

Active

## Scope Summary

Build the check `docs/designs/DESIGN-doc-code-drift.md` settled: a Rust
integration test at `tests/doc_names.rs` that walks `koto::cli::App::command()`
for koto's verb set, scans nine surfaces for anchored code-font candidates, and
resolves command tokens against the walk and path tokens against the filesystem.
Exceptions live in `tests/doc_names.allow`, keyed on `(kind, token)`, with a
category that decides whether an issue reference is required and a witness digest
that binds an intentional record to the text it protects.

The work also repairs what the check finds. Three genuine command defects
(`koto query` at four sites, `koto session info` at two, and the eight dead
design-document and guide paths) are fixed rather than recorded;
`koto session rebind` is recorded as promised against koto#215 because that verb
is another change's to build. Two `CLAUDE.md` tokens the design's path anchor
deliberately cannot reach are fixed by hand.

## Decomposition Strategy

**Walking skeleton, then widening, with the repairs first.**

The repairs lead because they stand on their own: every prose fix in issue 1 is
correct whether or not the check ever lands, and putting them first means the
issue that turns the check on has only mechanism left in it. That ordering was
the design's, and it is what keeps the last issue from being the one that
carries both a new gate and every repair it demands.

The repairs are two issues rather than one because the skill-authoring guide is
a different size of job from the rest: 594 lines built on an example skill that
is not in the tree, named forty times. Folding that into the same issue as a
handful of citation corrections would hide it.

After that the slicing is by claim type and then by machinery, because the two
resolution rules share an extractor and nothing else: command resolution (issue
3) proves the load-bearing mechanism — the clap walk, anchored extraction, the
parent check — against fixtures including the v0.12.0 case; path resolution
(issue 4) reuses the same walk with a different resolver; the allowlist (issue
5) is orthogonal to both and is the only part with its own state format. Issue 6
turns the repository scan on, which is the first moment the check can fail CI,
and issue 7 produces the durable record the PRD's R16 and R20 require.

The grouping rule inside issues 3 through 5 is one issue per resolver or
mechanism, not one per function. Every issue leaves `cargo test` green.

The whole plan is one pull request. koto's Delivery Preference is the default
`consolidated`, and the units here are not independently valuable: issues 3
through 5 add a test that is `#[ignore]`d until issue 6, so shipping any of them
alone would land dead code.

## Issue Outlines

### Issue 1: repair every name the check will report

**Goal**: Fix every genuine defect the check will find, before it exists, so
that the issue turning it on carries mechanism alone.

**Acceptance Criteria**:
- The four `koto query` findings are gone: the two `///` doc comments at
  `src/engine/types.rs:883` and `:1039`, and the two markdown sites in
  `plugins/koto-skills/skills/koto-user/references/command-reference.md` and
  `.../references/batch-workflows.md`. The batch-workflows site is inside a bash
  fence presenting it as runnable; it is replaced with a command that exists or
  the line is removed. Plain `//` occurrences elsewhere in the same file are out
  of scope and may remain.
- `koto session info` no longer appears in `src/engine/respawn.rs`. Both sites
  are in `RESUME_CONTEXT_PROMPT` and its test; `tests/respawn.rs` asserts the
  prompt byte for byte, so that assertion is updated in the same change. The
  same sentence names `koto session list --parent`, a flag that lives on
  `session start`; it is corrected too, though no check covers flags.
- The four dead design-document citations in `src/` resolve:
  `src/engine/batch_validation.rs`, `src/cli/batch_view.rs`, and
  `src/cli/task_spawn_error.rs` name the batch-child-spawning design and
  `src/workflows_surface/contract.rs` names the native-workflows-render design,
  each rooted at `docs/designs/` where the live document is under
  `docs/designs/current/`.
- The four citations of a request-store design document that exists nowhere, in
  `docs/STABILITY.md` and `docs/workspace-layout.md`, are corrected or removed.
- `CLAUDE.md`'s structure block no longer names `cmd/koto/` or a `gate/`
  directory, and its table no longer cites `src/gate/`. The maintenance rule at
  the same file no longer tells contributors to watch `src/` or `cmd/`, since
  `cmd/` does not exist, and the skills table names all three skills rather than
  two.
- `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` are clean.

**Dependencies**: None

**Complexity**: testable

**Type**: fix

**Files**: `src/engine/types.rs`, `src/engine/respawn.rs`,
`src/engine/batch_validation.rs`, `src/cli/batch_view.rs`,
`src/cli/task_spawn_error.rs`, `src/workflows_surface/contract.rs`,
`tests/respawn.rs`, `plugins/koto-skills/skills/koto-user/references/command-reference.md`,
`plugins/koto-skills/skills/koto-user/references/batch-workflows.md`,
`docs/STABILITY.md`, `docs/workspace-layout.md`,
`CLAUDE.md`

---

### Issue 2: rewrite the skill-authoring guide around a skill that exists

**Goal**: `docs/guides/custom-skill-authoring.md` builds its entire walkthrough
on `plugins/koto-skills/skills/hello-koto/`, which is not in the tree. The guide
is 594 lines and names `hello-koto` forty times, so this is a rewrite rather than
a citation fix, and it gets its own issue for that reason.

**Acceptance Criteria**:
- Every repo-relative path the guide cites resolves. Six dead tokens are in
  scope: `plugins/koto-skills/skills/hello-koto/`, `.../hello-koto/SKILL.md`,
  `.../hello-koto/hello-koto.md`, `plugins/koto-skills/eval.sh`,
  `plugins/koto-skills/evals/`, and `plugins/koto-skills/evals/hello-koto/`. The
  last three are the eval-harness citations and are dead independently of the
  skill.
- The walkthrough is anchored on a skill that exists under
  `plugins/koto-skills/skills/`. The DESIGN recommends `koto-adhoc` as the
  smallest. Verify it against the guide's actual structure before committing to
  it: the guide's two-file premise, its Steps 1 through 3, its template-locality
  section, and its worked example each have to survive the substitution or be
  rewritten. If `koto-adhoc` does not fit, `koto-author` is the alternative, and
  the choice is recorded in the commit message.
- The example skill is not authored. Adding one would ship plugin surface no
  requirement asks for, which then needs evals under `check-evals-exist.sh` and a
  row in the hand-maintained skills table in `CLAUDE.md`.
- The eval-harness instructions describe the format that exists. Evals live at
  `plugins/<plugin>/skills/<name>/evals/evals.json`, carrying a `skill_name` and
  an `evals` array of `{id, name, prompt, expected_output, files, assertions}`,
  and are run through `scripts/run-evals.sh <skill-name>`. The guide currently
  describes a per-case directory of `prompt.txt`, `skill_path.txt` and
  `patterns.txt` matched by regex, and a per-plugin `eval.sh`; neither exists.
  The stale `plugin.json` literal in the guide — `"version": "0.1.0"` with
  `"skills": ["./skills/hello-koto", ...]` — is corrected in the same pass.
- `cargo test` is green and `scripts/check-evals-exist.sh` still passes.

**Dependencies**: None

**Complexity**: testable

**Type**: docs

**Files**: `docs/guides/custom-skill-authoring.md`

---

### Issue 3: command resolution against the clap walk

**Goal**: Establish `tests/doc_names.rs` and prove the load-bearing mechanism:
the verb walk, anchored extraction, and the two-structure resolver that makes
`session rebind` a finding rather than a match on `session`.

**Acceptance Criteria**:
- `verbs()` walks `koto::cli::App::command()` recursively into the path set and
  the parent set, including aliases, with no new dependency in `Cargo.toml`.
- The surface list covers exactly the nine surfaces the design names, with the
  extractor chosen by extension per the design's table, `#[cfg(test)]` modules
  skipped, and the default root taken from `CARGO_MANIFEST_DIR`.
- Anchored extraction: a candidate is produced only where `koto` begins an
  inline code span, begins a line inside a fence after an optional `$ ` prompt,
  begins a Rust string literal or a backticked span inside one, or begins a
  backticked span inside a `///` or `//!` comment. `koto` preceded by `-` or
  followed by anything but whitespace or end-of-span is never a candidate, and
  an occurrence yielding no word is not a candidate rather than an empty one.
- Rust string continuations are joined before extraction. A fixture reproduces
  `src/cli/mod.rs`'s wrapped literal and the check finds `session rebind` in it;
  deleting the joining makes that fixture fail.
- The resolver: a token in the path set resolves; a two-word token not in the
  path set resolves only when its first word is in the path set and owns no
  children. Fixtures cover `session rebind` failing, `status my-flow` resolving,
  `workflows publish` resolving, bare `workflows` resolving, and
  `workflows garbage` failing.
- A fixture holds the three v0.12.0 string literals verbatim and asserts all
  three are found. Removing the detection fails it.
- The root resolves from `KOTO_DOC_NAMES_ROOT` when set and `CARGO_MANIFEST_DIR`
  otherwise. Under an alternate root the allowlist is read from that root when
  present and treated as empty when absent.
- A fixture whose supplied verb set carries a verb the invoking repository
  lacks, with a document in that fixture naming it, produces no finding;
  inverted — the invoking repository has the verb and the supplied set does not —
  it produces one. An implementation resolving against the invoking repository's
  own verb set rather than the supplied one fails both halves.
- Words following `koto` are kept while they match `^[a-z][a-z0-9-]*$`, stopping
  at the first that does not and at two. `koto workflows <action>` produces no
  finding.
- A backticked span nested inside a fence anchors, with a fixture reproducing
  `docs/reference/error-codes.md`'s shape: a backticked span inside a JSON string
  inside a ```json fence.
- The same fenced command produces a finding whether the fence carries a `bash`
  tag or none.
- A planted unresolvable token produces a finding in each of the nine checked
  surfaces and no finding in `docs/designs/`, `docs/prds/`, `docs/briefs/`,
  `CHANGELOG.md`, `tests/`, `test/`, `benches/`, `scripts/`, or `.github/`.
- `scan(root, verbs, allow)` is a pure function that fixtures call with their own
  verb set; the repository scan is present but `#[ignore]`d.
- `cargo test` is green.

**Dependencies**: None

**Complexity**: critical

**Type**: feat

**Files**: `tests/doc_names.rs`, `tests/fixtures/doc_names/`

---

### Issue 4: path resolution

**Goal**: Add the second resolver against the same extractor.

**Acceptance Criteria**:
- A path candidate is rejected when it carries `<`, `{{`, or `*`, or when its
  leading segment is `.claude` or `target`; `.github` is not excluded.
- A trailing `:<line>`, `:<start>-<end>`, or `::<symbol>` suffix is stripped
  before resolution. That a `path` record matches after stripping is issue 5's
  criterion, since it is a property of the allowlist rather than of the
  resolver.
- Candidacy requires the leading segment to be a member of `read_dir(root)`.
  Fixtures assert that `cmd/koto/` produces no finding, recording the accepted
  false negative in code.
- Lexical normalization rejects any token whose segments include `..` and any
  absolute token, before joining against the root. `fs::canonicalize` is not
  used, since it fails on the non-existent paths the check exists to find.
- `symlink_metadata` rather than `metadata`, so a dangling symlink reports as
  present.
- Fixtures cover: a path whose leading segment exists and whose full path does
  not, producing a finding; the same path with `:120` and `:120-140` producing
  the same finding; and `feature/anchor`, `CI/CD`,
  `path/to/your-template.md`, `~/.koto/sessions`, `gates.*`, a URL, and a bare
  tree child producing none.
- `cargo test` is green; the repository scan is still `#[ignore]`d.

**Dependencies**: Issue 3

**Complexity**: critical

**Type**: feat

**Files**: `tests/doc_names.rs`, `tests/fixtures/doc_names/`

---

### Issue 5: the allowlist and its five staleness shapes

**Goal**: Add the exception mechanism, which is the only part of the check with
its own state format and the only part that can rot.

**Acceptance Criteria**:
- `tests/doc_names.allow` parses with `splitn(6, '\t')`; `#` and blank lines are
  skipped; a missing or empty file yields no records.
- Each of these is an error naming the offending line: fewer than six fields, a
  `kind` outside `{command, path}`, a `category` outside
  `{promised, intentional}`, a promised record whose issue is `-` or is not
  `owner/repo#N`, an intentional record carrying an issue, an empty reason, a
  duplicate `(kind, token)`, and a witness that is neither `-` on a promised
  record nor `?` or eight hex characters on an intentional one.
- A `path` record's token matches after suffix stripping, so one record covers
  the bare path and its `:line` and `:start-end` forms.
- The member key is `(root-relative file, ordinal of this token's occurrences
  within that file)`. The witness is sha256 over the `\n`-joined sorted
  `<file>\t<ordinal>\t<normalized-span>`, hex, first eight characters. The
  normalized span is the innermost enclosing code span.
- Five message shapes, assigned per category: a promised record reaches shape 1
  only; an intentional record reaches 2 through 5. Additions and removals are
  evaluated independently so a span moving between files produces shapes 4 and 5
  together.
- Shape 3's message asks the author to confirm every remaining site still
  belongs under the record rather than asserting nothing is wrong.
- A `?` witness reports the digest the record should carry and fails, so adding
  an intentional record is a deliberate two-pass operation.
- A stale-record finding prints the record's reason text verbatim.
- Fixtures cover: a well-formed promised record suppressing a finding and its
  deletion restoring it; a well-formed intentional record accepted with no issue
  reference; each parse error; a promised record whose token now resolves
  reported as stale; an intentional record whose protected text changed reported
  as needing re-affirmation; and the set-grew and set-shrank cases.
- `cargo test` is green; the repository scan is still `#[ignore]`d.

**Dependencies**: Issue 3

**Complexity**: critical

**Type**: feat

**Files**: `tests/doc_names.rs`, `tests/doc_names.allow`,
`tests/fixtures/doc_names/`

---

### Issue 6: turn the repository scan on

**Goal**: Remove the `#[ignore]` and make the check green against koto's own
tree, which is the first moment it can fail CI.

**Acceptance Criteria**:
- With `tests/doc_names.allow` emptied, the repository scan produces a
  `(file, token)` set that is captured here and reproduced on a second run; with
  the five records in place it exits zero. Stating both halves is what makes a
  scanner that has silently stopped finding things fail this criterion — "exits
  zero" alone is satisfied by a scan that finds nothing. Issue 7 commits that set
  as R16's artifact and re-verifies it against the final tree.
- The scan exits non-zero when it has findings, so CI routes on the status
  without parsing output.
- `tests/doc_names.allow` carries exactly five records: one promised for
  `session rebind` against `tsukumogami/koto#215`, whose reason names the four
  prose passages stating the verb does not exist; one intentional for `migrate`;
  and three intentional path records for the tokens that survive R3a-i. Every
  promised record names a filed issue and every record carries a reason.
- Findings accumulate and are reported together through `panic!` with the report
  as its message, so the whole repair is visible without `--nocapture`.
- Every finding names the token, the file, the line, how to fix it, and how to
  record it as an exception. Every site is printed rather than elided behind a
  count. The reported line is the physical line carrying the phrase, not the
  line a joined literal opens on.
- No workflow, job, or `paths:` filter is added anywhere. The check rides
  `validate.yml`'s existing unfiltered `cargo test` job, which is what puts a
  `src/`-only change in scope. This is a negative decision with nothing in a diff
  to review against, so it is stated as a criterion.
- `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` are clean,
  and the `koto-stability-tests` crate's tests pass.

**Dependencies**: Issue 1, Issue 2, Issue 4, Issue 5

**Complexity**: critical

**Type**: feat

**Files**: `tests/doc_names.rs`, `tests/doc_names.allow`

---

### Issue 7: the v0.12.0 demonstration and the finding record

**Goal**: Produce the durable evidence the PRD requires, so that the acceptance
bar is demonstrated rather than asserted — without the evidence itself becoming
a finding.

**Acceptance Criteria**:
- The v0.12.0 run is executed — a detached `git worktree add` of the tag, then
  `KOTO_DOC_NAMES_ROOT=<path> cargo test --test doc_names -- --nocapture` — and
  its output committed. The output shows the three `koto session rebind` string
  literals at `src/cli/mod.rs` and `src/cli/next_types.rs`, and the untagged
  fence in `docs/guides/default-action-authoring.md`.
- The committed record states that the root argument carries the corpus and not
  the verb set, so the run resolves v0.12.0's prose against today's command
  surface, and why that is sound here.
- R16's pre-exception list is committed as a sorted set of
  `(file, token, classification)` without line numbers, captured against the
  branch tree with the records removed, and genuine entries outnumber
  correct-as-written entries.
- The check's contribution to `cargo test` is measured and is at most ten
  seconds on a warm build.
- The evidence lives at `tests/doc_names_evidence.md`, not under `docs/`.
  `docs/testing/` is one of R5's nine checked surfaces, and the evidence quotes
  every token the check just reported — `session rebind`, `query`,
  `session info`, and the dead path tokens. Written into a checked file those
  become live findings with no records behind them, and the last issue in the
  plan would turn red the gate the issue before it turned green. `tests/` is
  outside R5, which is what makes it the right home.
- `tests/doc_names.allow` is unchanged by this issue except to confirm its five
  records are intact; the Files entry exists so a reviewer knows the file was
  looked at, not edited.
- `CLAUDE.md` names the check, so a contributor who has never seen it can find
  it and knows to run `cargo test --test doc_names`.
- `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` are clean,
  and the repository scan still exits zero after this issue's edits — including
  the edit to `CLAUDE.md`, which is itself a checked surface.

**Dependencies**: Issue 6

**Complexity**: simple

**Type**: docs

**Files**: `tests/doc_names_evidence.md`, `tests/doc_names.allow`, `CLAUDE.md`

---

## Implementation Sequence

**Batch 1 — three independent starts.** Issues 1, 2 and 3 have no dependencies
and no file overlap: issue 1 touches source strings and short documents, issue 2
rewrites one guide, issue 3 adds a new test file and its fixtures.

Start with issue 3, and the reason is worth stating because it appears to
contradict the Decomposition Strategy's "repairs lead". The repairs lead in the
*dependency* order — issue 6 cannot go green until they land — but issue 3 is the
one that can invalidate the plan. Its acceptance criteria encode measurements
taken from a prototype; if the clap walk or the anchoring rule does not behave as
the design measured, issues 4 through 7 change shape and so does the list of
repairs issue 1 owes. Doing issue 3 first is what makes the repairs a known
quantity rather than a guess.

**Batch 2 — the two resolvers' remainder.** Issues 4 and 5 both depend on issue
3 and on nothing else. They can be worked in either order, but not concurrently:
both edit `tests/doc_names.rs`, so a single implementer takes them sequentially
and a second implementer would conflict. Issue 5 is the larger and the one with a
state format to get right; issue 4 is mostly filters over an extractor that
already exists.

**Batch 3 — the gate goes live.** Issue 6 needs 1, 2, 4 and 5. It is the first
commit where a mistake anywhere upstream shows up as a red `cargo test`, which is
the point: everything before it is inert.

**Batch 4 — the record.** Issue 7 is evidence. It is last because the
demonstration it captures is of the check as it finally shipped, and its own
output has to be placed somewhere the check does not read.

## References

- `docs/designs/DESIGN-doc-code-drift.md` — the approach, the alternatives, and
  the measurements the acceptance criteria above restate as conditions.
- `docs/prds/PRD-doc-code-drift.md` — the requirements each criterion discharges.
- `tests/lib_reexports.rs` and `tests/integration_test.rs` — the two in-tree
  precedents for a `tests/` drift guard and for resolving a root from
  `CARGO_MANIFEST_DIR`.
