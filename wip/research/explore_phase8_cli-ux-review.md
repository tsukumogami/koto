# CLI UX Review: DESIGN-cross-agent-delegation

Reviewer role: CLI UX specialist
Design reviewed: `docs/designs/DESIGN-cross-agent-delegation.md`
Existing CLI code reviewed: `cmd/koto/main.go`, `pkg/controller/controller.go`
Supporting docs reviewed: `docs/guides/cli-usage.md`, `docs/reference/error-codes.md`, `plugins/koto-skills/skills/hello-koto/SKILL.md`

---

## 1. Command Ergonomics: `koto delegate submit`

### Current command tree

koto's top-level commands are all single-word verbs that operate directly on workflow state: `init`, `next`, `transition`, `query`, `status`, `rewind`, `cancel`, `validate`, `workflows`, `version`. The one exception is `template`, which is a noun grouping authoring tools (`template compile`). This is consistent with the stated principle: workflow operations are top-level, supporting tools are grouped under nouns.

### Analysis of `koto delegate submit`

The proposed `delegate submit` follows the `template compile` pattern -- a noun group with a verb subcommand. This is structurally consistent. But there are some UX concerns.

**Problem 1: `submit` implies a queue, not a synchronous call.** In CLI convention, "submit" suggests dropping something off for later processing. `kubectl apply`, `gh pr create`, `slurm sbatch` all use "submit" or synonyms when the result isn't immediate. But `koto delegate submit` is synchronous -- it blocks until the delegate responds. The name sets wrong expectations.

Better alternatives:
- `koto delegate run` -- direct, implies synchronous execution (like `docker run`)
- `koto delegate invoke` -- slightly more formal but accurate
- `koto delegate exec` -- follows the `docker exec` and `kubectl exec` convention for "run something and wait"

**Recommendation:** Rename to `koto delegate run`. It's the shortest option that correctly signals synchronous behavior.

**Problem 2: Will `delegate` ever have other subcommands?** The design mentions only `submit`. If `delegate` will only ever have one subcommand, the nesting adds a word for no reason. Compare:

```bash
koto delegate run --prompt /tmp/prompt.txt    # two-level
koto delegate --prompt /tmp/prompt.txt        # flat
```

The flat form is more ergonomic if no other subcommands are planned. But the design hints at possible future expansion (multi-turn delegation, streaming), so reserving the namespace is reasonable. Keep the two-level form but be prepared to question it if no second subcommand materializes within a few releases.

**Problem 3: The command name doesn't help with discoverability.** An agent encountering `koto next` output with a `delegation` field has no obvious path to the submission command. The field name is "delegation" but the command is "delegate." This minor inconsistency could be friction in practice. Consider whether the JSON field should be `delegate` to match the command, or the command should be `delegation`.

**Verdict:** The two-level nesting is fine given future extensibility. Rename `submit` to `run`. Align the JSON field name and command noun.

---

## 2. Flag Naming: `--prompt` for a File Path

### The problem

`--prompt` names the semantic content (this is a prompt), not the mechanical action (read from this path). When a flag accepts both a file path and `-` for stdin, CLI convention leans toward naming it for the input mechanism. Consider:

- `docker build -f Dockerfile` -- `-f` names the file
- `kubectl apply -f manifest.yaml` -- `-f` names the file
- `jq --jsonargs < file` -- the flag names what it is
- `claude -p "prompt text"` -- `-p` is the prompt itself, as a string argument

The confusion with `--prompt` is that it looks like it should accept prompt text directly (like `claude -p`), but it actually takes a file path. An agent writing `koto delegate run --prompt "Analyze the codebase"` would get a "file not found" error.

### Recommendation

Use `--prompt-file` when the argument is a file path. This is unambiguous:

```bash
koto delegate run --prompt-file /tmp/prompt.txt
echo "prompt text" | koto delegate run --prompt-file -
```

The `-` convention for stdin still works with `--prompt-file` because the `-` convention is universal and understood as "read from stdin instead of a file."

Alternatively, accept both forms:
- `--prompt "text"` -- inline prompt string
- `--prompt-file /path` -- read from file

This dual-input pattern is common in tools that handle variable-length input. But given that prompts can be very large (potentially megabytes of context), the inline string form would hit shell argument length limits. Accepting only `--prompt-file` is the safer design. Inline strings can always be piped through stdin:

```bash
echo "short prompt" | koto delegate run --prompt-file -
```

**Verdict:** Rename to `--prompt-file`. It's explicit about what the argument is and avoids confusion with inline prompt text.

---

## 3. Output Format: Delegate Response JSON

### Proposed shape

```json
{
  "response": "...",
  "delegate": "gemini",
  "duration_ms": 12345,
  "success": true
}
```

### Analysis

**Field naming is mostly good.** `response`, `delegate`, `success`, and `error` are intuitive. Two issues:

**Issue 1: `duration_ms` is an uncommon suffix pattern.** Most CLIs that report timing use either a bare number with documented units or a human-readable string. But for machine consumption (which is this command's primary audience), `duration_ms` is fine -- it's self-documenting and avoids ambiguity about units. This is actually the right call for a JSON-first CLI.

**Issue 2: `truncated` field appears only on truncation.** The design mentions `"truncated": true` when output exceeds 10 MB, but this field is absent from the normal response. This is the right pattern (omitempty) for JSON, but agents need to know the field exists. Documenting it isn't enough -- consider always including `"truncated": false` so agents can code against a consistent schema. Or, accept the omitempty approach since agents that don't know about truncation will still work (they just won't know the response is incomplete).

**Recommendation:** Keep `truncated` as omitempty. Agents that care about completeness will read the docs; agents that don't will work fine either way.

**Issue 3: Missing `exit_code` field on failure.** When `success: false`, the `error` field contains a string like "delegate process exited with code 1". But the exit code is structured data buried in a string. Consider:

```json
{
  "response": "",
  "delegate": "gemini",
  "duration_ms": 5000,
  "success": false,
  "error": "delegate process exited with non-zero status",
  "exit_code": 1
}
```

This lets agents distinguish between "delegate returned an error" (exit code 1) and "delegate timed out" (no exit code, different error string) without parsing the error message.

**Recommendation:** Add `exit_code` (integer, omitempty) to the response for programmatic failure analysis.

### Missing field: `matched_tag`

The delegate response doesn't carry which tag triggered the delegation. The `koto next` output has `matched_tag` in the DelegationInfo, but the `koto delegate run` response doesn't echo it back. For logging and debugging, the response should include the tag that caused this delegation. The agent shouldn't need to correlate across two command outputs.

**Recommendation:** Add `matched_tag` to the delegate response JSON.

---

## 4. Config UX: Two-Level Config with Opt-In

### Analysis

The two-level config (user at `~/.koto/config.yaml`, project at `.koto/config.yaml`) follows established convention: Docker, Git, npm, and kubectl all use layered config with user-level defaults and project-level overrides.

**The opt-in gate is well-designed.** Requiring `allow_project_config: true` in user config before project-level delegation rules take effect is the right security boundary. The design correctly identifies the supply-chain risk of a cloned repo shipping delegation config.

**The command-field restriction is excellent.** Preventing project config from defining what binary a target name maps to is a smart separation. Target naming is shared vocabulary; binary invocation is a user trust decision.

### Concerns

**Concern 1: Silent dropping of project rules for unknown targets.** When a project rule references a `delegate_to` target not defined in user config, the rule is silently dropped. This will confuse template authors who add delegation to their templates and wonder why it doesn't work.

Consider logging a warning to stderr when project rules are dropped:

```
warning: project delegation rule for tag "security" references unknown target "gemini"; define "gemini" in ~/.koto/config.yaml
```

This is consistent with koto's existing pattern of printing warnings to stderr (the `template compile` command does this for heading collisions).

**Concern 2: What does `default: self` mean?** The design mentions a `default` field in DelegationConfig but doesn't explain the semantics. If `default: self` means "handle locally when no rule matches," that's the zero-value behavior anyway (no delegation when no rule matches). If it means something else, it needs clarification.

Suggestion: Either remove the `default` field entirely (the absence of a matching rule already means "handle locally") or document what values it accepts and what each one does. An undocumented field in config is worse than no field at all.

**Concern 3: Config validation errors.** What happens when `~/.koto/config.yaml` has a syntax error? The design shows `Load()` silently returning `nil` on load errors:

```go
userCfg, _ := loadFile(userConfigPath())
```

Swallowing YAML parse errors is dangerous. A user with a typo in their config will get no delegation and no indication of why. The `_` on that error return is a red flag.

**Recommendation:** `Load()` should return an error when a config file exists but can't be parsed. Missing files are fine (return nil), but present-but-broken files should fail loudly.

**Concern 4: No `koto config` command for introspection.** Users have no way to see the resolved config. With two levels of merging and opt-in gates, debugging "why isn't delegation working" requires mentally simulating the merge logic. A `koto config show` command (or even `koto config validate`) that prints the resolved config would help.

This doesn't need to be in the initial design, but it's worth noting as a gap that will generate support questions.

---

## 5. Error Messages and Exit Codes

### The proposed convention

- Exit 0 + `success: false` in JSON: delegate was invoked but failed
- Non-zero exit: koto-level error (config missing, binary not found, file unreadable)

### Analysis

This is the right pattern for a JSON-first CLI where the consumer is a machine (agent). It follows the same logic as HTTP status codes: 200 with an error body means "the server handled your request and the result is an error," while 500 means "the server couldn't handle your request at all."

**Comparable precedents:**
- `gh api` returns exit 0 with HTTP error responses in the JSON body
- `docker inspect` returns exit 0 even when the container is in an error state
- `kubectl get` returns exit 0 with empty results rather than failing

The key insight: exit code signals whether *koto* succeeded at its job (invoking the delegate and capturing output), not whether the *delegate* succeeded at its job. This is correct.

**Potential confusion:** koto's existing commands all use exit 0 = success, non-zero = error, with no "exit 0 but the operation failed" case. The `delegate run` command introduces a new semantic where exit 0 doesn't mean everything worked. Agents already parsing koto output by exit code alone would need to change their logic.

**Recommendation:** This is fine as-is because `delegate run` is a new command with no backward compatibility constraint. Document the exit code semantics clearly in the CLI usage guide.

**Missing: specific exit codes.** The design says "non-zero for koto errors" but doesn't specify which non-zero code. koto currently uses `os.Exit(1)` for everything. Consider defined exit codes:

| Exit Code | Meaning |
|-----------|---------|
| 0 | Delegate invoked (check `success` in JSON) |
| 1 | General koto error |
| 2 | Config error (missing or invalid) |
| 3 | Binary not found |

Distinct exit codes let shell scripts handle errors without parsing JSON:

```bash
koto delegate run --prompt-file prompt.txt
case $? in
  0) ;; # parse JSON for success/failure
  2) echo "Fix your koto config" ;;
  3) echo "Install the delegate CLI" ;;
esac
```

**Recommendation:** Define specific exit codes for the `delegate run` command. Even if agents always parse JSON, scripts and humans benefit from distinct codes.

---

## 6. Discoverability

### Can users figure out delegation from the CLI alone?

**No.** The delegation feature has several discovery gaps.

**Gap 1: No help text.** koto has no `--help` flag on any command. Running `koto` with no arguments prints `usage: koto <command> [flags]` but doesn't list commands. Running `koto delegate` with no subcommand would presumably print an error. There's no way to discover delegation exists without docs.

This is an existing problem (not introduced by this design), but delegation makes it worse because the feature has multiple moving parts (tags, config, the delegate command) that need explanation.

**Gap 2: `koto next` output hints but doesn't explain.** When `koto next` returns a `delegation` field, the agent sees the target and availability. But the response doesn't include a hint about what to do with this information -- no "run `koto delegate run --prompt-file <path>` to delegate" message.

Consider adding a `hint` or `command` field to DelegationInfo:

```json
{
  "delegation": {
    "target": "gemini",
    "matched_tag": "deep-reasoning",
    "available": true,
    "command_hint": "koto delegate run --prompt-file <path>"
  }
}
```

This is particularly useful for agent skill authors who are reading `koto next` output for the first time.

**Gap 3: No config scaffolding.** Users need to know the config YAML structure to set up delegation. There's no `koto config init` or `koto delegate setup` command that generates a starter config. Users must read docs and write YAML manually.

For an initial release this is acceptable, but consider adding a `koto config init` command that writes a commented example to `~/.koto/config.yaml`.

**Gap 4: Fallback behavior is invisible.** When delegation falls back to self-handling (delegate unavailable, no config), the agent gets `fallback: true` in the DelegationInfo. But there's no indication in the agent's output to the user that delegation was attempted and fell back. The user doesn't know they could have gotten a better result if they'd installed the delegate CLI.

This is a design trade-off (silent degradation vs noisy warnings), and the design chose correctly for v1. But consider a `--verbose` flag on `koto next` that prints delegation resolution details to stderr.

---

## 7. `koto next` Output Changes

### Proposed additions

```json
{
  "action": "execute",
  "state": "deep-analysis",
  "directive": "Analyze the codebase...",
  "tags": ["deep-reasoning"],
  "delegation": {
    "target": "gemini",
    "matched_tag": "deep-reasoning",
    "available": true
  }
}
```

### Analysis

**Tags as a top-level array is correct.** Tags are metadata on the state, not on the delegation. An agent might use tags for purposes beyond delegation (logging, metrics, UI rendering). Putting them at the top level makes them accessible regardless of whether delegation is configured.

**DelegationInfo as a nullable object is correct.** Using `omitempty` on a pointer means the field is absent when there's no delegation. This is the right pattern for optional structured data in JSON -- it's cleaner than always including a `delegation: null` or `delegation: {}`.

**Field naming within DelegationInfo is good.** `target`, `matched_tag`, `available`, `fallback`, `reason` are all clear.

### Issues

**Issue 1: `available` is ambiguous about timing.** It means "the delegate binary was found in PATH at the time `koto next` was called." But between `koto next` and `koto delegate run`, the binary could be installed or removed. The design acknowledges this (the run command re-checks), but the field name implies a durable fact.

Consider renaming to `binary_found` or adding a note that this is a point-in-time check. Or keep `available` but add a clarifying comment in the user guide. The current name is fine in practice -- users won't overthink it.

**Issue 2: No `command` field in DelegationInfo.** The agent doesn't know what command will be run. For transparency (and for skill authors debugging delegation), including the resolved command would help:

```json
{
  "delegation": {
    "target": "gemini",
    "matched_tag": "deep-reasoning",
    "available": true,
    "command": ["gemini", "-p"]
  }
}
```

This lets agents log what's about to happen. It also lets skill authors verify that config resolution worked correctly.

**Counter-argument:** Exposing the command in the JSON output means agents could bypass `koto delegate run` and invoke the command directly. The design wants koto to own the invocation (for timeout enforcement, structured error reporting, etc.). Exposing the command undermines that.

**Verdict:** Don't include `command` in the output. The indirection through `koto delegate run` is intentional. If debugging is needed, a `koto config show` command is the right place to see resolved commands.

**Issue 3: Directive field naming collision.** The `Directive` struct has a field called `Directive` (the instruction text). This is already confusing in Go code (`d.Directive`), and adding `d.Delegation` and `d.Tags` doesn't make it worse, but the struct name collision is worth noting as tech debt. Not a blocker for this design.

---

## 8. Additional Observations

### Stdin piping semantics

The design says koto pipes the prompt to the delegate CLI via stdin:

```go
cmd.Stdin = bytes.NewReader(prompt)
```

But the prompt is read from a file first (`os.ReadFile(promptPath)`), then written to stdin. This means the entire prompt is held in memory twice (file read + bytes.Reader). For large prompts this is fine (most prompts are well under 10 MB), but the design should note the memory implication.

Also: when `--prompt-file -` is used (read from stdin), the prompt is read from koto's stdin, then piped to the delegate's stdin. This is a data-through path, not a stdin passthrough. The design correctly handles this by reading the full prompt before invoking the delegate, but it means `koto delegate run --prompt-file -` doesn't support streaming the prompt to the delegate. This is acceptable for v1.

### Timeout UX

The timeout is in the config (`timeout: 300`), not on the command line. This means all delegations share the same timeout. Some delegations (deep-reasoning) might need 10 minutes while others (quick classification) need 30 seconds.

Consider per-rule timeouts:

```yaml
delegation:
  rules:
    - tag: deep-reasoning
      delegate_to: gemini
      command: ["gemini", "-p"]
      timeout: 600
    - tag: quick-check
      delegate_to: gemini
      command: ["gemini", "-p"]
      timeout: 30
```

This is a minor enhancement that could be added later without breaking changes, so it's fine to defer.

### `io.LimitWriter` doesn't exist in stdlib

The design uses `io.LimitWriter(&stdout, 10*1024*1024)` but Go's `io` package has `LimitReader`, not `LimitWriter`. This would need a custom implementation or use `io.LimitReader` wrapped around the pipe's read end. Minor implementation detail, but worth noting since someone implementing this verbatim would hit a compilation error.

---

## Summary of Recommendations

### Must-fix (blocking issues)

1. **Rename `submit` to `run`.** "Submit" implies async; this is synchronous.
2. **Rename `--prompt` to `--prompt-file`.** The current name suggests inline text, but the argument is a file path.
3. **Don't swallow config parse errors.** `Load()` must return an error when a config file exists but has invalid YAML. Silent failure will cause hard-to-debug delegation outages.

### Should-fix (significant UX improvements)

4. **Add `exit_code` to delegate response JSON.** Structured failure data shouldn't be buried in an error string.
5. **Add `matched_tag` to delegate response JSON.** Helps with logging and debugging without cross-referencing `koto next` output.
6. **Warn on dropped project rules.** When project config references unknown targets, log a warning to stderr so template authors can diagnose missing delegation.
7. **Clarify or remove the `default` field.** Its semantics are undocumented and its zero-value behavior already covers the "handle locally" case.

### Nice-to-have (consider for v1 or defer)

8. **Define specific exit codes** for the `delegate run` command (config error, binary not found, etc.).
9. **Add a `command_hint` field** to DelegationInfo in `koto next` output for discoverability.
10. **Consider per-rule timeouts** in delegation config.
11. **Plan a `koto config show` command** for debugging resolved config (can be a follow-up issue).
12. **Fix the `io.LimitWriter` reference** -- this doesn't exist in Go's stdlib and would need a custom implementation.
