# Lead: What changes are needed in `koto next` template loading?

## Findings

Issue #47 updates the template format; issue #48 defines the full `koto next` output
contract (`expects`, `advanced`, error codes). The boundary matters: #47 should load
v2 templates correctly, but the full output schema is #48's job.

Current `koto next` output:
```json
{"state": "current_state", "directive": "...", "transitions": ["target1", "target2"]}
```

With v2 templates, `transitions` internally changes from `Vec<String>` to structured
`Vec<Transition>` with `target` and `when` fields. The CLI must load these correctly.

### Option A: Output structured transitions
Output the full v2 transition objects in `koto next`. This preempts #48 by changing
the CLI output schema before the output contract is designed.

### Option B: Keep flat transition list as stopgap
Extract `target` from each structured transition, output as `Vec<String>`. Add
`accepts` to the output so agents know what evidence fields exist. #48 then layers
on the full `expects` field.

### Option C: Minimal -- just load v2, keep same output
Load v2 templates internally but keep the exact same output format. The `accepts`
and `when` data are loaded and available for #48/#49 to use.

## Implications

Option C is cleanest for issue boundaries. #47's job is the template format, not
the CLI output. Loading v2 templates and keeping the same output means #48 can
design the output contract without working around interim formats.

The key change in `koto next` is that `template_state.transitions` changes from
`Vec<String>` to `Vec<Transition>`. The output line needs to map
`transitions.iter().map(|t| &t.target)` to preserve the current output shape.

## Surprises

None -- the boundary between #47 and #48 is clean if #47 doesn't change the CLI
output schema.

## Open Questions

- Should `koto next` output `accepts` before #48? Or wait for the full `expects`
  design?
- If a template has `integration` on the current state, should #47's `koto next`
  mention it in output? Or wait for #49?

## Summary

Issue #47 should load v2 templates internally but keep the same `koto next` output
format, extracting target names from structured transitions. The full output contract
with `expects` and `accepts` is #48's responsibility. This keeps issue boundaries
clean.
