# Design Review: koto CLI and Template Tooling

**Reviewer**: Claude (explore phase 4)
**Document**: `docs/designs/DESIGN-koto-cli-tooling.md`
**Upstream**: `docs/designs/DESIGN-koto-template-format.md`
**Date**: 2026-02-22

---

## 1. Problem Statement Assessment

**Verdict: Specific enough, with one gap.**

The problem statement identifies three concrete gaps (no compilation path in CLI, no template discovery, no evidence from CLI) and grounds each in actual code (`pkg/template/compile.Compile()`, `--template <path>`, `WithEvidence()`). The gaps are observable -- you can verify each by looking at `cmd/koto/main.go`, where `cmdInit` calls the legacy `template.Parse()` and `cmdTransition` has no evidence support.

The statement also correctly scopes what it doesn't cover (LLM linting, registry, built-in templates, versioning). This is well-defined.

**Gap identified**: The problem statement focuses on gaps for *template authors and CLI users* but doesn't articulate a problem for the `loadTemplateFromState` path. Today, `loadTemplateFromState` (used by `transition`, `next`, `rewind`, `validate`, `cancel`) reads the template path from the state file and re-parses it with the legacy `Parse()`. The design's Solution Architecture section covers how `cmdInit` changes, and mentions `cmdValidate` updating to use the compiler path, but it's silent on how `loadTemplateFromState` migrates. This function is the primary consumer of `Parse()` across six commands -- more than `cmdInit`. The problem statement should call this out as a fourth gap, or the solution architecture should explicitly address it.

This matters because the hash stored at init time will change from the legacy hash (SHA-256 of raw file content, computed in `Parse()`) to the new hash (SHA-256 of compiled JSON, computed via `compile.Hash()`). Running `koto transition` against a workflow initialized with the old hash format and a CLI that now computes the new hash format will cause a spurious `template_mismatch` error. The design doesn't address this migration.

## 2. Missing Alternatives

### Decision 1 (Compilation Flow)

The three options (implicit, explicit-required, implicit-with-cache) cover the practical design space well. No significant alternatives are missing.

One minor consideration: **compile-on-write** (watch mode or git hook), where templates are compiled when saved. This is uncommon enough to not warrant inclusion -- it's more of a future workflow optimization than a design alternative.

### Decision 2 (Template Search Path)

**Missing alternative: XDG Base Directory Specification.**

The design chooses `~/.koto/templates/` for user-global templates. On Linux, the XDG standard says user config goes in `$XDG_CONFIG_HOME` (default `~/.config/`), user data in `$XDG_DATA_HOME` (default `~/.local/share/`). Templates are closer to data than config, so `~/.local/share/koto/templates/` would be the XDG-correct location. macOS has its own convention (`~/Library/Application Support/`).

This is worth mentioning as a considered alternative and rejected for simplicity. `~/.koto/` is simpler to discover, type, and document. But XDG compliance is a real expectation for some Linux users. The design should at least acknowledge the trade-off.

**Missing alternative: Template name with path separator for namespacing.**

What happens if a template name contains hyphens that look like directory separators? The design says a `/` triggers path mode, which is correct, but doesn't address whether `koto init --template myorg/quick-task` should look for `templates/myorg/quick-task.md` (subdirectory namespacing) or be treated as a path. The heuristic "contains `/`" would treat it as a path. This is probably fine, but the design should state that template names are flat (no directory nesting in search paths).

### Decision 3 (LLM Validation)

The two alternatives (lint stub, full linter) are well-chosen. No missing options.

### Decision 4 (Template Management Commands)

**Missing alternative: Cobra/pflag-style command framework.**

The current CLI in `main.go` uses hand-rolled argument parsing (`parseFlags`). Adding a `template` subcommand group with three sub-subcommands (`compile`, `inspect`, `list`) will strain this approach. The design doesn't mention whether the hand-rolled parser will be extended or whether this is the point to adopt a framework like Cobra. This is an implementation detail, not a design choice per se, but the design's feasibility depends on the parser being able to handle nested subcommands cleanly. Worth a brief note.

## 3. Rejection Rationale Assessment

### Decision 1: Explicit compilation required

> Rejected because it makes the simple case harder without clear benefit.

**Fair.** The rationale is accurate. Interactive workflow startup (`koto init`) is the dominant case, and adding a mandatory pre-step hurts it. The design correctly identifies that the explicit compile command exists for debugging, satisfying power users without burdening everyone.

### Decision 1: Implicit with cached `.compiled.json` files

> Rejected because the complexity isn't justified by the performance characteristics.

**Fair, and well-grounded.** Templates are kilobytes; compilation is sub-millisecond. The rationale correctly identifies filesystem side effects and cache invalidation as costs that don't pay for themselves. If templates ever grow complex enough to make compilation slow, the interface doesn't change -- caching can be added later.

### Decision 2: Configurable search path (environment variable)

> Rejected because the fixed three-level hierarchy covers all foreseeable cases without configuration burden.

**Mostly fair, slightly overstated.** "All foreseeable cases" is strong. There's at least one case where the fixed hierarchy falls short: monorepos with multiple projects that each have templates. If project A and project B are in the same git repo, the git root's `templates/` directory is shared. There's no project-scoped template isolation without explicit paths. This is a real scenario for koto's target audience (AI-assisted development in larger codebases). The rejection is still correct -- explicit paths handle it -- but the rationale could be more precise.

### Decision 2: Explicit paths only

> Rejected because discoverability matters for adoption.

**Fair.** Short and direct. The verbosity argument (`./templates/quick-task.md` vs `quick-task`) is genuine, and conventions for template location enable ecosystem growth.

### Decision 3: Design a lint command stub

> Rejected because compile already validates, and the LLM surface is the interesting part of linting.

**Fair.** Having both `compile` and `lint` that do the same thing (deterministic validation) is confusing. Saving the `lint` command name for when it actually does something distinct (LLM-assisted checks) is the right call.

### Decision 3: Full linter design

> Rejected because we'd be designing around unknowns.

**Fair.** The unknowns are genuine: which checks, what prompts, what models, what offline behavior. Deferring is appropriate.

### Decision 4: Flat commands

> Rejected because the top-level namespace should be reserved for workflow operations.

**Fair.** The existing top-level commands (`init`, `transition`, `next`, `query`, `status`, `rewind`, `cancel`, `validate`, `workflows`) are all workflow verbs. Template management is a different concern and deserves its own namespace.

**No strawmen detected.** Each rejected alternative is a real design option that someone might reasonably choose. The explicit-compilation-required option is the closest to a strawman (few CLI tools require pre-compilation for common operations), but it's a legitimate pattern used by Protocol Buffers, Terraform (with remote state), and others, so its inclusion is justified.

## 4. Unstated Assumptions

### A1: The legacy hash and the new hash are incompatible

The legacy `Parse()` computes `sha256:<hex>` of the **raw source file**. The new compiler computes `sha256:<hex>` of the **compiled JSON** (via `compile.Hash()`). These will produce different hashes for the same template.

The design says `koto validate` will "recompile the source template" and compare hashes. But what about workflows initialized with the old hash? Any workflow started before the CLI migration will have the old hash in its state file. After the CLI migration, `cmdTransition` (and `rewind`, `validate`) will compute the new hash. The hashes won't match. This creates a hard break for existing workflows.

**Recommendation**: Either (a) handle both hash formats during a transition period, (b) require a `koto migrate` command for existing workflows, or (c) document this as a breaking change. The design should state the choice explicitly.

### A2: Template path resolution works the same at init time and load time

At init time, `koto init --template quick-task` resolves the template and stores an absolute path in the state file. At load time (transition, next, etc.), the state file's `template_path` is used directly. But the design introduces template resolution for init, and `loadTemplateFromState` bypasses it entirely because it already has a path.

This means template search path resolution only runs at init time. That's probably correct, but it's unstated. It also means if a user moves a template after init, the absolute path in the state file becomes stale. The design should state that the template path is captured at init time and not re-resolved.

### A3: `--search-dir` on `template list` implies there's only one additional directory

The `--search-dir` flag on `koto template list` adds a single extra directory. The design doesn't say whether it's repeatable. Given the `--var` and the proposed `--evidence` flags are repeatable, the convention in koto is that repeatable flags are explicit. If `--search-dir` is a single-value flag, say so.

### A4: The `parseFlags` function can handle subcommands

The current CLI dispatches on `os.Args[1]`. Adding `koto template compile` means the CLI needs to handle `os.Args[1] == "template"` and then dispatch on `os.Args[2]`. The hand-rolled `parseFlags` function doesn't handle this nesting. This is an implementation detail, but the design should acknowledge that the command dispatching model needs extension, or it risks underestimating the implementation effort.

### A5: Template inspect reads source, not compiled

The design says `koto template inspect <path>` takes a `<path>`. But the output format shows information that comes from the YAML frontmatter (name, version, description, variables, gates). This means `inspect` compiles the source internally and then formats the result. It doesn't read pre-compiled JSON. This is probably right (source is the primary artifact), but the `<path>` argument should be documented as accepting source files, not compiled JSON.

### A6: `field_equals` with empty value in `--evidence`

The evidence parsing section says `key=` produces `{"key": ""}` (empty value, valid). But the compiled template's `ParseJSON` validation rejects `field_equals` gates where `gd.Value == ""`. This means a gate checking for an empty string is impossible. The CLI's evidence parsing supports empty values, but the template format doesn't support checking for them. This is a minor inconsistency that doesn't originate in this design (it's in the upstream compiled template validation), but the CLI design should note it since it surfaces the `--evidence` format.

## 5. Strawman Analysis

**No options are strawmen.** Each alternative represents a genuine design trade-off:

- **Explicit compilation** is the standard in build systems (protobuf, Terraform). It's rejected for UX reasons, not because it's technically unsound.
- **Cached `.compiled.json`** is how many template engines work (Hugo, Jekyll). It's rejected because koto's templates are small enough to not need caching.
- **Configurable `KOTO_TEMPLATE_PATH`** is how PATH-based resolution works everywhere. It's rejected for YAGNI reasons, with an honest note that explicit paths remain available.
- **Explicit paths only** is the current behavior. It's rejected because it limits discoverability, which is a real adoption concern.
- **Lint stub** and **full linter** are genuine options that represent different points on the now-vs-later spectrum. Both are rejected for clear, specific reasons.
- **Flat commands** is a legitimate CLI organization pattern. It's rejected because of namespace management concerns, which is a real trade-off.

## 6. Additional Observations

### The `--template` heuristic has an edge case

The design says: "Contains `/` or ends in `.md` = file path; otherwise = search by name." But the Solution Architecture section says: "Contains `/` or `.` with extension `.md`." These are different rules. The first triggers on any `/` anywhere; the second triggers on `/` or `.md` extension. What about `--template ./quick-task`? It contains `/`, so both rules treat it as a path. What about `--template quick-task.md`? It ends in `.md`, so both rules treat it as a path. The inconsistency is between the two descriptions of the same rule. They should be reconciled.

More importantly: `--template quick-task.md` triggers path mode. But the user might intend name-based search, just with the extension. This is arguably fine (the name-based search adds `.md` anyway), but a user typing `--template quick-task.md` when no such file exists in CWD will get a "file not found" error instead of the search path kicking in. The design should state this explicitly.

### The `koto template inspect` output format mixes concerns

The proposed `inspect` output shows `Gates: assess/task_defined (field_not_empty), implement/tests_pass (command)`. This is a flat list with `state/gate_name` notation. For templates with many gates, this could become hard to read. Consider whether `inspect` should organize output by state, like:

```
States:
  assess (-> plan, escalate)
    gate: task_defined (field_not_empty on TASK)
  plan (-> implement)
  implement (-> done)
    gate: tests_pass (command: go test ./...)
  done (terminal)
  escalate (terminal)
```

This is a minor UX concern, not a design issue.

### The `--evidence` flag should appear in `koto next` output

The design mentions that `koto next` in the upstream design shows accumulated evidence alongside the directive. The CLI design should state whether the controller's `Next()` output changes or whether evidence display is handled at the CLI layer. Currently `ctrl.Next()` returns a `Directive` and the CLI prints it as JSON. If evidence is added to the output, the controller needs updating, which is an engine change the design claims is out of scope.

## 7. Summary of Findings

| # | Finding | Severity | Recommendation |
|---|---------|----------|----------------|
| 1 | Hash format migration not addressed | High | Add migration plan for existing workflows or document as breaking change |
| 2 | `loadTemplateFromState` migration not covered | High | Explicitly describe how all six commands using `loadTemplateFromState` will switch to the compiler path |
| 3 | XDG base directory not considered as alternative | Low | Mention and reject in Decision 2 alternatives |
| 4 | Template name with `/` treated as path, blocking namespaced names | Low | State that template names are flat; namespacing deferred |
| 5 | `--template` heuristic described inconsistently | Medium | Reconcile the two descriptions of the path-vs-name detection rule |
| 6 | Subcommand dispatching model not addressed | Low | Note that the CLI's command routing needs extension for nested subcommands |
| 7 | `field_equals` with empty value impossible | Low | Note the interaction between `--evidence key=` and `field_equals` validation |
| 8 | `koto next` evidence display not specified | Low | Clarify whether `Next()` output changes or not |

## 8. Recommendations

1. **Address the hash migration** (Finding 1, 2). This is the highest-risk gap. The design should add a subsection to the implementation approach describing how existing workflows transition. Options: dual-hash detection, migrate command, or documented breaking change with major version bump.

2. **Reconcile the path detection heuristic** (Finding 5). Pick one formulation and use it consistently in both the Decision 2 section and the Solution Architecture section.

3. **State the template-path-is-captured assumption** (Assumption A2). Add a sentence to the search path section: "Template search path resolution runs only at init time. The resolved absolute path is stored in the state file and used directly for all subsequent operations."

4. **Keep the rest as-is.** The low-severity findings are documentation clarifications, not design flaws. The design is solid in its core decisions.
