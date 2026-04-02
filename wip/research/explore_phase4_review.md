# Architect Review: DESIGN-cross-agent-delegation.md

Reviewer: architect-reviewer
Date: 2026-03-01
Status: Advisory findings + structural questions

---

## 1. Problem Statement Assessment

The problem statement is specific enough to evaluate solutions. It identifies a concrete gap: koto states carry no metadata beyond transitions, gates, and terminal flags, so all steps look identical to the orchestrator. The example contrast (security audit vs. file-editing step) is grounded.

One weakness: the problem statement blends two distinct problems without separating them. Problem A is "templates can't describe step characteristics." Problem B is "koto has no config system for routing." These are coupled in the proposed solution but are independently useful. Tags have value without delegation (the design acknowledges this in Consequences). The config system has value without tags (future features). The design would be clearer if these were framed as two sub-problems that the solution addresses together.

The statement also doesn't define success criteria. How do we know delegation is working well? Measurable criteria (e.g., "an agent can delegate a step to gemini and receive a response without koto-specific code in the agent") would sharpen evaluation.

## 2. Missing Alternatives

### Decision 1 (Schema Versioning)

The alternatives are reasonable. No missing options.

One factual claim to verify: the design states "koto doesn't use `DisallowUnknownFields`". Confirmed by code review -- `ParseJSON()` at `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled.go:45` uses plain `json.Unmarshal` without a decoder, so unknown fields are silently ignored. The backwards compatibility argument holds.

However, the design says the JSON schema uses `additionalProperties: false` on `state_decl` (line 69 of the design). Confirmed at `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/template/compiled-template.schema.json:72`. This means external validators using the *old* schema will reject templates with tags. The design acknowledges this ("External validators using the updated schema accept tags; validators using the old schema reject them") but doesn't discuss the practical impact: any CI pipeline or editor using the schema as a linter will break on tagged templates until the schema is updated. This isn't a design flaw, just an unstated operational consequence worth noting.

### Decision 2 (Tag Format)

**Missing alternative: structured tags (key-value pairs).** Instead of flat strings, tags could be `map[string]string`: `tags: {capability: deep-reasoning, context-size: large}`. This separates the dimension (capability, context-size) from the value, making config matching more expressive: a rule could match on `capability=*` rather than listing individual tags. Rejection rationale would be: more complex for minimal v1 benefit, and flat tags can evolve to structured tags later if needed.

**Missing alternative: single-tag-per-state.** If the primary use case is delegation routing and first-match-wins, most states will have zero or one meaningful tag. A `delegate_hint: deep-reasoning` field instead of an array would be simpler. Rejection rationale: arrays allow orthogonal tagging (a step can be both `deep-reasoning` and `security`), and other future consumers might care about non-delegation tags.

### Decision 3 (Where Tags Live)

The alternatives are well-considered. No significant missing options.

### Decision 4 (Config System)

**Missing alternative: config as flags + environment + file (layered).** Most mature CLIs support all three: flags override env vars override file. The design considers flags-only and env-vars-only as standalone alternatives and rejects them, but doesn't consider a layered approach where the YAML file is the base and env vars or flags can override specific values. For example, `KOTO_DELEGATE_TIMEOUT=60` could override `delegation.timeout` from config. This is common in tools like Docker and kubectl.

Rejection rationale would be: premature for v1, and the config file covers the persistence need. Layered overrides can be added later without breaking the file format.

**Missing alternative: TOML instead of YAML.** The design rejects JSON in favor of YAML citing Docker Compose and k8s, but doesn't consider TOML, which is the standard for Go project config (go.toml, Cargo.toml) and avoids YAML's well-known pitfalls (Norway problem, implicit type coercion). Since koto already depends on `gopkg.in/yaml.v3` for template compilation, YAML doesn't add a new dependency, which is a fair counterpoint.

### Decision 5 (Delegation Resolution)

**Missing alternative: lazy resolution (resolve at `delegate submit` time only).** Instead of resolving delegation in `Next()`, have `Next()` just pass through tags and let `delegate submit` handle the resolution. This avoids duplicating resolution logic between `Next()` and `submit` (the design explicitly says `submit` re-resolves, line 312). The downside: the agent doesn't know whether delegation is available before deciding what to do.

### Decision 6 (Delegate Subcommand)

**Missing alternative: delegation via `koto transition` with a `--delegate` flag.** Instead of a separate `koto delegate submit` command, the agent could call `koto transition --delegate next-state --prompt-file /tmp/prompt.txt`. This keeps the flow to two koto calls (next + transition) rather than three (next + delegate submit + transition). The agent already calls transition; adding delegation to the transition step is ergonomically simpler.

Rejection rationale would be: it conflates delegation (which is about executing the current step) with transition (which is about moving to the next step). A delegate response might not result in a transition -- the agent might need to review the response first.

## 3. Rejection Rationale Fairness

### "Delegation as a gate" (Decision 6, last alternative)

This alternative is undersold. The rejection says it "couples delegation to transitions rather than directives, forces responses through the evidence map, and overloads the evidence mechanism." The first point is fair. The second point is partially fair -- evidence is `map[string]string`, which can't carry structured delegate responses. But the third point ("overloads evidence") is vague. Evidence already stores arbitrary agent-produced data. The real issue is that gates run during transition validation, which is too late -- the agent needs to know about delegation *before* doing work, at `koto next` time. The rejection should lead with this timing argument.

### "Agent invokes delegate directly" (Decision 6, first alternative)

This alternative is **a strawman**. The rejection says "it pushes deterministic work into the agent" and "every agent platform would need its own delegation implementation." But the invocation is just `exec.Command(target, "-p")` with stdin piping -- maybe 15 lines of code per agent. The real argument for koto owning invocation is: (a) timeout handling, (b) structured error reporting, (c) availability checking, and (d) koto can evolve the invocation (retries, logging, metrics) without touching agent skills. The design should make these arguments instead of overstating the burden on agents.

### "Flags only" (Decision 4, first alternative)

Fair rejection.

### "Enum in JSON schema" (Decision 2, first alternative)

Fair rejection. The extensibility argument is solid.

## 4. Unstated Assumptions

### A1: delegate_to values map to binary names

The design assumes `delegate_to: gemini` means there's a binary called `gemini` in PATH. But the design also says (line 839) "A config entry like `delegate_to: "rm -rf /"` maps to nothing -- koto reports an unknown delegate error." This implies a hardcoded allowlist inside koto (`delegateArgs` function, line 700). These two statements are in tension:

- If there's an allowlist, adding a new delegate requires a koto release.
- If there's no allowlist, any binary name works and the security claim on line 839 is wrong.

The design needs to pick one. If it's an allowlist, it should be explicit about what's in the list and how it's extended. If it's open, the security section needs updating.

### A2: delegate CLIs have a consistent invocation pattern

The design maps target names to CLI args (`"gemini" -> ["-p"]`). This assumes all delegates accept prompts via stdin with a predictable flag. But different CLIs have different patterns:
- `claude -p` reads from stdin
- `gemini` -- invocation pattern isn't standardized
- A custom internal tool might use `--input` or `--prompt`

The design doesn't address how users configure the invocation pattern per delegate. The `delegateArgs` function is hardcoded inside koto. If a user has a delegate CLI that doesn't match the hardcoded pattern, they can't use it.

**Missing option:** Make the CLI invocation pattern part of the config:

```yaml
delegation:
  rules:
    - tag: deep-reasoning
      delegate_to: gemini
      command: ["gemini", "-p"]
```

This removes the hardcoded `delegateArgs` mapping and makes delegation extensible to arbitrary CLIs.

### A3: Controller constructor change is non-breaking

The design adds `delegationCfg` to the Controller but doesn't show how it gets there. `controller.New()` currently takes `(*engine.Engine, *template.Template)`. Adding a delegation config parameter changes the public API:

```go
// Current (controller.go:31):
func New(eng *engine.Engine, tmpl *template.Template) (*Controller, error)

// Proposed (not shown explicitly, but implied):
func New(eng *engine.Engine, tmpl *template.Template, cfg *config.DelegationConfig) (*Controller, error)
```

This is a breaking change to the `pkg/controller` public API. The design doesn't discuss this. Options:
- Add a functional option: `controller.New(eng, tmpl, controller.WithDelegation(cfg))`
- Use a setter: `ctrl.SetDelegationConfig(cfg)`
- Accept the break since koto is pre-1.0

The functional option pattern is more consistent with how the engine already handles extensibility (`TransitionOption`).

### A4: Single-prompt-single-response model

The design assumes delegation is a synchronous single exchange: agent sends prompt, delegate returns response. It doesn't consider:
- Multi-turn delegation (delegate needs to ask clarifying questions)
- Streaming responses
- Delegates that need tool access (file reading, command execution)

These are probably out of scope for v1, but the assumption should be stated explicitly so future designs can reference it.

### A5: The prompt is the agent's responsibility

The design says "Agent reads directive, gathers context, crafts a self-contained prompt" (line 372). This means prompt quality depends entirely on the orchestrating agent's skill instructions. koto doesn't help construct the prompt beyond providing the directive text. This is a reasonable separation, but it means delegation quality is fragile -- it depends on the agent skill author writing good delegation prompts.

### A6: Hash breakage for in-flight workflows

The Consequences section mentions (line 892): "Adding tags to existing templates changes the compiled hash, breaking in-flight workflows." This is correct but the design doesn't propose a mitigation. A template author who adds tags to a template with active workflows will get `ErrTemplateMismatch` on every koto command. The only recovery is `koto init` (restart the workflow). This should be called out more prominently, possibly with a recommended workflow (finish in-flight workflows before adding tags, or use `koto rewind` + re-init).

## 5. Strawman Check

**Decision 6, "Agent invokes delegate directly"** is presented as weaker than it is. See section 3 above. The invocation burden on agents is overstated.

No other alternatives appear to be strawmen. The design gives each alternative a specific rejection reason tied to concrete technical concerns.

## 6. Verified Code Claims

| Claim | File | Verified |
|-------|------|----------|
| `ParseJSON()` rejects `format_version != 1` | `compiled.go:49` | Yes |
| koto doesn't use `DisallowUnknownFields` | `compiled.go:45` | Yes, uses plain `json.Unmarshal` |
| `sourceStateDecl` has no `Tags` field | `compile.go:47-49` | Yes, only Transitions/Terminal/Gates |
| `StateDecl` has no `Tags` field | `compiled.go:33-38` | Yes |
| Schema has `additionalProperties: false` on `state_decl` | `compiled-template.schema.json:72` | Yes |
| `controller.New()` takes `(*Engine, *Template)` | `controller.go:31` | Yes |
| `Directive` struct has only Action/State/Directive/Message | `controller.go:21-25` | Yes |
| `Template` struct has no Tags field | `template.go:35-44` | Yes |
| `ToTemplate()` doesn't populate tags | `compiled.go:110-131` | Yes (no Tags field to populate) |
| `Next()` returns directive with interpolated text | `controller.go:51-90` | Yes |
| Engine uses variadic `TransitionOption` pattern | `engine.go:38-49` | Yes |

All code claims in the design are accurate against the current codebase.

## 7. Structural Fit Assessment

### Tags in template pipeline (Decision 3): **Fits well.**

The data flow `sourceStateDecl -> StateDecl -> Template -> Directive` follows the existing pattern for directive text and transitions. Tags skip `engine.MachineState`, which respects the engine/controller separation. No new package dependencies are introduced in the template layer.

### Config system (Decision 4): **Structural concern.**

A new `pkg/config` package with YAML parsing adds `gopkg.in/yaml.v3` as a dependency of `pkg/config`. Currently, yaml.v3 is confined to `pkg/template/compile/`. This is by design -- the engine reads JSON only, and YAML is a compiler concern. Adding YAML to `pkg/config` means a second package depends on yaml.v3, broadening its footprint. Not blocking (the dependency already exists in the module), but worth noting that the "yaml confined to compiler" invariant is being relaxed.

### Controller changes (Decision 5): **Structural concern.**

The controller currently has a clean interface: `New(eng, tmpl)` and `Next()`. Adding delegation resolution to `Next()` mixes two concerns: "what should the agent do" and "who should do it." The existing `Next()` is 15 lines of read-only logic. With delegation, it gains subprocess execution (availability checking via `exec.LookPath`) and config resolution. Consider whether delegation resolution belongs in a separate function that the CLI calls after `Next()`, keeping the controller focused.

### Delegate subcommand (Decision 6): **New pattern, acceptable.**

`koto delegate submit` introduces a nested subcommand with subprocess invocation (exec.CommandContext). This is a new capability category for koto -- existing commands are state file operations. The subprocess management adds failure modes (timeouts, exit codes, stdin piping) that don't exist elsewhere. This is inherent to the feature and acceptable, but the code should be isolated in its own file/package rather than mixed into the controller.

## 8. Summary of Findings

### Should address before accepting

1. **Resolve the allowlist tension (A1).** The design simultaneously claims `delegate_to` maps to hardcoded args inside koto AND that config is user-controlled. Pick one, or make invocation patterns configurable.

2. **Show how delegationCfg reaches the controller (A3).** The `controller.New()` signature change is a public API break. Use functional options (consistent with engine's `TransitionOption`) or state the break explicitly.

3. **State the single-exchange assumption (A4).** Delegation is one prompt in, one response out, synchronous. Future designs will need this boundary.

### Should consider

4. **Configurable CLI invocation patterns (A2).** Hardcoding `delegateArgs` limits delegation to CLIs koto knows about. Config-driven invocation would make the feature usable with arbitrary tools.

5. **Controller concern mixing (structural).** Delegation resolution in `Next()` adds subprocess execution to a currently side-effect-free function. Consider separating resolution from directive generation.

6. **"Agent invokes delegate directly" alternative (section 3).** Strengthen the rejection with concrete arguments (timeout, structured errors, single point of evolution) rather than overstating invocation burden.
