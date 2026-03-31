# Decision Report: What Should Overrides Override?

## Question
What should overrides actually override -- the gate output (substituting
default values so the transition resolver picks a path normally), or the
transition destination (bypassing the resolver and jumping to a target)?

## Chosen: Option A (Override substitutes gate output)

## Confidence: High

## Rationale

All three validators converged on Option A after adversarial revision.

The core argument: override means "treat this gate as passed." The engine
substitutes the gate's override_default values and runs the transition resolver
normally. One code path handles all transitions -- overridden or not. Template
authors control routing via when clauses that the compiler validates.

Validator B (who initially argued for Option B: target destination) conceded
on the composability argument: under Option A, gate output and agent evidence
merge into the same resolver pipeline. Under Option B, overridden transitions
bypass the resolver, breaking any logic that depends on evidence merging.

The "agents shouldn't need template topology knowledge for simple overrides"
argument from A and C was decisive against Option B. Most overrides are "treat
as passed" -- the agent doesn't care about the target, they just want the gate
unblocked. Option B would force agents to know state names for every override.

The "synthetic data in the audit trail" concern from Validator B is addressed
by the GateOverrideRecorded event, which captures both actual and substituted
gate output. The audit trail is transparent about what happened.

## Key insight from peer revision

Validator C raised a refinement worth recording: let agents inject explicit
gate output values instead of always using the default. This would make
`--override-rationale --gate check --output '{"status": "partial"}'` possible,
letting the agent override to a non-passing value and route through a
non-default path. This is deferred as future work -- the use case isn't
validated, and it adds CLI complexity. But the data model supports it: if
override_default is replaceable at call time, the mechanism generalizes.

## Assumptions
- Most overrides (80%+) are "treat as passed" and don't need routing control
- The existing --to command covers the rare case where agents need explicit
  destination control (though without gate-specific rationale)
- Template authors should control override routing, not agents
- The resolver being the single routing authority is worth preserving

## Rejected

**Option B (target transition destination)**: Validator B conceded. Key
issues: forces agents to know template topology for every override, bypasses
the resolver (breaking evidence composability), and is essentially --to with
rationale -- incremental value doesn't justify a new mechanism.

**Option C (hybrid)**: Validator C conceded this collapses into "Option A +
existing --to." Not a new design, just a documentation note. The escape hatch
already exists.

## Future extension (from Validator C's refinement)

Allow agents to provide explicit gate output values at override time instead
of always using override_default. This would let agents override to non-
passing values and influence routing without bypassing the resolver. Deferred
because the use case isn't validated and adds CLI complexity, but the data
model supports it if needed later.
