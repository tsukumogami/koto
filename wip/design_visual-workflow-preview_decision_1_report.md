# Decision 1: CLI Command Structure

## Chosen: Option C -- Export subcommand + Preview subcommand

`koto template export foo.md --format mermaid` for text export, `koto template preview foo.md` for interactive HTML.

## Rationale

koto's existing `template` subcommand group uses distinct verbs for distinct operations: `compile` transforms source to JSON, `validate` checks a compiled artifact. Each subcommand does one thing. Option C follows this pattern by introducing two verbs that each have a clear, singular purpose:

- **`export`** produces a text representation of the template graph. The `--format` flag selects the output format (mermaid for MVP, with DOT and SVG as natural additions later). Output goes to stdout by default, making it composable with pipes (`koto template export foo.md --format mermaid | pbcopy`). An optional `--output` flag writes to a file instead.

- **`preview`** generates a self-contained HTML file and opens it in a browser. This is a side-effect-heavy operation (writes file, launches browser) that doesn't belong on a text-export command. Giving it its own subcommand makes the side effects explicit and discoverable.

This separation maps cleanly to the two capabilities described in the feature spec. Users running `koto template --help` see four verbs -- compile, validate, export, preview -- each self-explanatory.

### Criteria evaluation

1. **Consistency with existing patterns**: The existing TemplateSubcommand enum uses one variant per operation. Adding `Export` and `Preview` variants follows the same shape. No existing subcommand overloads its meaning with flags that change output type.

2. **Discoverability**: Four subcommands in `koto template --help` is easy to scan. Each verb communicates its intent without needing to read flag descriptions.

3. **Separation of concerns**: `compile` builds the JSON artifact. `export` renders a text visualization. `preview` opens an interactive view. No verb does double duty.

4. **Composability**: `export` defaults to stdout, so `koto template export foo.md --format mermaid | mmdc -i -` works naturally. `preview` writes a file and launches a browser -- inherently non-composable, which is fine since that's its purpose.

5. **Future extensibility**: Adding DOT output is `--format dot`. Adding SVG is `--format svg`. If a future format needs its own flags (e.g., SVG dimensions), they can be conditionally required. The `--format` flag scales better than one-off flags like `--mermaid`, `--dot`, `--svg`.

## Alternatives Considered

**Option A: Flags on compile** -- Rejected. `compile` currently has a clear contract: YAML in, JSON out. Adding `--format mermaid` changes what "compile" means. It also makes `--preview` a surprising side effect on a command that otherwise just writes JSON. This muddies the mental model and makes `compile --help` confusing.

**Option B: Separate visualize subcommand** -- Rejected. Combining text export and browser preview under one verb (`visualize`) conflates two operations with very different characteristics. Text export is pure and composable; browser preview has side effects. Using `--format html` to mean "generate HTML and open a browser" is misleading -- the user might expect HTML on stdout. The verb "visualize" is also vague enough that users might not know what it does without reading help.

**Option D: Flags on existing + new subcommand** -- Rejected. Adding `--mermaid` to `compile` has the same "compile means too many things" problem as Option A, just scoped to one format. It also doesn't extend well: if we later add DOT, do we add `--dot` to compile too? The inconsistency of having some formats as flags on `compile` and others as a separate subcommand creates a confusing split.

## Assumptions

- The `--format` flag on `export` will start with a single value (`mermaid`) but won't feel over-engineered because the flag communicates "more formats are possible."
- `preview` can accept either a source template (compiled on the fly) or a pre-compiled JSON, matching how `compile` and `validate` handle input today.
- Users will primarily pipe Mermaid output or redirect it to a file; defaulting to stdout is the right call.
- Browser launching via `opener` with graceful fallback (print path if launch fails) is sufficient for `preview`.

## Confidence: High

The existing CLI patterns strongly favor one-verb-per-operation. Option C is the only option that respects this while cleanly separating the two capabilities. The main risk is minor: having four subcommands under `template` instead of two, but that's well within reasonable limits.
