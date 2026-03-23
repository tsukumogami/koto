# Lead: How should koto init validate variables and report errors?

## Findings

### Current init flow (src/cli/mod.rs)
The init command currently:
1. Parses CLI args (name, --template)
2. Loads and validates the compiled template
3. Creates the WorkflowInitialized event with empty variables
4. Writes the initial state file

No variable parsing, validation, or error handling exists.

### Variable declaration structure (src/template/types.rs)
`VariableDecl` has three fields relevant to validation:
- `required: bool` — must be provided at init time
- `default: String` — used when not provided (empty string means no default)
- `description: String` — useful for error messages

### Existing error patterns in koto
- Init uses structured JSON errors: `serde_json::json!({"error": "message", "command": "init"})`
- Exit codes: 1=transient, 2=caller error, 3=infrastructure
- The `next` command has typed errors (`NextError`, `NextErrorCode`) as a more mature
  pattern to follow
- Template validation in `CompiledTemplate::validate()` provides contextual errors:
  "state X gate Y: message"

### Clap integration
No repeatable flags used yet in koto CLI. Clap v4 supports this via:
```rust
#[arg(long = "var", value_name = "KEY=VALUE")]
vars: Vec<String>,
```

### Validation sequence
1. **Parse `--var KEY=VALUE`**: Split on first `=`. Reject if no `=` found. Handle
   duplicate keys (last wins, or error — error is safer).
2. **Load template**: Extract `variables` declarations from compiled template.
3. **Reject unknown keys**: User-provided vars not declared in template.
4. **Check required vars**: Template declares required but user didn't provide.
5. **Apply defaults**: Optional vars not provided get their default value.
6. **Sanitize values**: Apply allowlist validation (from sanitization lead).
7. **Build final map**: Store in WorkflowInitialized event.

### Error message quality
Following koto's contextual error pattern:
- `error: unknown variable "FOO" (template declares: ISSUE_NUMBER, ARTIFACT_PREFIX)`
- `error: missing required variable "ISSUE_NUMBER" (The task identifier)`
- `error: duplicate --var key "ISSUE_NUMBER" (provided twice)`
- `error: invalid value for variable "ISSUE_NUMBER": contains forbidden character ";"
  (allowed: alphanumeric, hyphens, underscores, dots, forward slashes)`
- `error: malformed --var argument "KEYVALUE" (expected KEY=VALUE format)`

### Variable name validation
Variable names in templates use `UPPER_SNAKE_CASE` (ISSUE_NUMBER, ARTIFACT_PREFIX,
TASK). The `--var` flag should accept the name as-is from the template declaration.
Name validation: `^[A-Z][A-Z0-9_]*$`.

## Implications

The validation flow is straightforward — parse, cross-reference with template
declarations, validate values, store. The error messages should include the variable's
description from the template declaration to help users understand what's expected.

Exit code 2 (caller error) is appropriate for all validation failures since they're
caused by incorrect user input.

## Surprises

koto's error handling is inconsistent — init uses ad-hoc JSON errors while next has
typed error codes. The design should follow the more mature next pattern.

## Open Questions

1. Should duplicate `--var` keys error or use last-wins semantics? Error is safer
   and avoids silent bugs.
2. Should variable name validation be case-sensitive? Yes — templates use UPPER_SNAKE
   and the names should match exactly.

## Summary

The validation sequence is: parse --var strings, cross-reference with template
declarations (reject unknown, enforce required, apply defaults), sanitize values,
then store. Error messages should be contextual and actionable, including the
variable's description from the template. Exit code 2 (caller error) for all
validation failures.
