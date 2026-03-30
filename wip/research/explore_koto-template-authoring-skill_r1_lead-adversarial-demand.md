# Lead: Is there evidence of real demand for this?

## Demand-Validation Questions

### 1. Is demand real?
**Confidence: Absent**

No GitHub issues request a koto template authoring skill. No distinct issue reporters, no explicit requests, no maintainer acknowledgment of this specific need.

### 2. What do people do today instead?
**Confidence: Medium**

The current workaround is manual template writing:
- Write Markdown template by hand
- Compile and extract via jq
- Write SKILL.md by hand
- Custom skill authoring is documented in issue #39

Two shipped/in-progress templates demonstrate that the manual process works but is not guided.

### 3. Who specifically asked?
**Confidence: Absent**

No specific issue numbers, comment authors, or PR references requesting this capability.

### 4. What behavior change counts as success?
**Confidence: Absent**

No acceptance criteria, stated outcomes, or measurable goals found in issues or linked docs.

### 5. Is it already built?
**Confidence: Low**

Not built. One design doc (DESIGN-koto-agent-integration.md) mentions a related feature (`koto generate`) but explicitly defers it as "may be useful later." This indicates consideration but deprioritization rather than rejection.

### 6. Is it already planned?
**Confidence: Low**

Not explicitly planned as a standalone item. The `koto generate` mention in the design doc is the closest thing to a plan. Issues #72 and #73 relate to work-on integration which is adjacent but distinct.

## Calibration

**Demand not validated.** The majority of questions returned absent or low confidence, with no positive rejection evidence. The deferred `koto generate` mention is consideration, not rejection. The manual workaround (documented in issue #39) exists and works.

This is "no evidence found" rather than "evidence it was rejected." The user's direct request to build this skill is the primary signal. Another round or user clarification may surface friction reports from template authoring that the repo doesn't capture.

## Summary

No external demand signal exists in the koto repo for a template authoring skill. The current workaround is fully manual (write template by hand, compile, write SKILL.md). One design doc defers a related `koto generate` feature as "may be useful later," indicating consideration but not rejection. Demand is not validated but also not validated as absent.
