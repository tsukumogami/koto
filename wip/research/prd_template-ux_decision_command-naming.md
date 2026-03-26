# Decision: Command naming for diagram generation

## Alternatives Evaluated

### 1. export --format mermaid

The design doc's current choice. `export` is a generic verb that signals "produce output in a specified format." The `--format` flag provides extensibility to DOT, PlantUML, or SVG later without adding subcommands.

**Strengths:**
- Extensible by design -- adding `--format dot` requires no CLI schema change
- Follows a well-established pattern in tools like `docker export`, `kubectl get -o`, `gh issue list --json`
- The design doc already uses this naming throughout, including the implementation plan, file layout (`src/export/`), and security review
- Pairs cleanly with `preview`: `export` = text artifact (pure, composable), `preview` = side-effect-heavy browser launch
- `koto template --help` reads: compile, validate, export, preview -- four verbs with distinct purposes

**Weaknesses:**
- "Export" implies converting from one format to another, which could suggest the compiled JSON is being re-serialized rather than visualized
- Requires `--format` flag even when there's only one format (mermaid) at launch
- Slightly longer to type in CI scripts: `koto template export foo.md --format mermaid` vs `koto template diagram foo.md`

### 2. diagram

The codebase analyst's suggestion. Direct, self-documenting verb.

**Strengths:**
- Immediately communicates intent: "this command makes a diagram"
- Shorter invocation: `koto template diagram foo.md`
- No ambiguity -- users don't need to guess what formats are available
- Matches the mental model well: compile (transform), validate (check), diagram (visualize)

**Weaknesses:**
- What happens when DOT output is needed? `koto template diagram --format dot` works but then "diagram" is doing the same job as "export" with a more restrictive name
- DOT, PlantUML, and SVG are all diagrams, so the name doesn't actually restrict extensibility -- but it does feel like the `--format` flag is fighting the verb name
- Doesn't pair as naturally with `preview`: diagram and preview are both "visualization" commands, while export and preview have a clearer pure-vs-side-effect distinction
- The design doc, security review, and implementation plan all use `export` -- changing requires updating multiple artifacts

### 3. render --format mermaid

**Strengths:**
- "Render" implies visual output, which is what's happening

**Weaknesses:**
- "Render" strongly implies producing a visual image (pixels), not text. Mermaid output is a text DSL that gets rendered later by GitHub or a Mermaid renderer. Koto isn't rendering anything -- it's generating a description that something else renders.
- Creates confusion with `preview`, which actually does render (via Cytoscape.js in a browser)
- No strong precedent in comparable CLI tools

**Verdict:** Eliminated. The semantic mismatch between "render" and "emit text that will be rendered elsewhere" is too confusing.

### 4. graph

**Strengths:**
- Matches `terraform graph` which outputs DOT format to stdout -- almost identical use case
- Short, technical, accurate (koto templates are directed graphs)
- `koto template graph` reads well

**Weaknesses:**
- Ambiguous with runtime state. `koto` is a state machine engine -- "graph" could mean "show me the current workflow's state graph" (runtime) rather than "generate a diagram from a template" (build-time). This is the critical problem.
- `terraform graph` works because Terraform doesn't have a runtime graph concept. Koto does -- every workflow IS a running graph.
- Adding `--format` to `graph` reads oddly: `koto template graph --format mermaid` (a graph in mermaid format? or a mermaid graph?)

**Verdict:** Eliminated. The ambiguity with koto's runtime state graph is a real source of confusion, not a theoretical one.

### 5. export as umbrella (including preview)

The question: should `preview` become `export --format html` instead of a separate command?

The design doc rejected this, and the reasoning holds:

- `export` is pure: reads a template, writes text to stdout or a file. No side effects. Composable with pipes.
- `preview` is side-effect-heavy: writes an HTML file to a temp location, then opens a browser. Not composable.
- `export --format html` would either (a) print raw HTML to stdout (useless -- users want it opened in a browser) or (b) silently write a file and open a browser (breaking the contract that `export` is a pure text operation).
- The `--format` flag suggests format variants are interchangeable. But mermaid output goes to stdout for CI pipelines while HTML output goes to a browser for debugging. These are fundamentally different workflows.

**Verdict:** Eliminated. The design doc's reasoning is sound. Separate commands for separate concerns.

## Recommendation

**Use `export --format mermaid`** (the design doc's current choice).

The deciding factors:

1. **Extensibility without schema churn.** When DOT or PlantUML support arrives, it's `--format dot` -- no new subcommand, no documentation restructuring, no CI script updates. With `diagram`, you'd either add `--format` anyway (making the verb name redundant) or create separate `diagram-dot` commands (ugly).

2. **Clean separation from preview.** The `export` / `preview` pairing communicates a meaningful distinction: pure text output vs. side-effect-heavy browser launch. The `diagram` / `preview` pairing is muddier -- both are "visualization" commands, and the difference isn't obvious from the names.

3. **Precedent in the codebase.** The design doc, security review, architecture review, and implementation plan all use `export`. The `src/export/` module layout is already designed around this name. Changing it means updating multiple artifacts for marginal naming improvement.

4. **The `--format` flag cost is minimal.** Yes, there's only one format at launch. But `--format mermaid` can be the default (elided when there's only one option), or it can serve as documentation -- users see `--format mermaid` and understand this is one of potentially several output formats. The design doc already specifies mermaid as the default, so `koto template export foo.md` works without the flag.

The codebase analyst's `diagram` suggestion has genuine appeal for its directness, but it creates a naming awkwardness that grows over time as formats are added. `export` starts generic and stays consistent.

## Impact on PRD

The PRD should:

- Use `koto template export` as the command name, with `--format mermaid` as the initial (and default) format
- Note that `--format` defaults to `mermaid` when omitted, so the minimal invocation is `koto template export <source>`
- Frame `export` and `preview` as complementary: `export` for committed text artifacts (CI-enforced), `preview` for interactive local debugging
- Mention that the `--format` flag provides a natural extension point for DOT or PlantUML without CLI breaking changes
- In CI examples, show both forms: `koto template export foo.md` (implicit mermaid) and `koto template export foo.md --format mermaid` (explicit)
