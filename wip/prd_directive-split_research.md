# Research: Directive/Details Split

## Research conducted

Investigated five critical unknowns by reading source code, template fixtures, design docs, and the existing PRD.

### 1. How does the current template format define `directive`? What types are supported?

**Source**: `src/template/types.rs:47-61`, `src/template/compile.rs:379-425`

`TemplateState.directive` is a plain `String`. In the source template format (.md with YAML frontmatter), the directive is not declared in the YAML -- it comes from the markdown body. The compiler (`extract_directives`) splits the body on `## <state-name>` headings: everything between two state headings becomes that state's directive string (trimmed, newlines preserved). Non-state `##` headings inside a state section are kept as content, not treated as boundaries.

The compiled JSON stores `directive` as a flat string. No structured sub-fields, no arrays, no rich type -- just a string that may contain markdown formatting, `{{VAR}}` references, and newlines.

### 2. How does the engine track state visits? Can it distinguish first visit from retry?

**Source**: `src/engine/persistence.rs:221-228`, `src/engine/types.rs` (Event/EventPayload), `src/engine/advance.rs`

The engine derives current state by replaying the JSONL event log: `derive_state_from_log` scans events in reverse for the most recent `Transitioned`, `DirectedTransition`, or `Rewound` event and returns its `to` field. The advance loop (`advance_until_stop`) tracks visited states within a single invocation using a `HashSet<String>` for cycle detection, but this set is not persisted.

**Visit counting is not currently implemented.** There is no `visit_count` field, no per-state visit tracker, and no mechanism to distinguish "first time entering state X" from "re-entering state X after a retry." However, the JSONL log contains all the information needed: counting `Transitioned { to: "X" }` events would give the visit count. This is a straightforward derivation from existing data -- no schema changes to the event log are needed.

The `Rewound` event type also enters a state, so visit counting must include rewind-to events. `DirectedTransition` (from `--to`) also counts as a visit.

### 3. How large are directive texts in practice?

**Source**: Test fixtures and the hello-koto plugin template.

Current directives are short -- typically 1-3 lines:
- `"Choose a route: setup or work."` (multi-state.md, entry state)
- `"Check whether wip/check.txt exists..."` (simple-gates.md, start state -- one sentence)
- `"You are {{SPIRIT_NAME}}, a tsukumogami spirit...\n\nWrite a greeting..."` (hello-koto.md -- 2 paragraphs, ~200 chars)
- `"Analyze the task: {{TASK}}"` (var-substitution test)

Production templates for complex workflows like work-on are still under design (DESIGN-shirabe-work-on-template.md), but the design doc anticipates state directives that include multi-paragraph instructions with context, checklists, and examples. The PRD-koto-next-output-contract.md explicitly mentions that templates "tell the agent to 'read file X' in the directive text, forcing an extra tool call" as the current workaround for long instructions. This confirms the motivation: extended instructions can be much longer than the summary.

### 4. Are there existing patterns in the codebase for conditional field inclusion?

**Source**: `src/cli/next_types.rs` (NextResponse serialization)

Yes. The codebase uses two patterns:

1. **`#[serde(skip_serializing_if = "...")]`** on struct fields -- used throughout template types for optional fields (e.g., `accepts`, `integration`, `default_action`). Fields are omitted from JSON when `None` or empty.

2. **Custom `Serialize` impl with conditional field inclusion** -- `NextResponse` uses a hand-written `Serialize` implementation that controls exactly which fields appear in each variant. For example, `Terminal` omits `directive` entirely. `GateBlocked` includes `blocking_conditions` but `EvidenceRequired` doesn't. This is the pattern the `details` field would follow: include it in the map only when non-empty and first-visit conditions are met.

The `NextResponse::with_substituted_directive` method applies variable substitution to the directive field post-construction. A `details` field would need the same treatment -- the method already pattern-matches on all variants, so adding `details` substitution is mechanical.

### 5. How does variable substitution interact with directive text?

**Source**: `src/engine/substitute.rs`, `src/cli/mod.rs:1346-1349`, `src/cli/vars.rs`

Variable substitution is applied at response time, not compile time. The flow:

1. Compile: directive is extracted verbatim from markdown (with `{{VAR}}` references intact)
2. At `koto next` time: `with_substituted_directive` applies two substitution passes:
   - `cli::vars::substitute_vars` replaces runtime variables (`{{SESSION_DIR}}`, `{{SESSION_NAME}}`)
   - `variables.substitute` replaces template variables from `WorkflowInitialized` event

Both use the `{{KEY}}` pattern (uppercase letters, digits, underscores). Values are validated against an allowlist regex (alphanumeric, dots, hyphens, slashes). Substitution is single-pass (no re-expansion).

A `details` field would need the same two-pass substitution. Since `with_substituted_directive` already applies the transformation, extending it to also substitute `details` is straightforward.

## Findings

### Option 1: Two template fields (`directive` + `details`)

**Template source impact**: The YAML frontmatter has no `directive` field today -- directives come from the markdown body. Adding `details` as a YAML field would break the pattern. Instead, the markdown body would need a convention to split directive content into summary and details sections within a state's `## heading` block.

Possible sub-approaches:
- **YAML `summary` field**: Add an optional `summary` field in the YAML state declaration. If present, it becomes `directive` in the compiled JSON and the markdown body becomes `details`. If absent, current behavior (markdown body = directive, no details).
- **Markdown separator**: Use a marker within the state's markdown section (e.g., `---` or a specific heading like `### Details`) to split summary from details. The compiler extracts the part before the separator as `directive` and the part after as `details`.

**Compiled JSON**: Would add `details: String` to `TemplateState` with `#[serde(default, skip_serializing_if = "String::is_empty")]`.

**Response impact**: `NextResponse` variants that include `directive` would gain an optional `details` field. The custom `Serialize` impl conditionally includes it based on (a) the field being non-empty and (b) first-visit logic.

### Option 2: Single template field with separator

**Template source impact**: Minimal. Authors add a separator (e.g., `---`) within their state's markdown section. Everything before it is the summary, everything after is extended instructions. The compiler splits on the separator.

**Risk**: `---` is common in markdown (horizontal rule, YAML delimiter). Need an unambiguous marker. Could use `<!-- details -->` (HTML comment) which won't render in GitHub previews.

**Compiled JSON**: Same as Option 1 -- the compiled format still has two fields. The difference is only in the source format.

### Option 3: File reference (`details_file`)

**Template source impact**: Add an optional `details_file` field to the YAML state declaration. Points to a markdown file relative to the template.

**Risk**: Introduces file resolution at compile time or runtime. The compiled JSON would need to inline the content (compile-time) or carry the path (runtime). Inlining at compile time is the simpler approach and keeps the engine dependency-free.

**Advantage**: Keeps directive sections short and readable. Extended instructions live in separate files, which is natural for very long instructions.

**Disadvantage**: Template authors manage multiple files. Discovery is harder. Variable substitution must apply to the referenced file content too.

### Visit tracking implementation

All three options need the same visit-tracking mechanism. The JSONL log already contains all state-entry events (`Transitioned`, `DirectedTransition`, `Rewound`). A function `count_state_visits(events: &[Event], state: &str) -> usize` would scan the log and return how many times the state was entered. First visit = count of prior entries is 0 at the moment the current entry was appended.

This function would live in `src/engine/persistence.rs` alongside `derive_state_from_log` and `derive_evidence`.

### Force-full mechanism

A `--full` flag on `koto next` would bypass the first-visit check and always include `details`. This is orthogonal to the template format choice. Implementation: the flag propagates to the dispatch layer, which conditionally includes `details` regardless of visit count.

## Assumptions made

1. **Compiled JSON is the right place to store details.** The compiled format already inlines directive content. Inlining details at compile time keeps the engine simple (no file I/O at runtime for details).

2. **Visit counting from JSONL replay is acceptable.** For workflows with thousands of events, scanning the full log for visit counts adds cost. In practice, koto workflows have tens to low hundreds of events, so this is not a concern.

3. **Variable substitution applies to details identically to directive.** No reason to treat them differently -- both are template author content with `{{VAR}}` references.

4. **Backward compatibility means "absent field, not null."** The codebase uses `skip_serializing_if` for optional fields, producing absent keys rather than `null` values. States without `details` should produce no `details` key in JSON output.

## Clean summary

The `directive` field is a plain string extracted from markdown body sections at compile time and stored in the compiled JSON. Variable substitution happens at response time via `with_substituted_directive`. The engine has no visit counter, but the JSONL event log contains all entry events, making visit counting a pure derivation with no schema changes.

All three template format options converge to the same compiled representation: two string fields (`directive` + `details`) in `TemplateState`, with `details` omitted when empty. The options differ only in how template authors specify the split in the source format.

The response layer (`NextResponse` custom serialization) already supports conditional field inclusion per variant. Adding `details` follows the established pattern. The `with_substituted_directive` method needs extension to also substitute variables in `details`.

The force-full mechanism (`--full` flag) is orthogonal to the format choice and straightforward to implement as a boolean propagated through the dispatch chain.

Recommended approach: Option 2 (markdown separator within state sections) for the source format, using `<!-- details -->` as the marker. This minimizes changes to the template authoring experience (no new YAML fields, no external files), keeps templates self-contained, and the compiler change is localized to `extract_directives`. The compiled format and response layer changes are identical regardless of source format choice.
