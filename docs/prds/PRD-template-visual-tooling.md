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

**Documentation readers** browsing templates on GitHub or GitHub Pages want
to quickly understand the workflow a template enforces. They aren't going to
clone the repo and run commands. If there's no rendered diagram next to the
template source, they're left reading YAML front-matter and mentally
constructing the graph themselves.

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
- Documentation readers can see a rendered workflow diagram when browsing
  templates on GitHub or GitHub Pages, without cloning the repo or running tools
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

**As someone browsing template documentation** on GitHub, I want to see a
rendered state diagram alongside the template source, so that I can
understand the workflow structure without cloning the repo or running tools.

**As a repo maintainer**, I want a CI check that fails when committed
diagrams don't match the current template source, so that stale diagrams
can't be merged.

**As a repo maintainer setting up CI**, I want a reusable GitHub Actions
workflow I can add to my repo with minimal configuration, so that I don't
have to write template validation logic from scratch.

## Requirements

### Functional

**R1. Interactive HTML preview.** A command produces a self-contained HTML
file from a compiled template and opens it in the default browser. The HTML
displays an interactive directed graph with: state nodes (color-coded by type),
labeled transition edges, hover tooltips showing gate conditions and evidence
schemas, click-to-highlight for tracing paths, and pan/zoom for large graphs.

**R2. Text diagram export.** `koto template export` produces a Mermaid
`stateDiagram-v2` text representation of a compiled template. Output goes to
stdout by default. A `--format` flag selects the output format (defaulting to
`mermaid` when omitted). An `--output` flag writes to a specified file path.
The diagram shows states, transitions with condition labels, and gate
annotations.

**R3. Deterministic output.** The text diagram output must be byte-identical
for the same input across runs. This is a hard requirement for CI drift
detection. No timestamps, no random ordering, no platform-dependent output.

**R4. Source and compiled input.** Both the preview and diagram commands
accept either a source template `.md` file (compiled on the fly) or a
pre-compiled `.json` file. Users shouldn't need to run a separate compile
step first.

**R5. Committed diagram artifact.** The text diagram is written to a
`.mermaid.md` file that lives as a sibling of the source template (e.g.,
`my-workflow.mermaid.md` next to `my-workflow.md`). This file is committed
to version control and renders natively on GitHub.

**R6. Built-in freshness check.** `koto template export --output <path> --check`
compares the generated output against the existing file without writing.
Exits 0 if fresh, exits non-zero if stale or missing. Requires `--output`
(comparing against stdout is meaningless). When the target file doesn't exist,
exits non-zero with a message identifying the missing file. When content
differs, prints a one-line message pointing to the stale file and the command
to fix it.

**R7. CI freshness workflow.** A reusable GitHub Actions workflow that runs
`koto template export --output <path> --check` for each template in a
configurable path. Fails if any diagram is stale or missing. Uses a
downloaded release binary, not a source build.

**R8. Reusable workflow distribution.** The GHA workflow lives in the koto
repository and is callable by other repos via `uses:` with a tag reference.
It downloads a release binary rather than building from source, so consuming
repos don't need a Rust toolchain.

**R9. Actionable error messages.** When the freshness check fails (whether
via `--check` or CI), the error output includes the exact command to run
locally to fix the drift (e.g.,
`koto template export my-workflow.md --output my-workflow.mermaid.md`).

### Non-functional

**R10. Preview file size.** Generated HTML preview files should be under
30 KB (excluding CDN-loaded dependencies). The HTML loads JavaScript from
CDN rather than inlining it.

**R11. Compilation latency.** The compile + diagram generation path should
complete in under 500ms for templates up to 30 states. Current compile time
is single-digit milliseconds; diagram generation adds minimal overhead.

**R12. Offline degradation.** The interactive HTML preview requires internet
access (CDN dependencies). The text diagram command works fully offline.

## Acceptance criteria

- [ ] Running the preview command on a multi-state template produces an HTML
  file that opens in a browser and displays an interactive state graph
- [ ] Hovering over a gated state in the preview shows gate name and command
  in a tooltip
- [ ] Hovering over a state with an accepts block shows the evidence schema
- [ ] Click-to-highlight traces the selected state's incoming and outgoing
  transitions
- [ ] Running the diagram command twice on the same template produces
  byte-identical output
- [ ] The diagram command with `--output` writes a `.mermaid.md` file that
  GitHub renders as a state diagram
- [ ] The Mermaid output includes `[*]` markers for initial and terminal states
- [ ] Transition edges in the Mermaid output show `when` conditions as labels
- [ ] Gates appear as `note` annotations in the Mermaid output
- [ ] `koto template export --output fresh-file.mermaid.md --check` exits 0
- [ ] `koto template export --output stale-file.mermaid.md --check` exits
  non-zero and prints the command to fix it
- [ ] `koto template export --output missing-file.mermaid.md --check` exits
  non-zero and identifies the missing file
- [ ] `--check` without `--output` produces an error
- [ ] The reusable GHA workflow uses `--check` to detect stale or missing
  diagrams and fails the check
- [ ] The GHA workflow error output includes the command to fix the drift
- [ ] The GHA workflow accepts a configurable template path input
- [ ] The GHA workflow downloads a koto release binary, not building from source
- [ ] The preview command accepts both `.md` source templates and `.json`
  compiled templates
- [ ] The diagram command accepts both `.md` source templates and `.json`
  compiled templates
- [ ] The HTML preview includes dark mode via `prefers-color-scheme`
- [ ] The HTML preview includes a `[*]` start marker node connected to the
  initial state
- [ ] All CDN script tags in the preview HTML include SRI integrity hashes

## Out of scope

- **Watch mode / live reload for preview.** Compile + generate is under 100ms.
  Re-running the command is fast enough for v1. Watch mode can be added later
  if users request it.
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
- **Offline interactive preview.** The HTML preview loads JS from CDN. A future
  `--inline` flag could bundle dependencies for offline use, but both target
  use cases (local dev, GitHub Pages) are online.
- **Alternative layout engines.** ELK.js (1.3 MB) is too heavy for default use.
  Can be revisited if dagre layout quality degrades at 30+ states.
- **Vendored JS dependencies.** Inlining Cytoscape.js would inflate each
  preview file to ~435 KB. CDN with SRI hashes is the right trade-off.

## Known limitations

**CDN dependency for preview.** The interactive HTML won't render without
internet access. The Mermaid text diagram works offline, so there's always a
fallback for basic structure inspection.

**GitHub Mermaid rendering.** The text diagram relies on GitHub's native
Mermaid rendering in markdown files. GitHub supports `stateDiagram-v2` but
rendering quality varies (especially for `note` annotations and large graphs).
If GitHub's renderer regresses, the committed file still contains readable
Mermaid text.

**Mermaid omits rich metadata.** The text diagram shows states, transitions,
conditions, and gates. Evidence schemas, default actions, integration names,
and full directive text are only available in the interactive preview. This is
intentional: the Mermaid diagram is a structural overview, not a complete
specification.

**CDN version maintenance.** The preview HTML pins CDN library versions with
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

**Interactive preview as a separate command.** The preview generates an HTML
file and opens a browser, which is a side effect. The text diagram outputs to
stdout, which is composable. These are different interaction models that serve
different moments (debugging vs. committing). Combining them into one command
with a `--format` flag would conflate composable text output with
side-effect-heavy browser interaction.

**`export` over `diagram` for the command name.** The `--format` flag provides
clean extensibility to DOT or PlantUML without CLI schema changes. The
`export`/`preview` pairing communicates a meaningful pure-vs-side-effect
distinction that `diagram`/`preview` doesn't. `diagram` has directness appeal
but creates naming awkwardness as formats are added. `render` and `graph` were
also evaluated and eliminated (`render` implies pixels, `graph` conflicts with
koto's runtime state graph concept).

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
