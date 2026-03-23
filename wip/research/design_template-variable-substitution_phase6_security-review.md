# Security Review of Template Variable Substitution

Reviewer role: maintainer-reviewer (misread risk focus)
Inputs: DESIGN-template-variable-substitution.md, phase5 security report, current codebase

## 1. Attack vectors not considered

### 1a. State file tampering bypasses init-time sanitization (Blocking)

The entire security model rests on init-time allowlist validation: values are sanitized once at `koto init`, stored in the event log, and trusted thereafter. But the event log is a JSONL file on disk (`koto-<name>.state.jsonl`). Any process with write access to the working directory can edit the state file between `koto init` and `koto next`.

The design and the phase5 report both treat the stored values as trusted after init. The `Variables::from_events` constructor reads from the event log with no re-validation:

```rust
pub fn from_events(events: &[Event]) -> Self {
    // ... reads variables map directly, no sanitization check
}
```

If an attacker (or a buggy script, or a careless manual edit) modifies the `variables` map in the state file to contain `; rm -rf /`, the next `koto next` call will substitute that unsanitized value into an `sh -c` command.

The template format design already has a SHA-256 hash check for template tampering (DESIGN-koto-template-format.md line 669). The state file has no equivalent integrity check.

**Mitigation options (pick one):**
- Re-validate the allowlist in `Variables::from_events` or `substitute()` at runtime. This is ~3 lines and eliminates the attack entirely. Cost: one regex match per variable per substitution.
- Add integrity checking to the state file (heavier, broader scope).

The first option is strongly recommended. It closes the gap without architectural changes and makes the security property local to the `Variables` type rather than depending on a system-wide invariant about who writes state files.

### 1b. Workflow name in state file path (acknowledged but under-weighted)

The design acknowledges this is out of scope, and the phase5 report doesn't flag it. But the interaction is worth stating explicitly: the workflow name goes into `format!("koto-{}.state.jsonl", name)` (discovered in `src/discover.rs:95` and `src/engine/persistence.rs:318`). A name containing `../` could write the state file to an arbitrary directory. This is a separate issue from variable substitution, but since the design is adding path-containing values to the system, the next developer might assume "paths are validated" when they aren't -- not for workflow names, and not for variable values used as paths.

This is correctly scoped out of *this* design. Noting it here so the dependency is tracked.

### 1c. Regex denial of service

Not a realistic concern. The allowlist regex `^[a-zA-Z0-9._/-]+$` has no backtracking-prone patterns. The substitution regex `\{\{([A-Z][A-Z0-9_]*)\}\}` is also linear. No action needed.

### 1d. Environment variable leakage through gate commands

Pre-existing, not introduced by this design. Gate commands inherit the full shell environment (`sh -c` with no env filtering). A template could read `$AWS_SECRET_ACCESS_KEY` in a gate command today. Variable substitution doesn't change this surface. Not applicable to this review.

## 2. Are mitigations sufficient for identified risks?

### Command injection via allowlist: sufficient with one gap

The allowlist character set is well-chosen. The phase5 report's character-by-character analysis is thorough and correct. The set `[a-zA-Z0-9._/-]` excludes every shell metacharacter that enables injection. No combination of allowed characters can break out of a shell command context.

The gap is the one identified in 1a above: the allowlist is only enforced at init time, not at substitution time. If the stored values are trusted unconditionally, the allowlist is a speed bump, not a wall.

### Regex anchoring: sufficient in the design, needs implementation verification

The design doc (line 375-376) explicitly states "The regex must be anchored to ensure the entire value matches." The phase5 report flags this as an implementation-level verification item. Both are correct. The design intent is clear; the implementation just needs to match.

### Single-pass substitution invariant: sufficient, well-documented

Both the design (line 389) and the phase5 report (line 65) identify and document this. The design calls it out as an invariant that "must be maintained." This is the right approach -- it's a comment-level safeguard against future regressions, and it's stated clearly enough that a future developer adding a second pass would see the warning.

### Compile-time reference validation: sufficient

This is defense-in-depth, not the primary security boundary. It prevents template authors from accidentally creating unresolved references. It doesn't protect against malicious values (that's the allowlist's job). The layering is correct.

### Empty value rejection: sufficient

The `+` quantifier in the regex rejects empty strings. The design explicitly addresses the empty-default case (line 405-408). This is handled.

## 3. "Not applicable" justifications in the phase5 report

### External Artifact Handling: correctly not applicable

Variable values come from CLI flags. Templates are local files. No network input. Agreed.

### Supply Chain or Dependency Trust: correctly not applicable

No new crates. The regex crate is already present. Agreed.

### Data Exposure: mostly correct, one nuance

The phase5 report says variable values "aren't transmitted anywhere." This is true today. But variable values are stored in plaintext in the state file, which is committed to git branches (per the CLAUDE.md: "wip/ artifacts are committed to feature branches"). If a variable contains sensitive data (e.g., a token accidentally passed as `--var TOKEN=abc123`), it would be committed to version control.

The allowlist mitigates this somewhat -- tokens typically contain characters outside `[a-zA-Z0-9._/-]` (like `+`, `=`, or longer base64 strings with mixed case that could pass). But this is a data-handling concern, not a code vulnerability. The design's allowlist is doing security work here that it doesn't get credit for: by rejecting most token-like strings, it accidentally prevents the most common sensitive data patterns from being stored.

**Verdict:** The "not applicable" is defensible. This is an edge case that doesn't warrant a design change, but a documentation note ("don't store secrets in template variables") would be prudent.

### Permission Scope: correctly identified as low severity

The phase5 report correctly notes that gate execution permissions are pre-existing and unchanged. Variable substitution doesn't expand the permission surface. The path traversal note is accurate and matches the design's own residual risk analysis.

## 4. Residual risk that should be escalated

### Must-fix before implementation: re-validate at substitution time

The state file tampering vector (1a) is the only finding that changes the security posture from "sound" to "has a gap." The fix is small: add allowlist re-validation in `Variables::from_events` or `substitute()`. If a stored value fails the regex, return an error (or panic, consistent with the "corrupted state file" handling already planned for undefined references).

This should be a requirement in the implementation spec, not a "nice to have."

### Track but don't block: workflow name validation

The design already calls this out. The phase5 report doesn't. Both designs (template format and variable substitution) acknowledge that workflow names go into file paths unvalidated. This is a separate issue but the risk increases as the system gets more path-containing values. Should be tracked as a follow-up.

### Accept: path traversal and flag injection in variable values

The design and phase5 report both correctly identify these as template-author-responsibility risks. The `--` separator recommendation is the right mitigation. These don't need escalation -- they're documented, low severity, and dependent on template content rather than framework code.

## Summary

| Finding | Severity | Action |
|---------|----------|--------|
| State file tampering bypasses init-time sanitization | Blocking | Re-validate allowlist in `from_events` or `substitute` |
| "Don't store secrets in variables" undocumented | Advisory | Add a line to the CLI help text or template authoring guide |
| Workflow name path traversal | Out of scope | Track as separate issue (already identified in parent design) |
| All other phase5 findings | Correct | No changes needed |

The design's security model is sound in its intent. The allowlist strategy, compile-time validation, and single-pass invariant are well-reasoned. The one structural gap is that sanitization happens only at the trust boundary (init) but the stored data crosses a second trust boundary (disk) before use. Adding runtime re-validation closes this gap with minimal cost.
