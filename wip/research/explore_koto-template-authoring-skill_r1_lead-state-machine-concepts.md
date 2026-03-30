# Lead: What does the skill need to know about koto's state machine?

## Findings

### Core architecture

Koto's state machine is a compiled, event-sourced workflow engine. Key characteristics:
- Templates compile from Markdown source to JSON
- State transitions are recorded as an event log
- The engine routes agents through states based on directives, gates, and evidence

### Essential concepts for template authors

1. **States**: nodes in the workflow graph. Each has:
   - A **directive**: instructions for the agent (what to do in this state)
   - Outgoing **transitions**: edges to other states

2. **Transitions**: edges between states, with optional routing:
   - Simple transitions: always fire (one outgoing edge)
   - Evidence-routed transitions: `when` conditions select which edge fires based on submitted evidence
   - AND logic across `when` conditions, with compile-time mutual exclusivity validation

3. **Evidence**: structured data agents submit to trigger routing:
   - `accepts` blocks define the schema (what fields the state expects)
   - Evidence values match `when` conditions on transitions
   - Compile validates that evidence routing is mutually exclusive (no ambiguous paths)

4. **Gates**: environmental prerequisites that block state entry:
   - `command`: run a shell command, pass if exit code 0
   - `context-exists`: check a file exists
   - `context-matches`: regex match on file content

5. **Variables**: named parameters with `{{UPPERCASE}}` interpolation:
   - Defined in template frontmatter
   - Substituted in directives and actions at runtime

### Minimal mental model

For an agent authoring a template, the minimum understanding is:
- States have directives and outgoing transitions
- Evidence values match `when` conditions to select which transition fires
- Gates block until environmental conditions pass
- Variables interpolate in directives and actions

### Validation guarantees

The compiler catches:
- Missing transition targets
- Invalid regex patterns in gates
- Unreferenced variables
- Non-mutually-exclusive evidence routing
- 13+ total validation rules

## Implications

The skill should teach this mental model in layers:
1. Basic: states with directives and simple transitions (linear workflow)
2. Intermediate: evidence-based routing (branching workflow)
3. Advanced: gates, variables, default_action, integration hooks

Starting with a linear workflow scaffold and building up is the right pedagogical approach.

## Surprises

The compile-time mutual exclusivity check for evidence routing is a strong constraint. Template authors must design routing so that for any given set of evidence values, exactly one transition can fire. This is non-obvious and the skill should teach it explicitly.

## Open Questions

- What does the event-sourced log look like, and should template authors understand it?
- How do `default_action` and `integration` work? Are they advanced topics to defer?
- What's the simplest valid template (minimal states and transitions)?

## Summary

Koto's state machine routes agents through states based on directives, gates, and evidence. The minimal mental model for authoring: states have directives and outgoing transitions, evidence values match `when` conditions to select transitions, gates block until environmental conditions pass, and variables interpolate via `{{UPPERCASE}}`. The skill must teach evidence routing mutual exclusivity (a compile-time constraint) and should layer concepts from basic linear workflows up to branching with evidence.
