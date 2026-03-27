# Test Plan: Visual Workflow Preview

Generated from: docs/plans/PLAN-visual-workflow-preview.md
Issues covered: 7

---

## Scenario 1: Default format is mermaid when --format omitted
**ID**: scenario-1
**Category**: Infrastructure
**Testable after**: Issue 1
**Commands**:
- `koto template export test-template.md`
**Expected**: Exit code 0. Stdout contains `stateDiagram-v2`. No file written to disk.
**Status**: pending

## Scenario 2: Mermaid output to stdout for .md source
**ID**: scenario-2
**Category**: Infrastructure
**Testable after**: Issue 1
**Commands**:
- Write `multi-state.md` fixture template to temp dir
- `koto template export multi-state.md`
**Expected**: Exit code 0. Stdout contains `stateDiagram-v2`, state names (`entry`, `setup`, `work`, `done`), and `[*]` markers for initial and terminal states.
**Status**: pending

## Scenario 3: Mermaid output written to file with --output
**ID**: scenario-3
**Category**: Infrastructure
**Testable after**: Issue 1
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md`
**Expected**: Exit code 0. File `workflow.mermaid.md` exists and contains valid `stateDiagram-v2` syntax. Stdout prints the output path.
**Status**: pending

## Scenario 4: Mermaid output includes gate annotations
**ID**: scenario-4
**Category**: Use-case
**Testable after**: Issue 1, Issue 2
**Commands**:
- Write `simple-gates.md` fixture (has `check_file` gate) to temp dir
- `koto template export simple-gates.md`
**Expected**: Exit code 0. Output contains `note left of start : gate: check_file` (or equivalent gate annotation syntax).
**Status**: pending

## Scenario 5: Mermaid output includes transition labels from when conditions
**ID**: scenario-5
**Category**: Use-case
**Testable after**: Issue 1, Issue 2
**Commands**:
- Write `multi-state.md` fixture to temp dir
- `koto template export multi-state.md`
**Expected**: Exit code 0. Output contains transition labels derived from `when` conditions (e.g., `route: setup`, `route: work`).
**Status**: pending

## Scenario 6: Mermaid includes [*] markers for initial and terminal states
**ID**: scenario-6
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 2
**Commands**:
- `koto template export multi-state.md`
**Expected**: Output contains `[*] --> entry` (initial) and `done --> [*]` (terminal).
**Status**: pending

## Scenario 7: Mermaid determinism -- byte-identical across runs
**ID**: scenario-7
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 2
**Commands**:
- `koto template export multi-state.md --output run1.mermaid.md`
- `koto template export multi-state.md --output run2.mermaid.md`
- Compare run1.mermaid.md and run2.mermaid.md byte-for-byte
**Expected**: Files are byte-identical. `diff run1.mermaid.md run2.mermaid.md` produces no output.
**Status**: pending

## Scenario 8: Single-state template produces valid Mermaid
**ID**: scenario-8
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 2
**Commands**:
- Write a single-state terminal template (one state, no transitions) to temp dir
- `koto template export single-state.md`
**Expected**: Exit code 0. Output contains `stateDiagram-v2` and is syntactically valid Mermaid. Contains `[*]` marker for the single initial+terminal state.
**Status**: pending

## Scenario 9: .md and .json inputs produce identical Mermaid output
**ID**: scenario-9
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 2
**Commands**:
- `koto template compile multi-state.md` (capture compiled JSON path)
- `koto template export multi-state.md --output from-md.mermaid.md`
- `koto template export <compiled.json> --output from-json.mermaid.md`
- Compare outputs
**Expected**: `from-md.mermaid.md` and `from-json.mermaid.md` are byte-identical.
**Status**: pending

## Scenario 10: Mermaid output uses LF line endings
**ID**: scenario-10
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 2
**Commands**:
- `koto template export multi-state.md --output check-lf.mermaid.md`
- Check file for CR characters
**Expected**: File contains no `\r` bytes. All line endings are `\n` only.
**Status**: pending

## Scenario 11: --check with fresh mermaid file exits 0
**ID**: scenario-11
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 3
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md`
- `koto template export multi-state.md --output workflow.mermaid.md --check`
**Expected**: Second command exits 0. No output to stdout. File unchanged.
**Status**: pending

## Scenario 12: --check with stale mermaid file exits 1 and prints fix command
**ID**: scenario-12
**Category**: Use-case
**Testable after**: Issue 1, Issue 3
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md`
- Modify `workflow.mermaid.md` (append a line)
- `koto template export multi-state.md --output workflow.mermaid.md --check`
**Expected**: Exit code 1. Stderr contains a fix command that includes `koto template export`. Stderr does not contain JSON.
**Status**: pending

## Scenario 13: --check with missing file exits 1
**ID**: scenario-13
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 3
**Commands**:
- `koto template export multi-state.md --output nonexistent.mermaid.md --check`
**Expected**: Exit code 1. Stderr identifies the missing file. Stderr contains fix command.
**Status**: pending

## Scenario 14: --check fix command resolves drift when executed
**ID**: scenario-14
**Category**: Use-case
**Testable after**: Issue 1, Issue 3
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md`
- Modify `workflow.mermaid.md`
- `koto template export multi-state.md --output workflow.mermaid.md --check` (capture fix command from stderr)
- Execute the captured fix command
- `koto template export multi-state.md --output workflow.mermaid.md --check`
**Expected**: Final --check exits 0. The fix command printed in the stale error actually resolves the drift.
**Status**: pending

## Scenario 15: --format html --output produces self-contained HTML
**ID**: scenario-15
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
**Expected**: Exit code 0. File `workflow.html` exists. Contents include `<html`, Cytoscape.js CDN script tags with `integrity=` attributes, and compiled template data as JSON. Contains no server-side directives (no `<?`, no `<%`, no `{{`-style server templates).
**Status**: pending

## Scenario 16: HTML contains SRI integrity hashes on all CDN script tags
**ID**: scenario-16
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
- Parse HTML for `<script src=` tags
**Expected**: Every `<script src=` tag that references a CDN URL contains an `integrity="sha384-` (or sha256/sha512) attribute and `crossorigin="anonymous"`.
**Status**: pending

## Scenario 17: HTML dark mode media query present
**ID**: scenario-17
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
**Expected**: HTML file contains `prefers-color-scheme: dark` in a media query.
**Status**: pending

## Scenario 18: HTML contains [*] start marker connected to initial state
**ID**: scenario-18
**Category**: Use-case
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
- Inspect graph data JSON embedded in HTML
**Expected**: The embedded graph data includes a start marker node and an edge connecting it to the initial state (`entry`).
**Status**: pending

## Scenario 19: HTML escapes </ as <\/ in injected JSON
**ID**: scenario-19
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4
**Commands**:
- Create a template whose directive text contains `</script>` as a string
- `koto template export malicious-template.md --format html --output workflow.html`
**Expected**: The generated HTML file does not contain a raw `</script>` inside the graph data injection. The sequence is escaped as `<\/script>`.
**Status**: pending

## Scenario 20: HTML determinism -- byte-identical across runs
**ID**: scenario-20
**Category**: Infrastructure
**Testable after**: Issue 4, Issue 5
**Commands**:
- `koto template export multi-state.md --format html --output run1.html`
- `koto template export multi-state.md --format html --output run2.html`
- Compare byte-for-byte
**Expected**: Files are byte-identical.
**Status**: pending

## Scenario 21: HTML file size under 30 KB for a 30-state template
**ID**: scenario-21
**Category**: Infrastructure
**Testable after**: Issue 4, Issue 5
**Commands**:
- Generate or write a 30-state template
- `koto template export large-template.md --format html --output large.html`
- Check file size
**Expected**: `large.html` is under 30,720 bytes (30 KB).
**Status**: pending

## Scenario 22: --format html --check with fresh file exits 0
**ID**: scenario-22
**Category**: Infrastructure
**Testable after**: Issue 3, Issue 4, Issue 5
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
- `koto template export multi-state.md --format html --output workflow.html --check`
**Expected**: Second command exits 0.
**Status**: pending

## Scenario 23: --format html --check with stale file exits 1
**ID**: scenario-23
**Category**: Infrastructure
**Testable after**: Issue 3, Issue 4, Issue 5
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
- Modify `workflow.html`
- `koto template export multi-state.md --format html --output workflow.html --check`
**Expected**: Exit code 1. Stderr contains fix command.
**Status**: pending

## Scenario 24: --format html without --output produces error
**ID**: scenario-24
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4, Issue 7
**Commands**:
- `koto template export multi-state.md --format html`
**Expected**: Non-zero exit code (exit 2). Stderr contains `--format html requires --output`. No file written.
**Status**: pending

## Scenario 25: --open without --format html produces error
**ID**: scenario-25
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4, Issue 7
**Commands**:
- `koto template export multi-state.md --open`
**Expected**: Non-zero exit code (exit 2). Stderr contains `--open is only valid with --format html`.
**Status**: pending

## Scenario 26: --open with --check produces error
**ID**: scenario-26
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 4, Issue 7
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html --open --check`
**Expected**: Non-zero exit code (exit 2). Stderr contains `--open and --check are mutually exclusive`.
**Status**: pending

## Scenario 27: --check without --output produces error
**ID**: scenario-27
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 3, Issue 7
**Commands**:
- `koto template export multi-state.md --check`
**Expected**: Non-zero exit code (exit 2). Stderr contains `--check requires --output`.
**Status**: pending

## Scenario 28: Non-existent input file produces clear error
**ID**: scenario-28
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 7
**Commands**:
- `koto template export nonexistent-file.md`
**Expected**: Exit code 2. Stderr contains an error message referencing the missing file.
**Status**: pending

## Scenario 29: Malformed JSON input produces clear error
**ID**: scenario-29
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 7
**Commands**:
- Write a file `bad.json` with `{not valid json`
- `koto template export bad.json`
**Expected**: Non-zero exit code. Stderr contains an error about invalid JSON.
**Status**: pending

## Scenario 30: Template that fails compilation produces clear error
**ID**: scenario-30
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 7
**Commands**:
- Write a file `broken.md` with invalid YAML frontmatter
- `koto template export broken.md`
**Expected**: Non-zero exit code. Stderr contains a compilation error message.
**Status**: pending

## Scenario 31: 30-state template exports in under 500ms
**ID**: scenario-31
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 7
**Commands**:
- Generate a 30-state template
- Time `koto template export large-template.md --format mermaid`
**Expected**: Command completes in under 500ms wall-clock time.
**Status**: pending

## Scenario 32: --open with --format html launches browser
**ID**: scenario-32
**Category**: Use-case
**Environment**: manual
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html --open`
**Expected**: Default browser opens with the generated HTML file. The graph renders with nodes for each state and edges for transitions. If the browser cannot be opened, the command prints a fallback message with the file path and does not fail.
**Status**: pending

## Scenario 33: HTML renders interactive graph in browser
**ID**: scenario-33
**Category**: Use-case
**Environment**: manual
**Testable after**: Issue 1, Issue 4
**Commands**:
- `koto template export multi-state.md --format html --output workflow.html`
- Open `workflow.html` in a browser
**Expected**: Graph renders with state nodes (entry, setup, work, done). Hovering over `setup` shows tooltip with gate name `config_exists` and command. Clicking a node highlights its direct incoming and outgoing edges. Pan/zoom works. [*] start marker is visible and connected to `entry`. Switching to OS dark mode changes the color scheme.
**Status**: pending

## Scenario 34: Mermaid renders on GitHub
**ID**: scenario-34
**Category**: Use-case
**Environment**: manual
**Testable after**: Issue 1, Issue 2
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md`
- Commit and push `workflow.mermaid.md` to a GitHub repo
- View the file on GitHub
**Expected**: GitHub renders the `stateDiagram-v2` as a visual diagram. States and transitions are visible. Gate notes render (GitHub Mermaid rendering quality may vary).
**Status**: pending

## Scenario 35: GHA reusable workflow detects stale diagrams
**ID**: scenario-35
**Category**: Use-case
**Environment**: manual (requires GitHub Actions runner)
**Testable after**: Issue 3, Issue 6
**Commands**:
- Add the reusable workflow to a test repo with `uses: tsukumogami/koto/.github/workflows/check-template-freshness.yml@v1`
- Configure `template-paths` input with a glob matching template files
- Commit a stale diagram alongside a modified template
- Push and observe CI run
**Expected**: Workflow fails. `::error` annotations appear with fix commands for each stale diagram. The workflow downloads a koto release binary (does not build from source).
**Status**: pending

## Scenario 36: GHA workflow accepts all documented inputs
**ID**: scenario-36
**Category**: Infrastructure
**Environment**: manual (requires GitHub Actions runner)
**Testable after**: Issue 6
**Commands**:
- Configure caller workflow with `template-paths`, `koto-version`, `check-html: true`, `html-output-dir: docs`
- Push and observe CI run
**Expected**: Workflow runs, respects all input values. Uses the specified koto version. Checks HTML freshness in the `docs` directory.
**Status**: pending

## Scenario 37: GHA workflow caller YAML is under 10 lines
**ID**: scenario-37
**Category**: Use-case
**Testable after**: Issue 6
**Commands**:
- Count lines of the minimal caller YAML example
**Expected**: A basic caller workflow that invokes `check-template-freshness.yml` with only `template-paths` requires under 10 lines of YAML (excluding the `on:` trigger block that the caller already has).
**Status**: pending

## Scenario 38: End-to-end author workflow -- create, export, check, modify, check
**ID**: scenario-38
**Category**: Use-case
**Testable after**: Issue 1, Issue 3
**Commands**:
- Write a template with 3 states to temp dir
- `koto template export workflow.md --output workflow.mermaid.md`
- `koto template export workflow.md --output workflow.mermaid.md --check` (should pass)
- Edit the template source (add a new state)
- `koto template export workflow.md --output workflow.mermaid.md --check` (should fail)
- Execute the fix command from stderr
- `koto template export workflow.md --output workflow.mermaid.md --check` (should pass again)
**Expected**: The full cycle works: generate, verify fresh, modify source causing staleness, detect staleness, fix, verify fresh again. This validates the intended author workflow end-to-end.
**Status**: pending

## Scenario 39: Error output goes to stderr as plain text, not JSON
**ID**: scenario-39
**Category**: Infrastructure
**Testable after**: Issue 1, Issue 3
**Commands**:
- `koto template export multi-state.md --output workflow.mermaid.md --check` (with stale file)
**Expected**: Stderr output is plain text (not parseable as JSON). Stdout is empty.
**Status**: pending
