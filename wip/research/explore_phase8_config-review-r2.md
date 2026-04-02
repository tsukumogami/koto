# Config System Review (Round 2): DESIGN-cross-agent-delegation

Reviewer perspective: developer tools configuration specialist.

Reviewed file: `docs/designs/DESIGN-cross-agent-delegation.md` (post-R1 revision).

Previous review findings that were incorporated:
- Config now separates targets (binary definitions) from rules (tag routing)
- `KOTO_HOME` explicitly applies to config resolution
- Project config cannot override timeout or define targets
- Removed vestigial `default` field
- `config.Load()` returns errors on malformed YAML (not swallowed)
- Merge explicitly drops project rules for tags already covered by user rules
- Stderr warnings when project rules reference unknown targets
- Unknown YAML keys should warn

---

## 1. Targets/Rules Separation

The new schema shape is clean and conventional. Separating `targets` (binary definitions) from `rules` (tag routing) eliminates the command duplication problem from R1 and makes the security boundary visible from the YAML shape alone.

### What works well

The two-layer structure maps directly to the trust model: user config owns both layers, project config can only add rules that reference existing target names. A reader seeing the YAML can immediately understand the boundary without reading prose.

```yaml
targets:     # user-only: "what binaries exist"
  gemini:
    command: ["gemini", "-p"]
rules:       # user + project: "which tags go where"
  - tag: deep-reasoning
    target: gemini
```

This mirrors patterns in routing infrastructure (nginx upstream blocks + location rules, Kubernetes Services + Ingress rules). The structure is familiar to anyone who's configured a reverse proxy or load balancer.

### Edge cases

**Target with zero rules.** A user defines `targets.claude` but no rule references it. This is valid -- the user may be preparing for a future template that uses a tag they'll route to `claude`. No error or warning needed. The Go types handle this naturally since `Targets` is a map and `Rules` is a slice with no cross-validation requirement.

**Rule referencing nonexistent target in user config.** The design specifies warnings for project config referencing unknown targets, but the user config path has no similar validation. If a user writes:

```yaml
targets:
  gemini:
    command: ["gemini", "-p"]
rules:
  - tag: deep-reasoning
    target: claud   # typo
```

The rule silently does nothing. At `resolveDelegation()` time, the `c.delegationCfg.Targets[rule.Target]` lookup fails and the `continue` skips the rule. The user gets no delegation for `deep-reasoning` with no indication of why.

**Recommendation:** Validate target references in rules during `Load()` -- for both user and project configs. For user config, a rule referencing a nonexistent target is almost certainly a typo and should produce a warning to stderr. For project config, this is already covered (warnings on unknown targets).

**Empty command array.** `DelegateTarget.Command` is `[]string`. A user could write:

```yaml
targets:
  gemini:
    command: []
```

The `execChecker.Available()` method indexes `command[0]`, which panics on an empty slice. The `invokeDelegate()` function has the same issue with `target.Command[0]` and `target.Command[1:]`.

**Recommendation:** Validate during `Load()` that every target's `command` has at least one element. Return an error, not a panic.

**Target name constraints.** Target names are map keys with no validation. A user could use `""` (empty string), `"my target"` (spaces), or emoji. None of these cause functional problems in Go (map keys are strings), but they'd look wrong in JSON output (`"target": ""`).

**Recommendation:** Low priority, but consider validating target names match a simple pattern (alphanumeric + hyphens, similar to tag validation). This prevents confusing output and keeps target names usable as CLI arguments if `koto config show` or similar subcommands are added later.

## 2. Merge Logic Correctness

The `merge()` function in the design is well-structured. Walking through it line by line against the Go types:

### What's correct

1. The `result := *user` shallow copy is safe because the subsequent code only replaces the `Delegation` pointer, never mutates the pointed-to struct in place. The `merged := *user.Delegation` copies the `DelegationConfig` value, including the `Targets` map reference. Since project targets are ignored, the user's `Targets` map is never modified -- the `merged.Rules` slice is the only thing that grows.

2. The `userTags` set correctly prevents project rules from overriding user-defined tag routing. The explicit drop (with `continue`) is clearer than the R1 approach of relying on list ordering.

3. The `user.Delegation.Targets[r.Target]` lookup correctly rejects project rules that reference targets the user hasn't defined.

### Bug: shared slice backing array

```go
merged := *user.Delegation
// ...
merged.Rules = append(merged.Rules, r)
```

The `merged := *user.Delegation` copies the `DelegationConfig` struct value, which copies the `Rules` slice header (pointer, length, capacity). If the user's `Rules` slice has spare capacity (which it often will -- Go's `append` over-allocates), the `append(merged.Rules, r)` writes into the user's backing array without allocating a new one. This means `user.Delegation.Rules` now has a corrupted backing array -- the appended project rule is present in memory past the user's length boundary.

This isn't immediately exploitable because the user slice's length hasn't changed (only the merged copy's length grows), but it violates the principle that merge should not mutate input. If any code later reslices the user's rules (e.g., `user.Delegation.Rules[:cap(user.Delegation.Rules)]`), the project rule leaks in.

More practically: if `merge()` is called twice with the same user config (say, in a test), the second call sees the corrupted backing array from the first call.

**Fix:**

```go
merged := *user.Delegation
// Deep copy the rules slice to avoid shared backing array
merged.Rules = make([]DelegationRule, len(user.Delegation.Rules))
copy(merged.Rules, user.Delegation.Rules)
```

### Bug: nil Delegation pointer on project config

```go
if userCfg == nil {
    return projCfg, nil
}
```

When user config doesn't exist (`userCfg == nil`) but project config does, the function returns the project config directly. But project config can contain `delegation.targets` and `delegation.timeout` entries (the YAML parser won't reject them). These fields are supposed to be ignored for project configs, but the "ignore project targets/timeout" logic only runs inside `merge()`. When there's no user config, `merge()` is never called, and the project config is returned as-is -- targets, timeout, and all.

This means a project without any user config can define arbitrary targets and set timeout, bypassing the security boundary.

**Fix:** Either:
- (a) Strip project-only-disallowed fields before returning: `projCfg.Delegation.Targets = nil; projCfg.Delegation.Timeout = 0`, or
- (b) Never return raw project config. If user config is nil, ignore project delegation entirely (since `allow_project_config` defaults to false, and there's no user config to set it to true).

Option (b) is simpler and more correct: if there's no user config, there's no `allow_project_config: true`, so project delegation should be a no-op regardless. The current code returns the full project config without checking `allow_project_config`.

**Recommended fix:**

```go
if userCfg == nil && projCfg == nil {
    return &Config{}, nil
}
if userCfg == nil {
    // No user config means allow_project_config is false (default).
    // Return empty config; project delegation is not allowed.
    return &Config{}, nil
}
if projCfg == nil {
    return userCfg, nil
}

return merge(userCfg, projCfg), nil
```

### Minor: Targets map not deep-copied in merge

The `merged := *user.Delegation` copies the `Targets` map header (pointer), not the map contents. Since project targets are intentionally ignored, no code modifies the map through `merged.Targets`. But the returned `result.Delegation.Targets` shares the same map as the input `user.Delegation.Targets`. If a caller modifies the returned config's targets, they mutate the user config.

This is low-risk (nothing in the design mutates config after loading), but a defensive deep copy of `Targets` would be consistent with the deep-copy patterns used elsewhere in the codebase (engine's `Snapshot()`, `Machine()`, `deepCopyState()`).

## 3. Security Model

The targets/rules separation makes the security boundary much clearer than R1. With R1's inline commands, the boundary was enforced by merge logic. Now it's enforced by schema structure -- project config physically can't define targets because the merge function ignores them, and this is obvious from the YAML shape.

### The nil-user-config bypass (see section 2)

This is the most significant security issue. The `Load()` function returns raw project config when user config is absent. This lets a project:
1. Define arbitrary targets (binaries)
2. Define rules that use those targets
3. Set arbitrary timeout values

All without the user ever opting in via `allow_project_config: true`.

The fix is straightforward (see section 2 recommendation), and the design's prose correctly describes the intended behavior ("Project-level delegation rules only take effect when the user's config includes `allow_project_config: true`"). The Go code just doesn't implement this for the no-user-config path.

### Project config validation timing

The design says: "Project config also cannot override timeout." But the YAML parser will happily deserialize a project config with `timeout: 999` into the `DelegationConfig` struct. The field is only ignored during merge. If someone adds a code path that reads `projCfg.Delegation.Timeout` before merge (e.g., in a debug command), they'll get the project's value.

**Recommendation:** Consider adding a `validateProjectConfig()` function that strips or warns about disallowed fields immediately after parsing the project config file. This makes the enforcement happen at load time rather than merge time, which is easier to reason about and less prone to bypass if new code paths are added.

### Stderr output in library code

The `merge()` function calls `fmt.Fprintf(os.Stderr, ...)` directly. This is a side effect in what should be a pure function. It makes merge hard to test (tests would need to capture stderr), and it couples the config package to an output mechanism.

**Recommendation:** Return warnings from `merge()` as a data structure (e.g., `[]string`). Let the caller decide how to surface them. This follows the pattern already used by `compile.Compile()`, which returns `[]Warning` alongside the result.

```go
func merge(user, project *Config) (*Config, []string) {
    var warnings []string
    // ...
    warnings = append(warnings, fmt.Sprintf("project config rule for tag %q references unknown target %q", r.Tag, r.Target))
    // ...
    return &result, warnings
}
```

## 4. Config Validation

### What's covered

- Malformed YAML returns an error (not swallowed)
- `loadFile()` distinguishes "file missing" (nil, nil) from "file broken" (nil, error)
- Project rules referencing unknown targets produce warnings
- Project rules for user-covered tags are dropped

### What's missing

**No validation of DelegationConfig internal consistency.** After parsing, there's no check that:
- `Targets` is non-nil when `Rules` is non-empty (rules without targets are useless)
- Each rule has a non-empty `Tag` and `Target`
- No two rules share the same `Tag` (duplicate rules are ambiguous -- only the first match wins, making the second rule dead code)

For user config specifically:
- Every rule's `Target` references a key in `Targets` (typo detection, as noted in section 1)
- Every target's `Command` is non-empty (panic prevention, as noted in section 1)

**Recommendation:** Add a `validate()` step after loading each config file and before merge. Return structured errors. This is more helpful than discovering problems at delegation resolution time (which may be minutes or hours later in a long workflow).

```go
func validate(cfg *Config, source string) error {
    if cfg.Delegation == nil {
        return nil
    }
    d := cfg.Delegation
    for name, t := range d.Targets {
        if len(t.Command) == 0 {
            return fmt.Errorf("%s: target %q has empty command", source, name)
        }
    }
    seen := make(map[string]bool)
    for i, r := range d.Rules {
        if r.Tag == "" {
            return fmt.Errorf("%s: rule %d has empty tag", source, i)
        }
        if r.Target == "" {
            return fmt.Errorf("%s: rule %d has empty target", source, i)
        }
        if seen[r.Tag] {
            return fmt.Errorf("%s: duplicate rule for tag %q", source, r.Tag)
        }
        seen[r.Tag] = true
    }
    return nil
}
```

**Partial configs are fine.** A config with targets but no rules, or delegation with only `timeout`, should be valid. These represent "infrastructure defined but not yet wired" or "config prepared for future use." The zero-value behavior is correct: no rules means no delegation.

### Unknown YAML keys

The design mentions that unknown YAML keys should warn. The implementation approach isn't specified. For the record, the standard Go approach with `gopkg.in/yaml.v3` is:

```go
decoder := yaml.NewDecoder(reader)
decoder.KnownFields(true)
err := decoder.Decode(&cfg)
```

`KnownFields(true)` makes the decoder return an error (not a warning) for unknown fields. If the design wants warnings instead of errors, the two-pass approach (decode into `map[string]interface{}` first, check keys, then decode into typed struct) is needed. The design should specify which behavior is intended.

**Recommendation:** Use `KnownFields(true)` for strict validation. Unknown keys in config files are almost always typos, and a typo in delegation config means no delegation with no indication of why. An error is more helpful than a warning here. Users can fix the typo immediately rather than debugging silent failures later.

This does diverge from the design's "warn" language. If backward compatibility or forward compatibility (future config keys in a new koto version read by old koto) is a concern, then the two-pass warning approach is better. But since koto has no config system today and this is the first version, strict validation is the safer default.

## 5. User Experience

### YAML examples are clear

The config examples are readable and self-explanatory. A user can look at the user config example and understand:
- `targets` defines named binaries
- `rules` maps tags to target names
- `allow_project_config` controls project-level overrides

The project config example correctly shows only `rules`, reinforcing that targets are user-only.

### Gaps in the examples

**No example of what happens with no config.** The design states "no delegation" but doesn't show the `koto next` output. Adding a short example of the directive JSON with and without delegation would help:

Without delegation config:
```json
{"action":"execute","state":"deep-analysis","directive":"Analyze the codebase..."}
```

With delegation config:
```json
{"action":"execute","state":"deep-analysis","directive":"Analyze the codebase...",
 "tags":["deep-reasoning"],
 "delegation":{"target":"gemini","matched_tag":"deep-reasoning","available":true}}
```

**No example of fallback behavior.** When a delegate binary isn't found, the output includes `"fallback": true, "reason": "binary \"gemini\" not found in PATH"`. Showing this in the design would help agent skill authors understand what to expect and handle.

**No example of a config with multiple rules for different targets.** The user config example maps both `deep-reasoning` and `large-context` to `gemini`. A more illustrative example would show different tags routing to different targets:

```yaml
rules:
  - tag: deep-reasoning
    target: gemini
  - tag: large-context
    target: claude
```

This makes it clearer that different tags can route to different tools, which is the whole point of the system.

### The `--prompt -` convention

The `koto delegate run --prompt -` convention (dash means stdin) is standard (curl, docker, cat). But the primary example shows `--prompt /tmp/prompt.txt`. For agent skill authors who'll be writing the SKILL.md integration, the stdin-pipe form is more practical:

```bash
echo "$PROMPT" | koto delegate run --prompt -
```

This avoids temp file creation and cleanup. The design should lead with this form since it's what agents will use most often.

## 6. Consistency

### Go types vs. YAML examples: consistent

The `DelegationConfig` struct fields (`AllowProjectConfig`, `Timeout`, `Targets`, `Rules`) match the YAML keys (`allow_project_config`, `timeout`, `targets`, `rules`). The `yaml` struct tags are correct.

`DelegateTarget.Command` is `[]string` with `yaml:"command"`, and the YAML shows `command: ["gemini", "-p"]`. Consistent.

`DelegationRule` has `Tag` and `Target` with matching YAML tags. The YAML examples use `tag:` and `target:`. Consistent.

### Go types vs. prose: one mismatch

The prose says "Project config can only add routing rules (`tag` -> `target` mapping)." The Go types allow project config to include `targets` and `timeout` fields (the YAML parser will populate them). The enforcement is in `merge()`, not in the types. This is fine for runtime behavior, but a reader comparing the prose to the struct definition might wonder why the struct allows fields the prose says are ignored.

This is a documentation-level concern, not a code bug. The prose could add: "Project config YAML may contain `targets` and `timeout` fields, but they are ignored during merge."

### Go merge code vs. prose: consistent after R1 fixes

The prose says "project delegation rules only take effect when the user's config includes `allow_project_config: true`." The merge code checks `user.Delegation.AllowProjectConfig`. Consistent.

The prose says "project rules for tags already covered by user rules are also dropped." The merge code builds `userTags` and skips matching project rules. Consistent.

### `resolveDelegation()` vs. types: consistent

The function iterates `c.delegationCfg.Rules` (slice), looks up `c.delegationCfg.Targets[rule.Target]` (map), and calls `c.checker.Available(target.Command)`. This matches the type definitions.

### One naming inconsistency

`DelegationRule.Target` (Go field) matches `target` (YAML key), but `DelegationInfo.Target` is the same field name used for the resolved target name in the output. The semantics are the same (the target name), so this is acceptable. But it could cause confusion: "is `DelegationInfo.Target` the target name or the resolved `DelegateTarget` struct?" The JSON output makes it clear (`"target": "gemini"` -- a string, not an object), so this is minor.

### resolveDelegation nested loop ordering

The `resolveDelegation()` function uses a nested loop:

```go
for _, rule := range c.delegationCfg.Rules {
    for _, tag := range tags {
        if tag == rule.Tag {
```

This iterates rules in the outer loop and tags in the inner loop. The prose says "iterate rules in config order, check if the state's tags contain the rule's tag, stop at first match." The code matches this: the outer loop is rule-order, and the inner loop is a membership check.

The implication is that rule priority takes precedence over tag order. If a state has `tags: [large-context, deep-reasoning]` and rules are `[{tag: deep-reasoning, target: gemini}, {tag: large-context, target: claude}]`, then `deep-reasoning -> gemini` wins because it's the first rule. The tag's position in the state's tag list doesn't matter. This is the right behavior (config author controls priority, not template author), and the code implements it correctly.

---

## Summary of Findings

### Must-fix (bugs)

1. **Shared slice backing array in merge.** `merged.Rules = append(merged.Rules, r)` can corrupt the user config's backing array. Fix: deep-copy the rules slice before appending.

2. **Nil user config bypasses security boundary.** When user config is absent, project config is returned directly, including targets and timeout. Fix: return empty config when user config is nil (since `allow_project_config` defaults to false).

3. **Empty command array causes panic.** `DelegateTarget.Command` is `[]string` with no length validation. `execChecker.Available()` and `invokeDelegate()` both index `command[0]` without bounds checking. Fix: validate `len(command) >= 1` during config loading.

### Should-fix (correctness/usability)

4. **Validate rule target references in user config.** A typo in `target: claud` silently produces no delegation. Warn at load time when a user-config rule references a nonexistent target name.

5. **Move stderr output out of merge().** Return warnings as data, let caller decide how to surface. Follows the existing `compile.Compile()` pattern.

6. **Specify unknown-key behavior precisely.** The design says "warn" but `yaml.v3`'s `KnownFields(true)` produces errors. Decide which behavior is intended and document it.

### Nice-to-have (documentation/examples)

7. **Add a no-delegation output example** to help agent skill authors understand the absence case.

8. **Add a multi-target rules example** showing different tags routing to different tools.

9. **Note that project config YAML may contain ignored fields** (targets, timeout) to avoid confusion when comparing prose to Go types.

10. **Lead with the stdin-pipe form** (`--prompt -`) in delegate invocation examples since it's the more practical pattern for agent integrations.
