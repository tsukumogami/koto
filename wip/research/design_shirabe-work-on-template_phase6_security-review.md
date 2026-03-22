# Security Review: shirabe-work-on-template (Phase 6)

## Review Scope

Full security review of `docs/designs/DESIGN-shirabe-work-on-template.md`, building on the
phase 5 analysis in `wip/research/design_shirabe-work-on-template_phase5_security.md`. This
review evaluates whether attack vectors were missed, whether mitigations are sufficient,
whether "not applicable" calls hold up under scrutiny, and what residual risk warrants
escalation before implementation begins.

---

## 1. Attack Vectors Not Considered in Prior Analysis

### 1.1 Workflow Name as an Injection Surface

The design specifies `koto init work-on-71` where the workflow name is caller-controlled.
The name is used to derive the state file path (`koto-work-on-71.state.jsonl`) via
`workflow_state_path`. A name containing path traversal sequences (e.g., `../../../etc/passwd`)
or embedded newlines could write state files outside the working directory or corrupt the JSONL
log format.

The design does address this in the Implementation Approach section (Phase 1 deliverables),
specifying a name validation rule of `^[a-zA-Z0-9][a-zA-Z0-9-]*$`. However, the Security
Considerations section does not mention it — a reader of that section alone would miss the
risk. The mitigations section correctly prescribes the fix; the location is just wrong.

### 1.2 Evidence Payload Size Not Mentioned in Security Considerations

The design notes a `MAX_WITH_DATA_BYTES = 1_048_576` (1 MB) limit on `--with-data` payloads.
Evidence fields like `rationale` and `context_summary` are free strings with no documented
per-field length constraints. A single oversized field within the 1 MB envelope could bloat
git history. The 1 MB cap is the only guard, and its existence isn't mentioned in the Security
Considerations section. This is low-severity but should be noted there so agents reading skill
instructions understand the boundary.

### 1.3 Template Path Traversal on Init

The `--template` flag accepts an arbitrary filesystem path. Pointing it at a non-template
file (e.g., `/etc/passwd`) causes the YAML compiler to attempt to parse it. Failure is
expected, but the error output might echo portions of the file's content in parse diagnostics.
This is low-severity because the caller already has filesystem access and is not operating in
a sandboxed context, but the Security Considerations section's "not applicable" on download
verification doesn't acknowledge that the template path itself is an input that benefits from
path-restriction (e.g., requiring project-relative paths or an explicit allowlist).

### 1.4 ci_monitor Gate: No PR Anchor

This is the most significant gap. The `ci_monitor` gate is specified as:

```
gh pr checks $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number \
  --jq '.[0].number // empty') --json state \
  --jq '[.[] | select(.state != "SUCCESS")] | length == 0' | grep -q true
```

The `// empty` guard handles the case where no PR is found. However, two problems remain:

**Problem 1 — Race condition after PR creation**: `pr_creation` immediately precedes
`ci_monitor`. A PR created by `gh pr create` may not be indexed by the GitHub API for
several seconds. If `koto next` is called immediately, `gh pr list --head <branch>` may
return an empty result, `// empty` fires, `gh pr checks` receives an empty argument, and
the command fails. Gate failure on a state with an `accepts` block routes to evidence
fallback. The agent can then submit `ci_outcome: passing` without any CI having run.
The design acknowledges this as "evidence fallback also serves as the retry mechanism for
the brief indexing window" — but submitting passing evidence before CI has started is
not retrying, it's bypassing. The wording normalizes the bypass.

**Problem 2 — Multiple PRs on the same branch**: If a branch has both a draft and a
ready PR, `.[0].number` selects whichever comes first in the API response. The checked PR
may not be the one the agent created. This is an edge case but worth noting.

The root issue is that `ci_monitor` lacks a PR anchor known at gate evaluation time. The
`pr_url` submitted as evidence at `pr_creation` would provide the right anchor, but gates
currently have no mechanism to read prior-state evidence. Until that capability exists,
`ci_monitor` should be documented as evidence-only for the session immediately following
PR creation, with the gate serving as a convenience poll in subsequent sessions.

### 1.5 Stale Artifact Glob Matches (Gate False Positives)

The `analysis` gate uses `ls wip/task_*_plan.md 2>/dev/null | grep -q .` for free-form
workflows. The `finalization` gate uses a summary file existence check. Both patterns match
any file in `wip/` matching the glob, including artifacts left over from prior workflows
in the same directory. If a previous workflow's plan or summary file is present and the
current workflow hasn't created one yet, the gate passes incorrectly.

The issue-backed variants would be anchored to `wip/issue_{{ISSUE_NUMBER}}_plan.md` once
`--var` ships. The free-form variants have no such anchor — the task slug used in file
naming is not a template variable. This means free-form gates are prone to false positives
from prior workflow artifacts for the lifetime of the design. The design notes the `analysis`
glob limitation in Consequences but doesn't address `finalization`. Neither glob has a
proposed fix.

---

## 2. Are Mitigations Sufficient for Identified Risks?

### 2.1 --var Shell Injection

**The proposed mitigation**: "reject values containing characters outside a safe set
(alphanumeric, hyphens, dots, slashes) or quote and escape them."

The allowlist approach (alphanumeric, hyphens, dots, slashes) is sufficient for
`ISSUE_NUMBER`, which is the only variable the design uses in gate commands. An issue
number like `71` or `123` trivially passes. The mitigation is adequate for the current
template variables and for any future variables that follow the same pattern (numeric IDs,
slugs, paths). A future variable holding a free-text description (e.g., a task title)
would require the quoting approach instead.

One inaccuracy: the design states sanitization must happen "at `koto init` time, before
storing variables in the `WorkflowInitialized` event." This is correct for the early-reject
path. But the design also says "during template compilation" in an adjacent sentence
(referencing the compile cache). The compiled template cache is variable-agnostic and does
not have access to `--var` values — they aren't known until `koto init` is called. If a
Phase 1 implementer follows the "compile time" instruction literally and adds the check to
`src/template/compile.rs`, it will have no effect. The sanitization must be in the
`koto init` handler (or the gate evaluator). This is a documentation error that could
misdirect implementation.

### 2.2 Workflow Name Path Traversal

The design specifies validation via `^[a-zA-Z0-9][a-zA-Z0-9-]*$`. This pattern correctly
rejects slashes, dots, underscores, and whitespace. Adequate.

### 2.3 Event Log Visibility

The mitigation is "agents should not include secrets or credentials in evidence fields;
the skill instructions should make this explicit." This is adequate for the stated risk
(accidental exposure). It is not a defense against a deliberate attempt to exfiltrate data
via the evidence log, but that threat model (malicious agent) is out of scope for this
design. The mitigation is proportionate.

### 2.4 Supply Chain / GitHub API Trust

The design acknowledges that a compromised GitHub API could return false SUCCESS for
`ci_monitor`. The stated mitigation is "this is a pre-existing risk, not introduced by
this design." That's accurate but incomplete as a mitigation — it explains why the risk
is acceptable, not what reduces it. A more precise framing: CI enforcement is only as
strong as the GitHub API's integrity; this design doesn't weaken that property and doesn't
need to solve it. The mitigation framing should be corrected but does not require new
controls.

---

## 3. "Not Applicable" Justifications That May Actually Apply

### 3.1 Download Verification: Correctly Not Applicable

koto does not download binaries or files during workflow execution. Gate commands invoke
`gh` and `git`, which are already-installed system tools. No downloads occur at runtime.
The "not applicable" call is correct and complete.

### 3.2 Execution Isolation: Identified and Addressed

The design correctly identifies that gate commands run in the user's working directory with
the user's credentials, and that `--var` introduces a new injection surface. The mitigation
is specified. This dimension is correctly marked as applicable and addressed.

One gap: the design says gate commands are "static strings in the compiled template" as
justification for why no injection risk exists today. This is accurate for the current
codebase. But the design then introduces `--var` substitution as a Phase 1 deliverable.
The "static strings" framing should not be read as a permanent property — it applies only
until `--var` lands. The design makes this clear in context but a standalone reading of the
Security Considerations section might suggest the injection risk is already mitigated when
it is not yet implemented.

### 3.3 Supply Chain: Understated

The design says "low residual risk" from the GitHub API trust issue. Two additional surfaces
are not mentioned:

**Template file integrity in the project directory**: The shirabe skill copies the template
to `.koto/templates/work-on.md`. If that directory is writable by other processes (shared
CI environments, multi-user systems), a replacement template could be written there. The
content hash cache would detect the modification and produce a new cache entry — but the new
cache entry would be compiled and used, not rejected. The hash cache prevents silent staleness
from edits to the original plugin file; it does not protect against substitution of the
project-local copy. This is low-severity in typical single-user developer environments, but
applies in shared build environments.

**GitHub CLI TLS integrity**: `gh pr checks` trusts the GitHub API over TLS. The design
notes a compromised GitHub API as a risk. A narrower variant — a MITM attack on the TLS
connection between `gh` and GitHub — is the same risk class but is implicitly mitigated by
`gh`'s TLS validation. Not a new finding, but the "supply chain risks: not applicable"
framing obscures that this gate is the only state where the workflow's enforcement outcome
depends on external data integrity.

---

## 4. Residual Risk That Should Be Escalated

### 4.1 High: ci_monitor Gate Enforcement Gap

The `ci_monitor` gate can fail silently (API lag after `pr_creation`, empty PR list result)
and fall through to evidence fallback where an agent can submit `ci_outcome: passing`
without verified CI. This state is the final enforcement gate before the workflow marks
work done. The design frames this as "evidence fallback also serves as the retry mechanism
for the brief indexing window," which normalizes a bypass as a feature.

This gap undermines the primary enforcement guarantee: "an agent can't reach `ci_monitor`
without passing through `pr_creation`" — but reaching `done` via `ci_monitor` evidence
fallback is not the same as CI having passed. The design should choose one of:

- Accept that `ci_monitor` is evidence-only for the first `koto next` call after PR creation,
  document this explicitly, and require agents to verify CI status before submitting passing
  evidence.
- Add a short sleep to the `ci_monitor` directive instructing agents to wait before submitting
  evidence (acknowledged workaround, not a structural fix).
- Track the PR URL as a workflow variable to anchor the gate command to a specific PR.

This is the one finding that affects stated enforcement guarantees. It should be resolved
before the template is used in production.

### 4.2 Medium: --var Sanitization Timing Claim

The design's Security Considerations section says sanitization happens "at `koto init` time"
but an adjacent reference to "during template compilation" contradicts this. The compile
cache is variable-agnostic; variables are not known at compile time. If a Phase 1 implementer
reads the Security Considerations section in isolation and places sanitization in
`src/template/compile.rs`, the mitigation will be a no-op. The design should be corrected
to remove the "during template compilation" phrasing before Phase 1 starts.

### 4.3 Low: Free-Form Gate Glob False Positives

The `analysis` and `finalization` gates for free-form workflows use broad globs that match
artifacts from prior workflows in the same directory. This can cause spurious auto-advancement.
No fix is possible without a task-slug template variable for free-form workflows. The design
should acknowledge this in Security Considerations (not just in Consequences) and note that
free-form workflows in directories with prior workflow artifacts may need manual gate
inspection.

### 4.4 Adequately Handled (No Escalation Needed)

- Workflow name path traversal: validation pattern specified in Phase 1 deliverables.
- --var injection via metacharacters: allowlist approach adequate for the current variable set.
- Event log visibility: proportionate mitigation; responsibility delegated to skill instructions.
- Gate privilege model: user credentials, not elevated.
- Template cache content-hash keying: prevents stale compiled templates from prior plugin versions.

---

## 5. Gate Command Analysis

### 5.1 context_injection Gate

`test -f wip/IMPLEMENTATION_CONTEXT.md`

Simple file existence check. No injection surface (no variable substitution). Sound.

### 5.2 setup_issue_backed / setup_free_form Gates

"Branch is not main/master, baseline file exists."

The exact gate command is not specified in the design. If implemented as
`git rev-parse --abbrev-ref HEAD | grep -vE '^(main|master)$'`, it correctly rejects both
branch names. If implemented as `grep -v main`, branches named `main-feature` fail
incorrectly. The design should specify the exact command to avoid ambiguity.

### 5.3 analysis Gates

Issue-backed: `test -f wip/issue_{{ISSUE_NUMBER}}_plan.md` (requires `--var`).
Free-form: `ls wip/task_*_plan.md 2>/dev/null | grep -q .`

The issue-backed variant is correctly anchored once `--var` ships. The free-form variant
matches any `task_*_plan.md` file — false positive risk documented in section 1.5.

### 5.4 introspection Gate

`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`

Correctly anchored via template variable. Sound once `--var` is implemented. Until then,
always fails and routes to evidence fallback — this behavior is documented and correct.

### 5.5 finalization Gates

`test -f wip/*_summary.md` and `go test ./...`

Glob issue same as analysis (section 1.5). The test command is language-specific; the design
proposes `TEST_COMMAND` as a mitigation but places it outside Phase 1 deliverables. Non-Go
projects will always fall to evidence fallback for this gate.

### 5.6 ci_monitor Gate

Detailed in section 1.4. The command structure is specified but has the race condition and
anchor issues described there. The command itself is syntactically correct and would work
correctly in steady-state (PR indexed, single PR on branch).

---

## Recommendations

1. **Correct the --var sanitization timing claim** in Security Considerations. Remove
   references to "during template compilation." State clearly that sanitization happens in
   the `koto init` handler when writing variables to the `WorkflowInitialized` event, before
   any gate evaluation.

2. **Resolve the ci_monitor enforcement gap** before production use. The gate can silently
   fail immediately after PR creation and route to evidence fallback. Document that agents
   must verify CI status before submitting `ci_outcome: passing`, or make `ci_monitor`
   evidence-only for the first invocation after `pr_creation`.

3. **Specify exact gate commands for setup states**. "Branch is not main/master" is
   ambiguous. The template should include the exact `git` command to avoid branches named
   `main-feature` from being mishandled.

4. **Move workflow name validation into Security Considerations**. The rule is correctly
   specified in Phase 1 deliverables but invisible to a reader of the security section alone.

5. **Acknowledge free-form gate glob risk in Security Considerations** (not just in
   Consequences). Free-form workflows in directories with prior workflow artifacts may
   experience false-positive auto-advancement.
