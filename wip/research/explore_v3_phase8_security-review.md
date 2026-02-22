# Phase 8 Security Review: koto Template Format v3

## Summary

The security section of DESIGN-koto-template-format.md correctly identifies command gates as the primary attack surface and makes a sound architectural decision to prohibit variable interpolation in command strings. However, the document understates the threat surface of command gate execution itself, has a gap around evidence key/value validation that could enable control-character injection, and underspecifies the command timeout model in ways that matter for real workloads. The "Download Verification: Not applicable" dismissal is correct for the engine but incomplete when considering the template search path, which resolves user-global templates from `~/.config/koto/templates/`.

## Findings

### Blocking

**1. Evidence values are unvalidated strings passed to the state file**

The design states that evidence is supplied via `koto transition <target> --evidence key=value` and accumulated in the state file. Evidence values are strings stored as-is. There is no mention of validation on evidence keys or values.

Evidence keys flow into the gate evaluation scope (`field_not_empty`, `field_equals`). Evidence values flow into the interpolation context (the design says "evidence wins over variables" in the merge). This means an attacker who controls evidence input can:

- Overwrite any variable's interpolated value by supplying evidence with a matching key name.
- Inject arbitrary content into directive text (rendered to the agent) by setting evidence values containing markdown or instructions that manipulate the agent.

The document identifies that command gates don't interpolate -- good. But the directive interpolation path does merge evidence values, and the design doesn't address what happens when an evidence key collides with a variable name. If evidence keys can shadow variable names in the interpolation context, a malicious `--evidence` flag on any transition can rewrite the directives the agent sees for all subsequent states.

This is a state contract concern: the evidence namespace and variable namespace need explicit collision rules. The design should specify either (a) evidence keys cannot shadow variable names (rejected at the CLI layer), or (b) the precedence is documented and the caller accepts the risk. Currently, the design says "evidence wins" without flagging this as a security-relevant decision.

Reference: lines 566-575 (interpolation context), lines 551-552 (evidence CLI surface).

**2. Command gates inherit full user environment**

The design says commands run via `sh -c "<command>"` from the project root. It does not mention environment variable inheritance. Commands executed this way inherit the full user environment, including `PATH`, credentials in environment variables (`AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`, etc.), and any shell configuration sourced by `sh`.

For a tool with the same trust model as Makefiles, this is acceptable in principle. But the design's security section doesn't mention it. The mitigations table lists "SHA-256 hash verification; review before use" for malicious command gates, which is insufficient if the template comes from the user-global search path (`~/.config/koto/templates/`) and was placed there by a different installation or tool.

The document should acknowledge that command gates have access to the full user environment and that the template search path creates a privilege boundary: project-local templates (in the repo, reviewed via git) have different trust properties than user-global templates (installed separately, not necessarily reviewed). At minimum, command gates in user-global templates should produce a warning on first use.

Reference: lines 686-690 (command gate execution), lines 576-583 (search path).

### Advisory

**3. The 30-second timeout is too short for the example gate and too long for the threat model**

The design uses `go test ./...` as an example command gate (line 197, line 379). For any non-trivial Go project, `go test ./...` will take longer than 30 seconds, especially on first run when test binaries need compilation. A CI-style test suite easily takes minutes.

At the same time, the timeout exists to "prevent indefinite blocking" (line 688). If the security concern is a command that hangs forever (e.g., a gate that does `cat /dev/urandom`), 30 seconds is generous. If the intent is to support real test suites, 30 seconds is insufficient.

The design should either:
- Increase the default to something practical (120-300 seconds) and document that 30 seconds is a safety floor, not a recommended value.
- Or keep 30 seconds as the default but flag in the example that `go test ./...` would need a `timeout: 300` override, so template authors understand the default won't work for test suites.

The per-gate `timeout` field exists, which is the right mechanism. But the example in the design is misleading -- it shows `go test ./...` with no timeout override, implying the default is sufficient.

Reference: lines 543-550 (gate types table), lines 686-690 (timeout description).

**4. "Download Verification: Not applicable" is incomplete**

The design says "Not applicable. Templates are local files. No downloads at runtime." (line 682). This is true for the engine, but the template search path resolves templates from `~/.config/koto/templates/` (line 279). Templates at that path could have been placed there by:

- A package manager (`brew install`, `go install`) that fetched from a registry
- A manual `curl` or `wget` from an untrusted source
- A separate tool that syncs templates from a remote

The engine doesn't download anything, but the system as designed will execute command gates from templates found via the search path. The "not applicable" framing obscures the fact that template provenance matters and isn't verified for user-global templates. A "not applicable" that's technically correct but misleading is worse than no entry, because it suggests the question was considered and dismissed.

The document should reframe this as: "The engine does not download templates. Template provenance is the user's responsibility. Templates from the user-global search path should be reviewed before use, particularly templates containing command gates."

Reference: line 682 (download verification), lines 276-283 (search path).

**5. Evidence persistence across rewind enables gate bypass**

The design states: "Evidence persists across rewind. Rewind changes the current state but doesn't modify the evidence map." (line 560). Combined with the fact that gates are exit conditions evaluated at transition time, this creates a bypass path:

1. Agent is in state `implement` with gate `tests_pass` (type: `command`, command: `go test ./...`).
2. Tests are passing. Agent provides evidence and transitions to `verify`.
3. Something goes wrong. Agent rewinds to `implement`.
4. Agent makes code changes that break tests.
5. The `tests_pass` gate was already satisfied. But since gates are evaluated at transition time (line 549), the gate runs again on the next transition attempt -- so the gate is NOT actually bypassed if the command re-executes.

Wait -- for `command` type gates, the command runs fresh each time. So evidence persistence doesn't bypass command gates. But for `field_not_empty` and `field_equals` gates, the evidence from before the rewind is still present. If the agent provided `--evidence tests_pass=true` to satisfy a `field_equals` gate, and then rewinds, the evidence value is still there. On the next transition attempt, the gate passes without the agent needing to re-provide it.

This is the correct behavior for append/overwrite semantics -- the design acknowledges it explicitly. But it means that `field_not_empty` and `field_equals` gates provide weaker guarantees after rewind than `command` gates. The document should note this asymmetry: command gates re-evaluate on every transition; field gates check accumulated evidence that may be stale after rewind.

This isn't blocking because the design explicitly chose this model, but the security implications of the asymmetry between gate types after rewind should be documented.

Reference: lines 558-561 (evidence and rewind).

**6. No validation on command gate strings at compile time**

The design specifies that command gates are "literal strings -- no `{{VARIABLE}}` interpolation" (line 258). This is the right call. But there's no mention of compile-time validation of the command string itself. The compiler will accept:

- Empty command strings (`command: ""`)
- Commands with shell metacharacters that could interact badly with `sh -c` quoting (though since the command is a literal string, this is by design)
- Commands that reference absolute paths outside the project (`command: /usr/bin/rm -rf /`)

The first case (empty command) should be caught by the compiler's validation rules. The design's validation table (lines 436-449) includes "Gate missing required field" but not "Gate field has empty value." An empty command string technically satisfies `field != ""` but would cause `sh -c ""` to exit 0, making the gate trivially passable.

The compiler should validate that `command` fields are non-empty strings. This is a small gap in the validation table.

Reference: lines 436-449 (validation rules), lines 539-550 (gate types).

**7. No-interpolation rule relies on implementation discipline, not structural enforcement**

The design correctly identifies that command gate strings must not be interpolated and says this is "verified by explicit tests" (line 574). This is the right defense -- tests that prove the negative. But structurally, the command string sits in the same `GateDecl` struct alongside `Field` and `Value`, and the interpolation function (`template.Interpolate`) operates on arbitrary strings. Nothing in the type system prevents a future contributor from calling `Interpolate(gate.Command, ctx)`.

The design acknowledges this in the mitigations table: "Future contributor bypasses this" is listed as residual risk (line 714). This is honest and appropriate. The explicit test suite is the right mitigation. An additional structural defense would be to use a distinct type for command strings (e.g., `type LiteralCommand string`) that the interpolation function doesn't accept, but that's an implementation detail beyond the design's scope.

No action required -- the residual risk is correctly identified and the mitigation (explicit tests) is appropriate. Noting this for completeness.

Reference: lines 573-574 (no-interpolation rule), line 714 (residual risk).

### Strengths

**Separation of compiler and engine dependency trees.** Confining go-yaml to `pkg/template/compile/` while the engine reads JSON via `encoding/json` is architecturally sound. A vulnerability in go-yaml affects the compilation path but not the runtime execution path. This is the right boundary.

**SHA-256 hash verification with no override flag.** The decision to make template hash verification non-bypassable (no `--force` flag) is correct for a workflow integrity tool. The design explicitly states this is deliberate and explains why: "providing escape hatches undermines the core value proposition." This is a rare and good design decision.

**Command gates don't capture output.** The design specifies "exit codes only. stdout/stderr not captured or stored" (line 703). This limits information leakage from command execution -- a malicious gate can execute code, but can't exfiltrate data through the koto state file.

**No-interpolation rule for command gates with explicit test coverage.** The single most important security decision in the design. Variable interpolation in shell commands is the classic injection vector, and the design both prohibits it and requires tests that verify the prohibition. The residual risk (future contributor bypass) is honestly documented.

**Atomic writes with symlink detection.** The engine design's `atomicWrite` function checks for symlinks at the target path before renaming. This prevents a class of redirect attacks where a symlink at the state file path could cause writes to an unintended location. The TOCTOU window between the check and rename is acknowledged as negligible residual risk.

**Evidence and variable separation.** Gates check the evidence map only, not the merged context (line 553). This prevents variables from accidentally satisfying gates and keeps the gate evaluation scope narrow and auditable.
