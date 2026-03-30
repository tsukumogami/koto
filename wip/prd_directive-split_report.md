<!-- decision:start id="directive-split" status="assumed" -->
### Decision: How should the directive/details split be specified in the PRD?

**Context**

Issue #90 proposes splitting `koto next` response directives into a short summary (always returned) and extended instructions (first-visit only). The current `directive` field is a plain string extracted from markdown template bodies at compile time. There's no visit tracking, but the JSONL event log contains all state-entry events, making visit counting a pure derivation. The codebase already supports conditional field inclusion in responses via custom `Serialize` impls.

The question has two parts: (1) what should the PRD specify about the output contract, and (2) should the PRD also prescribe the template source format?

**Assumptions**

- Visit counting from JSONL replay is acceptable for typical workflow sizes (tens to hundreds of events)
- Variable substitution applies to `details` identically to `directive`
- Absent field (not null) is the right representation for omitted `details`, matching existing codebase patterns

**Chosen: PRD specifies the output contract (details field behavior), defers template source format to design**

The PRD should specify:
- A `details` field (string, optional) on all non-terminal response shapes
- Present on first visit to a state (when the template defines details for that state)
- Absent on subsequent visits and for states without details
- A `--full` flag on `koto next` that forces `details` to be included regardless of visit count
- States without `details` produce no `details` key (backward compatible, absent not null)

The PRD should NOT prescribe how template authors write the split (markdown separator vs. YAML field vs. file reference). That's a design-level decision about the template compiler, not a caller-facing contract requirement. The PRD defines what callers see; the design doc defines how template authors produce it.

**Rationale**

The PRD's scope is the caller-facing output contract. Callers care about the `details` field in the JSON response -- its presence, absence, and semantics. They don't care whether the template used a markdown separator or a YAML field to produce it. Mixing output contract requirements with template format requirements in the same PRD muddies the scope boundary.

The research confirmed that all three template format options converge to the same compiled representation (`directive` + `details` strings). The output contract is format-agnostic. Prescribing the template format in the PRD would overconstrain the design without benefiting callers.

**Alternatives Considered**

- **Markdown separator (`<!-- details -->`)**: Research recommended this for minimal template author friction. Good option for the design doc, but a template format choice, not an output contract choice. Rejected from the PRD, not from the project.
- **YAML `summary` field**: Explicit and discoverable, follows existing YAML patterns. Also a design-level choice. Rejected from the PRD scope.
- **External file reference (`details_file`)**: Most flexible for very long instructions. Adds file management complexity. Rejected from the PRD scope.
- **Prescribe format in the PRD**: Would make the PRD more concrete but crosses the what/how boundary. The user's instruction ("a portion of the step instructions to always return on next, and another, larger portion to only return when that step is seen for the first time") describes the output behavior, not the template format.

**Consequences**

- The PRD's R9 (directive split) becomes purely about the response contract: what `details` means, when it appears, how to force it
- A downstream design doc must decide the template source format
- Template authors don't get guidance from the PRD alone -- they need the design doc
- The output contract is testable independently of the template format (test with hand-crafted compiled templates)
<!-- decision:end -->
