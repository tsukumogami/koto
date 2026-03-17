# Security Review: koto-cli-output-contract

## Dimension Analysis

### Download Verification

**Applies:** No

This design does not download external artifacts. It defines the output contract for `koto next` -- a local CLI command that reads state files and templates from the local filesystem, evaluates gates by running local shell commands, and returns structured JSON. No network requests, no remote artifact fetching. The template hash verification (SHA-256 in the JSONL header) is an integrity check on local files, not a download verification mechanism.

### Execution Isolation

**Applies:** Yes

The gate evaluator (`src/gate.rs`) spawns `sh -c "<command>"` for each command gate, running with the user's full environment and permissions. This is the primary execution surface in this design.

**Risks identified:**

1. **Arbitrary command execution from templates.** Gate commands come from template files. If a malicious template is loaded, its gate commands execute with the user's full privileges. The design explicitly acknowledges this and scopes it to the trusted-source model (plugin-installed or PR-reviewed templates). This is an acceptable trust boundary for the current architecture.

2. **Process group isolation is correctly scoped.** The design specifies `setpgid`/`killpg` via `pre_exec` to ensure timeout kills reach child processes. This prevents zombie accumulation from hung gate commands. The 30-second default timeout bounds resource consumption. Both are appropriate mitigations.

3. **No privilege escalation.** Gate commands inherit the user's permissions -- no setuid, no capability elevation. koto itself requires no elevated privileges. The filesystem scope is the user's working directory (passed as `working_dir` to `evaluate_gates`).

4. **Environment variable exposure.** Gate commands inherit the full process environment, which may contain secrets (API keys, tokens). This is standard for developer tooling and consistent with how `make`, `npm scripts`, and similar tools operate. Not a defect, but worth noting that gate commands from untrusted templates would have access to the user's environment.

**Severity:** Low for the trusted-source model. The design correctly identifies that extending to untrusted template sources would require additional validation. The existing Security Considerations section covers this adequately.

### Supply Chain Risks

**Applies:** Yes (limited)

The supply chain here is the template itself, not external packages or binaries.

**Risks identified:**

1. **Template integrity.** The SHA-256 `template_hash` in the JSONL header ties event logs to a specific template version. Replaying events against a modified template is detected and rejected. This is the right mechanism -- it prevents a class of attack where someone modifies a template after a workflow is in progress to change gate commands or transition logic.

2. **No template signing.** Templates are verified by hash but not signed. In the trusted-source model (local files, PR-reviewed), this is sufficient. If koto later supports remote template registries, cryptographic signing would be needed. The design doesn't introduce this gap -- it exists in the current architecture.

3. **Integration name resolution.** The upstream strategic design specifies that integration names must resolve from a closed set (project config or plugin manifest), not from arbitrary strings in template files. The tactical design references `IntegrationOutput` and `IntegrationUnavailableMarker` types but does not implement the integration runner (deferred to issue 49). The constraint that integration names resolve from a closed config set is critical and must be enforced in the issue 49 implementation. This design does not violate it, but also does not enforce it since integration invocation is out of scope.

**Severity:** Low. The hash verification is solid. The integration name resolution constraint is documented upstream and deferred appropriately.

### User Data Exposure

**Applies:** Yes

**Risks identified:**

1. **Evidence payloads persisted in plaintext.** Agent-submitted evidence (via `--with-data`) is validated against the `accepts` schema and then appended to the JSONL event log as plaintext JSON. Evidence may contain sensitive data -- analysis results, file contents, credentials passed by agents. The upstream design notes that event logs should be protected like secret-containing files and are not suitable for committing to public repositories. This design inherits that guidance.

2. **No payload size limit specified.** The design's own Security Considerations section identifies this gap: "Size limits on `--with-data` payloads are not specified in this design. Large payloads could bloat the event log. A reasonable limit (e.g., 1MB) should be enforced at the CLI level." This is a self-identified gap. It should be addressed in implementation, not deferred further.

3. **Error messages may leak schema details.** Structured error responses (error code, message, per-field details) reveal the template's `accepts` schema to the caller. This is intentional -- agents need this to self-correct. In the current model where the agent is the intended consumer, this is fine. The design notes that state files should be file-permission protected (user's umask).

4. **State file permissions.** The upstream design specifies that event log files must be created with mode 0600. This tactical design does not repeat this requirement explicitly, but it also does not create state files -- it appends to existing ones created by `koto init`. The permission enforcement belongs in the init path, which is already implemented.

5. **Gate command output not captured in this design.** Gate evaluation returns pass/fail/timeout/error status but does not capture stdout/stderr from gate commands into the response or event log. This is correct -- it avoids leaking potentially sensitive command output into structured responses.

**Severity:** Low. The plaintext evidence persistence is an inherent property of the event log model and is documented in the upstream design. The missing size limit should be addressed during implementation.

## Recommended Outcome

**OPTION 2 - Document considerations:** The design already contains a solid Security Considerations section that covers the four main areas (command gate execution, evidence validation, state file atomicity, exit code information leakage). Two minor additions would make it complete:

Add to the existing Security Considerations section:

> ### Payload Size Enforcement
>
> The `--with-data` payload size limit mentioned in this section (1MB recommended) should be enforced at CLI argument parsing time, before any validation or event appending occurs. This prevents both event log bloat and potential memory exhaustion from pathologically large JSON payloads.

> ### Environment Inheritance
>
> Gate commands inherit the full process environment. This is consistent with standard developer tooling behavior but means that environment variables containing secrets (API keys, tokens from `.local.env` or shell profiles) are accessible to gate commands. Template authors should be aware that gate commands execute with the same environment as the `koto` process itself.

No structural design changes are needed. The trust model is coherent, the gate isolation is appropriate for the scope, and the evidence validation is strict.

## Summary

This design has a small security surface. It executes shell commands from trusted templates (with proper process group isolation and timeouts), persists agent-submitted evidence in a plaintext event log (validated against schema before storage), and returns structured JSON to the calling agent. The existing Security Considerations section is thorough. Two minor additions -- enforcing the payload size limit at parse time and documenting environment variable inheritance for gate commands -- would close the remaining gaps. No design changes are needed.
