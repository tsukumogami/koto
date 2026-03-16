# Lead: How should the `integration` field compile?

## Findings

The `integration` field is a string tag on a state that declares a processing
integration. The strategic design specifies:

- Integration names resolve from a closed set (project config or plugin manifest)
- A template declaring `integration: some-name` routes to the configured handler
- The actual command/process is in user/project configuration, not the template
- Graceful degradation: missing integration config is NOT a template load-time error
- `koto next` degrades to returning the directive without integration output

### Compiler behavior (Issue #47)

The compiler should:
1. Store `integration` as `Option<String>` in the compiled `TemplateState`
2. No validation of integration names against a config at compile time
3. Pass through the string tag verbatim from YAML source to compiled JSON
4. Basic validation only: non-empty string if present

### Source YAML

```yaml
states:
  delegate_analysis:
    integration: delegate_review
    accepts:
      interpretation:
        type: string
        required: true
    transitions:
      - target: next_step
```

### Compiled JSON

```json
{
  "delegate_analysis": {
    "directive": "...",
    "integration": "delegate_review",
    "accepts": { ... },
    "transitions": [{ "target": "next_step" }]
  }
}
```

### Runtime behavior before #49

When `koto next` encounters an integration state before the runner exists:
- The integration field is present in the loaded template but unused
- `koto next` returns the directive normally, ignoring the integration tag
- #49 adds the runner that invokes integrations and appends `integration_invoked` events

## Implications

The integration field is trivial for #47: just an `Option<String>` stored verbatim.
No compile-time validation of names. The complexity lives in #49 (runner) and #48
(output contract with `integration.available`).

## Surprises

None. The strategic design explicitly says missing integration config is not a
template load-time error, so the compiler has nothing to validate beyond basic
string presence.

## Open Questions

- Should the compiler warn if a state has `integration` but no `accepts`? The
  strategic design implies integration states should accept evidence (the
  integration output feeds into evidence submission).

## Summary

The `integration` field compiles to `Option<String>` in `TemplateState`, stored
verbatim with no name validation. The compiler just passes it through. Before #49
implements the runner, `koto next` ignores the field and returns the directive
normally.
