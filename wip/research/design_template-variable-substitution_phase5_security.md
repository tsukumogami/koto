# Security Review: template-variable-substitution

## Dimension Analysis

### External Artifact Handling

**Applies:** No

This design does not download, fetch, or process external artifacts. Variable values come exclusively from the local user via `--var` CLI flags at `koto init` time. Templates are local files that go through the compile step. There is no network input or remote artifact retrieval involved in variable substitution.

### Permission Scope

**Applies:** Yes (low severity)

Gate commands execute via `sh -c` with the permissions of the calling user. This is pre-existing behavior -- the design doesn't expand the permission surface. Gate evaluation already isolates child processes in their own process group (via `setpgid`) and enforces a 30-second default timeout with SIGKILL on expiry. Variable substitution doesn't change the execution model; it only changes the content of the command string before it reaches the same `sh -c` path.

One note: the design doesn't restrict the working directory for gate commands. A variable value containing a valid path (e.g., `../../somewhere`) could be substituted into a gate command that operates on files relative to the working directory. This isn't a new risk from this design -- the working directory is already set by the caller -- but it's worth noting that the forward slash in the allowlist means variable values can encode relative paths.

**Risk:** Minimal. No privilege escalation. The existing process isolation and timeout mechanisms apply unchanged.

### Supply Chain or Dependency Trust

**Applies:** No

The design adds no new external dependencies. The `Variables` newtype uses standard library types (`HashMap`, `String`) and a regex for pattern matching. The regex crate is already a transitive dependency of the project. No new crate imports are required.

### Data Exposure

**Applies:** No

Variable values are stored in the local event log (a JSON file on disk). They aren't transmitted anywhere. The values flow from CLI flags to the event file to runtime substitution in shell commands and directive text. All of this happens locally. There's no telemetry, logging to external services, or network transmission of variable values.

### Command Injection

**Applies:** Yes

This is the primary security concern of the design. Variable values are interpolated into strings that are then passed to `sh -c` (line 69 of `gate.rs`). The design's mitigation is an init-time allowlist: values must match `[a-zA-Z0-9._/-]` or they're rejected.

#### Allowlist character analysis

**Alphanumeric characters `[a-zA-Z0-9]`**: Safe. No shell metacharacter behavior.

**Dot `.`**: In shell context, a dot by itself is equivalent to `source` (`. script.sh` sources a script). However, this requires the dot to appear as a standalone word in a command position, which isn't achievable through variable substitution alone -- the template author controls the command structure, and the variable fills in a value slot. In path contexts, `..` enables directory traversal (e.g., `../../etc/passwd`). The design explicitly acknowledges this concern in the "Scope limitation" note but defers workflow name validation to a separate effort. For variable values, `..` in paths is a concern only if the template command treats the value as a trusted path without its own validation. Severity: low, because the template author controls the command structure and should validate path usage.

**Forward slash `/`**: Enables absolute and relative path construction. A value of `../../etc/passwd` matches the allowlist. Combined with dot, this allows path traversal. However, the variable is substituted into a command that the template author wrote -- if the command is `test -f wip/issue_{{ISSUE_NUMBER}}_context.md` and the value is `../../etc/passwd`, the resulting command `test -f wip/issue_../../etc/passwd_context.md` is syntactically odd but doesn't execute anything dangerous. The risk is higher in commands where the variable value directly controls a path operand without surrounding text, e.g., `cat {{FILE_PATH}}`. A malicious or careless value could read unintended files, but not execute arbitrary commands. Severity: low-to-medium, context-dependent on template design.

**Hyphen `-`**: Not explicitly in the allowlist regex shown (`[a-zA-Z0-9._/-]`) but is present in the character class. In regex character classes, `-` between characters denotes a range. The placement matters: `._/-` -- here `-` appears between `/` and `]`, which in most regex flavors means it's literal (at the end of the class). In Rust's `regex` crate, a `-` at the end of a character class is treated as a literal hyphen, so this is correct. However, hyphens can be used to pass flags to commands. A value like `--help` or `-rf` could alter command behavior if substituted into the right position. For example, if a gate command is `rm {{CLEANUP_TARGET}}` and the value is `-rf`, the command becomes `rm -rf`. This is mitigated by the fact that template authors control the command structure and should use `--` before variable-derived arguments. Severity: low, requires a poorly written template.

**Underscore `_`**: Safe. No shell metacharacter behavior.

#### Multi-byte / Unicode handling

The allowlist regex `[a-zA-Z0-9._/-]` uses ASCII ranges. Rust's `regex` crate, by default, treats patterns as Unicode-aware, but these character classes are ASCII-only when specified with explicit ranges. The key question is whether the *input validation* regex (applied at init time) correctly rejects multi-byte characters.

If the validation uses something like `^[a-zA-Z0-9._/-]+$` in Rust's regex crate, multi-byte UTF-8 characters won't match the ASCII ranges and will be rejected. This is correct behavior. However, the design should explicitly state that the validation regex uses `^..+$` anchoring to ensure the *entire* value matches, not just a substring. An unanchored regex would allow values like `safe;rm -rf /` to pass if only the `safe` portion is checked.

The design doc says "reject values with characters outside this set" which implies full-value validation, but the implementation must use anchored matching (`^[a-zA-Z0-9._/-]+$`) to enforce this. This should be called out explicitly as a requirement.

**Risk:** Low if anchored correctly. Implementation must verify anchoring.

#### Substitution timing

The design specifies that substitution happens in the gate closure, *before* the command string reaches `evaluate_gates` and ultimately `sh -c`. This is the right ordering -- the sanitized value replaces the `{{KEY}}` placeholder in the command string, producing a fully-formed command that is then passed to the shell. There's no double-interpretation risk because substitution happens once and the result is a plain string.

However, the design should ensure that the substitution output isn't processed by any other template or expansion mechanism before reaching `sh -c`. If a future feature adds a second substitution pass, values could be crafted to inject new `{{KEY}}` patterns. The current design doesn't have this problem, but it's worth documenting as an invariant: substitution must be single-pass.

#### Edge case: empty values

The allowlist regex `[a-zA-Z0-9._/-]+` (assuming `+` quantifier) rejects empty strings. But if the quantifier is `*`, an empty value would pass validation and substitute into a gate command, potentially changing command semantics. For example, `test -f {{FILE}}` with an empty value becomes `test -f ` which may behave unexpectedly. The design should explicitly require non-empty values (use `+` not `*`).

The design mentions that `VariableDecl` has a `default` field, and defaults could potentially be empty strings. If a default value is an empty string and the regex uses `*`, it would pass validation. The design should clarify whether empty defaults are valid.

#### Overall injection risk assessment

The allowlist approach is sound. The character set `[a-zA-Z0-9._/-]` excludes all shell metacharacters that enable command injection: semicolons, pipes, ampersands, dollar signs, backticks, quotes, spaces, parentheses, redirects, newlines, and null bytes. No combination of the allowed characters can break out of a string context in a shell command.

The residual risks are:
1. Path traversal via `../` -- not command injection, but could cause unintended file operations depending on template design
2. Flag injection via `-` prefixed values -- could alter command behavior in poorly written templates
3. Implementation correctness -- the regex must be anchored and use `+` quantifier

These are all low severity and are mitigated by template author responsibility and implementation review.

## Recommended Outcome

**OPTION 1: Accept the design as-is, with minor implementation guidance.**

The security model is solid. The allowlist approach is the right choice for this use case -- it's simpler and more reliable than escaping, and the character set is well-chosen. The compile-time and init-time validation layers provide defense in depth.

Three implementation-level items to verify during code review (not design changes):

1. The validation regex must be anchored: `^[a-zA-Z0-9._/-]+$` (not unanchored, not `*`)
2. Document the single-pass substitution invariant to prevent future regressions
3. Consider recommending that template authors use `--` before variable-derived path arguments in gate commands (a documentation suggestion, not a code change)

The path traversal concern is real but correctly scoped out to the separate workflow name validation effort. Variable values used in gate commands are under template author control, and the template author can add path validation within the command itself if needed.

## Summary

The design's security posture is strong for its scope. The init-time allowlist against `[a-zA-Z0-9._/-]` effectively eliminates command injection by excluding all shell metacharacters. Residual risks are path traversal via `../` and flag injection via `-` prefixes, both low severity and dependent on template authoring choices rather than framework flaws. The three-layer validation model (compile-time reference checking, init-time value sanitization, runtime substitution) provides solid defense in depth. Accept with minor implementation guidance on regex anchoring and the single-pass substitution invariant.
