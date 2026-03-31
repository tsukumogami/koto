# Decision Report: Gate Schema Ownership

## Question
How should gate output schemas be defined -- by the gate type (built-in, strongly typed) or by the template author (user-declared)?

## Chosen: Option C (Gate type provides base schema, future types extend)

## Confidence: High

## Rationale

The fundamental problem with user-declared schemas (Option B) is that there's
no mechanism connecting a gate's raw execution to the declared schema. A
command gate runs a shell command and gets an exit code. If the template
declares `output_schema: {status: enum [passed, failed]}`, nothing in the
system converts exit code 0 to `{status: "passed"}`. That parsing logic
doesn't exist and would have to be invented per-gate, per-template.

Option A (pure built-in) solves this completely -- the gate type defines both
the output shape and the parsing logic. But it's too rigid for the future
vision of richer gates. If a CI system returns JSON with coverage data, the
only way to expose that is a new gate type.

Option C gets both: simple, strongly typed base schemas for existing gate
types (command -> {exit_code: number}, context-exists -> {exists: boolean},
context-matches -> {matches: boolean}), with extensibility through new gate
types that have richer built-in parsing (json-command parses stdout as JSON,
http returns status code and headers, etc.).

The key insight is that parsing logic belongs to the gate type implementation,
not to the template. A template author picks a gate type and gets whatever
output that type produces. The compiler knows the schema from the type and
validates `when` clauses against it. No runtime surprises.

This also simplifies override_default and pass_condition: they're defined
against the gate type's known schema. The compiler can validate them fully
at compile time because it knows every field and its type.

## Assumptions
- Existing gate types (command, context-exists, context-matches) have simple
  enough output that a fixed schema per type is sufficient
- Future richer output will come through new gate types, not through extending
  existing types
- The compiler maintains a registry of gate type -> output schema mappings

## Rejected

**Option A (pure built-in)**: too rigid. Correct for today's gate types but
doesn't account for the user's stated vision of gates returning richer data
(coverage, matched content, severity). Option C is Option A with an
extensibility path.

**Option B (user-declared)**: has an unsolvable parsing gap. The template
declares a schema but nothing converts the gate's raw output to match it.
Would require a per-gate parsing DSL or scripting layer, which is massive
scope creep. The user identified this exact problem: "where would we keep
the logic to convert the exit code / log of the command back to an instance
of the schema?"
