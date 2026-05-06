# Crystallize Decision: session-schema-hygiene

## Chosen Type

PRD

## Rationale

The exploration's primary output is a requirements specification with precise field-level contracts. The four additions were identified at a high level before exploration, but the exact field names, types, required/optional status, ordering guarantees, and backward compatibility contracts were not. The exploration resolved all of these through codebase research. The result is requirements — what the implementation must do — not a technical architecture choice.

The PRD will stand alone without upstream references and must justify each field's non-back-fillable status from first principles. Multiple stakeholders (current and future koto contributors) need the written contract to implement and validate the additions consistently.

## Signal Evidence

### Signals Present

- **Single coherent feature**: All four additions are part of one schema hardening effort; they share scope and must ship together
- **Core question is "what to build and why"**: The exploration established not just that additions are needed but exactly what each field looks like (name, type, semantics, ordering guarantee)
- **Acceptance criteria missing from codebase**: No formal specification existed for any of the four additions; the PRD creates it

### Anti-Signals Checked

- **Requirements were provided as input**: Partially true at a high level (four fields identified). However the detailed specifications (field names, types, ordering contract, backward compat policy) were not given — exploration produced them. Tiebreaker: requirements were **identified** by exploration, not given. → PRD confirmed.

## Alternatives Considered

- **Design Doc**: Ranked second. Technical decisions were made (UUID v4, milliseconds, synchronous emission). But these are implementation choices that support the requirements, not architectural decisions that belong in a design doc. The core question is "what" not "how."
- **No Artifact**: Ruled out. Multiple contributors need the written contract; the scope requires multi-person coordination; four architectural choices were made that future contributors need to understand.
- **Decision Record**: Ruled out. Four interrelated decisions share scope — a design doc or PRD is more appropriate than four separate ADRs.

## Deferred Types

None applicable.
