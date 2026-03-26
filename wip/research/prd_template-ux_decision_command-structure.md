# Decision: Command structure for visual tooling

## Alternatives Evaluated

### 1. Unified export command

`koto template export --format mermaid|html [--output path] [--open]`

One command handles all visual output formats. `--format` selects between mermaid (text to stdout, default) and html (interactive Cytoscape.js file). `--open` is a convenience flag that launches the browser, only valid with `--format html`. When `--format html` is used without `--output`, defaults to `<stem>.preview.html`.

**Strengths:**
- Conceptually honest. Both mermaid and HTML are visualizations of the same compiled template graph. The operation is "visualize this template" -- the format is a parameter, not a different verb.
- The deploy pipeline use case (`koto template export workflow.md --format html --output docs/workflow.html`) reads naturally. There's no awkward "preview" verb when the output is production documentation.
- `--open` as an opt-in flag means the default behavior is pure: write a file, print its path. Side effects only happen when explicitly requested. This dissolves the original "export is pure, preview has side effects" argument -- the side effect was always just the browser launch, and that becomes a flag.
- `--check` works uniformly across formats: `koto template export --format mermaid --output diagram.md --check` and `koto template export --format html --output docs/workflow.html --check` both verify freshness. No need to decide which command owns `--check`.
- Fewer subcommands means simpler `koto template --help`: compile, validate, export. Three verbs, each with a clear purpose.
- Follows `gh issue list --json` / `kubectl get -o` pattern where format is a flag, not a subcommand.

**Weaknesses:**
- `--format html` on a command called "export" could suggest raw HTML to stdout. Mitigated: when `--format html`, the command writes a file (not stdout) and prints the path. This is a behavioral difference between formats that needs documentation.
- `--open` is only valid with `--format html`, creating a conditional flag. `koto template export --format mermaid --open` would need to error or warn.
- The previous command-naming decision explicitly rejected "export as umbrella" (alternative 5). That analysis argued mermaid goes to stdout while HTML goes to a browser -- "fundamentally different workflows." But that argument assumed the HTML use case was primarily browser-based local debugging. The deploy pipeline use case changes the calculus: HTML-to-file-for-documentation is the same workflow shape as mermaid-to-file-for-documentation.

**What `koto template --help` looks like:**
```
Subcommands:
  compile   Compile a YAML template source to FormatVersion=1 JSON
  validate  Validate a compiled template JSON file
  export    Generate a visual representation of a compiled template
```

**What `koto template export --help` looks like:**
```
Generate a visual representation of a compiled template

Usage: koto template export <SOURCE> [OPTIONS]

Arguments:
  <SOURCE>  Path to template source (.md) or compiled JSON (.json)

Options:
  -f, --format <FORMAT>  Output format [default: mermaid] [possible values: mermaid, html]
  -o, --output <PATH>    Write to file instead of stdout (required for html)
      --open             Open in browser after generation (html only)
      --check            Verify output file is up to date (requires --output)
```

### 2. Separate commands, rename preview to doc

`koto template export --format mermaid` + `koto template doc <source>`

Following `cargo doc` precedent. The HTML generation command is `doc`, conveying that it produces documentation rather than a temporary preview. `--open` flag (defaulting to true for local use) launches the browser. `--no-open` suppresses it for CI pipelines.

**Strengths:**
- `doc` communicates the artifact's purpose better than `preview`. A deploy pipeline running `koto template doc workflow.md --output docs/workflow.html --no-open` makes semantic sense.
- Maintains the verb-per-operation pattern: compile (transform), validate (check), export (text diagram), doc (HTML documentation).
- `cargo doc` / `cargo doc --open` is well-understood precedent in the Rust ecosystem that koto targets.
- Keeps export purely text-based -- no conditional behavior based on format.

**Weaknesses:**
- Four subcommands under `koto template` is more cognitive load than three. Each new verb needs its own help text, examples, and documentation.
- "doc" is ambiguous -- does it generate documentation about the template, or is it the template's documentation? `cargo doc` works because Rust doc comments have a specific meaning. Koto templates don't have a "doc" concept.
- The `--check` flag now lives on two separate commands with identical semantics. Users learn one pattern, then apply it to the other, but it's still duplicated implementation and documentation.
- `--no-open` as a default-true flag that CI must negate is backwards from how most CLI tools work. Typically, side effects are opt-in (`--open`), not opt-out (`--no-open`). But if `doc` defaults to not opening, then `koto template doc workflow.md` silently writes a file -- making it behaviorally identical to `koto template export --format html --output workflow.preview.html`. At that point, what's the separate command buying?

**What `koto template --help` looks like:**
```
Subcommands:
  compile   Compile a YAML template source to FormatVersion=1 JSON
  validate  Validate a compiled template JSON file
  export    Export a text diagram of a compiled template
  doc       Generate interactive HTML documentation for a template
```

### 3. Separate commands, keep preview name

Status quo from the design doc. `koto template export` for mermaid text, `koto template preview` for interactive HTML with browser launch.

**Strengths:**
- Already designed, documented, and reviewed. The design doc, security review, and architecture review all use this naming. Zero rework.
- The "preview" name is accurate for the local debugging use case, which is the initial primary use case.
- Clear separation: export is composable text to stdout, preview is a side-effect-heavy browser tool.

**Weaknesses:**
- "Preview" is misleading for the deploy pipeline use case. Running `koto template preview workflow.md --output docs/workflow.html` in a CI pipeline that generates project documentation feels wrong -- you're not previewing, you're producing a final artifact.
- The name implies temporary/draft output. But the HTML files are meant to be committed as documentation on GitHub Pages. "Preview" undersells the artifact.
- Forces future CI/deploy examples to use a command whose name suggests local-only, ephemeral use.
- The "export is pure, preview has side effects" distinction weakens when preview's primary use case shifts from "open browser" to "write HTML file for deployment." In the deploy case, preview is also a pure file-write operation.

**What `koto template --help` looks like:**
```
Subcommands:
  compile   Compile a YAML template source to FormatVersion=1 JSON
  validate  Validate a compiled template JSON file
  export    Export a text diagram of a compiled template
  preview   Generate an interactive HTML preview and open in browser
```

## Recommendation

**Alternative 1: Unified export command.**

The deploy pipeline use case fundamentally changes the argument for separation. The previous command-naming decision (alternative 5 in that analysis) rejected unification because "mermaid output goes to stdout for CI pipelines while HTML output goes to a browser for debugging -- fundamentally different workflows." But with HTML-as-documentation, the primary HTML workflow is the same as mermaid's: generate an artifact, write it to a file, commit it. The browser launch is a convenience, not the defining characteristic.

The key insight is that the "export is pure, preview has side effects" framing conflated the output format with the delivery mechanism. The side effect was never about HTML -- it was about opening a browser. Making browser launch an opt-in flag (`--open`) cleanly separates the two concerns. `koto template export --format html --output docs/workflow.html` is pure. `koto template export --format html --open` has a side effect. The user controls which behavior they want.

This also simplifies `--check` design. One command, one flag, works for all formats. No need to decide whether `--check` belongs on export, preview, doc, or all of them.

The conditional behavior of `--open` (only valid with `--format html`) is a minor wart, but it's the same pattern as `--output` behaving differently per format (mermaid defaults to stdout, html requires a file). Format-dependent flag semantics are a normal cost of format multiplexing.

The previous decision explicitly rejected this option, but it was working from the assumption that HTML was primarily a local debugging tool. The new information -- HTML as production documentation for project websites -- shifts the balance. This isn't revisiting a settled decision for aesthetic reasons; the use case changed.

## Impact on PRD

The PRD should:

1. **Replace the two-command structure with a single `koto template export` command** that supports `--format mermaid` (default, text to stdout) and `--format html` (interactive Cytoscape.js file).
2. **Add `--open` flag** to `export`, valid only with `--format html`. When provided, opens the generated file in the default browser via the `opener` crate. Graceful fallback: print the path.
3. **Update the `--check` flag specification** to work uniformly across formats: `koto template export --format html --output docs/workflow.html --check` verifies the deployed HTML is fresh against the current compiled template.
4. **Remove all references to `koto template preview`** as a separate subcommand. Update the design doc's "CLI command structure" decision, solution architecture, implementation phases, and security review sections.
5. **Rename `src/export/preview.rs` to `src/export/html.rs`** in the implementation plan, since it's no longer tied to a "preview" command. The module produces HTML export output, parallel to `mermaid.rs`.
6. **Update the default output filename** from `<stem>.preview.html` to `<stem>.html` (or keep `.preview.html` if the convention is useful for gitignore patterns -- this is a minor follow-up choice).
7. **Add deploy pipeline examples** to the PRD showing the HTML-as-documentation workflow: `koto template export workflow.md --format html --output docs/workflow.html` in CI, with `--check` for freshness verification.
8. **Update `koto template --help` description** in the PRD: three subcommands (compile, validate, export) instead of four.
