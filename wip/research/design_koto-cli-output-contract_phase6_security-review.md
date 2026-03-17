# Security Review: koto-cli-output-contract (Phase 6 - Architect Review)

## Scope

Review of the Phase 5 security analysis against the design doc and current codebase.
Focus: uncovered attack vectors, mitigation sufficiency, "not applicable" accuracy, residual risk.

## Assessment of Phase 5 Analysis

The Phase 5 review is solid and grounded in the codebase. Its recommendations (payload size enforcement, environment inheritance documentation) were already incorporated into the design's Security Considerations section. No structural errors in the analysis.

## Question 1: Attack Vectors Not Considered

### 1.1 Shell Injection via Gate Command String Interpolation (Low risk, worth documenting)

The design specifies `sh -c "<command>"` where `<command>` comes from the compiled template. The Phase 5 review treats this purely as a trusted-source concern. But there's a subtlety: if gate commands ever incorporate template *variables* (declared in `variables` in the template YAML and passed at `koto init` time via the `variables` HashMap), then user-supplied values flow into shell commands. The current template compiler doesn't perform variable interpolation into gate commands, but nothing in the design prohibits it, and the `variables` field exists in the compiled template.

**Current state:** Not exploitable. `variables` are stored in the `workflow_initialized` event but not interpolated into gate commands.

**Risk:** If a future change adds variable interpolation to gate commands without escaping, it becomes a shell injection vector. The design should state explicitly that gate commands are literal strings from the compiled template, never interpolated.

### 1.2 TOCTOU on Template File Between Hash Check and Parse (Negligible)

In `cli/mod.rs:250-279`, the current `next` handler reads the template file, hashes it, compares to the stored hash, then parses it. These are separate operations on the same file. A race condition could swap the file between hash check and parse. In practice this is negligible -- the file is in a cache directory, the attacker would need write access to the cache, and if they have that they can modify the state file directly. Not worth a design change, but worth noting that the read-hash-parse should ideally operate on the same byte buffer (which the current code already does -- `std::fs::read` returns bytes, `sha256_hex` hashes them, `serde_json::from_slice` parses the same bytes). The current code is correct.

### 1.3 Event Log Injection via Malformed Evidence Values (Low risk)

Evidence values are validated for type and required-ness, but string values are persisted as-is into JSONL. If a string value contains a newline followed by valid JSON, a naive line-by-line JSONL reader could interpret it as a separate event. The `serde_json` serializer escapes newlines in string values (`\n`), so this is safe as long as evidence is serialized through serde rather than string concatenation. The design uses `append_event()` which serializes via serde. Not exploitable, but the invariant (all event serialization goes through serde) is load-bearing and should not be bypassed.

### 1.4 Concurrent Workflow Access (Not addressed)

Multiple agents or processes could call `koto next --with-data` simultaneously on the same workflow. The design specifies atomic event appending (one JSONL line per operation), but doesn't address what happens if two evidence submissions race. Both could validate against the same state, both could append, and the state machine could end up with two `evidence_submitted` events from the same state. The `append_event` function uses file-level appending with `fsync`, but there's no file locking.

**Severity:** Low for the current single-agent model. If koto ever supports concurrent agents on the same workflow, this needs file locking or compare-and-swap on sequence numbers.

## Question 2: Mitigation Sufficiency

### 2.1 Process Group Isolation: Sufficient

`setpgid`/`killpg` via `pre_exec` is the correct Unix mechanism. Combined with the 30s timeout, this bounds resource consumption from hung gate commands. The design correctly notes this is Unix-only.

### 2.2 Evidence Validation: Sufficient

Strict validation (required fields, type checking, enum constraints, unknown field rejection) is the right approach. The validator rejects before appending, so invalid data never reaches the event log.

### 2.3 Payload Size Limit: Sufficient with caveat

The 1MB limit at CLI argument parsing time is correct. One caveat: `--with-data` takes a JSON string as a CLI argument. On most Unix systems, `ARG_MAX` is already ~2MB, so the OS enforces a loose upper bound. But the explicit 1MB check is still needed because (a) `ARG_MAX` varies, (b) the limit should be stable across platforms, and (c) it prevents near-limit payloads that would succeed on Linux but fail on macOS.

### 2.4 Template Hash Verification: Sufficient

The current code reads the template bytes once and uses the same buffer for both hashing and parsing, eliminating the TOCTOU window. This is correct.

## Question 3: "Not Applicable" Justification Accuracy

### 3.1 Download Verification: Correctly marked N/A

No network operations in this design. The template cache is a local file created by `compile_cached()`. Correct.

### 3.2 Supply Chain: Correctly marked "limited"

The Phase 5 review correctly identifies the template as the supply chain concern and notes that hash verification (not signing) is appropriate for the trusted-source model. The note about integration name resolution being deferred to issue 49 is accurate -- I verified that the current codebase has no integration runner.

## Question 4: Residual Risk

### 4.1 Environment Secrets in Gate Commands (Accepted risk, documented)

Gate commands can read API keys, tokens, and other secrets from the process environment. The design documents this and accepts it as consistent with standard developer tooling. This is the right call for the trusted-source model.

### 4.2 Plaintext Evidence in Event Logs (Accepted risk, documented upstream)

Evidence containing sensitive data is stored as plaintext JSONL. The upstream design addresses this with 0600 file permissions (verified in `persistence.rs:719`). Acceptable.

### 4.3 No File Locking on Event Log (Residual risk)

As noted in 1.4, concurrent access to the same workflow is unguarded. This is acceptable for the current single-agent model but becomes a correctness issue (not just security) if concurrent access is supported later. Should be documented as a known limitation.

### 4.4 Gate Command Variable Interpolation (Latent risk)

As noted in 1.1, the `variables` mechanism exists but is not currently used in gate commands. If someone adds interpolation without shell escaping in a future change, it becomes exploitable. A one-line note in the Security Considerations section ("gate commands are literal template strings, never interpolated with runtime values") would make this invariant explicit and prevent the mistake.

## Summary

The Phase 5 security analysis is accurate and its recommendations have been incorporated. Two additional items merit documentation in the design:

1. **Explicit statement that gate commands are not interpolated with variables.** This prevents a future contributor from adding variable interpolation into shell commands without proper escaping. One sentence in the Command Gate Execution section.

2. **Concurrent access limitation.** Note that the event log append model assumes a single writer. This is a correctness constraint more than a security one, but it belongs in the design since it affects the safety guarantees of evidence submission.

Neither item requires a design change. Both are documentation additions to the existing Security Considerations section.

No risk escalation needed. The security surface is small and well-bounded.
