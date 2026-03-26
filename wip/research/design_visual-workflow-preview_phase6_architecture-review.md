# Architecture Review: DESIGN-visual-workflow-preview

Reviewer: architect-reviewer
Date: 2025-03-25
Design: docs/designs/DESIGN-visual-workflow-preview.md

## Summary Assessment

The design is structurally sound and fits the existing architecture. The two new
subcommands follow the established `TemplateSubcommand` enum pattern, the new
`src/export/` module respects dependency direction (depends on `template::types`,
not on `cli` or `engine`), and the `CompiledTemplate` struct is consumed
read-only -- no state contract changes.

## Question 1: Is the architecture clear enough to implement?

**Yes, with one gap.** The component layout, function signatures, data flow, and
CLI wiring are all specified with enough precision to implement directly. The one
gap is how the `<source>` argument is resolved: the design says both subcommands
"accept either a source template path (compiled on the fly via `compile_cached`)
or a pre-compiled JSON path" and "the CLI handler detects which based on file
extension." This needs a concrete specification:

- What extensions map to which path? `.md`/`.yaml`/`.yml` -> compile, `.json` ->
  load directly? The existing `compile` subcommand takes only source paths and
  `validate` takes only JSON paths. The new commands are the first to accept
  both, so the detection logic is new surface area.
- Where does this detection live? A shared helper in `cli/mod.rs` or duplicated
  in each match arm? This affects whether future subcommands get the same
  capability for free.

**Recommendation:** Define a `resolve_template(source: &str) ->
Result<CompiledTemplate>` helper in `cli/mod.rs` (or in `cache.rs`) that both
`export` and `preview` call. This prevents the detection logic from being
duplicated across two match arms.

## Question 2: Are there missing components or interfaces?

### Missing: format dispatch in `src/export/mod.rs`

The design lists `src/export/mod.rs -- format dispatch` but doesn't specify the
interface. For a single format (Mermaid), a format enum and dispatch function are
over-engineering. The CLI match arm can call `to_mermaid()` directly. The module
file just needs `pub mod mermaid; pub mod preview;`. Add dispatch when a second
format arrives.

### Missing: JSON serialization shape for preview

`generate_preview()` calls `serde_json::to_string()` on the `CompiledTemplate`.
The HTML/JS code needs to know the exact JSON shape to build the Cytoscape graph.
The design should specify (or at minimum reference) the `CompiledTemplate` struct
fields that the JS consumes: `initial_state`, `states` (keyed by name), each
state's `transitions`, `terminal`, `gates`, `accepts`, `default_action`. This is
the contract between the Rust serializer and the JS consumer.

This isn't a blocking architectural issue -- the `CompiledTemplate` is already a
`Serialize` type with a stable schema -- but implementers of the HTML template
need to know what to expect.

### Missing: error output contract

The existing `template compile` and `template validate` subcommands emit JSON
errors via `exit_with_error()`. The design doesn't specify whether `export` and
`preview` follow this pattern or use plain stderr. Since these are
developer-facing tools (not agent-consumed), plain stderr with anyhow's error
chain is more appropriate. But the decision should be explicit so the
implementation doesn't accidentally mix conventions.

## Question 3: Are the implementation phases correctly sequenced?

**Yes.** The phases have correct dependency ordering:

- Phase 1 (Mermaid export): standalone, no new dependencies, exercises the
  `CompiledTemplate -> text` path and the new `TemplateSubcommand::Export`
  variant. Can be shipped and used immediately.
- Phase 2 (HTML template): standalone HTML/CSS/JS work. No Rust changes. Can be
  developed and tested in a browser independently.
- Phase 3 (preview command): depends on Phase 1 (CLI infrastructure for the
  Export variant shows the pattern) and Phase 2 (the HTML template to embed).
  Adds the `opener` dependency.

One minor note: the `TemplateSubcommand::Preview` variant could be added in
Phase 1 as a stub (prints "not yet implemented") to avoid a second round of CLI
changes. But this is a convenience, not a sequencing error.

## Question 4: Are there simpler alternatives we overlooked?

### Alternative: `--format` on `preview` instead of separate `export` subcommand

A single `koto template preview <source> --format mermaid|html` with `--format
mermaid` writing to stdout and `--format html` writing to a file. This reduces
the CLI surface from two new commands to one. However, the design correctly
identifies that `export` (pure text, composable) and `preview` (side effects:
file write + browser open) have fundamentally different contracts. Merging them
would make `--format mermaid` a surprising no-side-effect mode on a command that
otherwise has side effects. The two-command design is the right call.

### Alternative: use `compile` output piped to a separate formatter

Instead of new subcommands, provide a standalone `koto-mermaid` binary or a
library function that reads compiled JSON from stdin. This is more Unix-y but
adds friction for the common case and doesn't fit the existing subcommand-group
pattern. Not simpler.

### Alternative: skip the HTML preview entirely, ship only Mermaid

Mermaid renders on GitHub and in VS Code with extensions. For many users, this
covers the use case. The interactive HTML adds significant implementation surface
(JS, CSS, CDN management, `opener` dependency) for the additional capability of
tooltips and click-to-highlight on large graphs. Whether this tradeoff is worth
it depends on how many users work with 15+ state workflows. The phased approach
handles this well: ship Mermaid first, gauge demand, build HTML preview if
needed. The design already structures the phases this way, which is the right
sequencing.

## Architectural Findings

### Finding 1: New module location fits -- Advisory

The proposed `src/export/` module is a new top-level module alongside `template/`,
`engine/`, `cli/`, etc. This is appropriate: export logic depends on
`template::types::CompiledTemplate` (downward dependency) and is consumed by
`cli/mod.rs` (upward caller). No dependency inversion.

The module needs to be registered in `src/lib.rs` as `pub mod export;`. The
design doesn't mention this, but it's obvious from Rust module conventions.

### Finding 2: No state contract changes -- Clean

The design reads `CompiledTemplate` without modification. No new fields on any
struct, no new event types, no changes to the JSONL state file format. Clean
from a state contract perspective.

### Finding 3: `compile_cached` returns path, not struct -- Advisory

The design's data flow shows `compile_cached() -> CompiledTemplate`. But the
actual `compile_cached()` in `src/cache.rs` returns `(PathBuf, String)` -- a
path and hash. The export/preview functions need the deserialized
`CompiledTemplate`, not just the path. The implementation will need to either:

(a) Call `compile_cached()` then read and deserialize the cached JSON file, or
(b) Call `compile::compile()` directly to get the `CompiledTemplate` struct.

Option (a) preserves the caching benefit. Option (b) bypasses the cache but is
simpler. The design should specify which. For export/preview, caching doesn't
matter much (the operation is fast), so calling `compile::compile()` directly is
acceptable. But if the source argument can be a pre-compiled JSON file, the code
needs a load-from-JSON path regardless, and `load_compiled_template()` already
exists in `cli/mod.rs` for this.

**Recommendation:** Extract `load_compiled_template()` from `cli/mod.rs` into a
shared location (e.g., `cache.rs` or a new `template::load` function) so both
the CLI match arms and the export module can use it without reaching into CLI
internals.

### Finding 4: `preview.html` path traversal claim is incomplete -- Advisory

The security section states: "The preview command writes to the current directory
using `Path::file_stem()` for filename derivation. This prevents path traversal
via `../` in template paths."

`Path::file_stem()` extracts the stem from the *last component* of the path, so
`../../evil` would produce stem `evil` and the output would be `evil.preview.html`
in the current directory. This is correct behavior -- no traversal. But the
`--output` flag allows arbitrary paths. The security section should note that
`--output` is user-specified and therefore trusted (same as any CLI file output
flag).

### Finding 5: `opener` is the only new runtime dependency -- Clean

Adding `opener` to Cargo.toml is a minimal dependency addition. It has no
transitive dependencies on Unix (just calls `xdg-open`). Consistent with the
project's approach of minimal dependencies.

## Verdict

**Architecturally sound.** The design respects the existing CLI subcommand
pattern, module dependency direction, and state contracts. No structural
violations.

Two items to clarify before implementation:

1. How source argument resolution works (file extension detection) and where the
   shared helper lives.
2. Whether `export`/`preview` errors use JSON format (like existing template
   subcommands) or plain stderr (more appropriate for developer-facing tools).

Neither is blocking -- they can be resolved during implementation review. The
phased approach is correctly sequenced and the separation of `export` vs
`preview` is the right structural choice.
