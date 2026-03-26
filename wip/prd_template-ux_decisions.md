# /prd Decisions: template-ux

## Decision Log

### D1: Mermaid output strategy
- **Status**: Confirmed
- **Decided**: Separate `.mermaid.md` sibling file, CI-enforced
- **Reasoning**: In-place source update ruled out (compiler parses H2 as states). Sibling file matches protobuf/sqlc precedent.

### D2: GHA workflow scope
- **Status**: Confirmed
- **Decided**: Reusable workflow, re-generate-and-diff via `--check`, fail-only, release binary
- **Reasoning**: Matches existing koto CI conventions. `--check` flag simplifies workflow.

### D3: Watch mode
- **Status**: Confirmed
- **Decided**: Not in v1. Re-run is sufficient.
- **Reasoning**: Compile + export is <100ms. Sub-2-second cycle.

### D4: Command naming
- **Status**: Confirmed
- **Decided**: `koto template export --format mermaid|html`
- **Reasoning**: `export` with `--format` extensible to DOT/PlantUML. `diagram`, `render`, `graph` eliminated.

### D5: --check flag
- **Status**: Confirmed
- **Decided**: Built-in `--check` on export command
- **Reasoning**: Follows `cargo fmt --check` pattern. Works for both formats uniformly.

### D6: Command structure (export vs preview split)
- **Status**: Confirmed (auto)
- **Decided**: Unified `koto template export` with `--format mermaid|html` and `--open` flag
- **Reasoning**: Deploy pipeline use case (HTML as website documentation) dissolved the "export is pure, preview has side effects" argument. The side effect is the browser launch, not the format. `--open` makes it opt-in.
- **Report**: `wip/research/prd_template-ux_decision_command-structure.md`
