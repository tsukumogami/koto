# Security Review: DESIGN-koto-template-format

## Scope

Review of security considerations in the template format design document. koto runs locally, reads local template files, manages local state files, and does not make network requests.

## Attack Vectors Assessed

### 1. Command Gate Injection

**Design claim**: Command gate strings are NOT interpolated (no `{{KEY}}` expansion in commands). This is the critical security decision.

**Assessment**: Sufficient for the stated design. The command string is a literal from the TOML header, passed to `sh -c`. Since there's no variable expansion into the command string, the user-controlled evidence/variable values can't reach the shell. The only way to change what runs is to modify the template file itself, which triggers the SHA-256 hash mismatch check.

**Residual risk**: If a future implementer accidentally passes the command through `template.Interpolate()` before `sh -c`, the injection prevention breaks. The design should note this as an implementation invariant to test explicitly (e.g., a test with a command gate containing `{{TASK}}` that proves it is NOT expanded).

### 2. Template Search Path Traversal

**Design**: Three-tier search: explicit path, `.koto/templates/<name>.md`, `~/.config/koto/templates/<name>.md`. Template names are used directly in path construction.

**Attack vector not covered**: The design says "if `--template` doesn't contain `/` or `.`, treat it as a name." A name like `../../etc/passwd` contains `/` and `.` so it would be treated as a path, not a name -- that's fine. But what about a name like `name` that resolves to `.koto/templates/name.md`? No traversal risk there.

However: what about a name containing `..` without `/`? For example `..` as a template name would resolve to `.koto/templates/...md` which is harmless. Names are used as a single path component between a fixed prefix and `.md` suffix.

**Assessment**: The `/` or `.` heuristic in the design is slightly underspecified. The document says "contains `/` or `.`" triggers path mode. A template name like `my.template` would be treated as a path rather than a name. This is a usability quirk, not a security issue. But a name like `templates/../../secret` would be treated as a path and could read any file. This is by design (the user explicitly asked for a path) and is equivalent to the user running `cat` on any file, so it's not a privilege escalation.

**Verdict**: No security issue. The user already has filesystem access.

### 3. State File Tampering

**Attack scenario**: An attacker modifies the state file to change `template_path` to point to a malicious template. On the next `koto transition`, the CLI reads the tampered state file, loads the attacker's template, and the template hash check catches it because the stored hash doesn't match the new template.

**But**: What if the attacker modifies BOTH `template_path` AND `template_hash`? Then the hash check passes and the malicious template is loaded. If the malicious template has command gates, those commands execute on the next transition.

**Assessment**: This is documented implicitly (hash verification detects modification) but the double-tamper scenario is not addressed. However, this requires write access to the state file, which means the attacker already has user-level access to the filesystem. At that point, they could just modify `.bashrc` or any other executable. This is not a privilege escalation -- it's the same threat model as any local file an attacker can write to.

**Verdict**: Not a real escalation beyond the existing threat model (local filesystem write access). No action needed, but the design could note that state file integrity assumes the filesystem is trusted.

### 4. TOML Parser Exploits

**Attack vector**: Crafted TOML input causes buffer overflow, excessive memory allocation, or other parser bugs in BurntSushi/toml.

**Assessment**: BurntSushi/toml is written in pure Go (memory-safe). Buffer overflows are not possible. Excessive memory allocation from deeply nested or extremely large TOML is theoretically possible but bounded by file size (templates are authored by the user). The library has been stable for 13 years with active maintenance.

The design correctly notes: BSD-licensed, ~3K lines, well-maintained, confined to `pkg/template/`. The supply chain risk is low.

**One gap**: The design doesn't mention pinning the dependency version. If `go.mod` uses a floating version, a compromised release could be pulled in. This is standard Go supply chain hygiene (use `go.sum`), not specific to this design. Go's module system handles this by default.

**Verdict**: Acceptable risk. No action needed.

### 5. Evidence Data Sensitivity

**Design claim**: Evidence values in state files follow the same pattern as variables. State files in `wip/` are cleaned before merge.

**Assessment**: The design correctly identifies this risk and the mitigation (clean before merge). One thing not mentioned: `koto query` outputs the full state including all evidence values to stdout as JSON. If an agent pipes this to a log file or includes it in a commit message, evidence values leak. This is an operational concern, not a design flaw.

**Verdict**: Adequately covered. The design could add a note that `koto query` exposes evidence values and callers should be aware.

### 6. "Download Verification" N/A Justification

**Question from reviewer**: Is marking "Download Verification" as N/A actually correct?

**Assessment**: Yes. The template format design introduces no downloads. Templates are local files. The TOML parser is a Go module dependency (fetched by `go mod download` during build, not at runtime). BurntSushi/toml is verified by `go.sum` at build time. The design correctly marks this N/A.

**Verdict**: Justified.

### 7. Command Gate Timeout (Denial of Service)

**Design states**: "There is no timeout in Phase 1; a hanging command blocks the transition indefinitely."

**Assessment**: This is explicitly acknowledged. A template with `command = "sleep infinity"` blocks the koto process forever. Since koto runs locally under the user's own process, they can Ctrl-C. This is the same behavior as running `make` with a rule that hangs.

**Concern**: If koto is invoked by an automated agent (the primary use case), a hanging command gate blocks the agent indefinitely. The agent has no mechanism to detect or recover from this. The design should note that a timeout mechanism is needed before koto is used in unattended/automated contexts.

**Verdict**: Acceptable for Phase 1 (human-supervised). Should be escalated as a requirement for any "agent runs unattended" scenario.

### 8. Command Gate Working Directory

**Design states**: "Commands run via `sh -c` with the working directory set to the directory containing the state file."

**Assessment**: This is reasonable and deterministic. However, the state file lives in `wip/` by default. Command gates like `go test ./...` expect to run from the project root, not from `wip/`. If the implementation sets cwd to the state file's directory, most useful commands will fail.

**This looks like a functional bug in the design, not a security issue.** The working directory should probably be the project root (git root or CWD at init time), not the state file directory. The security surface is unchanged either way.

**Verdict**: Not a security issue but likely a functional defect. Flag separately.

### 9. Fenced Code Block Parsing Limitation

**Design acknowledges**: "A `## state-name` line inside a fenced code block will be incorrectly treated as a state boundary."

**Security implication**: A template author could inadvertently split a directive in a way that changes the state machine's behavior. This is a correctness issue, not a security issue, because the template author controls the template content.

**Verdict**: Correctness concern, not security.

### 10. TOML Bomb / Resource Exhaustion

Not mentioned in the design. A template file could contain extremely large TOML (e.g., a gate with a 100MB command string, or thousands of states). Since templates are local files authored by the user, this is a self-inflicted issue. BurntSushi/toml will parse whatever it's given, bounded by available memory.

**Verdict**: Not a real concern for local-only tooling.

## Summary of Findings

### Adequately Covered

| Risk | Design Mitigation | Sufficient? |
|------|-------------------|-------------|
| Command injection via variable expansion | No interpolation in command gates | Yes |
| Template modification detection | SHA-256 hash check on every operation | Yes |
| TOML parser supply chain | Well-known library, confined to leaf package | Yes |
| Download verification | Correctly marked N/A | Yes |
| Evidence in state files | Same pattern as existing variables | Yes |

### Gaps to Address

| Gap | Severity | Recommendation |
|-----|----------|----------------|
| No command gate timeout | Medium (Phase 1 acceptable, blocks unattended agent use) | Document as known limitation; add timeout before unattended agent scenarios |
| Command gate cwd is state file directory, not project root | Low (functional, not security) | Verify this is the intended behavior; `go test ./...` from `wip/` will fail |
| Implementation invariant: command strings must never pass through Interpolate | Low (design is correct, implementation could regress) | Add explicit test proving `{{KEY}}` in command gate is not expanded |
| State file integrity assumes trusted filesystem | Informational | Add one sentence noting this assumption |

### No Action Needed

- Path traversal in template search (user has filesystem access already)
- TOML resource exhaustion (local files, self-inflicted)
- Fenced code block parsing (correctness, not security)
- Double-tamper of state file (requires existing filesystem write access)
- Evidence exposure via `koto query` (operational, not design)
