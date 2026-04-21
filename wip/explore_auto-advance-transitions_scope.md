# Explore Scope: auto-advance-transitions

## Visibility

Public

## Core Question

How should koto implement a `skip_if` predicate that auto-advances deterministic state transitions without requiring agent evidence, writes a synthetic event to the log to preserve resume-awareness, and chains consecutive auto-advancing states within a single advance loop turn?

The feature is motivated by plan-backed orchestrator workflows where 4 boilerplate states per child workflow require mechanical evidence submissions with no decision value. Without chaining, the feature saves evidence composition but not round-trips, delivering approximately 20% of the intended value.

## Context

The advance loop (`src/engine/advance.rs`, `advance_until_stop()`) already auto-advances states that have an unconditional fallback transition and no `accepts` block. Gate outputs are already synthesized into the evidence map. The template schema (`src/template/types.rs`) defines `TemplateState` with `accepts`, `gates`, `transitions`, and `skipped_marker` fields -- no `skip_if` concept exists yet.

The reporter identified three condition types needed: template variable existence/value, context key existence, and evidence field values from the current state. The `when` clause on transitions already matches evidence fields; `context-exists` and `context-matches` gates already check context keys; template variable access at advance-time is uncharted.

A synthetic `evidence_submitted` event (or new event type) must be written so a resuming agent can reconstruct why a state was passed -- not just that it was.

## In Scope

- `skip_if` predicate syntax in template YAML frontmatter
- Condition types: template variable existence/value, context key existence, evidence field value
- Chaining semantics: consecutive skip_if states resolving in a single loop turn
- Synthetic event written to the log when skip_if fires
- Engine advance loop integration point and modifications
- Template schema changes (new field, validation rules)

## Out of Scope

- Introspection tooling (listing which states are skip_if-eligible at runtime)
- Collapsing or removing states from existing templates
- Changes to `koto status` display format (unless forced by event schema)
- Multi-condition boolean logic (AND/OR combinators); single predicate is enough for v1

## Research Leads

1. **Where exactly in the advance loop does skip_if evaluation fit, and what is the minimal change to `advance_until_stop()` to support chaining?**
   Read `src/engine/advance.rs` lines 167-532 in detail. Map the gate evaluation → transition resolution → evidence blocking flow. Identify the insertion point for skip_if evaluation and the loop modification needed for chaining (continue vs. return). The cycle detection at line 472 is relevant -- does it already prevent infinite auto-advance chains?

2. **How are template variables accessed at advance-time, and what does a `skip_if` condition predicate need to evaluate?**
   The three condition types are: template variable (e.g., `SHARED_BRANCH` is set), context key existence (e.g., `context.md` present), and evidence field value. Understand how template variables flow from initialization into the advance loop. The `context-exists` gate evaluator already checks context keys -- can skip_if reuse that logic? What data structures are available at the evaluation point?

3. **What is the right synthetic event for the log -- reuse `evidence_submitted` with a marker field, or a new event type?**
   Read the existing `EvidenceSubmitted` event payload and how it's consumed during state reconstruction (persistence.rs, `derive_current_state()`). A resuming agent needs to reconstruct *why* a state was passed (e.g., "context.md existed"). Evaluate: does a `synthetic: true` flag on `evidence_submitted` suffice, or does a distinct event type (`auto_advanced`) give cleaner reconstruction semantics?

4. **What YAML frontmatter syntax should `skip_if` use, and how does it compose with `accepts`, `gates`, and `transitions`?**
   Read the existing template schema and the YAML parser (`src/template/`). Design the `skip_if` field: what is its structure, and what are the validation rules? Can a state have both `skip_if` and `accepts`? (If skip_if fires, accepts is bypassed; if skip_if conditions are unmet, falls through to normal evidence blocking.) How does skip_if interact with gates that have already evaluated on the current turn?

5. **Are there existing tests or functional scenarios that cover the current auto-advance path, and what test gaps would skip_if introduce?**
   Read `test/` for existing scenarios covering unconditional auto-advance. What new scenarios are needed: single skip_if firing, chained skip_if, skip_if with unmet conditions, resume after skip_if, and skip_if interacting with gates.
