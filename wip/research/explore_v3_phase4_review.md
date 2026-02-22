# Phase 4 Review: koto Template Format v3

## Summary

The design makes a sound structural decision: separate human-authored source (YAML frontmatter + markdown) from engine-consumed compiled output (JSON). The "programming language" framing works and the dependency isolation (go-yaml in compiler only, stdlib in engine) fits the existing architecture. However, three issues need attention: the heading collision "containment" claim is weaker than presented, the evidence gate design has a gap around evidence accumulation and rewind, and the `Engine.Transition` API change has backward-compatibility implications that need explicit handling.

## Findings

### Blocking

**1. Evidence accumulation and rewind interaction is unspecified**

The design says evidence accumulates in the state file across transitions and proposes `Evidence map[string]string` on `engine.State`. But the existing rewind semantics (from `DESIGN-koto-engine.md`) preserve full history without truncation. When a workflow rewinds from `implement` back to `plan`, what happens to evidence accumulated during `implement`?

Three options exist (clear it, keep it, clear only the rewound state's evidence), and each has different implications for gate re-evaluation. The engine design explicitly calls out that "rewind semantics will need to account for evidence cleanup" but this design doesn't resolve it.

This is blocking because the `Evidence` field on `engine.State` is a schema change (state file schema_version 2), and the rewind behavior determines whether evidence is a flat map or needs per-state scoping. Getting this wrong means a schema_version 3 migration shortly after 2. The design should either specify the rewind/evidence interaction or explicitly defer `Evidence` on `State` until it's resolved.

Location: `DESIGN-koto-template-format.md`, "Engine extension" section (lines 550-553) and "Evidence Gates" section (lines 536-548).

**2. `Engine.Transition` signature change breaks existing callers without a migration path**

The design proposes changing `Transition(target string) error` to `Transition(target string, opts ...TransitionOption) error`. This is a Go-compatible signature change (variadic is backward-compatible for callers passing one arg), but the current codebase has direct calls in:

- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go` line 208: `eng.Transition(target)`
- `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine_test.go` (multiple test cases)
- Integration tests in `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/integration_test.go`

The design states this uses `WithEvidence(map[string]string)` for backward compatibility, which is correct at the call-site level. But the design doesn't address what happens when the CLI receives `--evidence key=value` flags. The `cmdTransition` function in `main.go` needs to parse evidence flags and pass them through. The design should specify the CLI surface change (`koto transition <target> --evidence key=value`) alongside the API change, since the CLI is a compatibility surface.

Location: `DESIGN-koto-template-format.md`, "Engine extension" section (lines 550-553); `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go` lines 173-217.

### Advisory

**3. The heading collision "containment" claim understates real friction**

The design says heading collision is "contained" because the compiler uses the declared state list from YAML frontmatter to determine which `## headings` are state boundaries. This is correct for the compiler. But consider the authoring experience:

A template author writes a state named `plan` and within the `plan` directive includes `## Acceptance Criteria` as a subheading. This works -- the compiler sees `plan` in the declared states and `Acceptance Criteria` is not, so it's treated as content. But if the author later adds a state named `acceptance-criteria`, the heading won't collide because state names use lowercase-kebab while markdown subheadings use title case. The real risk is when an author uses a lowercase `## notes` subheading within a state's directive and later adds a state named `notes` -- the compiler silently reassigns the content.

The design mentions the linter catching this, but the linter is Phase 6 (last). Between Phase 2 (compiler) and Phase 6, authors get silent content reassignment. Consider adding a compiler warning (not error) when a body heading matches a declared state name and appears in a position that changes the content boundary of an adjacent state. This doesn't need to block the design, but "contained" overstates the situation.

Location: `DESIGN-koto-template-format.md`, lines 236-237.

**4. `command` gate lacks timeout and that's acknowledged, but the mitigation is incomplete**

The design says "No timeout in Phase 1" and "This must be addressed before unattended agent scenarios." But the implementation phases put evidence gates in Phase 4 and the linter in Phase 6. There's no Phase between 4 and 6 that adds timeouts. If agents use command gates in Phase 4, they'll encounter the no-timeout problem before any mitigation arrives.

Consider adding a default timeout (e.g., 30 seconds) in Phase 4 rather than deferring it. A stuck `go test ./...` blocking a transition indefinitely in an agent loop is a real operational risk, not an edge case.

Location: `DESIGN-koto-template-format.md`, lines 547 and 679.

**5. The `field_not_empty` gate checks the evidence map, but `field_equals` and `field_not_empty` don't specify whether they also check variables**

The interpolation section says "Evidence wins over variables (higher precedence)" for template interpolation. But the gate types reference only "evidence field" and "evidence map." If an author declares a `field_not_empty` gate on field `TASK`, and `TASK` is a variable set at init time (not evidence), does the gate pass?

The design should clarify: do gates evaluate against the evidence map only, or against the merged context (variables + evidence)? If evidence only, the gate names should reflect that (`evidence_not_empty`). If merged, the precedence rule from interpolation applies. Either is fine, but it needs to be explicit.

Location: `DESIGN-koto-template-format.md`, lines 536-548 and 559-563.

**6. go-yaml isolation is practical but not as clean as presented**

The design claims go-yaml is isolated to the compiler and the engine remains dependency-free. At the Go module level this is true -- `go.mod` won't list go-yaml as a dependency of `pkg/engine/`. But `pkg/template/compile/` (the compiler package) will be in the same Go module as `pkg/engine/`. Anyone who `go get github.com/tsukumogami/koto` gets go-yaml in their `go.sum` even if they only import `pkg/engine/`.

This is a real but minor cost. The design correctly identifies that the engine's parse path uses only `encoding/json`. But "zero dependencies for core engine" (Decision Driver 4) is a statement about the parse path, not the module. If this distinction matters to library consumers, the compiler could live in a separate module (`github.com/tsukumogami/koto/compiler`). If it doesn't matter enough for that, acknowledge that the module will have go-yaml in `go.sum`.

Location: `DESIGN-koto-template-format.md`, line 84 and lines 143-144.

**7. `initial_state` is declared in YAML but the current parser infers it from heading order**

The current template parser (`/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template.go`, line 125) sets `InitialState: stateNames[0]` -- the first `## heading` in the body is the initial state. The new design moves this to an explicit `initial_state:` field in the YAML frontmatter.

This is a good change, but the backward compatibility section (Phase 5) only mentions detecting legacy format by flat `key: value` syntax. It should also mention that legacy templates have no `initial_state` field, so the legacy compiler must infer it from heading order (preserving existing behavior). The test in `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/template_test.go` (`TestParse_InitialStateIsFirst`, line 389) documents this behavior.

Location: `DESIGN-koto-template-format.md`, lines 655-660.

### Strengths

**Source/compiled separation is the right call.** The current parser's hand-rolled YAML handling (lines 233-287 of `template.go`) already struggles with one level of nesting (variables). Evidence gates need two levels. Rather than building a progressively more complex hand-rolled parser, the design correctly recognizes this as a "switch to a real parser" moment and confines the dependency to the compiler.

**The "programming language" analogy holds up.** It's not just an analogy -- it accurately describes the relationship. The source format is designed for human authoring and version control. The compiled format is designed for machine consumption. The compiler is deterministic. The linter is optional. Every developer already understands this model. The design avoids overextending the metaphor.

**Dependency direction is preserved.** The new `pkg/template/compile/` package imports `pkg/template/` types (or new compiled types) and `pkg/engine/`. It doesn't create circular dependencies. The engine reads compiled JSON; it doesn't know about the source format. This matches the existing dependency flow: `template -> engine`, `controller -> engine + template`.

**LLM at the validation layer only is architecturally clean.** Keeping LLMs out of the compile path means the compiler is deterministic and testable. The linter is genuinely optional -- you can build, test, and ship templates without it. This avoids a common anti-pattern where LLM integration becomes load-bearing infrastructure.

**Evidence gate types are appropriately minimal for Phase 1.** `field_not_empty`, `field_equals`, and `command` cover the cases that the existing workflow-tool uses (the hardcoded `transitionEvidence` map in the original codebase checks for non-empty fields). The design doesn't over-engineer gate types it hasn't needed yet.

**CLI surface design (koto template compile/validate/lint/new) is a clean namespace.** Putting template tooling under `koto template` keeps the primary command namespace (`koto init`, `koto transition`, `koto next`) focused on workflow execution. This follows the existing pattern where `koto workflows` is a utility command, not an execution command.
