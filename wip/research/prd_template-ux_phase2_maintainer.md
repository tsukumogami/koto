# Phase 2 Research: Maintainer Perspective

## Lead 1: GHA reusable workflow design

### Findings

**Existing CI conventions in koto:**
The repo already has four workflow files in `.github/workflows/`:
- `validate.yml` -- the main gate: unit tests, fmt, clippy, audit, coverage, artifact checks, tsuku distributed install. Uses a fan-in `validate` job with `if: always()` and explicit result checks.
- `validate-plugins.yml` -- triggered only on `plugins/**` path changes. Builds koto from source, compiles all template `.md` files, runs hook smoke tests, validates JSON schemas. Path-scoped triggers keep it fast for non-plugin PRs.
- `eval-plugins.yml` -- prompt regression evals, also path-scoped to `plugins/**`.
- `release.yml` -- tag-triggered cross-compilation and GitHub release.

The existing pattern is clear: each workflow is self-contained, builds koto from source using `dtolnay/rust-toolchain@stable` + `Swatinem/rust-cache@v2`, and uses the built binary for checks. There are no reusable workflows (`workflow_call`) or composite actions today.

**What the template health workflow should enforce:**
1. **Template compilation succeeds** -- already done in `validate-plugins.yml` for the `plugins/` path. A reusable workflow would generalize this for any repo that has koto templates.
2. **Compiled output validates** -- `koto template compile` already validates during compilation (transition targets exist, initial_state declared, evidence routing consistent). A separate `koto template validate` step against the compiled JSON adds defense-in-depth but may be redundant.
3. **Mermaid diagram freshness** -- the new check. Re-generate the diagram, compare with committed version, fail if they differ.
4. **No stale compiled JSON** -- if repos commit compiled JSON alongside source templates, that should also be fresh. Same pattern as Mermaid drift.

**Reusable workflow vs composite action:**
- A **reusable workflow** (`on: workflow_call`) is the right choice. It encapsulates the full job (checkout, install toolchain, build koto, run checks) and callers just add `uses: tsukumogami/koto/.github/workflows/template-health.yml@main`. Composite actions can't define their own `runs-on` and can't use service containers.
- The workflow should live in the koto repo at `.github/workflows/template-health.yml`. GitHub supports cross-repo reusable workflow references for public repos.
- Callers pin to a tag or branch: `@v1`, `@main`, etc.

**Inputs the reusable workflow should accept:**
- `template-paths` (required): glob or directory path to find template `.md` files. Default: `skills/` or similar.
- `koto-version` (optional): which koto release to use. Default: `latest`. This avoids building from source in the caller's repo -- download a release binary instead.
- `check-mermaid` (optional, boolean): whether to enforce Mermaid freshness. Default: `true`.
- `exclude-patterns` (optional): files to skip (e.g., `SKILL.md`).

**Key design choice -- build from source vs download release binary:**
The existing `validate-plugins.yml` builds koto from source because it runs in the koto repo and tests the current branch. A reusable workflow for external consumers should download a pinned release binary instead. Building from source in every consumer repo wastes CI time and creates a Rust toolchain dependency in repos that otherwise don't need one.

**PR integration -- comments vs diff:**
Posting rendered Mermaid as a PR comment is appealing but creates maintenance burden: needs a GitHub token with write permissions, bot comment management (update vs create), and handling for PRs from forks. The simpler approach is sufficient: the committed `.mermaid.md` file renders natively in the PR diff view on GitHub. Reviewers see the old and new diagram side by side in the "Files changed" tab. A PR comment adds value only for repos where the diagram isn't committed (out of scope per the problem statement -- the committed artifact is the goal).

**Patterns from other projects:**
- **buf** (protobuf): `buf lint` and `buf breaking` run in CI against committed `.proto` files. No generated artifact drift -- they lint the source directly.
- **terraform fmt**: CI runs `terraform fmt -check -diff`, fails if formatting differs. Pure source check.
- **sqlc/protoc/go generate**: The standard pattern is "re-generate, `git diff --exit-code`, fail if dirty." This is exactly the right model for Mermaid drift.
- **prettier/eslint**: Same pattern -- `--check` flag exits non-zero if files would change.

The "re-generate and diff" pattern is battle-tested and maps directly to our use case.

### Implications for Requirements

1. The PRD should specify a reusable workflow (not composite action) that lives at `.github/workflows/template-health.yml` in the koto repo.
2. The workflow should download a release binary rather than building from source. This means the workflow depends on koto releases being published. The PRD should note this ordering constraint.
3. The workflow's Mermaid check should use the re-generate-and-diff pattern. The `koto template export` command needs a deterministic output (no timestamps, no random ordering) for diffing to work reliably. The `CompiledTemplate` already uses `BTreeMap` for states (sorted keys), which is good.
4. PR comments with rendered diagrams should be deferred or marked as optional. The diff view of the committed `.mermaid.md` file is the primary review surface.
5. The reusable workflow should have a clear versioning story. Callers pin to `@v1` tags. Breaking changes to inputs bump the major version.

### Open Questions

1. **Should the reusable workflow also check compiled JSON freshness?** If repos commit both source `.md` and compiled `.json`, drift can happen for either. But many repos may not commit compiled JSON at all (they compile at runtime). Should this be an opt-in input?
2. **What's the minimum koto version that supports `template export`?** The workflow needs to know which release to download. Should it default to `latest` or require an explicit version?
3. **Fork PRs**: reusable workflows called from fork PRs may have limited permissions. Should the workflow degrade gracefully (skip comment posting) or is it check-only from the start?

## Lead 2: Mermaid drift detection

### Findings

**The core pattern:**
The standard approach for generated-artifact freshness in CI is:
1. Re-generate the artifact from source
2. Compare with the committed version
3. Fail if they differ

In shell, this looks like:
```bash
# For each template source file
koto template export "$source" --format mermaid --output "$expected_output"
git diff --exit-code "$expected_output"
```

`git diff --exit-code` returns 0 if no changes, 1 if there are changes. It also prints the diff, which gives reviewers a clear error message showing what changed.

**Determinism is critical:**
For diffing to work, `koto template export --format mermaid` must produce byte-identical output for the same input every time. Potential sources of non-determinism:
- **Map iteration order**: The `CompiledTemplate` uses `BTreeMap` for `states`, `gates`, and `variables`, so iteration is sorted. Transitions are `Vec`, preserving insertion order from YAML. This should be deterministic as long as the YAML parser preserves order (serde_yml does).
- **Floating point or timestamps**: None in the current template format. Safe.
- **Trailing whitespace or newlines**: The Mermaid generator must be consistent about trailing newlines. A simple rule: always end with exactly one newline.

**Edge case -- committed compiled JSON missing:**
If a user commits the Mermaid diagram but not the compiled JSON, CI needs to compile from source first, then generate Mermaid from the compiled output. This is the natural flow anyway: `koto template export` takes a source `.md` file, compiles it internally (via `compile_cached`), then generates Mermaid. The compiled JSON is an intermediate artifact, not a prerequisite.

If the user *does* commit compiled JSON and it's stale, CI could catch that too. But this is a separate check (compiled JSON freshness). Mermaid drift detection should work from source templates directly.

**Edge case -- Mermaid file doesn't exist yet:**
First-time CI run after adding a template. The Mermaid file hasn't been committed yet. `git diff --exit-code` would show the new file as untracked, which `git diff` alone won't catch. The check needs to also run `git diff --exit-code --name-only` on tracked files AND check for untracked Mermaid files that should exist but don't. Or simpler: `git status --porcelain` after regeneration and fail if any Mermaid files are modified or untracked.

Recommended approach:
```bash
# Regenerate all diagrams
for source in $(find $TEMPLATE_DIR -name '*.md' ! -name 'SKILL.md'); do
  koto template export "$source" --format mermaid --output "${source%.md}.mermaid.md"
done

# Check for any changes
if ! git diff --exit-code; then
  echo "Mermaid diagrams are out of date. Run 'koto template export' locally."
  exit 1
fi

# Check for untracked mermaid files (new templates without committed diagrams)
UNTRACKED=$(git ls-files --others --exclude-standard '*.mermaid.md')
if [ -n "$UNTRACKED" ]; then
  echo "New Mermaid diagrams need to be committed: $UNTRACKED"
  exit 1
fi
```

**Auto-fix vs fail-only:**
Two approaches:
1. **Fail-only**: CI fails, developer runs export locally, commits. Simple, predictable, no bot commits. This is what terraform, prettier, and buf do.
2. **Auto-fix**: CI regenerates and pushes a commit. Requires write permissions, creates noise in git history, doesn't work for fork PRs, and obscures who introduced the drift.

Recommendation: **fail-only** for v1. Auto-fix can be added later as an opt-in workflow input. The `validate-plugins.yml` workflow in koto already follows the fail-only pattern (e.g., `cargo fmt --check` fails but doesn't auto-format).

**Mermaid output file naming and location:**
The Mermaid artifact needs a predictable path derived from the source template path. Options:
- `<source>.mermaid.md` -- sibling file, same directory. E.g., `skills/work-on/work-on.mermaid.md`.
- `<source-dir>/diagrams/<name>.mermaid.md` -- separate subdirectory.
- A configured output directory.

The simplest is a sibling file with `.mermaid.md` extension replacing `.md`. This keeps diagrams colocated with templates, easy to discover, and the path derivation is trivial. The CI check just needs to know the naming convention.

However, the PRD decisions file notes this is still an open question (D1). The drift detection pattern works regardless of naming -- the CI just needs to know the mapping from source to diagram path.

### Implications for Requirements

1. The PRD must specify that `koto template export --format mermaid` produces deterministic output. This should be a tested property (integration test: compile the same template twice, assert identical output).
2. The drift detection should use `git diff --exit-code` plus untracked file checks, not content hashing. Git diff is standard, gives readable error messages, and handles edge cases (permissions, line endings) correctly.
3. The PRD should specify fail-only behavior for v1, with auto-fix as a future opt-in.
4. The Mermaid file naming convention needs to be decided (D1 in decisions.md) before the CI workflow can be fully specified. The drift check depends on knowing how to map source -> diagram path.
5. The `koto template export` command should support a `--check` flag (like `cargo fmt --check` or `terraform fmt -check`) that exits non-zero if the output would differ from the existing file. This is cleaner than shelling out to `git diff` and works outside of git repos too.

### Open Questions

1. **Should `koto template export` have a built-in `--check` mode?** This would simplify CI scripts (one command instead of export + git diff) and work in non-git contexts. But it adds scope to the export command.
2. **What happens when the source template is invalid?** The CI workflow should report compilation errors clearly, separately from drift errors. The current `validate-plugins.yml` already handles this -- the reusable workflow should follow the same pattern (compile first, export second).
3. **Line ending normalization**: should the Mermaid output always use LF? Windows contributors with CRLF git settings could see false drift. Using `.gitattributes` with `*.mermaid.md text eol=lf` would prevent this.

## Lead 3: CDN maintenance

### Findings

**Current CDN dependencies (from prototype and design doc):**
- cytoscape@3.30.4 (design doc) / @3.33.1 (prototype) -- the production version will be pinned to one.
- dagre@0.8.5 -- last published to npm in 2016. Effectively abandoned but stable.
- cytoscape-dagre@2.5.0 -- last published in 2022. Thin adapter between cytoscape and dagre.

All loaded from unpkg.com with planned SRI hashes per the design doc's security section.

**Release cadence:**
- **Cytoscape.js**: Active project. Releases roughly every 1-3 months. v3.30.4 was recent, v3.33.1 is in the prototype. Minor versions tend to be backward-compatible.
- **dagre**: No releases since 0.8.5 (2016). The project is essentially feature-complete/abandoned. No maintenance concern -- it won't change.
- **cytoscape-dagre**: Last release 2.5.0 in 2022. Infrequent updates. Low maintenance burden.

**Dependabot and CDN deps:**
GitHub Dependabot doesn't natively support CDN URL version tracking. It supports npm, pip, cargo, etc. -- but not arbitrary URLs in HTML files. Options:
- **Renovate** (Mend): supports "regex" managers that can match version patterns in arbitrary files. Could be configured to detect `unpkg.com/cytoscape@3.30.4` and propose updates. However, Renovate requires non-trivial configuration for custom patterns and SRI hash updates.
- **Custom GHA workflow**: a scheduled workflow that checks npm for newer versions and opens a PR. This is what the design doc suggests ("CI could periodically check for new Cytoscape.js releases and open update PRs"). Simple to implement, full control, but another thing to maintain.
- **Manual updates**: given the low release cadence (dagre is frozen, cytoscape-dagre is infrequent), manual updates via PRs are realistic. Cytoscape.js is the only actively-updated dep.

**SRI hash approach:**
SRI (Subresource Integrity) is the right approach for CDN-loaded scripts. It's the standard browser security mechanism for this exact use case. The design doc correctly requires SRI hashes on all three script tags.

Maintenance impact: when bumping a CDN version, the developer must also compute and update the SRI hash. This is a two-step process:
```bash
curl -s https://unpkg.com/cytoscape@3.33.1/dist/cytoscape.min.js | openssl dgst -sha384 -binary | openssl base64 -A
```
Not hard, but easy to forget. A helper script or CI check could enforce that SRI hashes match the referenced versions.

**Vendoring as alternative:**
Vendoring the JS files (committing them to the repo) would eliminate CDN dependency entirely but:
- Adds ~200-300 KB of minified JS to the repo (cytoscape.min.js alone is ~200 KB).
- The generated HTML files would need to reference local paths, breaking the "open HTML file from anywhere" use case.
- OR the JS would be inlined via `include_str!`, inflating every preview file to ~435 KB (the design doc explicitly rejected this).

The CDN approach with SRI is the right trade-off for koto. The generated files are small, work from any location with internet access, and SRI provides integrity guarantees.

**Recommendation for maintenance:**
Given that dagre and cytoscape-dagre are effectively frozen, the only library that needs watching is cytoscape.js. A lightweight scheduled CI workflow (monthly or quarterly) that checks `npm view cytoscape version` and opens an issue if a new major version is available would be sufficient. Full automation (auto-PR with SRI hash updates) is over-engineering for one library with backward-compatible releases.

### Implications for Requirements

1. The PRD should acknowledge that CDN version updates are a manual but infrequent task. Of the three dependencies, only cytoscape.js is actively maintained. The other two are frozen.
2. SRI hashes are the right approach. The PRD should require that `preview.html` includes SRI hashes and that the build/CI verifies they match (a test that fetches the URL and compares the hash, or at minimum a documented update procedure).
3. Vendoring should remain out of scope. The design doc already decided against inlined bundles.
4. A scheduled check for cytoscape.js updates (even just opening an issue, not auto-PRing) would be a nice-to-have but shouldn't block v1.
5. The PRD should document the SRI hash update procedure so future maintainers don't have to figure it out.

### Open Questions

1. **unpkg.com reliability**: unpkg is a popular CDN but has had occasional outages. Should the HTML template include a fallback CDN (e.g., cdnjs, jsdelivr) or is a single CDN acceptable for a developer tool that's used locally?
2. **Should the CI workflow verify SRI hashes are correct?** This would catch cases where someone bumps the version number but forgets to update the hash. It requires network access in CI (to fetch and hash), which some environments restrict.

## Summary

The reusable GHA workflow should follow the existing koto CI conventions (fail-only, clear error messages, fan-in gate job) and use the proven "re-generate and diff" pattern for Mermaid drift detection. It should be a reusable workflow (not composite action), download a release binary rather than building from source, and accept template paths and a koto version as inputs. Deterministic output from `koto template export` is a hard requirement for drift detection to work. CDN dependency maintenance is a non-issue in practice -- dagre and cytoscape-dagre are frozen, and cytoscape.js releases are backward-compatible -- so SRI hashes with a documented update procedure are sufficient.
