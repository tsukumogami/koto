# Lead: How should variable values be stored — strings or JSON Value?

## Findings

### Current type definitions
- `EventPayload::WorkflowInitialized` uses `variables: HashMap<String, serde_json::Value>`
- `VariableDecl` in template types uses `default: String` (not Value)
- `WorkflowInitializedPayload` (deserialization helper) also uses `HashMap<String, serde_json::Value>`

### Template declarations are string-typed
`VariableDecl` has:
- `description: String`
- `required: bool`
- `default: String`

There's no `type` field on variable declarations. The default is a plain string.
Templates have no mechanism to declare typed variables.

### Usage context is string-only
Variables are substituted into:
1. Shell command strings (gates): `test -f wip/issue_{{ISSUE_NUMBER}}_context.md`
2. Directive text (plain text): "Working on task {{TASK_ID}} with prefix {{PREFIX}}"

Both are inherently string contexts. A JSON object or array makes no sense in
`test -f wip/issue_{{SOME_OBJECT}}_context.md`.

### The field is currently unused
The `variables` field in `WorkflowInitialized` is always `HashMap::new()`. No code
reads it, writes to it with real data, or processes it. Changing the type from
`serde_json::Value` to `String` has zero backward-compatibility impact.

### Serialization compatibility
State files are JSONL. A `HashMap<String, String>` serializes as `{"KEY": "value"}`
which is a subset of what `HashMap<String, serde_json::Value>` produces. If a future
version needs typed values, it can read string-only state files without issue.

## Implications

Narrowing to `HashMap<String, String>` is the right call:
- Matches template declarations (default is String)
- Matches usage context (shell commands and text)
- Matches CLI input (--var KEY=VALUE produces strings)
- Eliminates Value-to-String conversion at every substitution call site
- No backward-compatibility concern (field is unused)
- Forward-compatible: upgrading to Value later is additive, not breaking

## Surprises

The mismatch between `VariableDecl.default: String` and `EventPayload.variables:
HashMap<String, Value>` was likely an oversight during initial type definition —
the event type was speculative and never wired up.

## Open Questions

None — this is a clear decision point with strong evidence for String.

## Summary

Store variables as `HashMap<String, String>` in both the event and at runtime. This
matches template declarations (string-typed defaults), CLI input, and the usage context
(shell commands and directive text). The change is non-breaking since the field is
currently unused, and upgrading to Value later would be additive if typed variables
are ever needed.
