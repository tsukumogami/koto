# Architecture Review: koto CLI and Template Tooling Design

## Review Scope

Reviewed the CLI tooling design (`DESIGN-koto-cli-tooling.md`) against its upstream dependency (`DESIGN-koto-template-format.md`) and the existing codebase (`cmd/koto/main.go`, `pkg/template/`, `pkg/engine/`, `pkg/controller/`).

## Question 1: Is the architecture clear enough to implement?

**Yes, with minor gaps.** The design is unusually precise for a pre-implementation artifact. The compilation flow (steps 1-7 in the `koto init` section), evidence parsing rules, search path resolution order, and subcommand signatures are all specified with enough detail to write code against. Error messages are specified verbatim.

### What's clear

- The init flow: resolve -> read -> compile -> validate -> build machine -> hash -> init engine. Each step maps to an existing function or a clearly defined new one.
- Evidence parsing: the four cases (`key=value`, `key=`, `key`, `=value`) are enumerated with expected behavior.
- Search path precedence: three levels, first match wins, `.md` auto-append.
- The `template` subcommand group and its three subcommands (`compile`, `inspect`, `list`).

### What's not clear enough

1. **`resolveTemplatePath()` path detection heuristic.** The design says: "Contains `/` or `.` with extension `.md` -> treat as file path." But the Solution Architecture section says: "Contains `/` or ends in `.md`." These are different. The string `my-template.md` contains a `.` but doesn't contain `/`, so the first formulation treats it as a path (because it has `.` with extension `.md`), while the second formulation also treats it as a path (ends in `.md`). They happen to agree for `.md` files, but the first formulation would also match `templates.v2/quick-task` (contains `.` but no `.md` extension). The second formulation (contains `/` or ends in `.md`) is simpler and unambiguous. **Recommendation: use the Solution Architecture wording consistently.**

2. **`koto template inspect` output format.** The example output shows fields like `Gates:` with a compact representation (`assess/task_defined (field_not_empty)`), but the exact formatting rules aren't specified. How are command gates displayed? Do they show the full command string? What about timeouts? This is a minor detail that can be resolved at implementation time, but it's worth calling out.

3. **`loadTemplateFromState()` migration path.** The design says `cmdInit` switches to the compiler path, but doesn't specify whether `loadTemplateFromState()` also changes. Currently this function calls `template.Parse()` (the legacy parser). After the design is implemented, workflows initialized with the new compiler path will have templates that might not parse correctly with the legacy `Parse()` function (they use YAML frontmatter with nested `states:` blocks that the legacy parser can't handle). **This is a functional gap** -- after Phase 1 ships, `cmdTransition`, `cmdNext`, `cmdQuery`, etc. will still call `loadTemplateFromState()`, which uses `Parse()`. If the template uses gates or nested YAML, those commands will break.

4. **Hash compatibility.** The legacy `Parse()` hashes the raw file content (`sha256 of data`). The compiler hashes the compiled JSON output (`compile.Hash()`). These produce different hashes for the same source file. The design mentions this in the `koto validate` section (Phase 3: "Updated to use compiler path"), but doesn't address the transition for `cmdTransition` and `cmdRewind`, which check hashes in Phase 1. **If `cmdInit` uses the compiler hash but `cmdTransition` still uses the legacy hash, every transition will fail with `template_mismatch`.**

## Question 2: Are there missing components or interfaces?

### Missing: `loadTemplateFromState()` migration

This is the most significant gap. The function currently:
1. Reads the state file to get `template_path`
2. Calls `template.Parse(path)` to get a `Template`
3. Returns the template and stored hash

After Phase 1, `cmdInit` will store a compiler-produced hash (hash of compiled JSON). But `loadTemplateFromState()` will still call `Parse()`, which computes its hash from the raw source file. The hashes won't match.

**Required change:** `loadTemplateFromState()` must also switch to the compiler path. It needs to:
1. Read the source file
2. Compile it with `compile.Compile()`
3. Hash the compiled output with `compile.Hash()`
4. Build a `Template`-equivalent from the `CompiledTemplate`

This means the `Template` struct or its replacement needs to carry the same information the controller needs (`Sections` for directive interpolation). The `CompiledTemplate` already has this (each `StateDecl.Directive` contains the directive text), but the controller currently expects a `*template.Template` with a `Sections` map.

### Missing: Controller adaptation

The `controller.Controller` takes a `*template.Template`. After migration, the compilation path produces a `*template.CompiledTemplate`. The controller needs to work with the compiled template's `StateDecl.Directive` field instead of `Template.Sections`.

Options:
- Create an adapter that builds a `Template` from a `CompiledTemplate` (adding a `Sections` map from the `StateDecl.Directive` fields).
- Change the controller to accept either type (or an interface).
- Add a `BuildTemplate()` method to `CompiledTemplate` that returns a `*Template`.

The simplest approach is option 3: `CompiledTemplate.BuildTemplate()` that constructs a `Template` with populated `Sections`, `Machine` (via `BuildMachine()`), `Hash`, and metadata. This keeps the controller unchanged.

### Missing: `--evidence` integration with `loadTemplateFromState`

The design adds `--evidence` to `cmdTransition`, but the current `cmdTransition` calls `eng.Transition(target)` without options. This is straightforward -- just parse the evidence flags and pass `WithEvidence(evidence)` -- but the flag needs to be registered in `parseFlags()` as a multi-value flag (like `--var`).

### Present and adequate

- `pkg/resolve/resolve.go` is specified with clear function signatures.
- The `template` subcommand dispatcher pattern is straightforward (add a case in `main()` for "template" that dispatches to `os.Args[2]`).
- Evidence parsing is fully specified.

## Question 3: Are the implementation phases correctly sequenced?

**Mostly, but Phase 1 has an internal ordering issue.**

### Phase sequence

| Phase | Content | Dependencies |
|-------|---------|--------------|
| 1 | Core CLI changes (init compiler path, evidence flag, search path) | Requires compiler + engine evidence (both done) |
| 2 | Template subcommands (compile, inspect, list) | Requires Phase 1 (search path) |
| 3 | Cleanup (validate migration, Parse() deprecation, integration tests) | Requires Phase 1 + 2 |

The phase ordering is correct at the macro level. Phase 2 depends on Phase 1 (search path logic is shared). Phase 3 is cleanup that depends on both.

### Internal ordering problem in Phase 1

Phase 1 lists:
1. Modify `cmdInit` to use compile path
2. Add `--evidence` flag to `cmdTransition`
3. Add `resolveTemplatePath()` with search path logic
4. Create `pkg/resolve/` package

Items 1 and 2 are independent and could be done in parallel. But item 1 creates the hash incompatibility described above -- if `cmdInit` switches to the compiler hash but other commands (`transition`, `next`, `query`, etc.) still use the legacy parser, the CLI breaks.

**Recommendation: Phase 1 should be split into two sub-phases:**
- Phase 1a: Migrate `loadTemplateFromState()` and all callers to the compiler path. Migrate `cmdInit` simultaneously. This keeps the hash scheme consistent.
- Phase 1b: Add `--evidence` flag to `cmdTransition`. Add search path resolution. Create `pkg/resolve/`.

Phase 1a is the breaking change. Phase 1b adds new features on a stable base.

### Evidence flag is low-risk

Adding `--evidence` to `cmdTransition` is isolated. The engine already has `WithEvidence()`. The only work is parsing flags and passing them through. This could ship independently of the init migration.

## Question 4: Are there simpler alternatives we overlooked?

### Alternative 1: Adapter function instead of new package

Instead of creating `pkg/resolve/resolve.go` as a separate package, the search path logic could live in `cmd/koto/main.go` as a `resolveTemplatePath()` function (which the design already mentions). The `Resolve()` function is only called from two places: `cmdInit` and `koto template list`. Creating a package for two call sites is reasonable if the logic is reusable, but the search path logic is CLI-specific (it uses git root detection, home directory expansion). It could also just be a function in `main.go`.

**Assessment:** A package is fine for testability. The function needs unit tests for the three-level search, and testing a package function is cleaner than testing an unexported function via integration tests. Keep the package.

### Alternative 2: Skip the `inspect` command

The `compile` command outputs JSON to stdout. Users can pipe to `jq` for ad-hoc queries, and the information shown by `inspect` is a subset of what's in the compiled JSON. The `inspect` command is convenience sugar.

**Assessment:** Worth keeping. The inspect output provides a quick overview without requiring `jq`. Template authors will use this often. It's also small to implement -- just format fields from the `CompiledTemplate`.

### Alternative 3: `BuildTemplate()` on `CompiledTemplate`

Rather than changing the controller, add a method to `CompiledTemplate` that builds a legacy `Template` struct. This avoids touching the controller entirely.

```go
func (ct *CompiledTemplate) BuildTemplate(path, hash string) *Template {
    sections := make(map[string]string, len(ct.States))
    for name, sd := range ct.States {
        sections[name] = sd.Directive
    }
    variables := make(map[string]string)
    for name, vd := range ct.Variables {
        variables[name] = vd.Default
    }
    return &Template{
        Name:        ct.Name,
        Version:     ct.Version,
        Description: ct.Description,
        Machine:     ct.BuildMachine(),
        Sections:    sections,
        Variables:   variables,
        Hash:        hash,
        Path:        path,
    }
}
```

**Assessment:** This is the simplest migration path. The controller stays unchanged. `loadTemplateFromState()` calls `compile.Compile()`, `compile.Hash()`, and `ct.BuildTemplate()`. The returned `*Template` is compatible with all existing callers.

### Alternative 4: Combine `loadTemplateFromState()` migration with `cmdInit` migration

Rather than treating them as separate tasks, change `loadTemplateFromState()` to use the compiler path at the same time as `cmdInit`. This avoids the hash incompatibility window entirely.

**Assessment:** This is the right approach. It's described in the Phase 1 resequencing recommendation above. The design doesn't call this out as a single atomic change, but it should be.

## Question 5: Does the design align with existing codebase patterns?

### Alignment: strong

**Flag parsing.** The design adds `--evidence` as a repeatable flag, matching the `--var` pattern exactly. The existing `parseFlags()` already supports multi-value flags via the `multiFlags` map parameter.

**Command dispatch.** The `template` subcommand adds a new case in `main()` that dispatches on `os.Args[2]`. This is a straightforward extension of the existing switch statement pattern. The dispatch is one level deeper (top-level command then subcommand), but the pattern is the same.

**Package structure.** `pkg/resolve/` follows the existing pattern of small, focused packages (`pkg/discover/`, `pkg/controller/`). Each package has a single responsibility.

**Error handling.** The design uses the existing `engine.TransitionError` type for gate failures and hash mismatches. This is consistent with how errors are handled throughout the CLI.

**JSON output.** The design's output format (`printJSON`) matches the existing pattern. `cmdInit` returns `{"state": ..., "path": ...}` today and the design doesn't change this.

### Misalignment: minor

1. **The test template in `main_test.go` uses the legacy format.** The `lifecycleTemplate` constant uses `**Transitions**:` lines in the body, which is the legacy parser format. After migration, this template would still parse correctly through the compiler (transitions come from YAML frontmatter only; `**Transitions**:` lines in the body are treated as directive content). But the tests won't exercise the new features (gates, nested YAML). **Recommendation: update the test template to use the new source format in Phase 1.**

2. **`loadTemplateFromState()` is not in the design's "Package Changes" section.** The design lists changes to `cmd/koto/main.go` and `pkg/template/template.go` but doesn't mention that `loadTemplateFromState()` needs to change. This is the gap identified in Question 2.

3. **The controller takes `*template.Template` but the new path produces `*template.CompiledTemplate`.** The design's Solution Architecture section describes how `cmdInit` builds a `Machine` from `CompiledTemplate.BuildMachine()`, but doesn't address how the controller (used by `cmdNext`) gets its `Template`. The `BuildTemplate()` adapter described in Alternative 3 resolves this.

## Summary of Key Findings

| Finding | Severity | Impact |
|---------|----------|--------|
| Hash incompatibility between legacy and compiler paths | High | `cmdTransition` will fail after `cmdInit` migration unless `loadTemplateFromState()` also migrates |
| `loadTemplateFromState()` not mentioned in Package Changes | High | All post-init commands break if not migrated together with `cmdInit` |
| Controller needs adapter from `CompiledTemplate` to `Template` | Medium | `cmdNext` breaks without a `BuildTemplate()` method or controller refactor |
| Phase 1 internal ordering needs sub-phases | Medium | Implementation could ship a broken intermediate state |
| Path detection heuristic inconsistency in design text | Low | Two slightly different descriptions of the same rule |
| `inspect` output format underspecified | Low | Resolvable at implementation time |
| Test templates need updating to new format | Low | Tests pass but don't exercise new features |

## Recommendations

1. **Add `CompiledTemplate.BuildTemplate()` method** before starting CLI migration. This is the bridge that makes the controller work with the compiler path.

2. **Migrate `loadTemplateFromState()` atomically with `cmdInit`.** These must change together or the CLI breaks. The design should specify this as a single work unit.

3. **Split Phase 1 into 1a (compilation path migration) and 1b (evidence flag + search path).** Phase 1a is the breaking internal change. Phase 1b adds new user-facing features.

4. **Standardize the path detection heuristic** to the Solution Architecture wording: "contains `/` or ends in `.md`."

5. **Update the test template** to use the new source format (YAML frontmatter with `states:` block, no `**Transitions**:` lines).

6. **Consider whether `--evidence` should work with `--state-dir` auto-selection.** The design doesn't say, but the existing patterns suggest it should (just like `--var` works with auto-selection).
