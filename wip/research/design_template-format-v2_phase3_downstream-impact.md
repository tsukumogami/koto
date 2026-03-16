# Phase 3 Research: Downstream CLI Impact

## Questions Investigated
- What changes in koto next for v2 templates?
- Does hello-koto plugin survive v2?
- What integration tests need updating?
- What's the impact on plugin CI?

## Findings

### koto next (src/cli/mod.rs:292-299)
Currently outputs `template_state.transitions` directly as JSON. In v2,
transitions serialize as objects (target + when) instead of strings. The output
changes from `["done"]` to `[{"target":"done"}]`. No code changes needed in
the Next command itself since serde handles serialization. Consumers parsing
the JSON must adapt.

For issue #47 scope: keep the same output shape by extracting targets from
structured transitions. The full output contract change is #48's job.

### template compile/validate
- compile() at compile.rs:158 hardcodes format_version: 1. Must change to 2.
- validate() at types.rs:74-78 hardcodes format_version != 1 check. Must accept 2.
- Both commands work unchanged once type definitions support v2.

### hello-koto plugin
Uses only command gates (type: command, command: "test -f wip/spirit-greeting.txt").
No field gates. Survives v2 unchanged. Template compiles to v2 format automatically.

### Integration tests (tests/integration_test.rs)
- Line 16: minimal_template() defines transitions: [done] (v1 string format)
- Line 220: asserts json["transitions"].is_array()
- Tests need updating: minimal_template() must use v2 structured transitions
- Tests checking transitions as string arrays need updating for object format

### Plugin CI (.github/workflows/validate-plugins.yml)
- Template compilation job calls koto template compile on all plugins
- Works transparently once compiler supports v2
- No workflow changes needed

## Implications for Design

The downstream impact is manageable. hello-koto survives unchanged. The main work
is updating integration tests to use v2 template format. The koto next output
change (strings to objects) is handled by serde automatically.

## Surprises

None. The impact is contained to template types, compiler, and tests. No
unexpected dependencies on field gates.

## Summary

hello-koto uses only command gates and survives v2. Integration tests need updating
for structured transitions. Plugin CI adapts transparently. The koto next output
change from string transitions to object transitions happens automatically via serde.
