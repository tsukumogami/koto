# Architecture Review: Cross-Agent Delegation Design

**Reviewer:** architect-reviewer
**Date:** 2026-03-01
**Document:** `docs/designs/DESIGN-cross-agent-delegation.md`
**Status:** Proposed

## Summary

The design adds three capabilities: (1) semantic tags on state declarations, (2) a general-purpose config system, and (3) a delegate invocation subcommand. It flows tags through the template pipeline (source -> compiled -> Template -> Directive) while keeping them out of the engine layer.

## Findings

### F1: controller.New() signature change -- Blocking

**Location:** Design section "Controller Constructor Change"

The design proposes changing `controller.New(eng, tmpl)` to `controller.New(eng, tmpl, ...Option)`. This is a public API change in `pkg/controller/`. While it's source-compatible in Go (existing callers compile unchanged because variadic args default to zero), it changes the function signature, which matters for anyone depending on the exact type `func(*engine.Engine, *template.Template) (*Controller, error)` -- for instance, code that stores `controller.New` as a function value.

More importantly, the current `cmdNext()` in `cmd/koto/main.go` (line 307) calls `controller.New(eng, tmpl)` without options. For delegation to work, the CLI must load config, build the option, and pass it. The design mentions this in the file change summary (`cmd/koto/main.go: Load config at startup, pass to controller`) but doesn't show the specific CLI changes.

**Recommendation:** Show the `cmdNext()` change explicitly. The functional option pattern is the right choice -- it matches `engine.TransitionOption`. Just make the CLI integration concrete.

**Severity:** Advisory. The pattern is sound; the gap is specificity, not structure.

### F2: Next() gains exec.LookPath side effect -- Blocking

**Location:** Design section "Decision 5", paragraphs on side effects; code in `Controller.Next()` -> `resolveDelegation()` -> `checkDelegateAvailability()`

Today, `Controller.Next()` is pure: it reads state + template data and returns a struct. The design adds an `exec.LookPath` call (filesystem probe) inside `Next()`. The design acknowledges this and dismisses it as "negligible."

The concern isn't performance -- it's testability and determinism. `Next()` is currently unit-testable with no filesystem setup. After this change, testing delegation resolution requires either (a) a real binary in PATH or (b) injecting an availability-check interface. The design doesn't address this.

Additionally, `exec.LookPath` in `Next()` means every `koto next` call probes the filesystem for the delegate binary, even if the agent has no intention of delegating. This is wasted work when delegation config exists but the current state has no tags.

The design's own code shows the guard: `if c.delegationCfg != nil && len(d.Tags) > 0` runs before `resolveDelegation()`. So LookPath only fires when there are tags AND config. That's good, but it's still a side effect inside what was a pure function.

**Recommendation:** Extract the availability check behind an interface:

```go
type DelegateChecker interface {
    Available(target string) (bool, string)
}
```

The default implementation uses `exec.LookPath`. Tests inject a stub. This follows the same pattern as the engine's `time.Now()` usage -- the real implementation is trivial, but the interface keeps the controller testable.

**Severity:** Blocking. Without an interface, every test of `Next()` with delegation config will need PATH manipulation or will couple to filesystem state.

### F3: Config loading swallows errors -- Advisory

**Location:** Design section "Config Loading" code

```go
userCfg, _ := loadFile(userConfigPath())
projCfg, _ := loadFile(projectConfigPath())
```

Both errors are discarded. A syntax error in `~/.koto/config.yaml` silently returns `nil`, and koto proceeds with no config. The user thinks delegation is configured; koto silently runs without it.

This is different from "file not found" (which should be silent). A file that exists but has bad YAML is a user error that should surface.

**Recommendation:** Distinguish between "file not found" (silent) and "file exists but parse failed" (error). Return the error on parse failure.

**Severity:** Advisory. The failure mode is confusing but contained -- it doesn't affect the engine or other packages.

### F4: delegate submit re-resolves delegation target -- Advisory

**Location:** Design section "Decision 6", step 2: "Re-resolves the delegation target from tags + config (same logic as Next())"

The `koto delegate submit` subcommand re-resolves the delegation target independently of the `koto next` output. This means:
1. The agent calls `koto next`, gets `delegation.target: "gemini"`
2. The user changes config between calls
3. The agent calls `koto delegate submit`, which re-resolves and gets a different target

This is probably fine (config changes between calls are the user's choice), but it means the `koto next` output's `delegation.target` field is informational, not authoritative. The actual invocation target is resolved at submit time.

**Recommendation:** Document this explicitly. Consider whether `koto delegate submit` should accept `--target` to let the agent pass through the resolved target from `koto next`, skipping re-resolution. This would make the flow deterministic: `koto next` resolves, `koto delegate submit --target gemini` invokes.

**Severity:** Advisory. The re-resolution is defensible (freshest config), but the semantics should be documented.

### F5: DelegationRule.Command vs DelegateTo redundancy -- Advisory

**Location:** Design type `DelegationRule`

```go
type DelegationRule struct {
    Tag        string   `yaml:"tag"`
    DelegateTo string   `yaml:"delegate_to"`
    Command    []string `yaml:"command"` // e.g., ["gemini", "-p"]
}
```

`DelegateTo` is a human-readable label ("gemini"). `Command` is the actual invocation (`["gemini", "-p"]`). The design doesn't specify what happens when `Command` is empty or when `Command[0]` doesn't match `DelegateTo`. These fields carry overlapping but separate information.

**Recommendation:** Document the relationship: `delegate_to` is the display name used in `DelegationInfo.Target` and logs; `command` is the actual invocation. Validate that `command` is non-empty when a rule is loaded. If `command` is omitted, either error or default to `[delegate_to]` (just the bare name).

**Severity:** Advisory. Missing validation, not a structural problem.

### F6: Tags in template pipeline -- architecturally sound

The decision to keep tags out of `engine.MachineState` and flow them through `template.Template.Tags` is correct. It follows the existing pattern where `Sections` (directive text) lives in `Template`, not in `Machine`. Tags are metadata for the controller, just like directive text. The engine never needs to evaluate tags.

The `ToTemplate()` change mirrors the existing `Sections` population pattern. The `Compile()` change mirrors the existing `Gates` copy pattern. Both are mechanical extensions, not new patterns.

### F7: Config as new pkg/ package -- architecturally sound with one concern

Adding `pkg/config` is appropriate. It's a new capability (config loading) that doesn't exist yet. The dependency direction is correct: `cmd/koto/` imports `pkg/config`, `pkg/controller/` receives config via functional option. `pkg/config` doesn't import any koto packages.

**Concern:** `pkg/config` introduces `gopkg.in/yaml.v3` as a dependency. The design notes from the template format v3 review confined `yaml.v3` to `pkg/template/compile/`. Adding it to `pkg/config` relaxes that confinement. This was previously flagged as "acceptable, not blocking" in the template format review, but it's worth noting: `yaml.v3` is now used in two packages rather than one.

### F8: Nested subcommand pattern -- consistent

`koto delegate submit` follows the existing `koto template compile` pattern. The CLI already handles nested subcommands via the `cmdTemplate()` dispatcher. `koto delegate` would follow the same pattern: a `cmdDelegate()` function that switches on `args[0]`.

### F9: parseFlags limitation -- no boolean flags

**Location:** Current `parseFlags` in `cmd/koto/main.go` (line 77)

The design proposes `koto delegate submit --prompt-file /tmp/prompt.txt` and `koto delegate submit --prompt-stdin`. The `--prompt-stdin` flag is boolean (no value argument). But the current `parseFlags` implementation requires every flag to have a value:

```go
if i+1 >= len(args) {
    return nil, fmt.Errorf("%s requires a value", arg)
}
```

This means `--prompt-stdin` can't be parsed by the current flag parser. Either:
- Change the design to `--prompt-stdin true` (ugly but works)
- Add boolean flag support to `parseFlags`
- Use `--prompt -` as a convention for stdin (single flag with sentinel value)

**Recommendation:** Use `--prompt-file /tmp/prompt.txt` vs `--prompt-file -` (dash for stdin). This avoids introducing boolean flags and fits the existing parser. Alternatively, add boolean flag support, but that's a larger change to a shared utility.

**Severity:** Blocking. The proposed `--prompt-stdin` flag literally cannot be parsed by the existing CLI infrastructure.

### F10: format_version at 1 -- correct

The analysis is sound. `json.Unmarshal` ignores unknown fields (koto doesn't use `DisallowUnknownFields`). Tags are optional (`omitempty`). Old koto reads new templates without error. The JSON schema update (`additionalProperties: false` on `state_decl`) requires adding `tags` to the schema, but that's documentation, not a runtime constraint -- koto doesn't validate against the JSON schema at runtime.

### F11: Missing: what happens to koto transition during delegation?

The design describes the flow: `koto next` -> agent crafts prompt -> `koto delegate submit` -> agent uses response -> `koto transition`. But it doesn't address what prevents the agent from calling `koto transition` while delegation is in progress. Nothing in the engine prevents it -- transition validation doesn't know about delegation.

This is probably fine because delegation is purely advisory metadata. The orchestrating agent decides whether to delegate or handle directly. But if a future design wants to enforce delegation (e.g., "this step MUST be delegated"), there's no enforcement point.

**Recommendation:** Document that delegation is advisory. The engine has no concept of delegation. Gates are the enforcement mechanism -- if a step must produce a specific evidence artifact, the gate enforces it regardless of who (agent or delegate) produced it.

**Severity:** Advisory. The current "advisory only" approach is correct for v1.

### F12: Implementation phases are correctly sequenced

Phase 1 (tags in template) has no dependencies and produces testable output.
Phase 2 (config) has no dependency on Phase 1 -- could run in parallel.
Phase 3 (delegation resolution in controller) depends on both Phase 1 and Phase 2.
Phase 4 (delegate subcommand) depends on Phase 3 for the delegation types.
Phase 5 (docs/skills) depends on Phase 4.

The sequencing is correct. Phases 1 and 2 could be parallelized.

### F13: Missing: error type for delegation failures

The design shows `DelegateResponse` with `success: false` and `error: string`. But `koto delegate submit` returns this as stdout JSON, not as a non-zero exit code. The design doesn't specify whether `koto delegate submit` returns exit code 0 on delegate failure (with `success: false` in JSON) or non-zero.

This matters for agent integration. Agents typically check exit codes first and parse stdout second. If `koto delegate submit` returns exit code 0 for delegate failures, agents must parse JSON to detect errors. If it returns non-zero, agents might treat it as a koto error (not a delegate error) and invoke their error-handling path.

**Recommendation:** Return exit code 0 with `success: false` in JSON when the delegate process fails. Return non-zero only for koto's own errors (config not found, state file missing, etc.). This separates "koto worked, delegate failed" from "koto failed."

**Severity:** Advisory. Needs specification, not a structural issue.

## Simpler alternatives considered

### Alternative A: Tags without config (agent-side routing)

Tags on states, no config system, no `koto delegate submit`. The agent reads tags from `koto next` output and decides what to do. This eliminates Phases 2, 3, and 4 of the design.

**Tradeoff:** This works for agents that are already programmed to handle delegation. It doesn't work for agents that read koto's SKILL.md and follow instructions -- those agents need koto to tell them exactly what command to run. The design's approach (koto owns invocation) is correct for the skill-based agent integration model.

**Verdict:** Not simpler in practice. The config+invocation complexity exists because agents follow documented instructions, not programmed logic.

### Alternative B: Delegation as template variables

Instead of tags + config, template authors specify the delegate directly:

```yaml
states:
  deep-analysis:
    delegate: "{{REASONING_AGENT}}"
    transitions: [implement]
```

The user sets `REASONING_AGENT=gemini -p` as a variable at init time.

**Tradeoff:** Couples the template to the delegation mechanism. A template that works without delegation would need a variable default of "" and conditional logic in the agent. Tags are cleaner because they're inert metadata -- no delegation config means no delegation, no special defaults needed.

**Verdict:** Tags + config is better. Delegation is an operational concern (which tool to use), not a template concern (what work to do).

### Alternative C: Delegation as a gate type

Already considered and rejected in the design (Decision 6, alternatives). Correct rejection -- gates run at transition time, but delegation needs to happen at directive time.

## Answers to Review Questions

### 1. Is the architecture clear enough to implement?

Yes, with two gaps. The `--prompt-stdin` boolean flag issue (F9) needs resolution before implementation. The `cmdNext()` CLI change (F1) should be shown explicitly. Everything else has enough detail for a developer to implement from.

### 2. Are there missing components or interfaces?

One missing interface: `DelegateChecker` for availability checking (F2). Without it, the controller gains a filesystem dependency that breaks testability.

One missing specification: exit code semantics for `koto delegate submit` (F13).

One missing validation: `DelegationRule.Command` non-empty check (F5).

### 3. Are the implementation phases correctly sequenced?

Yes. Phases 1 and 2 could run in parallel (no dependency between tags and config). Phase 3 correctly depends on both. Phase 4 depends on Phase 3. Phase 5 is last. No cycles, no missing dependencies.

### 4. Are there simpler alternatives we overlooked?

No. The alternatives I evaluated (agent-side routing, template variables, gate-based delegation) are all worse. The design's approach (tags as metadata, config for routing, koto owns invocation) is the right decomposition for the skill-based agent model. The complexity is load-bearing.

## Blocking Findings Summary

| ID | Finding | Location | Fix |
|----|---------|----------|-----|
| F2 | Next() gains exec.LookPath -- breaks testability | controller.Next() | Extract DelegateChecker interface |
| F9 | --prompt-stdin is boolean; parseFlags rejects boolean flags | koto delegate submit | Use --prompt-file - for stdin, or add boolean flag support |

## Advisory Findings Summary

| ID | Finding | Location | Fix |
|----|---------|----------|-----|
| F1 | controller.New() CLI integration not shown | cmdNext() | Show the specific code change |
| F3 | Config loading swallows parse errors | pkg/config Load() | Distinguish file-not-found from parse-error |
| F4 | delegate submit re-resolves target | koto delegate submit | Document semantics; consider --target flag |
| F5 | DelegateTo vs Command redundancy | DelegationRule | Document relationship; validate Command non-empty |
| F11 | Delegation is advisory, not enforced | Engine/controller boundary | Document explicitly |
| F13 | Exit code semantics unspecified | koto delegate submit | Specify: 0 for delegate failure, non-zero for koto failure |
