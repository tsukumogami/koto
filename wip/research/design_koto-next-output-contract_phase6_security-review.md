# Security Review: koto-next-output-contract (Phase 6)

Review of the phase 5 security analysis against the design doc and current codebase.

## Question 1: Are there attack vectors not considered?

### 1a. ReDoS via context-matches gate patterns (MISSED)

The `context-matches` gate type passes a template-authored regex pattern to `regex::Regex::new()` (src/gate.rs:107). The design introduces `blocking_conditions` which surfaces gate results -- but the underlying risk is pre-existing: a malicious or poorly-written template can include a pathological regex that causes catastrophic backtracking during gate evaluation.

The Rust `regex` crate is immune to catastrophic backtracking by design (it uses finite automata, not backtracking). So this is not an actual vulnerability. However, the security report should have noted this as a considered-and-mitigated vector rather than omitting it entirely.

**Verdict**: Not a gap. The `regex` crate's design neutralizes this. No action needed.

### 1b. Gate command injection through template content (PRE-EXISTING, NOT NEW)

Command gates run arbitrary shell commands via `sh -c` (src/action.rs:33-34). The design doesn't change gate evaluation, and templates are authored by the workflow creator. The security report correctly identifies this as pre-existing. No new attack surface.

**Verdict**: Not a gap. Correctly scoped as pre-existing.

### 1c. Event log manipulation for visit count bypass (LOW)

The new `derive_visit_counts` function trusts the JSONL event log as a source of truth. If an attacker can modify the state file, they could reset visit counts to force `details` to be re-emitted (or suppress them). However, state files are 0600 and the attacker would need local file access as the same user -- at which point they already have full control.

**Verdict**: Not a meaningful gap. The 0600 permission model is appropriate for a single-user CLI tool.

### 1d. `--full` flag as information disclosure bypass (NEGLIGIBLE)

The `--full` flag forces `details` emission regardless of visit count. The security report doesn't mention this. However, `details` contains template-authored instructions (not secrets), and the flag is a local CLI option requiring shell access. An attacker with shell access already has the template file.

**Verdict**: Not a gap. The flag is a convenience feature with no security implications.

## Question 2: Are mitigations sufficient for identified risks?

The single identified risk (data exposure via `blocking_conditions` and `details`) has three mitigations:

1. **State files created with 0600 permissions** -- Verified in code (src/engine/persistence.rs:16, line `opts.mode(0o600)`). Sufficient for single-user CLI.
2. **Output goes to stdout only** -- Verified. No new transmission channels. Sufficient.
3. **Template content is author-controlled** -- Correct. The exposed data is instructions written by the workflow creator, not user secrets or credentials.

**Verdict**: Mitigations are sufficient. The risk is correctly classified as informational-only.

## Question 3: Are any "not applicable" justifications actually applicable?

### External Artifact Handling: N/A -- CORRECT

The design does not introduce new external inputs. The `<!-- details -->` marker is parsed from already-loaded template content in `extract_directives` (src/template/compile.rs:385). Gate commands are pre-existing. Correctly N/A.

### Permission Scope: N/A -- CORRECT

No new file access, no new subprocesses, no privilege changes. `derive_visit_counts` operates on an in-memory `&[Event]` slice. The `--full` flag is a read-side serialization override. Correctly N/A.

### Supply Chain or Dependency Trust: N/A -- CORRECT

No new crate dependencies. All types are from std or existing project types. Correctly N/A.

### Data Exposure: Low/Informational -- CORRECT

The report correctly identifies this as applicable but informational. The data exposed (gate names, pass/fail status, template instructions) is not sensitive. The report's analysis that this is "by design" is accurate -- the entire point of the design is to surface previously-discarded information.

**Verdict**: All N/A justifications hold. No misclassifications.

## Question 4: Is there residual risk that should be escalated?

No. The residual risks are:

1. **Template authors can write gate commands that leak information via `blocking_conditions`** -- This is identical to the pre-existing risk that template authors can write gate commands that do anything. Template authorship is a trusted role.

2. **Long event logs increase `derive_visit_counts` scan time** -- Performance concern, not security. The design notes this is sub-millisecond for typical workflows.

3. **Breaking change to `action` values could cause caller confusion during rollout** -- Operational risk, not security. Mitigated by atomic release of all callers in the same repo.

None of these warrant escalation.

## Summary Assessment

The phase 5 security analysis is accurate and complete. The "N/A with justification" recommendation is correct. The design is a serialization-layer refactoring that does not introduce new trust boundaries, external inputs, or privilege changes.

The one area where the report could be slightly more thorough is explicitly noting the `regex` crate's backtracking immunity as a mitigating factor for context-matches gates, and briefly acknowledging the `--full` flag. Neither omission changes the outcome.

**Recommendation**: Accept the phase 5 security analysis as-is. No changes to the design are needed for security reasons.
