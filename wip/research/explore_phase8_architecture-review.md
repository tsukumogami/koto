# Architecture Review: DESIGN-koto-agent-integration

**Reviewer**: architect-reviewer
**Date**: 2026-02-23
**Document**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/docs/designs/DESIGN-koto-agent-integration.md`
**Codebase state**: koto v0.1.x, post CLI tooling design (issues #19-#22 done)

## Summary

The design proposes two capabilities: (1) `koto generate <platform>` to scaffold agent integration files from an existing template, and (2) a Stop hook for session continuity. It adds one new package (`pkg/generate/`) and extends the CLI. The architecture fits the existing codebase well. There are two blocking findings (both about the generated hook command not matching the actual CLI output) and four advisory findings.

---

## 1. Is the architecture clear enough to implement?

**Yes, with two gaps that need resolution before implementation starts.**

The design is concrete about what to build: `pkg/generate/` with two generation targets, surfaced through `koto generate <platform>`. The data flow diagrams match the existing CLI patterns. The decision to make the agent skill the distribution unit (rather than building search paths, go:embed, or a registry) is sound and eliminates infrastructure that koto doesn't need.

### Gap 1: TemplateMetadata extraction source is underspecified

The design defines a `TemplateMetadata` struct with `States []StateInfo`, `Variables []VariableInfo`, etc. It says the generator "compiles the template, extracts metadata" but doesn't specify whether metadata comes from the compiled `CompiledTemplate` or a new parsing path.

Looking at the existing code:
- `template.CompiledTemplate` (`pkg/template/compiled.go:13-21`) has `States map[string]StateDecl` and `Variables map[string]VariableDecl`
- `template.StateDecl` (`pkg/template/compiled.go:33-38`) has `Gates map[string]engine.GateDecl`, `Transitions []string`, `Terminal bool`
- `template.VariableDecl` (`pkg/template/compiled.go:25-29`) has `Description`, `Required`, `Default`

All the information `TemplateMetadata` needs is already in `CompiledTemplate`. The `TemplateMetadata` type hierarchy (`StateInfo`, `GateInfo`, `VariableInfo`) mirrors existing types with different names. This creates a schema-drift risk: if `GateDecl` gains a new field, `GateInfo` must be updated separately.

**Recommendation**: Use `CompiledTemplate` directly in the generator. Add projection helpers only if the generator needs computed fields not on `CompiledTemplate`. Don't define `TemplateMetadata` as a parallel public type speculatively.

### Gap 2: Hook merge semantics specified in prose but not concretely

The design says "merges hook entries (replace koto's entry, preserve others)" for `.claude/hooks.json`. It doesn't specify:
- How to identify "koto's entry" in the Stop hook array (command substring match? metadata field?)
- What happens when `hooks.json` doesn't exist, exists but isn't valid JSON, or has a `Stop` key that isn't an array

**Recommendation**: Match on the command string containing `koto workflows`. Document this convention in a code comment. Handle edge cases: missing file = create fresh, invalid JSON = error and abort, wrong type for `Stop` = error and abort.

---

## 2. Are there missing components or interfaces?

### 2a. Stop hook command doesn't match actual `koto workflows` output (Blocking)

The generated hook:
```sh
koto workflows --json 2>/dev/null | grep -q '"active":\[\]' || echo 'Active koto workflow detected.'
```

Current `cmdWorkflows` (`cmd/koto/main.go:475-492`) outputs a JSON **array** from `discover.Find()`, which returns `[]discover.Workflow`:
```json
[{"path":"wip/koto-foo.state.json","name":"foo","current_state":"planning",...}]
```

There's no `"active"` key in the output. The grep pattern `'"active":\[\]'` would never match. Additionally, the logic is inverted: `grep -q '"active":\[\]'` succeeds when the active list is *empty*, so `|| echo` fires when `grep` fails (no match = active list is not empty). But since the pattern can never match, the hook would always fire.

**Recommendation**: Since `koto workflows` already outputs JSON, drop `--json` and use:
```sh
koto workflows 2>/dev/null | grep -q '"current_state"' && echo 'Active koto workflow detected. Run koto next to continue.'
```

This checks if any state file metadata is in the output. Empty output (`[]`) has no `"current_state"`, so the warning only fires when workflows exist.

### 2b. `--json` flag cannot work with the current CLI flag parser (Blocking)

The hook command uses `koto workflows --json`. Looking at `parseFlags` (`cmd/koto/main.go:77-108`):

```go
func parseFlags(args []string, multiFlags map[string]bool) (*parsedArgs, error) {
    // ...
    if i+1 >= len(args) {
        return nil, fmt.Errorf("%s requires a value", arg)
    }
    next := args[i+1]
    if isFlag(next) {
        return nil, fmt.Errorf("%s requires a value", arg)
    }
    // ...
}
```

Every flag requires a value argument. `--json` alone would return `"--json requires a value"`. There's no boolean flag support.

**Recommendation**: Don't add `--json`. `koto workflows` already outputs JSON. The hook command should use `koto workflows` without `--json`.

### 2c. `koto transition --evidence` not yet implemented but referenced in execution loop (Advisory)

The design's execution loop (step 6) shows:
```
koto transition <target> --evidence key=value
```

The current `cmdTransition` doesn't handle `--evidence`. This was explicitly deferred in the CLI tooling design. Agents following the generated SKILL.md would fail when trying to supply evidence.

The design doesn't claim to implement `--evidence`, but the generated skill file would contain instructions to use it.

**Recommendation**: Either (a) implement `--evidence` as a prerequisite for Phase 2, or (b) Phase 2's generated SKILL.md omits evidence instructions and documents evidence support as "coming soon." The design should explicitly call out this dependency.

### 2d. `koto generate` subcommand fits the existing CLI dispatch pattern (No issue)

The current CLI uses `cmdTemplate` as a nested subcommand dispatcher. `koto generate` follows the identical pattern:

```go
case "generate":
    err = cmdGenerate(os.Args[2:])
```

with `cmdGenerate` dispatching to `cmdGenerateClaudeCode` and `cmdGenerateAgentsMD`. Clean fit.

---

## 3. Are the implementation phases correctly sequenced?

**Correct, with one implicit dependency.**

- **Phase 1 (Metadata Extraction)**: Depends on `pkg/template/compile` (exists). Clean foundation. No blockers.
- **Phase 2 (Claude Code Generation)**: Depends on Phase 1. Also implicitly depends on:
  - The hook command matching actual CLI output (findings 2a/2b, must be resolved first)
  - `--evidence` flag existence (finding 2c, can be deferred if SKILL.md is scoped accordingly)
- **Phase 3 (AGENTS.md Generation)**: Depends on Phase 1. Independent of Phase 2. Could run in parallel.

The phases are correctly ordered: extract metadata first, then use it to generate files. The missing explicit dependency is that Phase 2's hook generation requires fixing the `koto workflows` output assumption.

**Recommendation**: Add a Phase 0 prerequisite: resolve the hook command to match the actual `koto workflows` output format. This is a design-level fix, not a code change.

---

## 4. Does the proposed package structure fit the existing codebase?

**Good fit.**

### Dependency direction: correct

```
cmd/koto/main.go
  -> pkg/generate/           (NEW: CLI imports generate)
  -> pkg/template/compile/   (existing)

pkg/generate/
  -> pkg/template/           (reads CompiledTemplate -- correct downward dependency)
  -> pkg/template/compile/   (compiles source templates -- correct)
```

No upward dependencies. No circular dependencies. `generate` sits at the same level as `controller` -- both consume template/engine output.

### `pkg/generate/` vs `internal/generate/`

The design places the package under `pkg/`, consistent with all other koto packages (`pkg/engine/`, `pkg/template/`, `pkg/controller/`, `pkg/cache/`, `pkg/discover/`). The only `internal/` package is `internal/buildinfo/`.

If the generator is CLI-only (unlikely to be imported by external Go code), `internal/` would be more appropriate. But given the existing convention of everything under `pkg/`, this is consistent. Not worth changing.

### File organization: clean

```
pkg/generate/
  generate.go     -- shared metadata extraction
  claudecode.go   -- Claude Code target
  agentsmd.go     -- AGENTS.md target
```

Adding a new target (e.g., `cursor.go`) follows the pattern without touching existing files.

### No new external dependencies

The generate package uses `pkg/template/compile` (which uses `gopkg.in/yaml.v3`), `encoding/json` (for hooks.json merge), and stdlib filesystem ops. No new external deps. Matches koto's constraint.

---

## 5. Is the design appropriately minimal?

**Yes. The design does one thing (generate scaffold files) and avoids building infrastructure for hypothetical needs.**

### What the design correctly avoids:

1. **Template search paths**: Not needed; skills carry their template.
2. **Template registry**: Not needed; git is the distribution mechanism.
3. **go:embed templates**: Not needed; templates live in the project, not the binary.
4. **Auto-sync for stale generated files**: Deferred to hypothetical `koto doctor`. Version headers suffice.
5. **Complex hook protocol**: Single shell command, not a daemon.

### One area of potential over-engineering: `TemplateMetadata` type hierarchy

As noted in Gap 1, the design defines `TemplateMetadata`, `StateInfo`, `GateInfo`, and `VariableInfo` as new public types that mirror `CompiledTemplate`, `StateDecl`, `GateDecl`, and `VariableDecl`. Comparing the types:

| Design type | Existing type | Missing fields |
|---|---|---|
| `StateInfo.Name` | (map key in `CompiledTemplate.States`) | None |
| `StateInfo.Terminal` | `StateDecl.Terminal` | None |
| `StateInfo.Transitions` | `StateDecl.Transitions` | None |
| `StateInfo.Gates []GateInfo` | `StateDecl.Gates map[string]engine.GateDecl` | None |
| `GateInfo.Name` | (map key in `StateDecl.Gates`) | None |
| `GateInfo.Type` | `GateDecl.Type` | None |
| `GateInfo.Field` | `GateDecl.Field` | None |
| `VariableInfo.Name` | (map key in `CompiledTemplate.Variables`) | None |
| `VariableInfo.Description` | `VariableDecl.Description` | None |
| `VariableInfo.Required` | `VariableDecl.Required` | None |
| `VariableInfo.Default` | `VariableDecl.Default` | None |

Every field in the new types has a direct counterpart. The only structural difference is converting map keys into struct fields (`.Name`). This doesn't warrant parallel public types.

**Recommendation**: Use unexported helper functions that iterate over `CompiledTemplate.States` and `CompiledTemplate.Variables` maps directly when generating output. If a name-carrying struct is needed internally, make it unexported.

---

## 6. Additional findings

### 6a. Symlink protection on template copy (Advisory)

The design mentions the template copy uses "the same symlink protection as engine state file writes." The engine's check is in `atomicWrite()` (`pkg/engine/engine.go:500-503`), which is unexported. The generate package would need to duplicate the 3-line Lstat check. Acceptable duplication for now; not worth extracting a shared utility for one callsite.

### 6b. `--dry-run` flag introduces boolean flag pattern (Advisory)

The current `parseFlags` requires every flag to have a value. `--dry-run` is boolean. The implementer needs to either:
- Add boolean flag support to `parseFlags` (small change, benefits future boolean flags)
- Check for `--dry-run` before calling `parseFlags` (special case, simple)
- Treat it as `--dry-run true` (awkward for users)

**Recommendation**: Add boolean flag support to `parseFlags` by accepting a set of known boolean flag names. Small change with a clean pattern.

### 6c. Version header access (No issue)

The design's `Generator` struct has `KotoVersion string`, set by the CLI from `internal/buildinfo.Version()`. This correctly keeps `internal/buildinfo` out of `pkg/generate/` by passing the value through the CLI layer.

### 6d. The design correctly simplifies compared to its predecessor

The previous iteration of this design (visible in the prior version of this review file) proposed embedded templates, a registry package, search paths, and `go:embed` extraction. The current design eliminates all of that in favor of "the skill file is the distribution unit." This is a significant improvement: it removes two proposed packages (`pkg/registry/`, `pkg/templates/`), eliminates the `$KOTO_HOME/templates/<version>/` extraction path, and eliminates the `koto workflows` output format change. The reduced scope is appropriate.

---

## Findings summary

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 2a | Hook grep pattern doesn't match actual `koto workflows` JSON array output | Blocking | Fix hook command to check for `"current_state"` in array output |
| 2b | `--json` flag doesn't exist and can't work with current flag parser | Blocking | Drop `--json` from hook command; workflows already outputs JSON |
| 2c | `--evidence` flag not yet implemented but referenced in generated execution loop | Advisory | Implement before Phase 2, or omit from first-version SKILL.md |
| 1a | `TemplateMetadata` type hierarchy duplicates `CompiledTemplate` fields | Advisory | Use `CompiledTemplate` directly; add projection types only if needed |
| 6b | `--dry-run` is a boolean flag in a key-value-only parser | Advisory | Add boolean flag support or special-case before parseFlags |
| Gap 2 | Hook merge edge cases unspecified | Advisory | Document handling of missing/invalid hooks.json |

## Overall assessment

The design is structurally sound. It adds one new package at the correct level in the dependency graph, follows the existing CLI dispatch pattern, introduces no new external dependencies, and avoids building infrastructure koto doesn't need. The decision to make the agent skill the distribution unit is well-reasoned and eliminates complexity from the prior design iteration.

The two blocking findings are about the generated Stop hook command not matching the actual `koto workflows` CLI output. These are specification errors in the generated content, not architectural problems. They're straightforward to fix in the design document before implementation begins.

The biggest architectural risk is defining a parallel `TemplateMetadata` type hierarchy that drifts from `CompiledTemplate`. Using `CompiledTemplate` directly eliminates this risk.

The phase sequencing works, with the caveat that the generated SKILL.md will reference `koto transition --evidence` before that flag exists. This should be an explicit decision: either add evidence support as a prerequisite or scope it out of first-version generation.
