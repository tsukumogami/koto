# Security Review: shirabe-work-on-template

## Dimension Analysis

### Download Verification
**Applies:** No

This design produces a koto template file (markdown) and updates to two Rust source files
and a shirabe skill file. It does not add any download paths, binary execution, or network
requests to koto or shirabe. The template references `gh` CLI and `git` commands in gate
commands, but these are tools already present on the system; koto doesn't download them.

### Execution Isolation
**Applies:** Yes

koto already runs gate commands as shell subprocesses with the user's credentials. This
design adds gate commands to the template: `git rev-parse`, `git log`, `test -f`, `gh
issue view`, `gh pr checks`, and `go test ./...`. These have the same permissions as
running those commands manually. No privilege escalation.

The `--var` flag introduces caller-controlled strings that are substituted into gate
command templates. This creates a potential shell injection vector if variable values
contain metacharacters. The implementation must quote variable values before substitution
(wrap in single quotes, escape embedded single quotes). If not sanitized, a value like
`; rm -rf ~/` in `{{ISSUE_NUMBER}}` could execute arbitrary commands. Severity: High if
not mitigated. Mitigation: sanitize at compile/substitution time, document the constraint.

### Supply Chain Risks
**Applies:** Marginal

The template itself is shipped as part of the shirabe plugin, so it inherits shirabe's
trust model. koto's template cache is keyed by content hash, so a modified template file
produces a different hash and a new cache entry — no silent staleness. The gate commands
call `gh` (GitHub CLI) which makes network requests to the GitHub API; if GitHub returns
malicious CI status data, the `ci_monitor` gate could be fooled. This is a systemic risk
of trusting external APIs for gate decisions, not specific to this design. Not a new
attack surface.

### User Data Exposure
**Applies:** Yes (low severity)

The event log (`koto-<name>.state.jsonl`) is committed to the feature branch. It contains
agent-submitted evidence: issue summaries, PR URLs, rationale strings, implementation
status enums. Visible to anyone with repository read access. This is intentional — the
event log is the audit trail. Agents should not include secrets or credentials in evidence
fields. The design's skill instructions should explicitly state this. No data is
transmitted outside the local machine by koto itself.

## Recommended Outcome

**OPTION 2 - Document considerations:**

The `--var` injection risk is real and specific. The Security Considerations section
must document: (1) the injection risk and required sanitization in the --var
implementation, and (2) the event log visibility constraint for evidence fields.
The section should also note that gate commands run with user credentials (not elevated)
and that supply chain risk via the GitHub API is pre-existing and not introduced by
this design.

## Summary

This design has two security dimensions worth documenting: the `--var` substitution
injection risk (mitigated by sanitizing variable values before gate command substitution)
and the event log visibility exposure (evidence fields committed to the branch and visible
to repo collaborators). No download or privilege escalation concerns. The --var risk
is the most important to call out explicitly for the Phase 1 implementer.
