# Security Review: Template Format v2 (Phase 6)

Reviewer role: pragmatic-reviewer (security lens)

## Assessment of Phase 5 Security Report

The Phase 5 report is accurate in its conclusions. The design is declarative and narrowly scoped. No blocking security concerns.

Below are findings organized by the four requested questions.

---

## 1. Attack Vectors Not Considered

### 1a. Malicious `when` values causing matching bugs

**Risk: Low.** The design uses `serde_json::Value` equality for `when` matching. If the matching implementation uses structural equality on `serde_json::Value`, a template author could submit a JSON object or array as a `when` value (e.g., `when: {field: [1,2,3]}`). The compiler doesn't restrict `when` values to scalar types. This isn't a security vulnerability per se, but could cause surprising match/no-match behavior if the runtime comparison semantics differ from what the template author expects.

**Recommendation:** Compiler should reject non-scalar `when` values (objects, arrays, null). This is a correctness issue more than security.

### 1b. Field name injection in `accepts` schemas

**Risk: Negligible.** Field names in `accepts` are strings stored in a BTreeMap. They flow into compiled JSON and eventually into `koto next` output. If a field name contains characters meaningful to a downstream consumer (e.g., shell metacharacters, JSON special chars), it could cause issues in agent tooling that consumes the output. Serde handles JSON escaping correctly, so this is only a concern if agents parse `koto next` output with shell tools.

**Recommendation:** No action needed. This is a downstream concern, not a koto concern.

### 1c. Denial of service via template complexity

**Risk: Low.** The mutual exclusivity check groups transitions by field and checks for duplicate values. This is O(n) per state. However, `accepts` blocks have no limit on field count, and `when` conditions have no limit on the number of fields per condition. A pathologically large template could consume memory during compilation. This is theoretical -- template files are locally authored.

**Recommendation:** No action needed. Locally authored files don't warrant DoS protection.

---

## 2. Sufficiency of Mitigations

### 2a. Command gate isolation (existing risk, unchanged)

The Phase 5 report correctly notes command gates run with full process permissions. The design doesn't change this, so no new mitigation is needed. However, the report doesn't note that the **removal of field gates shifts more logic into command gates** for template authors who relied on field gates for simple checks. If a template author replaces `field_equals: status = ready` with a command gate like `command: test "$STATUS" = "ready"`, they've moved from a safe declarative check to shell execution.

**Assessment:** This is a usage pattern concern, not a design flaw. The `accepts`/`when` system is the intended replacement for field gates, not command gates. Documentation should make this clear, but this doesn't block the design.

### 2b. Integration tag as inert string

The Phase 5 report correctly identifies that the `integration` tag is inert at compile time. The trust boundary is the project configuration that maps tags to executables, which is user-controlled. This is sufficient. The integration runner (#49) owns execution isolation.

---

## 3. "Not Applicable" Justification Review

### 3a. Download verification: correctly not applicable

Templates are local files. No downloads. Correct.

### 3b. Supply chain risks: correctly not applicable

No new crate dependencies. Templates are local. The Phase 5 report correctly notes that integration tag resolution goes through user-controlled project config, not template content. Correct.

### 3c. No dimensions were incorrectly dismissed.

---

## 4. Residual Risk Requiring Escalation

**None.** The design is declarative. The only execution vector (command gates) is unchanged from v1. The new constructs (`accepts`, `when`, `integration`) are pure data. The `serde_json::Value` flexibility is a minor type-safety concern, not a security concern.

---

## Summary

The Phase 5 security report is sound. One minor gap: it doesn't mention that `when` values are unconstrained `serde_json::Value` from a type perspective (could be objects/arrays), which is a correctness concern worth a compiler validation rule but not a security blocker. No residual risk requires escalation.

| Finding | Severity | Action |
|---------|----------|--------|
| Non-scalar `when` values accepted | Advisory | Add compiler check rejecting non-scalar values |
| Field gate removal may push authors toward command gates | Advisory | Document that `accepts`/`when` replaces field gates, not command gates |
| All "N/A" justifications | Correct | No action |
| Residual risk | None | No escalation needed |
