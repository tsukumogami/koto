# Security Review: DESIGN-override-gate-rationale

## Scope

Review of the Security Considerations section in `docs/designs/DESIGN-override-gate-rationale.md`,
plus analysis of attack vectors, mitigation sufficiency, and residual risk.

## Assessment of identified risks

### 1. Rationale injection

**Design's claim**: rationale is stored as-is, never executed or interpreted as code. Same risk
as evidence field values and decision rationale.

**Assessment**: Mostly accurate, but incomplete. The rationale string flows into:
- JSONL state file (read by `derive_overrides`, `koto overrides list`)
- JSON output from `koto overrides list` (consumed by agents, future visualization)
- Potentially the HTML export (`src/export/html.rs` exists in the codebase)

The JSONL path is safe -- `serde_json::to_string` handles escaping. The CLI JSON output is
similarly safe. However, the HTML export path is a concern. If `src/export/html.rs` renders
event data into HTML without escaping, a crafted rationale string could produce XSS in a
browser context. The design doesn't mention the HTML export path at all.

**Verdict**: The risk identification is correct for the engine/CLI path. The HTML export
vector is unaddressed. Since the design says "same risk as evidence field values," this is
technically true -- evidence fields have the same exposure. But naming the risk explicitly
would be better, especially since override rationale is free-form text specifically designed
to carry human-readable explanations (making it a natural injection target).

### 2. Gate bypass authorization

**Design's claim**: any caller can bypass gates, this is by design, koto has no authorization
model, rationale creates an audit trail not access control.

**Assessment**: Accurate for the current single-user, single-agent model. The design correctly
identifies that koto is not an access-control system. The state file has 0600 permissions
(checked in `persistence.rs`), limiting access to the file owner. This is the right boundary
for a local-first tool.

**Verdict**: Adequate for current scope. See multi-tenant analysis below for forward-looking
concerns.

### 3. Event log integrity

**Design's claim**: same JSONL file, same persistence layer, no new file access patterns.

**Assessment**: Correct. `append_event` uses the same atomic write path for all event types.
The new event doesn't introduce new file operations or paths. The 0600 permission and
`sync_data()` call apply uniformly.

**Verdict**: Adequate.

## Attack vectors not considered

### A. Rationale as social engineering vector in audit review

The design focuses on technical injection (code execution) but doesn't consider that
rationale strings are specifically designed to be read by human reviewers. A malicious
or poorly supervised agent could craft rationale that misleads reviewers:

- "CI gate failed due to known infrastructure issue (approved by team lead)" -- false
  authority claim
- Extremely long rationale strings that obscure the override in log output

The design validates non-empty but sets no maximum length. The existing `MAX_WITH_DATA_BYTES`
(1 MB) limit applies to `--with-data` but the design doesn't specify whether `--override-rationale`
has a similar cap. If it doesn't, a rationale could be arbitrarily large, bloating the state file.

**Recommendation**: Apply the same `MAX_WITH_DATA_BYTES` limit to `--override-rationale`. This
is mentioned nowhere in the design but should be explicit.

### B. Override event flooding / denial of service on state file

An agent (or script) could repeatedly call `koto next --override-rationale` on the same
gate-blocked state. Each call would:
1. Evaluate gates (which run shell commands)
2. Append a `GateOverrideRecorded` event
3. Advance to the next state

Step 3 means repeated calls on the same state would fail (the engine moves forward), so
this isn't a realistic flooding vector for a single state. However, a template with a cycle
(state A -> gate-blocked B -> override -> A -> ...) could accumulate override events. The
`MAX_CHAIN_LENGTH` (100) limit caps per-invocation chains, and `CycleDetected` stops
revisiting states within one call. So this is bounded.

**Verdict**: Not a real concern given existing safeguards.

### C. Gate command execution side effects during override

When `--override-rationale` is provided, gates are still evaluated (the design says "gates fail
AND override_rationale is Some"). This means gate commands execute even when the caller intends
to override. Gate commands are shell commands defined in templates. This is existing behavior,
not new attack surface. But it's worth noting that override doesn't skip gate evaluation --
it skips the *blocking* behavior. Template authors should know gates always run.

**Verdict**: Not a new risk, but a documentation gap.

### D. Rationale string in JSONL without content-type validation

The rationale is a plain string. If a future consumer interprets the JSONL as something other
than plain text (e.g., a log aggregator that renders markdown or HTML), embedded formatting
could produce unexpected output. This is the same risk class as evidence fields.

**Verdict**: Low risk, existing precedent.

## Multi-tenant / shared-access analysis

The question asks specifically about `--override-rationale` abuse in multi-tenant or
shared-access scenarios. This requires examining the cloud backend.

### Current shared-access model

The `CloudBackend` in `src/session/cloud.rs` syncs state files to S3. Multiple agents or
users could theoretically access the same session if they share S3 credentials and the same
repo-id. The state file is the shared artifact.

**Risks in shared-access scenarios**:

1. **Unauthorized override**: Agent A creates a gate-blocked state. Agent B calls
   `koto next --override-rationale "..."` and bypasses the gate. The override event records
   the rationale but not WHO performed it. The event has a timestamp and seq but no caller
   identity. In a shared-access scenario, the audit trail answers "what was overridden and
   why" but not "by whom."

2. **Race condition on override + advance**: Two agents call `koto next --override-rationale`
   simultaneously on the same gate-blocked state. The `append_event` function reads the last
   seq, increments, and appends. With two concurrent writers to the same file (or S3 object),
   this could produce duplicate seq numbers or corrupted lines. The design inherits this risk
   from the existing persistence layer -- it's not new to overrides.

3. **Override rationale tampering via S3**: If the state file is synced to S3, anyone with
   bucket access can modify override events after the fact. The JSONL format has no integrity
   protection (no signatures, no merkle chain). Again, this is inherited, not new.

**Verdict**: The design's claim that "koto doesn't have an authorization model" is correct, but
the design should note that override events lack caller identity, which matters more for
overrides than for routine evidence submission. When a human reviewer sees "CI failure is
flaky test," they want to know which agent made that judgment. This is a gap worth calling
out explicitly.

## "Not applicable" justifications review

The design doesn't use "not applicable" labels, but it implicitly dismisses several concerns
with "same risk as existing systems." Let me evaluate each:

1. **"Rationale injection -- same risk as evidence fields"**: Partially valid. Evidence fields
   are structured (validated against an `accepts` schema with typed fields). Rationale is
   unstructured free-form text. The injection surface is wider for rationale because there's
   no schema validation beyond non-empty. The risk level is similar but not identical.

2. **"No new attack surface"**: True for the engine and persistence layers. The HTML export
   path is an existing surface that gains a new data source (override rationale), which is
   worth mentioning.

3. **"No authorization model"**: True today. But the design is building audit infrastructure
   (override events exist FOR human review). Audit without identity attribution is a known
   gap that becomes more visible with explicit override tracking.

## Residual risk assessment

| Risk | Severity | Likelihood | Residual? |
|------|----------|------------|-----------|
| HTML export XSS via rationale | Medium | Low | Yes -- depends on export implementation |
| Missing caller identity in override events | Medium | Medium | Yes -- architectural gap |
| Unbounded rationale string length | Low | Medium | Yes -- no max length specified |
| Misleading rationale in audit review | Low | Medium | Yes -- social, not technical |
| Concurrent writer corruption (cloud) | Medium | Low | Yes -- inherited from persistence layer |

### Should any be escalated?

The **missing caller identity** gap deserves explicit acknowledgment in the design's Security
Considerations section. It doesn't need to be solved now (adding identity is a separate
feature), but the design should note it as a known limitation. When the feature's stated
purpose is "audit trail for human reviewers," the absence of "who" from the audit record
is a meaningful gap.

The **unbounded rationale length** should be addressed in implementation (apply the existing
1 MB cap). This is a simple fix that the design should specify.

The remaining risks are acceptable for the current scope.

## Recommendations

1. **Add rationale length limit**: Specify that `--override-rationale` is subject to the
   same `MAX_WITH_DATA_BYTES` (1 MB) limit as `--with-data`. Add CLI validation.

2. **Note HTML export path**: Add a sentence to Security Considerations acknowledging that
   rationale strings flow through the same rendering paths as evidence and decision data,
   including any future HTML/visualization consumers. Ensure those consumers escape
   user-provided strings.

3. **Acknowledge identity gap**: Add a forward-looking note that override events don't
   carry caller identity, and that multi-agent or shared-access scenarios will need an
   identity mechanism for meaningful audit. This doesn't block the current design but
   should be documented as a known limitation.

4. **Clarify that gates still execute during override**: The design implies this (gates
   fail, then override happens) but should state explicitly that `--override-rationale`
   does not skip gate evaluation. Gate commands are still executed for their side effects
   and to capture failure context.

5. **Differentiate from evidence injection risk**: The claim "same risk as evidence fields"
   understates the difference. Evidence is schema-validated with typed fields. Rationale is
   free-form. Acknowledging this distinction strengthens the security analysis without
   requiring additional mitigations.
