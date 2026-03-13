# Security Review: unified-koto-next

## Dimension Analysis

### Download Verification

**Applies:** No

This design adds no download capability. The changes are entirely within koto's local
state machine execution: reading state files from disk, evaluating gates, executing local
shell commands (already supported), and calling local CLI binaries via `IntegrationRunner`.
No external URLs are fetched, no binaries downloaded, no artifact verification needed.

### Execution Isolation

**Applies:** Yes

The design introduces or extends three execution surfaces:

**Command gates** (`GateDecl.Type = "command"`)

Command gates already exist; this design extends their reach by adding per-transition gate
evaluation. Gate commands are declared in template YAML and invoked by the engine with a
timeout. Risk: if template YAML is authored by an untrusted party, malicious commands could
be injected. If evidence field values are ever interpolated into command strings, this
becomes a command injection vector.

*Severity:* Medium. Templates are authored locally or distributed through trusted channels;
koto does not yet have a signed-template distribution model.

*Mitigations:*
- Commands in gates are declared in YAML as static strings — no evidence interpolation into
  the command string itself (the current design passes evidence only as environment
  variables or via stdin, not inline substitution)
- Enforce PATH/HOME-only environment when invoking gate commands; strip inherited env vars
- Document that template authors are responsible for command safety; untrusted templates
  should not be executed

**IntegrationRunner invocation**

The `IntegrationRunner` interface invokes a delegate CLI when a state has a `Processing`
field. The CLI name comes from the template. Risk: if the processing field is set to an
arbitrary binary path, it could invoke unintended executables.

*Severity:* Medium. Controlled by template authorship.

*Mitigations:*
- `Processing` field specifies an integration name, not a raw binary path; the
  `IntegrationRunner` implementation resolves names to binaries via a configured registry
- Integration runners should be whitelisted to a known set of CLIs
- Binary paths should be verified (checksum or path allowlist) before invocation

**`--with-data` file reading**

`--with-data` reads a JSON file from disk and injects its contents as evidence. Risk: path
traversal if the path is not canonicalized; oversized files could cause memory pressure.

*Severity:* Low. The file path is provided by the CLI caller (the agent) running under the
same user account; no privilege boundary crossing.

*Mitigations:*
- Validate that the file path does not traverse outside expected directories
- Enforce a reasonable size limit on evidence JSON files (e.g., 1 MB)
- Validate evidence key and value formats (printable strings, bounded length)

### Supply Chain Risks

**Applies:** Partially

koto templates are the primary supply chain artifact. Templates declare gate commands and
integration runner names. A compromised or malicious template can invoke arbitrary shell
commands via command gates or invoke unintended CLIs via `Processing`.

*Severity:* Medium for templates distributed via untrusted channels. Low for templates
authored locally or in the same repository as the code under automation.

*Mitigations:*
- Template format v2 schema validation at compile time limits injection surface
- The existing compiled template cache (SHA-256 keyed) ensures integrity within a session;
  it does not verify template authorship
- Future: ECDSA template signing and verification before execution
- Document trust model: koto templates should be treated as code; distribute them with the
  same review processes as code

### User Data Exposure

**Applies:** Yes

Evidence submitted via `--with-data` is written to the state file on disk and archived to
history entries. Evidence values may include sensitive data (API keys, tokens, intermediate
results from agent runs).

*Severity:* Low to Medium. State files are local disk artifacts under the user's own
account. No data is transmitted externally by this design.

*Mitigations:*
- State files should be created with 0600 permissions (owner-read-only)
- Directive interpolation: if a directive template includes evidence values in output sent
  to external APIs (e.g., an agent forwarding directive text to an LLM), evidence
  containing secrets would be exposed. Document this risk.
- Future: provide an optional redaction flag for evidence keys that should not appear in
  history or output

## Recommended Outcome

**OPTION 2 - Document considerations**

Five areas the implementer must address:

1. **Command gate safety**: Gate command strings must not interpolate evidence values. Pass
   evidence only via a controlled mechanism (environment variables with explicit allowlist or
   stdin). Strip inherited environment when invoking gate commands (retain PATH and HOME only).
   Document that template commands are trusted code.

2. **Integration runner control**: The `IntegrationRunner` implementation must resolve
   `Processing` names through a configured allowlist, not treat the field as a raw binary
   path. Log all integration invocations. Verify runner binary paths before execution.

3. **Evidence file validation**: `--with-data` must enforce a size limit (suggested: 1 MB),
   validate JSON structure, and validate key/value formats before injecting into state.

4. **State file permissions**: Persist state files at 0600 (owner-read-only). Document that
   evidence in history may contain sensitive values and should be treated as confidential.

5. **Template trust model**: Document that koto templates are trusted code. Operators
   distributing templates through automated pipelines should apply the same review processes
   as source code. Future: template signing.

## Summary

The unified-koto-next design extends koto with evidence-driven gating, auto-advancement,
and integration delegation. The design provides strong foundations (atomic state persistence,
version conflict detection, process group timeout enforcement) but introduces attack surface
in four areas: command gate environment isolation, uncontrolled integration runner invocation,
evidence exposure in persistent storage, and untrusted template distribution. These risks are
manageable with the documented mitigations — none require design changes. Implementers should
prioritize command gate environment isolation and integration runner allowlisting before
production use.
