# Decisions: prd request-store-converge (--auto)

| id | artifact | tier | status | question |
|----|----------|------|--------|----------|
| D1 | PRD | 2 | assumed | Auto-promote terminal evidence into the closed result vs. explicit result-submission step |
| D2 | PRD | 2 | assumed | Result payload shape: free-form JSON vs. typed minimal envelope |
| D3 | PRD | 3 | deferred-to-design | Where the result lives + how converge reads it (index stays lean + pointer) |
| D4 | PRD | 2 | confirmed | Reuse children/gate/terminal-index; no new `koto request` command noun |
| D5 | PRD | 1 | confirmed | Complexity = Complex (P1 fires, P2 no, P3 fires; engine-substrate + new event family + converge-gate semantics + open arch decisions) |

## D1 — auto-promote vs explicit post (Tier 2, assumed)
Frame: Should a child's terminal evidence become its closed result
automatically, or require a separate result-submission step?
Gather: `koto next --with-data` already writes terminal evidence on the
completion path; `ChildCompleted` already carries a typed outcome. A second
explicit step adds an agent round-trip the fan-out exists to avoid.
Decide (--auto, recommend): lean toward terminal evidence
carrying/designating the result — no extra step. The exact mechanism
(which evidence kind, how designated) is design-altitude; the PRD records
the requirement that completion carry a result without a mandatory extra
agent action, and frames the trade-off.

## D2 — payload shape (Tier 2, assumed)
Frame: free-form JSON blob vs. typed envelope?
Gather: koto's idiom is typed (TerminalOutcome enum chosen over stringly
typed for exhaustive matching, stable wire format). Free-form maximizes
flexibility but defeats the parent reading results uniformly.
Decide (--auto, recommend): a typed-but-minimal envelope (status +
human-readable summary + optional structured payload). PRD requires the
shape's properties; exact field set finalized at design.

## D3 — where the result lives / how converge reads it (Tier 3, deferred)
Frame: result location + converge read path.
Gather: index line bounded to 4096 bytes on the hot scan path — cannot
hold an arbitrary result. Strongly implies result lives with the child
session; index carries at most a pointer/flag; parent's converge directive
dereferences and inlines.
Decide: PRD frames this as a requirement (index scan stays lean; converge
inlines the result) and DEFERS the concrete storage/pointer mechanism to
the DESIGN. Recorded as deferred.

## D4 — no new command noun (Tier 2, confirmed)
Brief Scope Boundary OUT explicitly excludes a new top-level request noun.
Confirmed by reserved `[request_store.recursion]` namespace + existing
gate/children/terminal-index surfaces. Reuse them.

## D5 — complexity (confirmed)
Matches scope-state r6 predicates: P1 fires (open arch alternatives),
P2 does-not-fire (extends existing engine), P3 fires (engine-substrate
change). => Complex. Routes downstream to /design.
