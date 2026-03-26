---
status: Draft
problem: |
  Koto workflow templates compile to JSON state graphs that are hard to review
  visually. Template authors debug stuck workflows by reading raw JSON. PR reviewers
  see YAML diffs without understanding the graph shape they represent. There's no CI
  enforcement that visual documentation stays in sync with source templates.
goals: |
  Template authors can visually inspect and debug workflow structure. PR reviewers see
  rendered diagrams in diffs. CI enforces that committed diagrams match their source
  templates. Each capability fits naturally into an existing workflow rather than
  requiring new habits.
source_issue: 86
---

# PRD: Template visual tooling

## Status

Draft

## Problem statement

Koto workflow templates are markdown files with YAML front-matter that compile
to directed state graphs. As templates grow past 5-10 states with branching
transitions, gate conditions, and evidence schemas, reviewing the structure
from raw JSON or YAML becomes tedious and error-prone.

Four groups of people feel this pain differently:

**Template authors** iterate on workflow design by compiling templates and
testing them with `koto init` / `koto next`. When a workflow gets stuck or
takes an unexpected path, they trace transitions by reading compiled JSON.
For small templates this works. For templates with 15+ states, conditional
branching, and gated transitions, it's slow and easy to miss structural
issues like unreachable states or dead ends.

**PR reviewers** see template changes as YAML diffs. A reviewer can tell
that transitions were added or gate conditions changed, but can't quickly
assess the structural impact. Did this change create a dead end? Is the
new state reachable? These questions require mentally reconstructing the
graph from YAML, which is exactly the kind of work a tool should do.

**Documentation readers** want to quickly understand the workflow a template
enforces. This plays out in two different contexts. Someone browsing the
source on GitHub sees the raw template markdown and needs a diagram that
renders natively in that medium — a committed Mermaid file handles this.
Someone visiting a project website (GitHub Pages or similar) expects a
richer experience: an interactive diagram they can click through, with
tooltips and pan/zoom — a static Mermaid block won't cut it there. These
are different mediums with different expectations.

**Repo maintainers** have no way to enforce that visual documentation stays
current. If a team commits workflow diagrams alongside templates, those
diagrams drift as templates evolve. There's no CI check for freshness,
so stale diagrams become misleading rather than helpful.

## Goals

- Template authors can generate an interactive visual representation of any
  compiled workflow for local debugging and inspection
- PR reviewers see a rendered diagram in the PR diff that updates automatically
  when the source template changes
- Repo maintainers can add a CI check that fails when committed diagrams
  are out of sync with their source templates
- Documentation readers browsing source on GitHub see a natively rendered
  Mermaid diagram alongside the template
- Documentation readers on a project website see an interactive diagram
  with the same fidelity as the local preview
- The visual tooling fits into existing workflows (compile -> inspect -> commit -> review)
  without requiring extra manual steps that people forget

## User stories

**As a template author debugging a stuck workflow**, I want to open an
interactive diagram of the compiled template in my browser, so that I can
visually trace transition paths, inspect gate conditions via tooltips, and
spot structural issues without reading JSON.

**As a template author about to commit changes**, I want to generate a
text-based diagram that I commit alongside my template, so that reviewers
can see the workflow structure without running local tools.

**As a PR reviewer**, I want to see a rendered state diagram in the PR's
file diff, so that I can assess the structural impact of template changes
(new states, changed transitions, removed paths) at a glance.

**As someone browsing template source on GitHub**, I want to see a rendered
state diagram alongside the template file, so that I can understand the
workflow structure without cloning the repo or running tools.

**As someone reading template documentation on a project website**, I want
to see an interactive diagram with hover tooltips and click-to-highlight,
so that I can explore the workflow in detail without installing anything.

**As a repo maintainer**, I want a CI check that fails when committed
diagrams don't match the current template source, so that stale diagrams
can't be merged.

**As a repo maintainer setting up CI**, I want a reusable GitHub Actions
workflow I can add to my repo with minimal configuration, so that I don't
have to write template validation logic from scratch.

## Requirements

### Functional

**R1. Single export command with format selection.** `koto template export`
is the single command for all visual output. A `--format` flag selects
between `mermaid` (default) and `html`. An `--output` flag writes to a
file. For `mermaid`, output goes to stdout when `--output` is omitted.
For `html`, `--output` is required (HTML to stdout isn't useful).

**R2. Mermaid format.** `--format mermaid` produces a `stateDiagram-v2`
text representation. The diagram shows states, transitions with condition
labels, gate annotations as notes, and `[*]` markers for initial and
terminal states.

**R3. HTML format.** `--format html` produces a self-contained HTML file
with an interactive directed graph: state nodes (color-coded by type),
labeled transition edges, hover tooltips showing gate conditions and
evidence schemas, click-to-highlight for tracing paths, and pan/zoom for
large graphs. The HTML loads Cytoscape.js and dagre from CDN. The generated
file works both as a local debugging tool and as static documentation
served on project websites (GitHub Pages or similar).

**R4. Browser open flag.** `--open` launches the generated file in the
default browser. Only valid with `--format html`. This is a convenience
for local debugging; deploy pipelines omit it.

**R5. Deterministic output.** Both format outputs must be byte-identical
for the same input across runs. This is a hard requirement for CI drift
detection. No timestamps, no random ordering, no platform-dependent output.

**R6. Source and compiled input.** The export command accepts either a
source template `.md` file (compiled on the fly) or a pre-compiled `.json`
file. Users shouldn't need to run a separate compile step first.

**R7. Committed diagram artifacts.** Mermaid diagrams are written to
`.mermaid.md` sibling files (e.g., `my-workflow.mermaid.md` next to
`my-workflow.md`). These render natively on GitHub. HTML files are written
to a configurable output path for website deployment. Both are committed
artifacts that CI can verify for freshness.

**R8. Built-in freshness check.** `--check` compares what would be
generated against the existing file without writing. Exits 0 if fresh,
non-zero if stale or missing. Requires `--output`. Works uniformly across
both formats: `--format mermaid --check` verifies committed Mermaid
diagrams, `--format html --check` verifies deployed HTML documentation.
When stale, prints the command to fix it.

**R9. CI freshness workflow.** A reusable GitHub Actions workflow that runs
`koto template export --check` for each template in a configurable path,
for each configured format. Fails if any artifact is stale or missing.
Uses a downloaded release binary, not a source build.

**R10. Reusable workflow distribution.** The GHA workflow lives in the koto
repository and is callable by other repos via `uses:` with a tag reference.
It downloads a release binary rather than building from source, so consuming
repos don't need a Rust toolchain.

**R11. Actionable error messages.** When the freshness check fails, the
error output includes the exact command to run locally to fix the drift.

### Non-functional

**R12. HTML file size.** Generated HTML files should be under 30 KB
(excluding CDN-loaded dependencies). The HTML loads JavaScript from CDN
rather than inlining it.

**R13. Compilation latency.** The compile + export path should complete in
under 500ms for templates up to 30 states. Current compile time is
single-digit milliseconds; export adds minimal overhead.

**R14. Offline degradation.** HTML export requires internet access (CDN
dependencies). Mermaid export works fully offline.

## Acceptance criteria

### Export command
- [ ] `koto template export` with no `--format` defaults to mermaid
- [ ] `koto template export workflow.md` prints Mermaid to stdout
- [ ] `koto template export workflow.md --output workflow.mermaid.md` writes
  a file that GitHub renders as a state diagram
- [ ] `koto template export workflow.md --format html --output workflow.html`
  produces a self-contained HTML file
- [ ] `koto template export --format html` without `--output` produces an error
- [ ] `--open` with `--format html` opens the file in the default browser
- [ ] `--open` without `--format html` produces an error
- [ ] The command accepts both `.md` source templates and `.json` compiled
  templates

### Mermaid output
- [ ] Includes `[*]` markers for initial and terminal states
- [ ] Transition edges show `when` conditions as labels
- [ ] Gates appear as `note` annotations
- [ ] Running export twice on the same template produces byte-identical output

### HTML output
- [ ] Displays an interactive directed graph in the browser
- [ ] Hovering over a gated state shows gate name and command in a tooltip
- [ ] Hovering over a state with an accepts block shows the evidence schema
- [ ] Click-to-highlight traces selected state's incoming and outgoing transitions
- [ ] Pan/zoom works for navigating large graphs
- [ ] Includes dark mode via `prefers-color-scheme`
- [ ] Includes a `[*]` start marker node connected to the initial state
- [ ] All CDN script tags include SRI integrity hashes
- [ ] Works when served as a static page (GitHub Pages) without server-side
  processing
- [ ] Running export twice on the same template produces byte-identical output

### Freshness check
- [ ] `--check` with a fresh file exits 0
- [ ] `--check` with a stale file exits non-zero and prints the fix command
- [ ] `--check` with a missing file exits non-zero and identifies the missing file
- [ ] `--check` without `--output` produces an error
- [ ] `--check` works for both `--format mermaid` and `--format html`

### CI workflow
- [ ] The reusable GHA workflow detects stale or missing diagrams via `--check`
- [ ] The GHA workflow error output includes the command to fix the drift
- [ ] The GHA workflow accepts a configurable template path input
- [ ] The GHA workflow downloads a koto release binary, not building from source

## Out of scope

- **Watch mode / live reload.** Compile + export is under 100ms. Re-running
  the command is fast enough for v1. Watch mode can be added later if users
  request it.
- **In-place diagram injection into source templates.** The koto compiler
  parses H2 headings as state boundaries. Injecting content into the source
  `.md` risks breaking compilation or being interpreted as a state directive.
  This is a hard technical constraint.
- **Mermaid embedded in compiled JSON.** The compiled JSON is a machine contract
  consumed by the engine. Mixing display data into it muddies that contract.
- **PR comment bots.** Posting rendered diagrams as PR comments adds permission
  complexity and bot management. The diff view of the committed `.mermaid.md`
  file is sufficient for review.
- **Auto-fix in CI.** The GHA workflow fails on drift but doesn't auto-commit
  fixes. Auto-fix requires write permissions, creates noise in git history,
  and doesn't work for fork PRs.
- **Offline HTML export.** The HTML format loads JS from CDN. A future
  `--inline` flag could bundle dependencies for offline use, but both target
  use cases (local dev, GitHub Pages) are online.
- **Alternative layout engines.** ELK.js (1.3 MB) is too heavy for default use.
  Can be revisited if dagre layout quality degrades at 30+ states.
- **Vendored JS dependencies.** Inlining Cytoscape.js would inflate each
  HTML file to ~435 KB. CDN with SRI hashes is the right trade-off.

## Known limitations

**CDN dependency for HTML format.** The interactive HTML won't render without
internet access. Mermaid export works offline, so there's always a fallback
for basic structure inspection.

**GitHub Mermaid rendering.** The text diagram relies on GitHub's native
Mermaid rendering in markdown files. GitHub supports `stateDiagram-v2` but
rendering quality varies (especially for `note` annotations and large graphs).
If GitHub's renderer regresses, the committed file still contains readable
Mermaid text.

**Mermaid omits rich metadata.** The text diagram shows states, transitions,
conditions, and gates. Evidence schemas, default actions, integration names,
and full directive text are only available in the HTML format. This is
intentional: Mermaid is a structural overview, HTML is the detailed
inspection tool.

**CDN version maintenance.** The HTML export pins CDN library versions with
SRI hashes. Of the three dependencies, only Cytoscape.js is actively maintained
(dagre last released 2016, cytoscape-dagre last released 2022). Version updates
are infrequent and manual.

## Decisions and trade-offs

**Separate sibling file over in-place source update.** The Mermaid diagram is
written to `<stem>.mermaid.md` next to the source template, not injected into
the source `.md` file. The compiler's `extract_directives()` function treats
H2 headings as state boundaries; injecting a diagram section risks being parsed
as a state or requires fragile special-case exclusion. The sibling file pattern
matches how protobuf, OpenAPI, and sqlc handle generated artifacts.

**CI enforcement over "remember to run it."** The text diagram command could
exist as a standalone tool people run manually. History says they won't. CI
enforcement via the reusable GHA workflow makes diagram freshness automatic
rather than aspirational. This is the same pattern as `cargo fmt --check`
or `terraform fmt -check` in CI.

**Unified `export` command over separate `export` + `preview`.** The original
design proposed separate commands because "export is pure text, preview has
side effects (browser launch)." The documentation reader use case changed this:
HTML output for project websites is a pure file-write operation, same as
Mermaid. The side effect was never about the format — it was about opening a
browser. Making that an opt-in `--open` flag cleanly separates output
generation from delivery. One command with `--format mermaid|html` is simpler,
and `--check` works uniformly across both formats without duplication.
`diagram`, `render`, and `graph` were also evaluated as command names;
`export` with `--format` won on extensibility (DOT, PlantUML later) and
because `graph` conflicts with koto's runtime state graph concept.

**Built-in `--check` flag over `git diff` in CI.** Following `cargo fmt --check`,
`prettier --check`, and `terraform fmt -check`, the export command includes a
`--check` flag that exits non-zero when the output would differ from the
existing file. This keeps CI scripts to a single command, produces tailored
error messages, and works outside git repos. The alternative (`git diff
--exit-code` after re-generating) works but pushes glue logic onto every CI
consumer.

## Open questions

- **Line ending normalization**: should the diagram output always use LF?
  Windows contributors with CRLF git settings could see false drift. A
  `.gitattributes` rule (`*.mermaid.md text eol=lf`) in the GHA workflow
  docs may be sufficient.
