# Crystallize Decision: advanced-field-ux

## Chosen Type
PRD

## Rationale
The core gap is requirements, not architecture. The auto-advancement engine works correctly, but nobody ever wrote down what callers should see and how they should react. Every design doc specifies engine internals; none specify the caller-facing contract. Requirements emerged during exploration (not given as input), making PRD the right upstream artifact. A Design Doc can't meaningfully address "rename vs deprecate `advanced`" without first establishing what the contract requires.

## Signal Evidence
### Signals Present
- **Single coherent feature emerged**: The caller-facing behavioral contract for `koto next` -- a single, well-scoped feature that needs requirements definition.
- **Requirements are unclear or contested**: The `advanced` field has three different meanings. 9 of 14 outcomes are undocumented. No caller decision tree exists. The contract was never specified.
- **Multiple stakeholders need alignment**: The repo owner needs clarity on what the contract should be. AI agent callers need a stable contract to build against. Template authors need to understand what their templates produce.
- **Core question is "what should we build and why?"**: What should callers see for each state machine outcome? What should each field mean? What error codes should exist?
- **User stories or acceptance criteria are missing**: Issue #102 has narrow acceptance criteria ("callers can distinguish new phase from same phase"). The broader contract has none.

### Anti-Signals Checked
- **Requirements were provided as input**: Not present. Requirements emerged during exploration -- the user started by saying they don't understand the system's behavior.
- **Multiple independent features**: Not present. This is one coherent contract, not separable features.
- **Independently-shippable steps**: Not present. Contract changes need coordination across response shapes, error codes, and documentation.

## Alternatives Considered
- **Design Doc** (score 2, demoted): Has the "what to build is still unclear" anti-signal. Technical decisions (rename vs deprecate `advanced`, split error codes) exist but can't be made without requirements. Design follows PRD.
- **Plan** (score -2): No upstream artifact exists. Approach still debated. Can't sequence work that hasn't been decided.
- **No Artifact** (score -3): Others need documentation. Decisions were surfaced. Multiple callers affected. Not a "just do it" situation.
- **Decision Record** (deferred type): Partially fits for the `advanced` field question alone, but the scope exceeds a single decision. PRD captures all requirements.
