# Lead: What's the right sanitization strategy and safe character set?

## Findings

### How gates execute commands
In `src/gate.rs:69`, gate commands are passed to `sh -c` via:
```rust
cmd.arg("-c").arg(&gate.command)
```
The command string is passed as a single argument to the shell. If a variable value
contains `; rm -rf /` and is substituted into `test -f {{PREFIX}}/file.md`, the
shell interprets the semicolon as a command separator.

### Three strategies evaluated

**Strategy A: Character allowlist (reject at init time)**
- Allow: `[a-zA-Z0-9._/-]` (alphanumerics, dots, underscores, hyphens, forward slashes)
- Reject everything else with a clear error
- Pros: simplest, no injection possible, easy to reason about
- Cons: restrictive — can't use spaces, colons, or other characters in values
- Covers all known use cases: issue numbers (`42`), artifact prefixes (`issue_42`),
  paths (`wip/issue_42_context.md`), slugs (`add-retry-logic`)

**Strategy B: Shell escaping at substitution time**
- Wrap values in single quotes, escape internal single quotes
- Pros: allows any character
- Cons: fragile — depends on correct escaping for the target shell, hard to audit,
  edge cases with nested quoting in gate commands

**Strategy C: Environment variables instead of string interpolation**
- Pass variables as env vars to the shell process, reference as `$KEY` in commands
- Pros: no injection possible (shell handles env vars safely)
- Cons: changes the substitution model entirely, doesn't work for directive text
  (which isn't executed in a shell), requires two different mechanisms

### Real usage patterns from the parent design
- `ISSUE_NUMBER`: numeric (`42`, `71`) — easily fits allowlist
- `ARTIFACT_PREFIX`: `issue_42`, `task_add-retry-logic` — fits allowlist
- Gate commands: `test -f wip/issue_{{ISSUE_NUMBER}}_context.md`,
  `check-staleness.sh --issue {{ISSUE_NUMBER}}`

## Implications

Strategy A (allowlist) is the clear winner for the initial implementation. The
character set `[a-zA-Z0-9._/-]` covers every known use case from the parent design
while eliminating injection risk entirely. Strategy B is fragile and Strategy C
doesn't work for directive text.

The allowlist regex for values: `^[a-zA-Z0-9._/-]*$` (allows empty string if user
explicitly sets `--var KEY=`).

Variable names should also be validated: `^[A-Z][A-Z0-9_]*$` (uppercase with
underscores, matching the `ISSUE_NUMBER` / `ARTIFACT_PREFIX` convention).

## Surprises

The env var approach (Strategy C) initially seemed appealing but breaks down because
directive text isn't shell-executed — there's no shell to expand `$KEY` in a
directive string returned by `koto next`.

## Open Questions

1. Should the allowlist be extensible per-template, or fixed globally? Fixed globally
   seems right for now — keeps the security model simple.

## Summary

Allowlist-based sanitization at init time is the right strategy: reject values
containing characters outside `[a-zA-Z0-9._/-]` with clear error messages naming
the forbidden character. This covers all known use cases (issue numbers, artifact
prefixes, file paths) while eliminating shell injection risk entirely.
