# Phase 8 Architecture Review: koto Template Format v3

## Summary

The design is well-structured and implementable. The source/compiled separation is architecturally clean, and the phasing is mostly correct. However, there are three structural issues: the `Engine.Transition` signature change creates an API break that the phasing doesn't account for, the `MachineState` type needs gates but the design never shows where gate declarations land in the engine types, and the template hash semantics change silently when switching from source-hash to compiled-hash.

## Findings

### Blocking

**B1. Engine.Transition signature change is sequenced wrong (Phase 4 vs Phase 1)**

The design specifies changing `Engine.Transition(target string) error` to `Engine.Transition(target string, opts ...TransitionOption) error` in Phase 4. But Phase 1 redefines the compiled template format and how templates produce `Machine` instances. The existing `Machine` and `MachineState` types (`/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/types.go:34-46`) have no gate fields. The design adds gates to the compiled JSON (`StateDecl.Gates`) but never shows the corresponding change to `MachineState`:

```go
// Current (types.go:43-46)
type MachineState struct {
    Transitions []string
    Terminal    bool
}
```

The design's `CompiledTemplate.StateDecl` has a `Gates map[string]GateDecl` field, but there's no specification for how this maps to `engine.MachineState`. If `MachineState` gets a `Gates` field in Phase 1 (when the compiled format is defined), the engine needs to know about gates before Phase 4. If gates stay out of `MachineState` until Phase 4, then Phase 1's `CompiledTemplate` parsing can't produce a complete `Machine` -- it would parse gates from JSON but have nowhere to put them.

Resolution: decide explicitly whether `MachineState` gains a `Gates` field in Phase 1 (parsed but not evaluated until Phase 4) or whether the compiled template parser produces a separate gates map alongside the `Machine`. The design must show this type mapping. This is blocking because it affects the API surface of `pkg/engine/`, which is the anchor package -- every other package depends on its types.

**B2. Template hash semantics change is unspecified**

Currently, the template hash is SHA-256 of the source file content (`/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go:67-68`):

```go
sum := sha256.Sum256(data)
hash := "sha256:" + hex.EncodeToString(sum[:])
```

The design says `koto init` compiles source in memory. But hash of what? The design says "Compute SHA-256 hash of the compiled output" (compilation rule 6), but the security section says "SHA-256 hash stored at init time, verified on every operation" referencing template integrity.

If the hash is of the source file (current behavior), then changing whitespace in a directive body (no structural change) invalidates all running workflows. If the hash is of the compiled JSON, then two different source files could produce the same hash, and the hash doesn't detect source-level edits that don't affect compilation output. The design needs to pick one and state it clearly. The engine design says "SHA-256 of the full template file content" -- the template format design contradicts this.

This is blocking because hash semantics are a contract between `koto init` and every subsequent `koto` operation. Getting this wrong means workflows break on harmless edits or fail to detect meaningful changes.

**B3. State schema_version collision**

The design says "Bump `schema_version` to 2" for the state file when adding `Evidence map[string]string`. But the compiled template format ALSO uses `schema_version`, starting at 1. These are two different schemas (state file and compiled template) using the same field name with independent version numbers. Today the state file is at schema_version 1. The compiled template starts at schema_version 1. When evidence is added, the state file goes to 2 -- but there's no discussion of whether the compiled template also bumps.

More importantly, `engine.Load` currently reads the state file and doesn't check `schema_version` at all (`/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:70-94`). The design says "Load accepts v1 (empty Evidence map) and v2" but doesn't specify what happens when a v1 engine binary encounters a v2 state file.

This is blocking because schema versioning is the forward-compatibility mechanism. If both the state file and compiled template use `schema_version` without namespace separation, tooling that inspects JSON files can't distinguish which schema they're looking at. Use `template_schema_version` or put the version in a wrapper object to disambiguate.

### Advisory

**A1. Heading collision warning may be insufficient for the common case**

The compiler emits a warning when `## plan` appears inside the `assess` directive and `plan` is a declared state. The design says "The warning is: state %q directive contains ## heading matching state %q; is this intentional?" and the compiler still produces valid output.

In practice, this will be the most common authoring mistake. An author adds a `## Summary` heading inside a state directive, then later adds a state called `summary`. The compiler silently reassigns the content. A warning on stderr that the author might not see (especially in CI or agent-driven workflows) is insufficient.

Consider: make this a compiler error by default, with a `--allow-heading-overlap` flag to downgrade to warning. Authors who intentionally use state-name headings inside directives can opt in. This is advisory because the current behavior is documented and doesn't break any existing contract, but it will cause confusion.

**A2. Evidence and variable namespace overlap is underspecified**

The design says "Gates check the evidence map only" and "Evidence wins over variables (higher precedence)" in the interpolation context. This means the same key can exist in both namespaces with different semantics: `TASK` could be a variable (for interpolation) AND an evidence key (for gates). The interpolation context merges them with evidence winning.

This is fine for interpolation, but the separation claim ("These are separate concerns with separate namespaces") is weakened by the merge. If they're truly separate, evidence keys shouldn't shadow variables in interpolation. If shadowing is intentional (an agent can override a variable value via evidence), document that explicitly as a feature.

**A3. Command gate CWD is "project root" but not defined**

The design says command gates run from "the project root directory." In the current implementation, there's no concept of project root -- the CLI uses CWD. The design should specify how project root is determined (git root? directory containing `.koto/`? CWD at `koto init` time stored in state file?). This affects reproducibility of command gates across sessions where CWD might differ.

**A4. `koto template` subcommand is a new CLI surface level**

The current CLI uses flat subcommands (`init`, `transition`, `next`, etc.). The design introduces `koto template compile`, `koto template validate`, `koto template lint`, `koto template new` -- a nested subcommand pattern. The existing `koto validate` command (`/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:42`) validates a state file against its template. The new `koto template validate` validates a source template.

These are different operations, but the naming overlap (`validate` at two levels) will confuse users. Consider renaming the existing `koto validate` to `koto check` or folding it into `koto query --check-hash`, and reserving `validate` for template validation. This is advisory because the user base is currently zero and the CLI surface can change freely.

**A5. `ParseJSON` and the existing `Parse` will coexist in `pkg/template/`**

The design adds `ParseJSON([]byte) (*Template, error)` to `pkg/template/` alongside the existing `Parse(path string) (*Template, error)`. The existing `Parse` reads the legacy format (flat YAML + `**Transitions**:` in body). The new `ParseJSON` reads compiled JSON. Phase 5 adds backward-compatible detection.

During the transition, `pkg/template/` will have three parsing paths: legacy source, new source (via compiler), and compiled JSON. Make sure the package API makes it clear which one callers should use. Consider: `ParseJSON` in `pkg/template/`, `CompileSource` in `pkg/template/compile/`, and deprecation annotation on the existing `Parse`.

### Strengths

**S1. Source/compiled separation is architecturally clean.** The go-yaml dependency is confined to `pkg/template/compile/`, not the engine. The engine reads compiled JSON via stdlib. This preserves the zero-dependency guarantee for the core package. The design correctly identifies this as the key structural constraint and never violates it.

**S2. Evidence gate design integrates with the existing engine API without breaking it.** The variadic `TransitionOption` pattern (`Transition(target, ...TransitionOption)`) is backward-compatible -- existing callers continue to work. The `WithEvidence` option is a clean extension point. The "evidence persists across rewind" decision is the simplest correct model and avoids complex cleanup logic.

**S3. Gate evaluation scope is clear.** "Gates check the evidence map only, not the merged variables+evidence context" prevents a class of bugs where template authors accidentally rely on variable values for gate satisfaction. Evidence and variables serve different purposes and the design keeps them separate at the evaluation layer.

**S4. The phasing correctly sequences compiled format before compiler.** Phase 1 (JSON parsing, engine integration) can be built and tested with hand-written JSON. Phase 2 (compiler) produces that JSON from source. This means the engine's contract is verified before the compiler exists. Correct dependency ordering.

**S5. The security analysis is practical.** Command gates as shell execution with the same trust model as Makefiles is the right framing. No variable interpolation in command strings prevents injection. The 30-second default timeout prevents indefinite blocking. These are concrete mitigations, not theater.
