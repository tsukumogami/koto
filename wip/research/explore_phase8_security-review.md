# Security Review: koto State Machine Engine

## Reviewer Role

Pragmatic security review of DESIGN-koto-engine.md. Focus: attack vectors, mitigation sufficiency, residual risk, and "not applicable" justifications.

## 1. Unconsidered Attack Vectors

### 1.1 Command Gate Injection via Evidence Interpolation (HIGH)

The design says templates use `{{KEY_NAME}}` interpolation and that `command` gates run via `sh -c`. If command gate strings are subject to template interpolation, an agent (or a user crafting evidence values) can inject shell commands through evidence.

Example: template defines a command gate like `test -f {{BRANCH_NAME}}/README.md`. The agent supplies evidence `BRANCH_NAME="; rm -rf / #"`. After interpolation: `sh -c 'test -f ; rm -rf / #/README.md'`.

The design does not clarify whether command gate strings are interpolated before execution. This needs an explicit decision:
- If command gates are interpolated: this is a shell injection vector. Evidence values become untrusted input passed to `sh -c`. Every evidence-interpolated command gate is exploitable.
- If command gates are NOT interpolated: this should be stated explicitly as a design invariant, tested, and enforced.

**Recommendation**: Command gate strings MUST NOT be interpolated. If a command gate needs to reference evidence or variables, it should read them from the state file (e.g., `jq -r .evidence.branch_name <state_file>`), not via string substitution into a shell command. Document this as a security invariant.

### 1.2 Path Traversal in State File Path (MEDIUM)

`Engine.Init` and `Engine.Load` accept `statePath` as a caller-specified path. The design says "The engine reads and writes only to state files at a caller-specified path" but imposes no validation on that path. A malicious or buggy caller could specify:
- `/etc/cron.d/backdoor` (write arbitrary JSON to a system path)
- `../../.git/config` (overwrite git config)
- `/dev/null` (silently discard state)

For the CLI layer, `--state-dir` combined with the `koto-<name>.state.json` naming convention limits this. But the library API has no path validation. Library consumers are expected to know what they're doing, but the design should at minimum document that path validation is the caller's responsibility.

**Recommendation**: Document in the Engine API godoc that the engine does not validate the state file path. The CLI layer should validate that the resolved path is within the expected state directory.

### 1.3 State File Symlink Attack (MEDIUM)

The atomic write uses temp-file-then-rename. If `statePath` is a symlink, `os.Rename` follows the symlink and overwrites the target. An attacker with write access to the state directory could:
1. Replace `koto-workflow.state.json` with a symlink to `~/.ssh/authorized_keys`
2. Wait for the next `koto transition`
3. The atomic write overwrites the symlink target with JSON state data

This corrupts the target file. It's not arbitrary write (the content is JSON state), but it's still destructive.

**Recommendation**: Before atomic rename, verify that the target path is not a symlink (`os.Lstat` and check `Mode().Type()`). Or create the initial state file with `O_NOFOLLOW` semantics.

### 1.4 Temp File Race in Shared Directories (LOW)

The atomic write creates temp files with `.koto-*.tmp` in the same directory as the state file. If the state directory is a shared location (e.g., `/tmp`), another user could:
1. Predict the temp file pattern
2. Pre-create a symlink at the predicted path
3. The engine writes to the symlink target

Go's `os.CreateTemp` uses `O_CREATE|O_EXCL` which prevents this specific attack (it won't open an existing file). But the temp file is created with default permissions -- on some systems this means group/world-readable, exposing state content briefly.

**Recommendation**: Set explicit file permissions on temp files (`os.CreateTemp` followed by `os.Chmod(tmpPath, 0600)`). Low priority since state files are typically in project directories, not shared locations.

### 1.5 Denial of Service via History Growth (LOW)

The history array grows unboundedly. A workflow that loops (rewind and retry many times) accumulates history entries indefinitely. This is a slow DoS: the state file grows, reads and writes slow down, and JSON parsing allocates more memory.

In practice, workflows have a bounded number of states and retries are rare. But a pathological workflow (or a buggy agent in a loop) could create thousands of history entries.

**Recommendation**: Advisory only. Consider adding a history size warning (not a hard limit) in a later phase. The engine could log a warning when history exceeds some threshold (e.g., 1000 entries).

### 1.6 Template Hash Does Not Cover Template Path (MEDIUM)

The template hash is SHA-256 of the template file *content*. The template *path* is stored but not covered by any integrity check. An attacker could:
1. Create a malicious template with identical content but at a different path
2. Modify the state file's `template_path` to point to the malicious path
3. Later, modify the malicious template (changing rules) -- the hash check reads from the new path

Wait -- this actually works correctly because the hash check re-reads the template from `template_path` and compares its hash to the stored hash. If the content at the new path differs, the hash check catches it.

The real risk is different: if the attacker replaces the template at the *original* path with different content that happens to have the same SHA-256 hash. This is the "hash collision" residual risk already noted in the mitigations table, and is indeed negligible with SHA-256.

However, there's a subtler issue: the state file stores `template_path` as a relative or absolute path. If the working directory changes between invocations, a relative `template_path` could resolve to a different file. The design doesn't specify whether the path is stored as absolute or relative.

**Recommendation**: Store `template_path` as absolute, or resolve it relative to the state file's directory (not CWD). Document the resolution rule.

### 1.7 Evidence Values as Vectors for Downstream Consumption (MEDIUM)

The engine stores evidence as opaque strings. But downstream consumers (template interpolation in directives, log output, git commit messages) may interpret evidence values in dangerous contexts:
- If a directive containing `{{COMMIT_LIST}}` is shown in a terminal, ANSI escape sequences in evidence could manipulate terminal output
- If evidence values end up in shell commands (outside the engine, in agent tooling), they're injection vectors

The engine can't fully control this, but the design should acknowledge that evidence values are untrusted input from the agent's perspective.

**Recommendation**: Document that evidence values should be treated as untrusted strings by any consumer. The template interpolation section should note that interpolated output may contain attacker-controlled content.

## 2. Mitigation Sufficiency Assessment

### State File Tampering (INSUFFICIENT for stated threat model)

The mitigation table says: "Version counter detects unexpected changes." But the version counter only detects changes if the engine reads a version it didn't write. A tamperer who reads the file, modifies it, and increments the version counter will evade detection entirely.

The residual risk column acknowledges "Tampering that correctly increments version," but the mitigation is weaker than described. The version counter detects *concurrent* writes, not *between-invocation* tampering. These are different threat classes. The mitigation table conflates them.

For Phase 1's threat model (bugs, not adversaries), this is acceptable. But the mitigation description should be precise: "Version counter detects concurrent modification; between-invocation tampering is not detected in Phase 1."

### Template Hash (SUFFICIENT)

SHA-256 on every operation with no override flag is a strong mitigation. The only bypass is modifying the state file to update the stored hash, which brings us back to state file integrity.

### Command Gate (INSUFFICIENT for future phases)

The mitigation says "Phase 1: local templates only; future: explicit confirmation." The "explicit confirmation" mitigation for registry templates has a known weakness (user confirms without reading), but more importantly, the confirmation model is undefined. What exactly is confirmed? The command string? What if it's obfuscated (`$(echo cm0gLXJmIC8= | base64 -d)`)? A list of command strings without context is security theater.

For Phase 1 (local templates), the mitigation is sufficient -- the user authored the commands. For the future registry phase, the design should note that a more rigorous model is needed (sandboxing, capability restrictions, or a command allowlist) rather than relying on user confirmation of opaque shell commands.

### Concurrent Writes (SUFFICIENT)

Atomic rename prevents corruption. Version counter detects races. Last-write-wins on true concurrency is the correct trade-off for single-writer workflows. This is well-handled.

### State File in Git (SUFFICIENT with existing controls)

CI enforces cleanup before merge. Feature branch exposure is acknowledged and accepted. This is appropriate for the data sensitivity level (task descriptions, branch names -- not credentials).

## 3. Residual Risk Escalation

### ESCALATE: Command Gate Injection (Section 1.1)

If command gates undergo template interpolation, this is a code execution vulnerability exploitable by any agent that can set evidence values. This should be resolved before implementation begins. It's a design-level decision, not an implementation detail.

### ESCALATE: No Timeout on Command Gates

The design explicitly says "No timeout is enforced by the engine -- the calling process's timeout applies." A command gate with `sleep infinity` or a command that hangs on network I/O will block the transition indefinitely. The calling process's timeout is the agent framework's timeout, which may be very long (minutes to hours for AI agent sessions).

This isn't a security vulnerability per se, but it's an availability concern. A malicious or buggy template can make `koto transition` hang forever.

**Recommendation**: Add a default timeout (30s) for command gates, overridable per-gate in the template. This is simple to implement (`context.WithTimeout` around `exec.CommandContext`) and prevents the hang scenario.

### MONITOR: Evidence Forgery

The design correctly identifies this and defers to Phase 2 hash chains. The residual risk ("local user can modify their own state file") is accurately described. In the AI agent context, the "local user" includes the agent itself -- an agent could write directly to the state file to skip gates. This is an inherent limitation of file-based state that can't be fully solved without a trusted mediator. The hash chain makes it detectable but not preventable.

## 4. "Not Applicable" Justification Review

### Download Verification: "Not applicable"

**Verdict: Correctly not applicable.** The engine downloads nothing. Templates and state files are local. Binary distribution is explicitly deferred to a separate design. No issues here.

### Implicit "Not Applicable": Network-based Attacks

The design states "The engine makes no network requests." This is correct for the engine itself, but the `command` gate type can execute arbitrary shell commands, which *can* make network requests. The design acknowledges this under "Command gates in templates" but doesn't explicitly connect it to the "no network requests" claim.

**Recommendation**: Qualify the "no network requests" statement: "The engine makes no network requests directly. Command gates may execute commands that make network requests; this is bounded by the command gate trust model."

### Implicit "Not Applicable": Authentication/Authorization

The design doesn't discuss auth because the engine is a local tool. This is correctly not applicable -- there are no remote users, no multi-tenant access, no privilege levels. The engine runs as the invoking user, period.

### Implicit "Not Applicable": Cryptographic Key Management

No keys are generated or stored. SHA-256 is used as a content hash, not for authentication. Correctly not applicable.

## 5. Summary of Findings

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1.1 | Command gate strings must not undergo template interpolation | HIGH - potential shell injection | Resolve before implementation |
| 1.3 | Symlink following on atomic write | MEDIUM | Add symlink check before rename |
| 1.6 | Template path resolution undefined (relative vs absolute) | MEDIUM | Specify resolution rule |
| 1.7 | Evidence values are untrusted input for downstream consumers | MEDIUM | Document in design |
| 1.2 | Library API has no path validation | MEDIUM | Document as caller responsibility |
| 2.1 | Version counter mitigation description overstates protection | LOW | Clarify language in mitigation table |
| 2.3 | "Explicit confirmation" for future registry is insufficient | LOW (future) | Note need for stronger model |
| 3.2 | No timeout on command gates | MEDIUM | Add default timeout |
| 1.4 | Temp file permissions in shared directories | LOW | Set 0600 permissions |
| 1.5 | Unbounded history growth | LOW | Advisory; consider future warning |
