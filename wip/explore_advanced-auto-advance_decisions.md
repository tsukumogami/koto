# Exploration Decisions: advanced-auto-advance

## Round 1
- Issue #89 fits koto's philosophy: the double-call is emergent overhead, not intentional design
- No state machine invariants at risk: transitions recorded, evidence epochs clean, gates still execute
- The fix belongs in the engine layer (advance_until_stop), not the CLI or caller convention

## Round 2
- Agent-vs-engine semantic distinction: not worth encoding in CLI response (event log handles it)
- Behavioral fix and response contract evolution are independent; behavioral fix proceeds first
- `advanced` field: keep for backward compat, deprecate as decision signal

## Round 3
- Response stays lean: `transition_count` for lightweight observability, not `passed_through`
- Rich observability deferred to `koto state` command (already designed, post-#49)
- `passed_through` unnecessary in response: event log + koto state handle detailed observability
