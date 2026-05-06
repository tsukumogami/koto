# Lead: Demand validation — session schema hygiene

## Findings

### Q1: Is demand real?

**Medium confidence.** The issue requesting this PRD was opened by the project maintainer with acceptance criteria authored by the same party. No independent issue reporters. The maintainer's explicit formulation ("fields that cannot be back-filled once external consumers adopt the schema") reflects a design constraint, not a user request.

### Q2: What do people do today instead?

**Absent.** No workarounds exist because the schema fields don't exist yet. Sessions lack UUIDs, timestamps are whole-second, context adds go unlogged, and rationale is not captured.

### Q3: Who specifically asked?

The issue was filed by the project maintainer as part of planned pre-adoption work. No external contributors cited.

### Q4: What counts as success?

The issue's acceptance criteria: PRD at an agreed path in the repo, merged to main, covering all four field additions with field name, type, required/optional, and default behavior.

### Q5: Is it already built?

**No.** Source search confirms:
- No `session_id` or UUID field on `StateFileHeader`
- Timestamps are whole-second strings (`now_iso8601()` calls `.as_secs()` only)
- No `context_added` event in `EventPayload`
- No `rationale` field on `DirectedTransition` or `Rewound`

### Q6: Is it already planned?

**Yes.** A planning issue exists for this PRD in the issue tracker. No design doc or prior PRD covers this topic.

## Calibration

**Demand not independently validated.** All evidence comes from a single source (the project maintainer's own planning). This is consistent with planned pre-adoption schema hardening work — the demand is technical necessity, not user-reported friction. No evidence of rejection or prior evaluation. The absence of independent reporters reflects the early-stage nature of external adoption, not lack of need.

## Summary

No independent external demand exists; the maintainer identified these fields as non-back-fillable prerequisites before external adoption begins. The technical necessity argument (append-only log, immutable headers) is sound, and no prior implementation or rejection evidence was found. This is planned foundational work, not a feature request — the demand validation framework finds "demand not independently validated" rather than "demand validated as absent."
