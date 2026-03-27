# Lead: Gate evaluation with koto-owned context

## Findings

### Current gate model
- Gates are shell commands defined in templates (e.g., `test -f {{SESSION_DIR}}/plan.md`)
- Engine evaluates gates via closure-based evaluator — flexible architecture
- Gate evaluation happens inside `advance_until_stop` loop, per-state

### Three options

**Option A: Internal gate evaluation with new types**
- Define gate types: `exists`, `matches`, `content-contains`
- Gates become first-class koto operations, no shell dependency
- Cleanest long-term, but breaks existing templates

**Option B: Expose CLI for shell gates**
- `koto ctx exists <session> --key plan.md` returns exit code 0/1
- Shell gates rewrite: `koto ctx exists my-session --key plan.md` instead of `test -f`
- Maintains backward compatibility with shell gate model
- Minimal engine changes

**Option C: Hybrid**
- Built-in gate types for common checks (exists, content-match)
- Shell fallback for complex logic
- Non-breaking upgrade path: old templates still work, new templates use built-in types
- Covers 80% of use cases natively; 20% remains shell-based

### Engine flexibility
- The closure-based gate evaluator can support any option without engine changes
- The decision is CLI design, not architectural constraint

## Implications
- Option C (hybrid) is the pragmatic choice for incremental migration
- Existing templates keep working while new ones use built-in gate types
- `koto ctx exists` is needed regardless — useful for resume logic too

## Surprises
- The engine architecture doesn't constrain the gate model at all — it's purely a CLI/template design choice

## Open Questions
- Should built-in gate types be defined in the template YAML or in the engine?
- How do gate timeouts work with koto-internal evaluation (no shell process to kill)?
- Can `koto ctx exists` be fast enough for gate evaluation in tight loops?

## Summary
The engine's closure-based gate evaluator supports any approach without changes. A hybrid model (built-in gate types for common checks + shell fallback) offers a non-breaking upgrade path. `koto ctx exists` is needed for both gate evaluation and resume logic, making it a high-value primitive.
