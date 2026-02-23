# Architecture Review: DESIGN-koto-agent-integration.md

**Reviewer**: architect-reviewer
**Date**: 2026-02-23
**Design**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/docs/designs/DESIGN-koto-agent-integration.md`
**Scope**: Solution Architecture, Implementation Approach, Consequences

## Summary

The design proposes three capabilities: (1) embedded template registry with search path, (2) `koto generate` for platform-specific agent integration files, (3) extended `koto workflows` discovery. The architecture is mostly sound and fits the existing codebase patterns. There are five findings: two blocking, three advisory.

---

## Finding 1: `pkg/registry/` duplicates home directory resolution from `pkg/cache/`

**Severity**: Blocking (parallel pattern)

The design introduces `pkg/registry/` with its own `~/.koto/templates/` resolution and `$KOTO_HOME` handling. The codebase already has this pattern in `pkg/cache/cache.go:17-26`:

```go
func cacheDir() (string, error) {
    if kotoHome := os.Getenv("KOTO_HOME"); kotoHome != "" {
        return filepath.Join(kotoHome, "cache"), nil
    }
    home, err := os.UserHomeDir()
    if err != nil {
        return "", fmt.Errorf("resolve home directory: %w", err)
    }
    return filepath.Join(home, ".koto", "cache"), nil
}
```

`pkg/registry/extract.go` will need identical logic for `~/.koto/templates/<version>/`. When `$KOTO_HOME` semantics change (or a `--koto-home` flag is added), both packages need updating.

**Recommendation**: Extract a shared `kotoHome()` function (or small `pkg/paths/` package) that returns the root `~/.koto` (or `$KOTO_HOME`) path. Both `cache` and `registry` derive their subdirectories from it. This is a small change but prevents the pattern from being copied a third time when `koto doctor` or template cleanup is added.

---

## Finding 2: `pkg/generate/` imports `pkg/registry/` but the dependency direction is unclear in the design

**Severity**: Advisory

The design shows `Generator` holding a `*registry.Registry` field. This is fine for the current proposal. But the generated skill file content needs to include template metadata (names, descriptions, state lists), which means `generate` depends on both `registry` and the `template` package for parsing frontmatter.

The dependency chain would be: `generate -> registry -> template -> engine`. This is a clean downward flow, consistent with the existing pattern (`controller -> template -> engine`). No issue with the proposed direction.

However, the design doesn't show how `generate` gets state machine information for templates. The `TemplateInfo` struct has `Description` and `Name` but no `States` field. The skill file is supposed to include "the template's state machine description so the agent understands the workflow structure." To get state lists, `generate` would need to parse/compile each template, which means it depends on `compile` (and transitively on `yaml.v3`).

**Recommendation**: Add a `States []string` field to `TemplateInfo` (the design's JSON for `koto workflows` already shows it). Document that `Registry.List()` compiles each template to extract state names. This makes the dependency on `compile` explicit in the registry, not hidden in generate.

---

## Finding 3: `koto workflows` output format changes break the existing JSON contract

**Severity**: Blocking (state contract violation)

Currently `koto workflows` (line 475-492 in `cmd/koto/main.go`) returns:

```go
func cmdWorkflows(args []string) error {
    // ...
    workflows, err := discover.Find(stateDir)
    // ...
    return printJSON(workflows)
}
```

This outputs a JSON array:
```json
[{"path":"...", "name":"...", "current_state":"...", ...}]
```

The design proposes changing it to an object:
```json
{"templates": [...], "active": [...]}
```

This is a breaking change to the JSON output that `koto workflows` currently produces. Any agent or script parsing the array format breaks silently -- they get an object where they expect an array. The design says "Preserve backward compatibility: existing JSON output format for `active` is unchanged" but this is incorrect: the top-level shape changes from array to object.

**Recommendation**: One of two approaches:

(a) **Version the output.** Add `--format v2` or just accept the break since koto is pre-1.0. If going this route, document it as a breaking change in the release notes and bump the minor version.

(b) **Separate commands.** Keep `koto workflows` returning the existing array format. Add `koto workflows --all` (or `koto discovery --json`) for the combined templates + active output. This is slightly more complex but avoids breaking existing consumers.

Option (a) is simpler and acceptable for a pre-1.0 project. But the design must acknowledge the break explicitly rather than claiming backward compatibility.

---

## Finding 4: Template extraction to `~/.koto/templates/<version>/` creates a second path-resolution concern for `loadTemplateFromState`

**Severity**: Advisory

When `koto init --template quick-task` extracts to `~/.koto/templates/v0.2.0/quick-task.md` and stores that absolute path in the state file, the existing `loadTemplateFromState()` function (line 598-679 in `main.go`) will read from that path on every `koto next` / `koto transition`. This works correctly -- the path is stable and versioned.

However, if the user deletes `~/.koto/` or changes `$KOTO_HOME` between init and next, the absolute path breaks. The design mentions this implicitly ("Configurable via `$KOTO_HOME`") but doesn't address what happens when the env var changes mid-workflow.

This is not a new problem -- explicit `--template /some/path` has the same risk. The design doesn't make it worse. But it's worth noting in the Uncertainties section because the built-in template path is less obvious to users than an explicit path they chose.

**Recommendation**: No code change needed. Add a sentence to the Uncertainties section noting that `$KOTO_HOME` changes mid-workflow will cause "template file not found" errors, and the fix is to either set `$KOTO_HOME` back or re-init the workflow.

---

## Finding 5: Hook implementation in the design contradicts itself

**Severity**: Advisory

The design shows two different hook implementations:

In the "Generated File Content" section (line 437):
```json
{
  "hooks": {
    "Stop": [{
      "type": "command",
      "command": "koto workflows --json 2>/dev/null | grep -q '\"active\":\\[\\]' || echo 'Active koto workflow detected.'"
    }]
  }
}
```

But in the Security section (line 494):
> The generated Claude Code hook runs a shell command (`ls wip/koto-*.state.json`) on every Stop event.

These describe different detection mechanisms. The `koto workflows --json` approach is better because it works regardless of `--state-dir` configuration (as the design itself argues at line 443). The `ls wip/koto-*.state.json` approach hardcodes the default state directory.

Additionally, the `grep -q '"active":\[\]'` pattern will break if the JSON is pretty-printed or if there are spaces in the serialization. A more reliable approach:

```sh
koto workflows --json 2>/dev/null | grep -q '"active":\s*\[\s*\]' || echo '...'
```

Or even better, since `koto` controls its own output format, add a dedicated exit code: `koto workflows --check-active` returns 0 if active workflows exist, 1 if not. This avoids fragile grep parsing.

**Recommendation**: Make the Security section consistent with the hook definition. Consider whether `koto workflows` should have a `--check-active` flag that returns a non-zero exit code when no active workflows exist, eliminating the grep.

---

## Question 1: Is the architecture clear enough to implement?

**Yes, with caveats.** The three components (registry, generate, discovery) are well-defined and the interfaces are concrete enough to code against. The `Registry` struct and `Generator` struct have clear methods and return types.

Two gaps need clarification before implementation:

1. **How does `Registry.List()` get state names from templates?** The design shows state names in the `koto workflows` JSON output but `TemplateInfo` doesn't include them. Getting states requires compilation, which requires reading the template source. The design should specify whether `List()` does a full compile or reads a cached compiled form.

2. **What happens when `koto generate` runs but no templates are discoverable?** The design doesn't specify the error case. Should it generate a skill file with an empty templates section, or error out?

---

## Question 2: Are there missing components or interfaces?

One missing component: **a shared `kotoHome` resolver.** The `pkg/cache/` package already has `cacheDir()` with `$KOTO_HOME` logic. The new `pkg/registry/` will duplicate this. A small shared package (even just a `pkg/paths/koto_home.go` file) prevents the parallel pattern.

One missing interface detail: **the `Registry.Resolve()` method's extraction behavior.** The design says "koto extracts the embedded template to a versioned location on first use" but the `Resolve()` signature just returns `(string, error)`. The extraction side-effect should be documented on the method. Callers need to know that `Resolve` may write to the filesystem.

---

## Question 3: Are the implementation phases correctly sequenced?

**Mostly yes.** Phase 1 (registry) must come before Phase 2 (generate) because generate needs registry to enumerate templates. Phase 3 (discovery extension) is independent of Phase 2 and could be done in parallel.

**Phase 4 (quick-task template) should be Phase 1.** The design says Phase 1 creates the registry and embeds the template. But the template content needs to exist to embed it. Writing the quick-task template is a prerequisite for the `go:embed` directive. Phase 4 should be the first thing done, or folded into Phase 1.

Recommended order:
1. Write the quick-task template (current Phase 4, moved up)
2. Template registry and search path (current Phase 1, including `go:embed` of the template)
3. Extended discovery (current Phase 3) -- can start in parallel with Phase 2
4. Integration file generation (current Phase 2)

---

## Question 4: Are there simpler alternatives we overlooked?

**For template distribution: the current approach is appropriate.** `go:embed` is the simplest way to ship templates with zero runtime cost and no network dependency. The search path (project -> user -> built-in) follows standard conventions (like `git config` or `npm` resolution).

**For agent integration: consider a single-file approach first.** The design generates three files for Claude Code (skill, command, hook). The command file (`koto-run.md`) duplicates what the skill file already teaches the agent. An agent that reads the skill file already knows how to run `koto init` + `koto next`. The slash command adds convenience but also adds a file to maintain.

Simpler alternative: start with skill file + hook only. Add the command file later if users request it. This reduces the generated surface area from 3 files to 2.

**For discovery: the combined endpoint is the right call.** Requiring agents to make two calls (`koto template list --json` + `koto workflows --json`) when one suffices adds friction for no benefit. The combined response is correct.

---

## Question 5: Does the proposed package structure fit the existing codebase?

**Yes, with one adjustment.** The existing structure:

```
cmd/koto/main.go
internal/buildinfo/
pkg/cache/
pkg/controller/
pkg/discover/
pkg/engine/
pkg/template/
pkg/template/compile/
```

The proposed additions:
```
pkg/registry/       # NEW
pkg/generate/       # NEW
pkg/templates/      # NEW (embedded files)
```

**`pkg/templates/` (embedded template files) should be `pkg/registry/templates/` or just embedded in `pkg/registry/`.** Having a `pkg/templates/` alongside `pkg/template/` is confusing. One is a parser package, the other is raw file storage. Nesting the embedded templates under `pkg/registry/` makes the relationship clear: the registry owns both the resolution logic and the built-in template files.

The dependency graph after the change:

```
cmd/koto/main.go
  -> pkg/registry      (template resolution, embeds built-in templates)
  -> pkg/generate      (integration file generation)
  -> pkg/controller    (directive generation)
  -> pkg/discover      (state file scanning)
  -> pkg/cache         (compilation cache)
  -> pkg/engine        (state machine core)
  -> pkg/template      (template parsing, compilation)
  -> internal/buildinfo

generate -> registry -> template/compile -> template -> engine
                     -> cache (for $KOTO_HOME path resolution, shared)
controller -> template -> engine
discover -> engine (types only, for JSON tag alignment)
```

No circular dependencies. All new packages flow downward. The only concern is the `pkg/templates/` vs `pkg/template/` naming collision, which is easily resolved by nesting.

---

## Architecture Fit Summary

| Aspect | Assessment |
|--------|-----------|
| Package structure | Fits existing layout; rename `pkg/templates/` to avoid collision with `pkg/template/` |
| Dependency direction | Clean downward flow; no inversions |
| CLI surface | `koto generate` is a new subcommand (slot already exists in the dispatch switch); `koto template list` extends existing `koto template` subcommand |
| State contract | No new state fields; existing state file format unchanged |
| Zero-dependency constraint | `go:embed` is stdlib; no new external deps. `yaml.v3` already present for `compile` |
| Parallel patterns | Home directory resolution duplicated between `cache` and `registry` -- extract shared helper |

The design is implementable and architecturally sound. The two blocking findings (home directory duplication, workflows JSON contract break) are straightforward to address before implementation begins.
