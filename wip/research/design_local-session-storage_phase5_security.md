# Security Review: DESIGN-local-session-storage Phase B

**Reviewer role**: Security
**Design**: `docs/designs/DESIGN-local-session-storage.md`
**Focus**: Phase B (content ownership) additions
**Date**: 2026-03-26

## Outcome: OPTION 2 -- Document considerations

The design's existing security section (lines 473-499) covers the major threat categories. The issues below are completeness gaps in that section, not design-level changes.

---

## 1. External artifact handling -- untrusted input processing

**Finding: Regex pattern in `context-matches` gates accepts template-authored patterns, not agent-authored ones. Low risk.**

The `context-matches` gate evaluates a regex from the template `pattern` field against content from the store. Template authors are trusted (they define the workflow). The content is untrusted (agent-submitted), but it's the haystack, not the needle -- a malicious haystack can't exploit a regex engine in the way a malicious pattern can.

However, the design doesn't specify which regex engine or whether patterns are bounded. A pathologically complex pattern authored in a template (e.g., catastrophic backtracking like `(a+)+$` against a large content blob) could hang the gate evaluator. The existing command gate has a timeout mechanism (line 62-66 in `src/gate.rs`); the `context-matches` evaluation should have an equivalent bound.

**Recommendation**: Document that `context-matches` regex evaluation must be bounded (either by using a regex engine with linear-time guarantees like Rust's `regex` crate, or by applying a timeout). If the implementation uses `regex::Regex`, this is already safe -- worth stating explicitly.

**Severity**: Low. The `regex` crate's default is linear-time, so this is likely a non-issue in practice. But the design should state the assumption.

## 2. Permission scope -- filesystem access

**Finding: `--from-file` reads arbitrary paths; `--to-file` writes arbitrary paths. Acceptable given trust model.**

`koto context add --from-file <path>` reads an arbitrary filesystem path and stores its contents. `koto context get --to-file <path>` writes content to an arbitrary path. This is equivalent to `cat <path>` -- it runs as the current user with the user's permissions.

The design's trust model (line 485-486) correctly notes that agents already have filesystem access via file tools. The `--from-file`/`--to-file` flags don't expand the attack surface beyond what the agent already has.

No additional consideration needed.

## 3. Supply chain / dependency trust

**Finding: No new external dependencies identified in the design.**

The design uses files-with-manifest storage, advisory flock, and atomic rename -- all standard library or existing dependency capabilities. The regex evaluation for `context-matches` will presumably use the `regex` crate already in the Rust ecosystem's standard toolkit.

**Recommendation**: If `context-matches` introduces the `regex` crate as a new dependency, note it. If it's already a transitive dependency, no action needed.

**Severity**: Informational.

## 4. Data exposure -- what moves through the content CLI

**Finding: The security section's "No secrets in context" paragraph (lines 497-499) is adequate for local backend. One gap: `--from-file` and piped stdin could inadvertently ingest secrets.**

An agent could run `koto context add session api-keys.md --from-file ~/.ssh/id_rsa` and the content store would happily accept it. This isn't a koto vulnerability -- the agent chose to do it -- but the design could note that content stored in `ctx/` has no encryption at rest and will be transmitted verbatim by future cloud sync backends.

**Recommendation**: The security section already says "Cloud sync adds exposure -- addressed in the cloud backend design." This is sufficient. No change needed.

## 5. Path traversal via content keys

**Finding: Key validation rules are well-specified but need precise component-level definition.**

Decision 12 specifies: `[a-zA-Z0-9._-/]`, no leading/trailing slashes, no consecutive slashes, no `.`/`..` components. This blocks the standard path traversal vectors (`../../../etc/passwd`, `foo/../../bar`).

Two edge cases worth confirming in implementation:

**5a. Component-level `.`/`..` rejection must be per-component, not substring.**
A key like `foo/research..bar/baz.md` contains `..` as a substring but not as a path component. The design says "no `.`/`..` components" which implies per-component splitting on `/`. The implementation must split on `/` and reject any component that is exactly `.` or `..`, not reject keys containing the substring `..`. The validate.rs pattern for session IDs (lines 1-30) uses character-level allowlisting; the key validator should combine character allowlisting with component-level `.`/`..` checks.

**5b. Keys mapping to files means OS-level path length limits apply.**
A key like `a/b/c/d/.../z.md` with many nesting levels could hit `PATH_MAX` (4096 on Linux) when combined with the session directory path (`~/.koto/sessions/{16-char-hash}/{name}/ctx/`). The design doesn't specify a max key length or depth.

**Recommendation**: Add to security section: "Key validation rejects `.` and `..` as individual path components (splitting on `/`), not as substrings. Maximum key length is bounded at N characters to stay within filesystem path limits." Pick a reasonable N (e.g., 512 characters).

**Severity**: Low. The character allowlist already blocks most creative traversal. But specifying component-level semantics prevents implementation ambiguity.

## 6. Manifest integrity -- crash safety and atomicity

**Finding: Write ordering and atomic rename are correctly specified. One concurrency gap.**

The design specifies (lines 320-324):
- Write content file first, then manifest second
- Manifest writes use atomic rename (write-to-temp, rename)
- Per-key advisory flock on the content file prevents concurrent writes to same key

**6a. Concurrent writes to different keys both update the manifest.**
Two agents writing different keys concurrently both need to update `manifest.json`. The design specifies per-key flock on the content file, but the manifest is shared. Without a manifest-level lock, the sequence could be:

1. Agent A reads manifest, adds key "plan.md"
2. Agent B reads manifest, adds key "review.md"
3. Agent A writes manifest (contains plan.md)
4. Agent B writes manifest (contains review.md, but not plan.md -- A's write is lost)

The design says "Per-key advisory flock on the content file prevents concurrent writes to the same key" but doesn't address manifest-level serialization for different keys.

**Recommendation**: Add to the manifest atomicity description: manifest updates must be serialized. Either use a manifest-level flock, or use a read-lock-modify-write-unlock pattern on the manifest file itself. This is a correctness issue more than a security issue, but data loss (silent key eviction) in concurrent scenarios could cause gate evaluation to incorrectly fail.

**Severity**: Medium (correctness, not security). The "multiple agents submit context concurrently" requirement (line 57) makes this a realistic scenario.

---

## Summary of recommendations for security section

The existing security section covers the right categories. Three additions would make it complete:

1. **Regex evaluation bounds**: State that `context-matches` uses a linear-time regex engine (Rust's `regex` crate) or has a timeout, to prevent pattern-induced hangs.
2. **Key validation precision**: Clarify that `.`/`..` rejection is per path component, and add a maximum key length.
3. **Manifest serialization**: Address concurrent manifest updates from different keys. (This is called out in the Consequences/Negative section as manifest complexity, but the specific concurrent-update scenario isn't covered.)

None of these require design-level changes. They're specification tightening for the implementer.
