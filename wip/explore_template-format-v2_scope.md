# Explore Scope: template-format-v2

## Core Question

What's the precise tactical design for koto's template format v2? The strategic
design defines `accepts`/`when`/`integration` blocks replacing `transitions: []string`,
but the tactical design needs exact YAML syntax, compiled JSON schema, Rust types,
compiler validation rules, and interaction model with existing gates.

## Context

Issue #47 implements template format v2 for koto. The strategic design
(`DESIGN-unified-koto-next.md`) defines the high-level shape: `accepts` blocks for
evidence field schema, `when` conditions per transition for routing, and `integration`
string tags. The current v1 format uses simple `transitions: Vec<String>` and `gates`
(field_not_empty, field_equals, command). koto has no users, so this is a clean break
with no migration concerns.

The event log format (#46) is merged. Issues #48 (CLI output contract) and #49
(auto-advancement engine) are parallel/downstream work.

## In Scope

- Template source YAML syntax for `accepts`, `when`, `integration`
- Compiled JSON schema (FormatVersion=2)
- Rust type definitions for v2 structures
- Compiler validation rules including mutual exclusivity
- Validator updates for v2 schema
- `koto next` template loading changes for v2
- Interaction between `accepts`/`when` and existing `gates`

## Out of Scope

- Auto-advancement engine (#49)
- Full CLI output contract with `expects` field (#48)
- Evidence submission (`--with-data`)
- Integration runner execution
- Snapshot mechanism for long logs

## Research Leads

1. **What's the exact v2 compiled JSON schema?**
   The v1 format has `transitions: Vec<String>`. V2 needs `accepts`, structured
   transitions with `when` conditions, and `integration`. How do these serialize
   in the compiled JSON output?

2. **How should mutual exclusivity validation work in the compiler?**
   The strategic design says single-field cases are validated; multi-field cases
   are the author's responsibility. What's the algorithm and what error messages
   does it produce?

3. **How do `accepts`/`when` interact with existing gates?**
   Gates are koto-verifiable conditions (command, field_not_empty, field_equals).
   States with no `accepts` and no `when` auto-advance when gates pass. What's
   the precise interaction model?

4. **What changes are needed in `koto next` template loading?**
   Currently `koto next` reads `transitions` and `directive`. With v2, it needs
   to read `accepts` and structured transitions. What does the output look like
   before #48 adds the full output contract?

5. **How should the `integration` field compile?**
   It's a string tag -- the runner is in #49. What does the compiler store, and
   what does `koto next` do when it encounters one before the runner exists?
