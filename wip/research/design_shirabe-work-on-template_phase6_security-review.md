# Security Review: shirabe-work-on-template (Phase 6)

## Review Scope

Full security review of `docs/designs/DESIGN-shirabe-work-on-template.md`, building on the
phase 5 analysis in `wip/research/design_shirabe-work-on-template_phase5_security.md`. This
review also examines the actual koto codebase to evaluate whether the design's mitigation
claims are accurate given the current implementation state.

---

## 1. Attack Vectors Not Considered in Prior Analysis

### 1.1 Workflow Name as an Injection Surface (New)

The design specifies `koto init work-on-71` where the workflow name is caller-controlled.
The name is used to derive the state file path (`koto-work-on-71.state.jsonl`) via
`workflow_state_path`. If the name contains path traversal sequences (e.g., `../../../etc/passwd`
or a name with embedded newlines), it could write state files outside the working directory or
corrupt the JSONL log format. The design does not discuss validation of the workflow name, and
the current `Init` handler in `src/cli/mod.rs` performs no sanitization before constructing the
path. This is low-severity in practice (the caller controls the process anyway), but warrants
an explicit name validation rule in the Phase 1 implementation.

### 1.2 Evidence Content Written to JSONL (New)

The phase 5 analysis notes that the event log is committed to the branch and visible to repo
collaborators. It doesn't address a related risk: the `--with-data` payload is written verbatim
into the JSONL file without content limits on individual field values. A `rationale` or
`context_summary` field could carry a very large string (megabytes) that bloats git history.
The design already enforces `MAX_WITH_DATA_BYTES = 1_048_576` (1 MB) for the outer payload,
which caps the worst case, but this limit is not mentioned in the design's Security
Considerations section and agents reading the skill instructions won't know it exists.

### 1.3 Template Path Traversal on Init (New)

The `--template` flag on `koto init` accepts an arbitrary filesystem path. A malicious or
misconfigured invocation could point to a file outside the project (e.g.,
`--template /etc/passwd`). The compiler would attempt to parse it as YAML and likely fail,
but the failure mode is an error message that may leak the file's contents in the parse error
output. This is low-severity because the caller must already have filesystem access, but the
design's "not applicable" on supply chain risks should note that template path validation
(restricting to project-relative paths) would reduce the attack surface.

### 1.4 ci_monitor Gate: No PR Number Specified

The design describes the `ci_monitor` gate as `gh pr checks` for the current branch's PR.
The design does not specify whether the gate command includes a PR number or relies on `gh`
to infer it from the current branch. If the command is literally `gh pr checks` (without a
`--json` flag or explicit PR reference), it exhibits two problems:

1. **Context-dependent behavior**: `gh pr checks` without a PR number relies on the current
   branch having exactly one open PR in the repository. If the branch has multiple PRs
   (e.g., a draft and a ready PR), or if the git HEAD is detached, the command may check
   the wrong PR or fail silently. A gate that fails silently routes to the evidence fallback,
   which means the agent could mark CI as passing by submitting `ci_outcome: passing` without
   any CI actually having run.

2. **Race condition on newly created PRs**: `pr_creation` is the state immediately before
   `ci_monitor`. A PR created by `gh pr create` may not be indexed by the GitHub API for
   a few seconds. If `koto next` is called immediately, `gh pr checks` may return an empty
   or error response, causing the gate to fail and routing to the evidence fallback where
   an agent could submit `ci_outcome: passing` prematurely.

This is a meaningful enforcement gap. The gate should use the `pr_url` evidence submitted at
`pr_creation` to anchor `gh pr checks` to a specific PR. Since the design proposes template
variables that are fixed at init time, `pr_url` is only available at runtime as submitted
evidence — this points to a missing capability: runtime evidence values feeding into gate
command construction. The design doesn't address this.

---

## 2. --var Injection Risk: Is Quoting Sufficient?

### 2.1 Current Implementation State

The `--var` flag is **not implemented**. The current `Init` command in `src/cli/mod.rs`
accepts only `name` and `template` — there is no `--var` parameter. The `WorkflowInitialized`
event is written with `variables: HashMap::new()` hardcoded. The compile step in
`src/template/compile.rs` parses a `variables:` block from the template front-matter and stores
`VariableDecl` entries in the compiled output, but performs no substitution. The test at line
402 in `compile.rs` confirms that `{{TASK}}` in a directive is stored verbatim as the string
`"Analyze the task: {{TASK}}"` — substitution is explicitly deferred to runtime.

The gate evaluator in `src/gate.rs` receives a `Gate` struct with a `command: String` field
and passes it directly to `sh -c`. There is no substitution step anywhere between template
compilation and gate execution. The design's statement that "gate commands are static strings
in the compiled template" is accurate for today's code, but the `--var` feature would make
gate commands dynamic at init time, which is a new execution path that doesn't exist yet.

### 2.2 Is Single-Quote Wrapping Sufficient?

The design proposes: "wrap in single quotes and escape any single quotes within the value."
This is the standard POSIX shell quoting technique and is sufficient for preventing injection
via metacharacters (`;`, `|`, `$`, backticks, newlines) — with one important caveat.

Single-quote escaping must handle embedded null bytes and newlines correctly. Most shell
implementations treat a null byte as a command terminator regardless of quoting. If
`ISSUE_NUMBER` is sourced from an untrusted channel (not just the CLI caller), a null byte
in the value could truncate the command unexpectedly. In practice, `ISSUE_NUMBER` comes from
the agent-controlled CLI invocation, so this is theoretical for the intended use case.

More concretely: the design says sanitization must be applied "during template compilation,
not at gate evaluation time." This is **wrong for the --var use case**. Template compilation
happens before `--var` values are known — the compiled template cache is keyed by the template
file's content hash, not by the variable values passed at init time. Variable substitution must
happen at init time (or gate evaluation time), not at compile time. The design's own
architecture contradicts this statement: it describes `--var` as a flag on `koto init`, and
the `WorkflowInitialized` event already has a `variables` field intended to store them.

The correct model is: substitute (with quoting) at init time when writing the resolved gate
commands to the state file, or substitute lazily at gate evaluation time reading the variables
from the `WorkflowInitialized` event. Either approach works, but the design's claim that it
happens "during template compilation" is inaccurate and will mislead the Phase 1 implementer.

### 2.3 Summary

The quoting technique is sound in principle. The implementation timing claim in the design is
incorrect and must be fixed before Phase 1 implementation to avoid placing sanitization in the
wrong layer (the compile cache, which is variable-agnostic).

---

## 3. "Not Applicable" Justifications

### 3.1 Download Verification: Correctly Not Applicable

The design creates a local markdown template file. No binaries are fetched. The gate commands
call `gh` and `git`, which are already-installed system tools. The "not applicable" call is
correct and the rationale is complete.

### 3.2 Supply Chain Risks: Partially Applicable (Understated)

The phase 5 analysis marks this "Marginal" and the design marks it as handled via the content
hash cache. Both miss two residual supply chain surfaces:

**GitHub CLI responses in ci_monitor**: The `gh pr checks` gate passes or fails based on
output from the GitHub API. A compromised or spoofed GitHub API response (e.g., via MITM on
the `gh` CLI's TLS connection) could return false SUCCESS status, causing the gate to pass and
the workflow to advance to `done` without CI actually having passed. The design notes this as
"not a new attack surface" (correct), but doesn't acknowledge that `ci_monitor` is the specific
state where this matters most — it's the final enforcement gate before marking work done.

**Template file path**: If the shirabe plugin copies the template to `.koto/templates/work-on.md`
and that directory is writable by other processes in the environment, a template substitution
attack is possible. The content hash cache would detect the modification (different hash = new
cache entry), but the new cache entry would be compiled and used. This is an unlikely attack
in practice but worth noting in environments with shared build directories.

The "not applicable" is too strong here. "Low residual risk, pre-existing attack surface" is
more accurate.

---

## 4. Residual Risk Assessment

### 4.1 Risks That Should Be Escalated

**High: ci_monitor gate ambiguity (no PR anchor)**
The `gh pr checks` gate without an explicit PR reference creates an enforcement gap at the
most critical state in the workflow — the final CI verification before marking work done.
If the gate fails silently (no PR found, API lag) and the agent submits `ci_outcome: passing`
as evidence fallback, the workflow advances to `done` without verified CI. This undermines
the primary enforcement guarantee. The design should either:
- Require a `PR_NUMBER` or `PR_URL` template variable (set as a side-effect of `pr_creation`
  via a separate mechanism), or
- Make `ci_monitor` a pure evidence state (no gate) with a directive that requires the agent
  to fetch and submit the CI URL, or
- Accept that the gate is best-effort and document that `ci_monitor` relies on agent evidence
  in the first session after PR creation.

This is the one risk that affects the design's stated enforcement guarantees and should be
resolved before the template is used in production.

**Medium: --var substitution timing**
The design's claim that sanitization happens "during template compilation" is architecturally
incorrect given the compile cache is variable-agnostic. If a Phase 1 implementer follows this
guidance literally and adds sanitization to `src/template/compile.rs`, it will have no effect
because `--var` values are not available at compile time. Sanitization must be in the init
handler or the gate evaluator. This should be clarified in the design before implementation.

**Low: Workflow name validation**
The name is used to construct a filesystem path without validation. A name validation rule
(alphanumeric, hyphens, underscores, max 128 chars) should be added to `koto init` to prevent
path traversal. This is straightforward to fix and has no design implications.

### 4.2 Risks Already Adequately Addressed

- Event log visibility: correctly identified and appropriately scoped.
- Shell injection via `--var`: the proposed mitigation (single-quote wrapping) is sound once
  the timing issue above is corrected.
- Gate privilege model: gate commands run with user credentials, not elevated. Correctly noted.
- Template cache content-hash keying: prevents stale compiled templates.

---

## 5. Gate Command Analysis

### 5.1 context_injection Gate

`gh issue view {{ISSUE_NUMBER}} --json number --jq .number`

Uses `--json` with `--jq` to extract a specific field. The `--jq` output is not used as input
to any further command, so there's no secondary injection risk from the API response. The gate
passes if the issue is accessible and the field is present. Sound.

### 5.2 setup Gates

`git rev-parse --abbrev-ref HEAD | grep -v main` (branch check, inferred from design)
`test -f wip/issue_{{ISSUE_NUMBER}}_baseline.md`

The file existence check is straightforward. The branch check using `grep -v` is correct for
rejecting main/master. However, the design doesn't specify how "branch is not main/master" is
verified — if it's `git rev-parse --abbrev-ref HEAD | grep -vE '^(main|master)$'`, that's
fine; if it relies on a broader pattern, branches named `main-feature` would incorrectly fail.
The exact gate command text should be specified in the design to avoid ambiguity.

### 5.3 finalization Gates

`test -f wip/*_summary.md` and `go test ./...`

The glob in the file check matches any `*_summary.md` file in `wip/`, including ones from
prior workflows. If a previous workflow left a summary file and the current workflow hasn't
created one yet, the gate passes incorrectly. The design acknowledges this for `analysis` but
doesn't address it for `finalization`. Using `wip/issue_{{ISSUE_NUMBER}}_summary.md` (with
template variable substitution) would close this gap.

`go test ./...` is language-specific. The design notes this in Consequences and proposes
`TEST_COMMAND` as a template variable, but doesn't include it in the Phase 1 deliverables.
If the template is used on a non-Go project before this is addressed, the gate will always
fail and route to evidence fallback, which is functional but loses the auto-advancement benefit.

### 5.4 ci_monitor Gate

`gh pr checks` (no PR reference specified)

As detailed in section 1.4 and 4.1, this gate is underspecified. The design should specify
the exact command. If no PR anchor is available as a template variable, this state should be
documented as evidence-only in the first session after PR creation.

### 5.5 introspection Gate

`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`

Sound. Uses the template variable to anchor to the specific workflow's artifact. No issues.

### 5.6 analysis Gate

`test -f wip/*_plan.md`

Same glob issue as finalization: matches any `*_plan.md` file, including leftovers from prior
workflows. Should use `wip/issue_{{ISSUE_NUMBER}}_plan.md` or `wip/task_<slug>_plan.md`.

---

## Recommendations Summary

1. **Fix the --var substitution timing claim**: The design states sanitization happens "during
   template compilation." This is architecturally wrong — the compile cache is variable-agnostic.
   Sanitization and substitution must happen at `koto init` time (or at gate evaluation time
   reading from the `WorkflowInitialized` event). Correct the design before Phase 1 starts.

2. **Anchor ci_monitor to a specific PR**: `gh pr checks` without an explicit PR reference
   creates an enforcement gap at the final CI gate. Either add a mechanism to pass `PR_URL` or
   `PR_NUMBER` into gate commands (runtime evidence feeding into gates), or make `ci_monitor`
   a pure evidence state. This is the only finding that undermines a stated enforcement guarantee.

3. **Replace glob-based gate commands with template-variable-anchored paths**: The `analysis`
   and `finalization` gates use `wip/*_plan.md` and `wip/*_summary.md` globs that match
   artifacts from prior workflows. Use `{{ISSUE_NUMBER}}`-anchored paths to prevent false
   positives.

4. **Add workflow name validation to koto init**: Names are used to construct filesystem paths
   without sanitization. Add an alphanumeric-plus-hyphens validation rule to prevent path
   traversal and JSONL corruption via embedded newlines.

5. **The "not applicable" on download verification is correct; supply chain risks are
   understated**: The design should say "low residual risk from GitHub API responses and
   template file integrity, pre-existing attack surface not introduced by this design" rather
   than treating it as marginal. No escalation needed, but the framing should be accurate.
