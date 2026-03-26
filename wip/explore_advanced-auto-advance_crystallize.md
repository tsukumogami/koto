# Crystallize Decision: advanced-auto-advance

## Chosen Type
Design Doc

## Rationale
The exploration made architectural decisions across three rounds that need a permanent home. The wip/ directory is cleaned before merge, so without a design doc, the rationale behind engine-layer placement, transition_count selection, and advanced field deprecation would be lost. The "what to build" is clear (issue #89's intent is validated), but "how to build it" has layering, contract, and backward-compatibility considerations that a design doc captures for future contributors.

## Signal Evidence
### Signals Present
- **What to build is clear, how is not**: Issue #89's goal is validated but the implementation approach needed investigation -- engine vs CLI vs caller convention, response contract evolution
- **Technical decisions between approaches**: Three architecture layers compared (engine, CLI, caller); two observability approaches compared (passed_through vs transition_count); response contract redesign vs extension
- **Multiple viable implementation paths**: Engine loop extension, CLI handler loop, caller convention all investigated as options
- **Decisions made during exploration**: Engine layer chosen, transition_count chosen over passed_through, advanced deprecation path established, koto state deferred for rich observability

### Anti-Signals Checked
- **No meaningful technical risk**: Partially present -- the behavioral fix is low-risk, but response contract evolution (backward compat, advanced deprecation) has trade-offs. Outweighed by the volume of decisions that need recording.

## Alternatives Considered
- **Plan**: Ranked second. Work decomposes cleanly but no upstream design doc exists to reference. Decisions would be lost when wip/ is cleaned.
- **No Artifact**: Ranked low. Multiple architectural decisions were made across 3 rounds; "just do it" loses the rationale.
- **Update issue #89 directly**: Not a framework type, but considered. Would be fastest but loses architectural context.
