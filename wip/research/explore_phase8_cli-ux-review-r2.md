# CLI UX Review (Round 2): DESIGN-cross-agent-delegation

Reviewer role: CLI UX specialist (second pass)
Design reviewed: `docs/designs/DESIGN-cross-agent-delegation.md`
Existing CLI code reviewed: `cmd/koto/main.go`, `pkg/controller/controller.go`, `pkg/template/compiled.go`, `pkg/template/template.go`, `pkg/template/compile/compile.go`
Round 1 review: `wip/research/explore_phase8_cli-ux-review.md`

---

## Round 1 Fixes Verified

The following round 1 findings were addressed. Verification notes inline.

| R1 Finding | Status | Notes |
|-----------|--------|-------|
| Rename `submit` to `run` | Fixed | All references now say `koto delegate run` |
| Fix `config.Load()` to not swallow parse errors | Fixed | `loadFile` now returns `(nil, nil)` for missing, `(nil, error)` for broken |
| Add `exit_code` to response JSON | Fixed | Present in both success and failure examples |
| Add `matched_tag` to response JSON | Fixed | Present in all response examples |
| Warn on dropped project rules | Fixed | `merge()` emits stderr warning for unknown targets |
| Remove `default` field | Fixed | Gone from `DelegationConfig` struct |
| Fix `io.LimitWriter` to `io.LimitReader` | Fixed | Now uses `io.LimitReader` on pipe read end |

All seven round 1 must-fix and should-fix items have been addressed. The round 1 nice-to-haves (specific exit codes, `command_hint`, per-rule timeouts, `koto config show`) were intentionally deferred, which is fine.

---

## New Findings

### Finding 1 (Medium): `--prompt` was not renamed to `--prompt-file`

Round 1 recommended renaming `--prompt` to `--prompt-file` because the flag takes a file path, not prompt text. The design still uses `--prompt`:

```bash
koto delegate run --prompt /tmp/prompt.txt
echo "prompt text" | koto delegate run --prompt -
```

The round 1 rationale still stands: `--prompt` looks like it accepts inline text (matching `claude -p "text"`). An agent writing `koto delegate run --prompt "Analyze the codebase"` will get a file-not-found error. The `-` stdin convention mitigates this somewhat, but the flag name is misleading for the file-path case.

This was listed in the round 1 must-fix category (item 2) but wasn't addressed. It may have been an intentional decision to keep the shorter name. If so, the rationale should be documented. If it was an oversight, the rename should be applied.

**Recommendation:** Either rename to `--prompt-file` or add a note explaining why `--prompt` was retained despite the round 1 recommendation. At minimum, the design's examples should show the `-` convention prominently so agents learn stdin piping first (which sidesteps the naming ambiguity).

---

### Finding 2 (Medium): `koto delegate run` integration with main.go's command dispatch

The existing CLI dispatches commands through a flat `switch` in `main()`:

```go
switch os.Args[1] {
case "version": ...
case "init": ...
case "transition": ...
case "next": ...
// ...
case "template":
    err = cmdTemplate(os.Args[2:])
default:
    printError("unknown_command", ...)
}
```

The `template` command handles subcommand dispatch inside `cmdTemplate()`. The design proposes the same for `delegate`. This is consistent. However, the design doesn't show what happens when a user runs `koto delegate` with no subcommand.

Looking at the existing pattern, `cmdTemplate` returns a usage error:

```go
func cmdTemplate(args []string) error {
    if len(args) == 0 {
        return fmt.Errorf("usage: koto template <subcommand>\navailable subcommands: compile")
    }
```

The delegate command should follow the same pattern. The design should specify the `cmdDelegate` shell function and its no-argument error message. This is a small detail, but implementers reading the design need it to stay consistent.

**Recommendation:** Add a note that `koto delegate` with no arguments should return `usage: koto delegate <subcommand>\navailable subcommands: run`, matching the `template` command's pattern.

---

### Finding 3 (Low): Config readability -- `targets` and `rules` separation works but ordering isn't obvious

The config now separates targets from rules:

```yaml
delegation:
  targets:
    gemini:
      command: ["gemini", "-p"]
    claude:
      command: ["claude", "-p", "--model", "opus"]
  rules:
    - tag: deep-reasoning
      target: gemini
    - tag: large-context
      target: gemini
```

This reads well for the common case. The separation is clear: targets are "what can I call," rules are "when do I call it." Two observations:

**Observation 3a: Rule ordering significance isn't visible in YAML.** The design says "first match wins" for tag resolution. YAML arrays preserve order, so rule ordering is semantically meaningful. But nothing in the config communicates this. A user who puts their catch-all rule first will get unexpected behavior. The config itself could include a comment in the example:

```yaml
  rules:  # first matching rule wins
    - tag: deep-reasoning
      target: gemini
```

**Observation 3b: The target/rule indirection adds a lookup step.** When reading config, you see `target: gemini` in a rule, then need to scroll up to find what `gemini` actually runs. For configs with two targets, this is fine. For configs with many, it's a minor readability cost. This is the standard trade-off of normalization -- you eliminate duplication at the cost of indirection. The design made the right call here since multiple rules often map to the same target.

**Recommendation:** Add `# first matching rule wins` as a comment in the example YAML. No structural change needed.

---

### Finding 4 (Medium): Delegate interface contract is clear but has one gap

The delegate interface contract table is a good addition:

| Aspect | Contract |
|--------|----------|
| Input | Raw prompt text piped to stdin |
| Output | Raw text captured from stdout |
| Working directory | Same as koto |
| ...etc. |

This table is the single best reference for skill authors and delegate CLI implementers. Two issues:

**Gap 4a: What happens to delegate stderr?** The contract specifies stdin and stdout but says nothing about stderr. Looking at the `invokeDelegate` code:

```go
cmd := exec.CommandContext(ctx, binary, target.Command[1:]...)
cmd.Stdin = bytes.NewReader(prompt)
```

There's no `cmd.Stderr` assignment, which means the delegate's stderr inherits koto's stderr (Go's `os/exec` default behavior). This means delegate progress messages, warnings, and errors will appear on koto's stderr, which is also where koto prints its own warnings (e.g., project rule drops). There's no separation between koto stderr and delegate stderr.

This is probably the right default -- the user should see delegate errors. But the contract table should document it explicitly:

| **Stderr** | Delegate stderr passes through to koto's stderr (visible to user) |

**Gap 4b: No mention of signal handling.** If the user sends SIGINT (Ctrl+C) during delegation, what happens? With `exec.CommandContext`, the context timeout cancels the process, but a user interrupt is different from a timeout. Go's default behavior propagates SIGINT to child processes in the same process group. This means Ctrl+C kills both koto and the delegate, which is correct behavior. But if koto wants to capture the partial response or report "delegation canceled," it needs explicit signal handling.

For v1, the default behavior (SIGINT kills everything) is acceptable. But the contract should mention it:

| **Signals** | Delegate receives signals from the same process group (Ctrl+C kills both koto and delegate) |

**Recommendation:** Add stderr and signal rows to the interface contract table.

---

### Finding 5 (Medium): Response JSON shape -- `exit_code: 0` on success is noise

The response now includes `exit_code` in all cases:

```json
{
  "response": "...",
  "delegate": "gemini",
  "matched_tag": "deep-reasoning",
  "duration_ms": 12345,
  "exit_code": 0,
  "success": true
}
```

The `exit_code: 0` on successful responses adds a field that carries no information -- if `success: true`, the exit code is necessarily 0. This creates a consistency question: should `exit_code` be present always (current design), or only when `success: false` (omitempty)?

There's a case for both approaches:

- **Always present (current):** Consistent schema. Agents don't need conditional field access. The response Go struct has `ExitCode int` with no omitempty, so 0 serializes to `0`, not absent.
- **Omitempty:** Reduces noise. `exit_code` is only interesting on failure.

The current approach (always present) is fine. Go's zero value for `int` is `0`, and including it always avoids the ambiguity of "was exit_code 0, or was the field absent?" that omitempty would create. Keep as-is.

However, looking at the Go struct, there's a problem:

```go
return &DelegateResponse{
    Response:   string(output),
    Delegate:   targetName,
    MatchedTag: matchedTag,
    DurationMs: duration.Milliseconds(),
    ExitCode:   0,
    Success:    true,
}, nil
```

The design doesn't show the `DelegateResponse` struct definition. The JSON examples show `exit_code`, `matched_tag`, and `duration_ms` (all snake_case), which is consistent with koto's existing JSON output conventions (`format_version`, `initial_state`, etc.). Good.

**Recommendation:** No change to the JSON shape. But the design should include the `DelegateResponse` struct definition for completeness, alongside the request-side `invokeDelegate` code already shown. Implementers need both.

---

### Finding 6 (Low): `koto delegate run` re-resolves delegation but doesn't accept `--state`

The `delegate run` command "reads the current state from the state file" and "re-resolves the delegation target." But the design doesn't show how it locates the state file. Every other state-dependent command in `main.go` accepts `--state` and `--state-dir` flags, resolved through `resolveStatePath()`:

```go
resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
```

The `delegate run` command should follow the same pattern. The design should show:

```bash
koto delegate run --prompt /tmp/prompt.txt
koto delegate run --prompt /tmp/prompt.txt --state wip/koto-my-workflow.state.json
koto delegate run --prompt /tmp/prompt.txt --state-dir ./state
```

Without `--state`/`--state-dir` support, `delegate run` can't be used in multi-workflow scenarios (where multiple state files exist and auto-detection fails). This would be a regression from every other state-dependent command.

**Recommendation:** Explicitly state that `delegate run` supports `--state` and `--state-dir` with the same semantics as `next`, `transition`, etc.

---

### Finding 7 (Low): `koto next` config loading not shown in cmdNext

The design says "the CLI integrates config loading in its startup path" and shows the controller constructor:

```go
ctrl, err := controller.New(eng, tmpl, controller.WithDelegation(cfg.Delegation))
```

But the existing `cmdNext` function creates the controller without options:

```go
ctrl, err := controller.New(eng, tmpl)
```

The design's file change summary lists `cmd/koto/main.go` with "Load config at startup, pass to controller via `WithDelegation()`" but doesn't show the updated `cmdNext` code. This is fine for a design doc (implementation details), but it creates a question: does config loading happen once at startup (affecting all commands) or per-command (only in `cmdNext` and `cmdDelegate`)?

Given that only `next` and `delegate run` use delegation config, loading per-command is more efficient. But if config loading moves to startup, then `init`, `transition`, `query`, etc. all pay the cost of YAML parsing they don't need.

**Recommendation:** Clarify that config loading happens in `cmdNext` and `cmdDelegate`, not at startup. This matches koto's existing pattern where each command loads only what it needs (e.g., `cmdInit` loads the template, `cmdQuery` loads the state file).

---

### Finding 8 (Low): The `Fallback` field in DelegationInfo is subtly confusing

```go
type DelegationInfo struct {
    Target     string `json:"target"`
    MatchedTag string `json:"matched_tag"`
    Available  bool   `json:"available"`
    Fallback   bool   `json:"fallback,omitempty"`
    Reason     string `json:"reason,omitempty"`
}
```

The design says: "When `Delegation.Available` is false and `Delegation.Fallback` is true, the agent handles the directive itself."

`Fallback` is always the logical inverse of `Available` in the current design. If the delegate isn't available, fallback is true. If it's available, fallback is false (omitted). There's no scenario where `Available: true` and `Fallback: true` (or `Available: false` and `Fallback: false`).

If `Fallback` always equals `!Available`, it's redundant. Agents can derive it: "if delegation is present and available is false, fall back." The extra field adds a concept without adding information.

The field could become useful if there were other fallback triggers besides availability (e.g., user config says "always fall back for this tag" or "this target is rate-limited"). But the current design has one trigger.

**Recommendation:** Consider removing `Fallback` and relying on `Available` alone. If `Fallback` is kept, document a scenario where `Fallback` diverges from `!Available` to justify its existence. If the plan is future extensibility (other fallback triggers), say so.

---

### Finding 9 (Low): The `truncated` field has no size information

When output exceeds 10 MB:

```json
{
  "response": "...(truncated)...",
  "truncated": true,
  ...
}
```

The agent knows the response was truncated but doesn't know how much was lost. Was the full response 10.1 MB (barely truncated) or 500 MB (mostly truncated)? Without `total_bytes` or `response_bytes`, the agent can't assess whether the truncated response is still useful.

Getting the total size is tricky -- you'd need to keep reading stdout after the limit to measure total output, which defeats the memory protection. But you can report how much was captured:

```json
{
  "response": "...",
  "truncated": true,
  "response_bytes": 10485760
}
```

This tells the agent "you got 10 MB" without revealing how much was lost.

**Recommendation:** Optionally add `response_bytes` (integer, omitempty, present when truncated) so agents know the captured size. Low priority -- this is an edge case.

---

### Finding 10 (Medium): Unknown YAML key detection is mentioned but not specified

The design says "unknown YAML keys produce a warning to stderr (catches typos like `delgation:`)" but doesn't show how. Go's `yaml.v3` package silently ignores unknown keys by default (like `encoding/json`). To detect unknown keys, you'd need either:

1. `yaml.Decoder` with `KnownFields(true)` -- which rejects unknowns with an error, not a warning
2. Two-pass parsing: unmarshal into the typed struct, then into `map[string]interface{}`, and diff the keys

Option 1 is too strict (errors when warnings are intended). Option 2 works but is non-trivial. The design should specify which approach is used, because the implementer needs guidance on this.

If the implementation uses `KnownFields(true)`, then a typo like `delgation:` will cause config loading to fail, not warn. This changes the UX: what the design calls "a warning" becomes a hard error that prevents any koto operation from running when config exists.

**Recommendation:** Choose between strict (error on unknown keys, simpler to implement, matches the "don't swallow errors" principle) and lenient (warn on unknown keys, requires two-pass parsing, more forgiving). Document the choice. Both are defensible; the current description implies lenient but doesn't commit.

---

## Summary of Round 2 Findings

### Should-fix

1. **R1 carry-over: `--prompt` not renamed to `--prompt-file`.** The flag name is misleading for file paths. Either rename or document the decision to keep it.
2. **Add stderr and signals to the delegate interface contract.** The contract table is incomplete without documenting where delegate stderr goes and what happens on Ctrl+C.
3. **Support `--state`/`--state-dir` on `delegate run`.** Every other state-dependent command supports these flags. Omitting them breaks multi-workflow usage.
4. **Specify unknown YAML key handling.** The design says "warning" but doesn't specify the mechanism, and the obvious implementation (KnownFields) produces errors, not warnings.

### Consider

5. **Document `koto delegate` with no subcommand behavior.** Match the `koto template` pattern.
6. **Include `DelegateResponse` struct definition.** The design shows `invokeDelegate` but not the response type.
7. **Add "first matching rule wins" comment to config examples.** Rule ordering is semantically important but not visible.
8. **Clarify that config loads per-command, not at startup.** Matches koto's existing per-command loading pattern.
9. **Evaluate whether `Fallback` is redundant with `!Available`.** If they always track, one should go.
10. **Optionally add `response_bytes` when truncation occurs.** Helps agents assess whether a truncated response is still useful.
