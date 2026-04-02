# Exploration Summary: Cross-Agent Delegation

## Problem (Phase 1)
koto workflows can't express that a step has specific processing characteristics (deep reasoning, large context). There's no template-level annotation for step type, no config system to act on annotations, and no mechanism for koto to route a step to a different agent CLI.

## Decision Drivers (Phase 1)
- Templates should describe step characteristics, not name specific CLIs
- Delegation routing belongs in user config, not templates
- koto has no config system today -- this is net-new infrastructure
- The compiled template JSON schema has `additionalProperties: false`, so any new field requires schema updates
- Tags are additive -- old koto should be able to read new templates (backwards compat)
- Go's `json.Unmarshal` ignores unknown fields, so format_version doesn't need bumping

## Research Findings (Phase 2)
- `sourceStateDecl` has 3 fields: Transitions, Terminal, Gates
- `StateDecl` (compiled) has 4 fields: Directive, Transitions, Terminal, Gates
- `MachineState` mirrors StateDecl without Directive
- JSON schema at `compiled-template.schema.json` enforces `additionalProperties: false` on state_decl
- `ParseJSON()` uses `json.Unmarshal` (no `DisallowUnknownFields`) -- silently ignores unknown fields
- `format_version` is checked as `!= 1` in ParseJSON -- bumping breaks old readers
- No config loading exists. YAML parsing confined to `compile/compile.go`
- `template.Template` is the intermediate repr; controller reads from it

## Options (Phase 3)
- Schema versioning: keep v1 (additive) vs bump to v2 (breaking)
- Tag vocabulary: enum in schema vs pattern-validated free-form vs prefix convention
- Tags placement: engine.MachineState vs template.Template only
- Config format: dedicated delegation section vs generic key-value

## Decision (Phase 5)
Keep format_version at 1. Use pattern-validated free-form tags with a documented initial vocabulary. Tags stay in the template layer, not the engine. Config system built as general infrastructure with a delegation section.

## Current Status
**Phase:** 5 - Decision
**Last Updated:** 2026-03-01
