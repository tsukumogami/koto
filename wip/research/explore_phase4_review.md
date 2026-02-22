# Design Review: DESIGN-koto-template-format.md

## Reviewer Context

Reviewed against the upstream vision document (DESIGN-workflow-tool-oss.md, vision issue #312), the implemented engine (DESIGN-koto-engine.md, status: Current), and the existing codebase (pkg/engine/, pkg/template/, pkg/controller/).

---

## 1. Problem Statement Specificity

**Verdict: Strong, with one gap.**

The problem statement identifies five concrete gaps: undocumented format, no evidence gates, header format limitations, heading collision, no search path, and no validation contract. Each gap is specific enough to evaluate solutions against -- the heading collision example is particularly good because it gives an exact failure scenario.

**Gap**: The problem statement doesn't mention the `## STATE:` prefix that exists in the upstream vision document's template examples (e.g., `## STATE: initial_jury`). The current codebase uses bare `## state-name` headings (no `STATE:` prefix). The design silently drops the `STATE:` prefix convention without acknowledging the divergence. This is probably correct (the prefix adds noise) but should be explicit, since someone reading the vision doc and then this design will wonder what happened to it.

---

## 2. Missing Alternatives

### Decision 1 (Header Format)

**Missing alternative: JSON front matter.** JSON is already in Go's standard library (no external dependency at all). It handles arbitrary nesting. The main objection is readability -- JSON is noisier than TOML for configuration -- but it deserves mention since it directly satisfies the zero-dependency driver better than TOML does. Even if rejected for readability, omitting it looks like an oversight given how prominently the design weighs the dependency question.

**Missing alternative: Structured comments in markdown.** Some tools (e.g., Hugo shortcodes, MDX) embed structured data in HTML comments or custom syntax within markdown. Probably a weak option for koto, but it would avoid the front-matter delimiter question entirely. Minor omission.

### Decision 3 (Search Path)

The alternatives are fine. No significant gaps.

### Decision 4 (Evidence Gates)

**Missing alternative: Regex-based field matching.** A `field_matches` gate type using regex would sit between `field_equals` (exact match) and `command` (arbitrary shell) in terms of flexibility. It's a common pattern in validation tools. Probably fine to defer, but worth acknowledging as a natural Phase 2 extension.

**Missing alternative: File existence gates.** The upstream vision document's /just-do-it template example shows evidence like `branch_name` and `pr_url` being passed as field values. But a common real-world gate is "does this file exist?" (e.g., "did the agent write wip/plan.md?"). This could be a `command` gate (`test -f wip/plan.md`) but it's common enough that a dedicated `file_exists` type might be warranted. Not blocking for Phase 1, but worth noting in the design as a likely future addition.

### Decision 5 (Interpolation)

**Missing alternative: Go's `text/template` package.** This is in the standard library and handles `{{.Key}}` syntax natively with zero dependencies. The design's custom interpolation is deliberately simpler (no conditionals, no loops), which is a defensible choice, but the alternative should be mentioned since it's the obvious stdlib option. The rejection would be: `text/template` allows arbitrary logic in templates which could introduce security issues and template debugging complexity.

### Decision 6 (Validation)

The alternatives are reasonable. No significant gaps.

---

## 3. Rejection Rationale Fairness

### Decision 1: YAML rejection -- Fair but slightly oversold

The YAML rejection cites "implicit typing" (`yes` becomes boolean, `1.0` becomes float) as a primary reason. This is a real YAML 1.1 problem, but go-yaml v3 uses YAML 1.2 which is less surprising (bare `yes` is still converted, but the behavior is more predictable). The rejection is directionally correct but slightly overstates the problem by not acknowledging YAML 1.2 improvements. The dependency size comparison (go-yaml ~15K lines vs BurntSushi/toml ~3K lines) is a stronger argument and should be the lead.

The "multiple representations" criticism of YAML (flow vs block, single vs double quotes) is fair. TOML also has multiple representations for tables (inline `{}` vs section headers `[table]`) but the design uses both without noting this parallel.

### Decision 2: Declared-state matching -- Fair

The escape syntax and heading level alternatives are genuine options with honest rejection reasons. The fenced sections alternative is also fair. No strawmen here.

### Decision 4: Transition-level gates -- Fair

The rejection of transition-level gates for Phase 1 is well-reasoned. The design correctly identifies the common case (state-level "you must have this before leaving") and defers the edge case (per-transition requirements) with a clear path forward.

### Decision 5: Separate namespaces -- Slightly unfair

The rejection of `{{var.TASK}}` / `{{evidence.CI_STATUS}}` namespaces calls it "verbosity without proportional benefit." But there's a real benefit: disambiguation. If a template variable and an evidence field share the same key, the merged context silently picks the evidence value. The design acknowledges this (evidence-wins precedence) but frames it as a feature rather than noting the footgun. A template author who defines variable `STATUS` with a default and later an evidence field `STATUS` gets accumulated would see the default silently overridden. This is probably the right behavior, but the namespace alternative deserves a fairer hearing that acknowledges the disambiguation benefit before rejecting it for verbosity.

---

## 4. Unstated Assumptions

### TOML table ordering

The design states: "The first declared state in the `[states]` table is the initial state. (TOML preserves insertion order.)" This relies on a BurntSushi/toml library implementation detail. The TOML specification (v1.0.0) says tables are unordered: "Since open and closing brackets are the delimiters, they can be used in any order." BurntSushi/toml preserves insertion order as a practical matter, but this isn't a spec guarantee. The design should either:
- Add an explicit `initial_state` field in the header, or
- Document this as a library-specific behavior with a test that verifies it, or
- Acknowledge the assumption and accept the risk

This is a real implementation concern, not a theoretical one. If the library changes behavior or koto switches TOML libraries in the future, the initial state selection silently breaks.

### State name format

The design says state names are "any non-empty string" (under "Not validated"). But the heading-matching rule requires state names to exactly match `## heading` text. This means state names can't contain leading/trailing whitespace (markdown heading parsing trims it), and they can't contain characters that markdown heading parsers normalize. The design should specify what characters are allowed in state names, even if the rule is permissive. Consider: what happens with a state named `my state` (space) or `## re:assess` (colon) or `states with "quotes"`?

### Case sensitivity

The declared-state matching rule says heading text must "exactly match" the state name. Does this mean case-sensitive matching? If someone writes `## Assess` but declares `assess` in the TOML header, is that a match? The design should be explicit. Case-sensitive matching is probably the right call (TOML keys are case-sensitive), but it needs to be stated.

### Backward compatibility period

The design mentions supporting both `---` (legacy) and `+++` (TOML) delimiters "during a transition period" (Implementation Phase 1) but doesn't define how long this period lasts or what triggers the end. Since the user base is currently zero, the transition period could be "one release" -- but it should be stated.

### Evidence accumulation in state file

The design's evidence gate system references evidence values being accumulated in the state file, but the current `engine.State` struct has only a `Variables` map -- no `Evidence` field. The design assumes the engine will be extended with evidence storage, but this structural change to the state file schema isn't addressed. Will evidence be stored in the existing `Variables` map? A new `Evidence` map? This affects the interpolation merge contract described in Decision 5.

### Command gate execution environment

The design says command gates run "via the user's shell" but doesn't specify which shell. The upstream vision doc says `sh -c` (not login shell). The design should be consistent with the upstream and specify the exact invocation mechanism, including:
- Working directory (project root? state file directory?)
- Environment variables (inherited from koto process? minimal?)
- Timeout (what if the command hangs?)
- Output capture (exit code only? stderr/stdout?)

The Security Considerations section partially addresses this ("Command output is not sandboxed" and "stdout/stderr is not captured") but the specification section should be more explicit.

---

## 5. Architecture Detail for Implementation

**Verdict: Mostly sufficient, with three gaps.**

### Sufficient areas

- The TOML header schema is well-specified with field tables, types, defaults, and examples.
- The parsing rules (6 numbered steps) are clear and implementable.
- The validation rules are enumerated with exact error messages.
- The examples (valid and two invalid) give good edge-case coverage.
- The implementation phases are logically ordered.

### Gap 1: Go struct definitions

The design defines the TOML schema but doesn't show the Go structs that will receive the parsed TOML. For an implementer, the struct definitions are the most important artifact. What does `TemplateConfig` look like? How do gate declarations unmarshal? The engine design showed full Go struct definitions; this design should too.

Suggested addition: a Go struct block showing at minimum the top-level config struct, the variable declaration struct, the state declaration struct, and the gate declaration struct.

### Gap 2: Transition declaration conflict resolution

The design says "If a state has transitions in both the TOML header and a `**Transitions**:` line in the body, the TOML header wins." But what if a state has transitions in the body but is NOT declared in `[states]`? The parsing rule says body headings not matching declared states are "regular markdown content." So a `## new-state` with `**Transitions**: [done]` in the body would be treated as content of the preceding state, and its transitions line would... be consumed by the parser? Or left as content? The parsing rules should handle this edge case explicitly.

My reading of the rules: since `new-state` doesn't match a declared state, the heading is content. The `**Transitions**:` line within that content would still be consumed by the parser (rule 5 says "a `**Transitions**:` line within a section is consumed"). This seems wrong -- if the heading is content, the transitions line should also be content. The consuming behavior should only apply within recognized state sections.

### Gap 3: Template search path in the template package vs CLI

The design correctly says template search is a CLI concern, not a `pkg/template/` concern. But it doesn't specify where the resolution function lives. Phase 3 says "Add search path resolution to the CLI layer" and mentions `resolveTemplatePath` in `cmd/koto/main.go`. This is sufficient for implementation.

---

## 6. Security Considerations Completeness

**Verdict: Good coverage with one gap.**

The four areas (download verification, execution isolation, supply chain, user data exposure) are well-covered. The command gate security model is honest about the trust boundary. The mitigation table is concrete.

### Gap: Command gate injection via TOML values

If a template variable is interpolated into a state directive, and that directive is what the agent reads, the design correctly identifies prompt injection as a risk. But what about command gates? Can a command gate reference template variables? The design shows static command strings (`"go test ./..."`) but doesn't prohibit variable interpolation in commands. If someone writes:

```toml
[states.build.gates.check]
type = "command"
command = "test -f {{OUTPUT_FILE}}"
```

And `OUTPUT_FILE` is set to `foo; rm -rf /`, the interpolated command becomes `test -f foo; rm -rf /`. The design should explicitly state whether interpolation applies to command gate strings. The safest answer is "no" -- command strings are literal, no interpolation.

---

## 7. TOML Choice vs Zero-Dependency Constraint

**Verdict: Well-justified, with a minor documentation improvement needed.**

The design makes a careful argument: the zero-dependency constraint applies to the core engine (pkg/engine/, pkg/controller/, pkg/discover/), not to every package. The template package is a leaf that nothing else imports except the CLI. Adding BurntSushi/toml to this leaf package doesn't compromise the engine's zero-dependency guarantee.

The justification is strong because:
1. The dependency boundary is real -- the import graph confirms template is a leaf.
2. BurntSushi/toml's profile (BSD, ~3K lines, stable since 2013, 4.5K+ stars, zero transitive dependencies) is as safe as external Go dependencies get.
3. The alternative (hand-rolling nested structure parsing) would be more code, more bugs, and harder to maintain.

**But**: the design doesn't mention that the upstream vision document says "YAML header" repeatedly (the template examples use `---` YAML delimiters, `states:` uses YAML indentation, evidence gates use YAML nesting). The vision document was written with YAML in mind. This design pivots to TOML without referencing the upstream expectation. The rationale is sound, but the explicit acknowledgment of the divergence from the upstream vision should be present -- especially since someone reading both documents will notice the conflict.

The YAML vs TOML decision should include a line like: "The upstream vision document (DESIGN-workflow-tool-oss.md) used YAML syntax in examples, reflecting the format of the hand-rolled parser that existed at the time. This design evaluates both options against the current requirements and selects TOML for the reasons above."

---

## 8. Declared-State Matching Robustness

**Verdict: Sound approach with edge cases that need specification.**

The declared-state matching is the design's best decision. It elegantly solves the heading collision problem by making the TOML header the source of truth. The parser doesn't have to guess which headings are state boundaries -- it knows.

### Edge cases that need specification

**Duplicate headings**: What if the body contains `## assess` twice? The design says states must have "corresponding `## heading` sections." Is it an error if a state name appears as a heading more than once? The likely correct behavior is: the second `## assess` heading starts a new occurrence that replaces (or appends to?) the first. This should be specified.

**Heading with trailing content**: What about `## assess - Phase 1`? This doesn't exactly match `assess`, so it would be treated as content. Good. But `## assess` followed by whitespace? The parser should trim before comparison, and this should be stated.

**Heading in code blocks**: A fenced code block containing `## assess` should not be treated as a state boundary. The current parser doesn't handle code blocks (it does line-by-line string matching). This is the same bug that exists today, and the design doesn't address it. For Phase 1, documenting this limitation is sufficient -- but it should be documented.

**Empty state sections**: The validation rule says "All `## heading` sections in the body that match declared state names have content." But what counts as "content"? Whitespace only? A single newline? The validation error message should specify the threshold.

---

## 9. Evidence Gate Type Sufficiency for Phase 1

**Verdict: Sufficient, with one observation.**

The three types -- `field_not_empty`, `field_equals`, `command` -- cover the primary use cases well:

- `field_not_empty`: Agent must provide evidence it did the work (filed a PR, wrote a plan, created a branch).
- `field_equals`: Gate on a specific outcome (scope decision = "proceed", CI status = "passed").
- `command`: Arbitrary verification via exit code (tests pass, linter clean, file exists).

This matches the upstream vision's evidence gate examples. The `command` type is the escape hatch for anything the declarative types don't cover.

**Observation**: There's no `field_not_equals` type. This means you can't gate on "the scope decision is NOT 'blocked'" without using a command gate (`test "$(koto query evidence scope_decision)" != "blocked"`). It's a minor gap -- `field_equals` with an affirmative value ("proceed" rather than checking not-"blocked") is the recommended pattern. But some use cases naturally express as negation. Worth adding to the "likely future additions" list.

**Also missing from Phase 1**: A way to combine gates with OR logic. All gates on a state are implicitly AND (all must pass). This is the right default, but the design should acknowledge the AND-only limitation and note OR as a potential future need. Without this note, an implementer might wonder whether gate composition was considered.

---

## 10. Validation Contract Gaps

### Parse-time gap: Variable shorthand ambiguity

The design allows `TASK = "default-value"` as shorthand for `TASK = {default = "default-value"}`. But what about `TASK = ""`? Is that shorthand for `{default = ""}` or `{default = "", required = false}`? And `TASK = {required = true}` has no default -- is the default then `""`? The shorthand is convenient but introduces edge cases that should be specified exactly.

### Parse-time gap: Gate field cross-reference

The parse-time validation checks that "Gate declarations have valid types and required type-specific fields." But it doesn't check that a `field_not_empty` or `field_equals` gate references a key that could plausibly exist. Currently, evidence keys are set at transition time via `--evidence key=value`, so there's no template-level declaration of valid evidence keys. The `koto validate` check ("Gate references field not in variables") partially covers this but only for variables, not evidence fields. This might be intentional (evidence keys are dynamic) but the gap should be acknowledged.

### Missing: Transition line format evolution

The `**Transitions**: [state1, state2]` format is carried forward for backward compatibility. But the TOML header now declares transitions too. The parse-time validation should check for consistency: if both declare transitions for the same state and they disagree, that's a warning. The design mentions a "parser warning for redundancy" but doesn't specify what happens when they conflict (different lists). Is the TOML header always the authority? What if the body lists `[plan, escalate]` but the TOML header says `transitions = ["plan"]`? The behavior should be specified: TOML wins, body is ignored, warning emitted noting the discrepancy.

### Missing: Template version semantics

The `version` field is "informational, not enforced." But the engine stores `template_hash` in the state file, and the hash changes when the template changes. If someone bumps the template version without changing anything else, the hash changes and running workflows break (template_mismatch error). If someone changes the template content without bumping the version, the hash also changes and workflows break. The `version` field has no functional role in the current design. Should it? At minimum, the design should state explicitly that version is metadata only and the hash is the integrity mechanism.

---

## 11. TOML vs YAML: Is the Justification Strong Enough?

**Verdict: Yes, the justification is strong enough, with one improvement.**

The case for TOML rests on three pillars:
1. **Explicit typing**: TOML values are what they look like. No `yes`-becomes-`true` surprises.
2. **Smaller dependency**: ~3K lines vs ~15K lines.
3. **Natural nesting**: `[states.assess.gates.task_defined]` reads cleanly.

All three are valid. The upstream vision document's YAML examples show simple flat key-value pairs that YAML handles fine, but the evidence gate nesting reveals YAML's weaknesses quickly. A YAML evidence gate declaration:

```yaml
states:
  assess:
    transitions: [plan]
    gates:
      task_defined:
        type: field_not_empty
        field: TASK
```

...is workable but indentation-sensitive. The TOML equivalent:

```toml
[states.assess]
transitions = ["plan"]

[states.assess.gates.task_defined]
type = "field_not_empty"
field = "TASK"
```

...is arguably clearer for deeply nested structures. The trade-off is that TOML is less familiar to most developers than YAML, which could affect template authoring adoption. The design should acknowledge this trade-off explicitly: TOML is technically superior for this use case, but YAML has broader name recognition. The technical advantages outweigh the familiarity gap because template authors are likely Go developers (koto's primary audience) who are accustomed to TOML from Go tools.

**The improvement needed**: as noted in section 7, explicitly acknowledge the divergence from the upstream vision document's YAML examples.

---

## Summary

### What's Strong

- **Declared-state matching** is the design's best contribution. It solves heading collision cleanly without escape syntax or format compromises. The template remains valid, readable markdown.
- **Separation of concerns** between TOML header (machine configuration) and markdown body (human/agent content) is principled and well-argued.
- **Evidence gate type selection** is pragmatic. Three types cover the practical use cases without over-engineering. The `command` type serves as an escape hatch.
- **Security considerations** are thorough and honest. The command gate trust boundary is clearly identified. The mitigation table is concrete.
- **Validation split** (parse-time structural vs explicit semantic) is the right design. Hot-path validation stays fast; development-time validation catches deeper issues.
- **Implementation phases** are logically ordered with clear boundaries.

### What Needs Attention

1. **Initial state ordering assumption**: The design relies on TOML table insertion order, which isn't guaranteed by the TOML spec. Add an explicit `initial_state` field or document the library-specific behavior with a test.
2. **State name format**: Specify allowed characters, case sensitivity, and whitespace handling for state names.
3. **Command gate interpolation**: Explicitly state whether `{{VARIABLE}}` placeholders are expanded in command gate strings (they should not be, for security).
4. **Evidence storage in engine.State**: The design assumes evidence accumulation but the current state struct has no evidence field. Specify how evidence is stored.
5. **Upstream divergence**: Acknowledge the YAML-to-TOML pivot from the vision document explicitly.
6. **Code block handling**: Document that `## headings` inside fenced code blocks will incorrectly be treated as state boundaries (known limitation for Phase 1).
7. **Go struct definitions**: Add the Go struct definitions for the TOML schema to bridge the gap between specification and implementation.
8. **Missing JSON alternative**: Add and reject JSON front matter as an alternative for Decision 1 (it satisfies zero-dependency better than TOML).
9. **`**Transitions**:` line in non-state content**: Clarify that the transitions line parser only consumes transitions lines within recognized state sections, not within arbitrary body content.

### Blocking Concerns

**One potential blocker**: The initial state ordering assumption (#1 above). If BurntSushi/toml doesn't guarantee map ordering in the way the design expects, or if a future TOML library change breaks ordering, the initial state selection silently breaks with no error. This is fixable with a simple `initial_state = "assess"` field in the header, which also makes templates self-documenting. The fix is small and should be incorporated before implementation begins.

Everything else is addressable through documentation clarifications and can be resolved during implementation without changing the architecture.
