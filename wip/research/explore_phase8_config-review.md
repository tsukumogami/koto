# Config System Review: DESIGN-cross-agent-delegation

Reviewer perspective: developer tools configuration specialist.

Reviewed file: `docs/designs/DESIGN-cross-agent-delegation.md` (Decision 4: Config System Architecture and related sections).

Existing codebase context: `pkg/cache/cache.go` already uses `~/.koto/` (via `KOTO_HOME` override) for the compilation cache. No config loading exists today. The CLI (`cmd/koto/main.go`) has zero config infrastructure -- all behavior comes from flags and state files.

---

## 1. Config File Locations

**Proposed:** `~/.koto/config.yaml` (user) and `.koto/config.yaml` (project).

**Assessment: Conventional and correct.**

This follows the dominant pattern for developer tools:

| Tool | User Config | Project Config |
|------|-------------|----------------|
| git | `~/.gitconfig` | `.git/config` |
| npm | `~/.npmrc` | `.npmrc` |
| Docker (Compose) | `~/.docker/config.json` | `docker-compose.yaml` |
| kubectl | `~/.kube/config` | n/a (env var) |
| Terraform | `~/.terraformrc` | `.terraform/` |
| ESLint | n/a | `.eslintrc.*` |
| Prettier | n/a | `.prettierrc` |
| Cargo | `~/.cargo/config.toml` | `.cargo/config.toml` |
| pip | `~/.config/pip/pip.conf` | `pyproject.toml` |

The `~/.koto/` directory is already established by the cache package (`~/.koto/cache/`), so putting config in `~/.koto/config.yaml` is natural. It avoids dot-file proliferation in `$HOME`.

The `.koto/config.yaml` project directory follows the git/cargo convention of a hidden directory, which is the right call for a tool config that most project contributors won't need to touch.

**One concern:** The XDG Base Directory Specification (`$XDG_CONFIG_HOME/koto/`) is the Linux-standard location for user config. Tools like `gh` (GitHub CLI) use `~/.config/gh/`. koto already diverges from XDG by using `~/.koto/` for cache. The design should acknowledge this and explain the choice. Since `KOTO_HOME` already exists as an override, this is defensible -- but worth documenting that `KOTO_HOME` serves as the escape hatch for users who want XDG compliance.

**Recommendation:** Add a note that `KOTO_HOME` also applies to config resolution (i.e., `$KOTO_HOME/config.yaml`). The cache package already does this. The config package should follow the same pattern. This is implied but never stated explicitly in the design.

## 2. Merge Semantics

**Proposed:** Project rules append after user rules. Project `default` overrides user `default`. Project `command` is ignored (only user config defines binaries).

**Assessment: The append model is unusual and potentially confusing. The command restriction is sound but the overall merge needs clearer documentation.**

### Comparison with other tools

Most tools use one of two merge strategies:

**Replace (last wins):** npm, Terraform. Project config completely replaces user config for the same key. Simple to understand.

**Deep merge with precedence:** Docker Compose, kubectl contexts. Fields are merged at the leaf level, with closer-to-project taking precedence.

koto proposes a third model: **append + selective override**. User rules come first, project rules are appended after (extending the list), but some fields (`default`) are overridden while others (`command`) are ignored at the project level. This is a hybrid that doesn't match any well-known pattern.

### Specific concerns

**Ordering matters but isn't obvious.** The design says "first match wins" for tag resolution. User rules come first in the merged list. This means a user rule for `deep-reasoning -> gemini` will always win over a project rule for `deep-reasoning -> claude`, because user rules are earlier in the list. This is the right behavior (user preference takes priority), but the mechanism (list ordering + first-match) is indirect. It would be clearer if the merge explicitly dropped project rules for tags already covered by user rules.

**The "silently drop unknown targets" behavior is a debugging hazard.** When a project config references `delegate_to: specialized-tool` and the user has no rule defining a command for `specialized-tool`, the project rule is silently dropped. The template author expects delegation; the user gets fallback with no indication of why. The design should require a warning to stderr when project rules are dropped due to missing user-defined targets.

**`default` override semantics are underspecified.** The design says project `default` overrides user `default`, but doesn't explain what `default` does. The struct shows `Default string` with a comment saying `"self" or empty`. When is `default` consulted? Presumably when a state has tags but no rule matches. But the `resolveDelegation()` code doesn't reference `Default` at all -- it returns nil when no rule matches. The `default` field appears vestigial in the current implementation.

### Recommendation

Specify that during merge, project rules for tags already defined in user rules are dropped (not just ordered later). This makes the "user wins" behavior explicit rather than dependent on list ordering. Also add stderr warnings for dropped project rules.

## 3. Trust Model

**Proposed:** Global `allow_project_config: true` opt-in. No per-project granularity.

**Assessment: The global opt-in is the right starting point, but the design should acknowledge the per-project gap and plan for it.**

### Comparison with other tools

| Tool | Project Trust Model |
|------|-------------------|
| git | `safe.directory` -- per-directory allowlist |
| npm | No project trust concept (`.npmrc` is always read) |
| Docker | N/A (Compose files always apply) |
| VS Code | Per-workspace trust prompt on first open |
| Cargo | `.cargo/config.toml` is always read (no trust gate) |
| direnv | Per-directory `.envrc` requires explicit `direnv allow` per directory |

The closest precedent is `direnv`, which requires `direnv allow /path/to/project` before `.envrc` files take effect. This is per-directory. Git's `safe.directory` is also per-path.

**Why per-project matters:** The global opt-in means trusting ALL project configs everywhere. A user who wants project config for their work repos also trusts every random clone. The threat isn't just arbitrary code execution (mitigated by the command restriction) -- it's unexpected routing. A malicious `.koto/config.yaml` could map `security` to `gemini` when the user prefers keeping security analysis on a specific provider for data residency reasons.

**However:** For v1, the global flag is defensible. The `command` restriction means the worst case is unexpected tag-to-target routing, not arbitrary execution. The impact is bounded to routing decisions among binaries the user has already defined.

### Recommendation

Keep the global flag for v1, but add a comment in the design noting that per-project trust (an allowlist of project paths, like `direnv allow`) is a natural evolution if users report friction. The config struct could evolve to:

```yaml
delegation:
  allow_project_config: true  # v1: global
  # Future: trusted_projects: ["/path/to/repo1", "/path/to/repo2"]
```

## 4. Config Schema

**Proposed:** YAML with `delegation.rules[]` containing `tag`, `delegate_to`, `command`.

**Assessment: The shape is reasonable, but the naming and nesting could be more conventional.**

### Naming comparisons

| koto | Docker Compose | kubectl | Terraform |
|------|---------------|---------|-----------|
| `delegate_to` | `depends_on` | `context` | `provider` |
| `command` | `command` | `exec.command` | n/a |
| `rules` | `services` | `clusters` | `required_providers` |

The `delegate_to` field name is clear but unusual. Most tools would use `target` (which the `DelegationInfo` struct already calls it) or `provider`. Using different names for the same concept in config (`delegate_to`) vs. output (`target`) is a minor inconsistency.

**The `command` as a string array is correct.** This follows Docker's convention (`["executable", "arg1"]`) and avoids shell parsing. Good call.

**The `rules` as an ordered list is the right structure** for first-match semantics. This matches firewall rules, nginx location blocks, and route tables -- all use ordered lists with first-match.

### Structural concern: command lives inside rules

The design puts `command` on each rule:

```yaml
rules:
  - tag: deep-reasoning
    delegate_to: gemini
    command: ["gemini", "-p"]
  - tag: large-context
    delegate_to: gemini
    command: ["gemini", "-p"]  # duplicated!
```

When two rules map to the same delegate, the command is duplicated. The merge logic already builds a `userCommands` map keyed by `delegate_to`, which implies the natural structure is to separate target definitions from routing rules:

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

This eliminates duplication, makes the security boundary clearer (targets define binaries, rules define routing), and maps directly to the merge logic (project config can add rules but not targets).

### Recommendation

Consider separating targets from rules. This is a structural improvement that makes the security model self-documenting: "project config can define rules but not targets" becomes obvious from the schema shape rather than needing a prose explanation.

## 5. Missing Config Features

### `koto config` subcommands

**Should exist: yes.** Three subcommands are worth having:

1. **`koto config show`** -- Display the effective merged config (user + project, after merge rules). This is the single most useful config debugging tool. `git config --list --show-origin` and `npm config list` serve this purpose. Without it, users will struggle to understand why delegation routes a certain way.

2. **`koto config validate`** -- Check that config YAML is syntactically valid and that `command` binaries exist. `docker compose config` does this. Useful for CI checks on project configs.

3. **`koto config init`** -- Generate a starter config with comments. `git config --global --edit` opens an editor; a simpler approach is writing a commented template to `~/.koto/config.yaml` if it doesn't exist. Lower priority than `show` and `validate`.

The design should at least mention `koto config show` as a planned subcommand. It's nearly essential for the merge model to be usable.

### Environment variable overrides

**Should exist for key settings, but not for rules.**

The cache package already uses `KOTO_HOME`. Config should support:

- `KOTO_HOME` -- Already exists; config loading should respect it (the design implies this but doesn't state it).
- `KOTO_DELEGATION_TIMEOUT` -- Override timeout without editing config. Useful for CI.
- `KOTO_NO_PROJECT_CONFIG` -- Disable project config for a single invocation. Safety escape hatch.

Full rules in env vars (`KOTO_DELEGATE_deep_reasoning=gemini`) were correctly rejected in the design. The structured list doesn't fit env var ergonomics.

### Schema validation

The design doesn't mention validating the YAML config against a schema. `gopkg.in/yaml.v3` (already a dependency) will silently ignore unknown keys by default. If a user writes `delgation:` (typo), they get zero delegation with no error.

**Recommendation:** After unmarshaling, check for unexpected top-level keys. This can be done with a two-pass unmarshal: first into `map[string]interface{}` to get all keys, then into the typed struct. Unknown keys produce a warning. This is what Docker Compose does (warns on unknown service keys).

## 6. Security Model

**Proposed:** Project config cannot set `command`. Only user config defines what binary each target maps to. Project config can only map tags to target names that the user has already defined.

**Assessment: The security boundary is well-placed. There are two edge cases worth addressing.**

### The boundary is correct

The design correctly identifies that the critical security boundary is "what binary gets executed." By restricting `command` to user config, a malicious project can't introduce arbitrary binaries. The worst a project config can do is route tags to different targets -- but those targets all resolve to user-defined binaries.

This is similar to git's approach: `.gitattributes` can set merge drivers and diff filters, but the actual binaries are defined in `~/.gitconfig`. The project says "use driver X," the user defines what driver X means.

### Edge case 1: Target name collision

A project config could define:

```yaml
rules:
  - tag: security
    delegate_to: gemini
```

The user has `gemini` mapped to `["gemini", "-p"]`. The project author intended a different `gemini` configuration (maybe with different flags). Since the target name is just a string, there's no namespacing.

**Impact:** Low. The user's binary definition is what runs. The project can't influence flags or binary path. The mismatch is in intent, not in execution safety.

**Mitigation:** None needed for v1. If this becomes a problem, target names could gain a namespace prefix, but that's over-engineering for now.

### Edge case 2: Delegation to unintended providers for data sensitivity

A user defines targets for both `gemini` (Google) and `claude` (Anthropic). They expect `security`-tagged steps to go to `claude` for data residency reasons. A project config maps `security -> gemini`. With `allow_project_config: true`, the project's rule is appended after the user's rules. If the user has no `security` rule, the project's mapping wins.

**Impact:** Medium. Sensitive code sent to an unintended provider violates the user's data handling preferences.

**Mitigation:** This is already covered by the merge semantics (user rules are checked first). But it only works if the user defines rules for ALL tags they care about, even if they wouldn't otherwise need a rule. The design should note this: "If you have data residency constraints, define rules for all sensitive tags in your user config to prevent project-level overrides."

### Edge case 3: Timeout as denial-of-service vector

The project config can presumably override `timeout`. A project could set `timeout: 86400` (24 hours). The design shows `timeout` at the `DelegationConfig` level but doesn't specify whether project config can override it.

**Recommendation:** Timeout should follow the same restriction as `command` -- only user config sets it. Project config timeout overrides should be ignored, or at most capped at the user's timeout value.

## 7. Future Extensibility

**Proposed:** `Config` struct with a `Delegation` section. The design states the config system is built as general infrastructure.

**Assessment: Good foundation, but one structural concern.**

### What works well

The `Config` struct with optional sections (`Delegation *DelegationConfig`) is the right pattern. Future features add their own section:

```go
type Config struct {
    Delegation *DelegationConfig `yaml:"delegation,omitempty"`
    Logging    *LoggingConfig    `yaml:"logging,omitempty"`     // future
    Cache      *CacheConfig      `yaml:"cache,omitempty"`       // future
    Registry   *RegistryConfig   `yaml:"registry,omitempty"`    // future
}
```

Each section has its own merge semantics. The `Load()` function handles all of them. This is the approach used by Docker (`daemon.json`), kubectl (`~/.kube/config`), and Terraform (`.terraformrc`).

### Structural concern: merge function coupling

The current `merge()` function has delegation-specific logic baked in. When a second config section is added, the merge function will need another block of section-specific code. This is fine for two or three sections, but the design should note that each new section requires merge logic.

A more extensible approach (for later, not v1) would be to define a merge interface:

```go
type Mergeable interface {
    Merge(project interface{}) interface{}
}
```

Each config section implements its own merge. But this is premature abstraction for v1 with one section.

### What's missing: config version

The config file has no version field. If the config schema changes (new required fields, changed semantics), there's no way to detect old-format configs. This is fine for v1 but should be added before v2 if the config format evolves.

**Recommendation:** Reserve a `version: 1` field at the top level of the config YAML. Don't require it yet (missing = 1), but document it as a future mechanism for schema evolution.

---

## Summary of Recommendations

### Must-address before implementation

1. **State that `KOTO_HOME` applies to config resolution**, not just cache. The config package should use the same resolution logic as `cache.cacheDir()`.
2. **Add stderr warnings when project rules are dropped** due to missing user-defined targets. Silent drops are a debugging nightmare.
3. **Clarify `default` field semantics.** The implementation code doesn't use it. Either remove it from the v1 schema or specify its behavior in the resolution flow.
4. **Specify that project config cannot override `timeout`.** The current design is ambiguous about which fields project config can set.

### Should-address (improve usability)

5. **Consider separating targets from rules** in the config schema. This eliminates duplication and makes the security model self-documenting.
6. **Plan a `koto config show` subcommand** for displaying effective merged config. Mention it in the implementation approach, even if it ships in a later phase.
7. **Warn on unknown YAML keys** during config loading. A typo in a key name silently produces a zero-value config with no feedback.

### Nice-to-have (document for future)

8. **Note the XDG divergence** and confirm that `KOTO_HOME` is the escape hatch.
9. **Reserve `version: 1`** in the config schema for future evolution.
10. **Note that per-project trust** (directory allowlist) is a natural evolution of the global flag.
11. **Deduplicate project rules for tags already defined** in user config during merge, rather than relying on list ordering + first-match.
