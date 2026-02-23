# Review: DESIGN-koto-agent-integration.md (Considered Options Section)

Reviewer role: Architect (structural fit).
Source: `docs/designs/DESIGN-koto-agent-integration.md`
CLI validated against: `cmd/koto/main.go`, `pkg/discover/discover.go`, `pkg/engine/engine.go`

---

## 1. Problem Statement Specificity

The problem statement is specific enough to evaluate against, with one gap.

The two stated flows (agent-driven, author-driven) are concrete. The engine behavior is accurately described -- `koto init` compiles and caches templates keyed by SHA-256 of source bytes (confirmed in `cmdInit`, lines 143-186 of `main.go`). The assertion that the author flow needs no new infrastructure is correct: `koto template compile` exists and works as described (`cmdTemplateCompile`, line 507 of `main.go`).

The gap: the problem statement describes the distribution problem in terms of skills getting installed, but does not name the constraint that makes distribution non-trivial. The engine stores absolute template paths in state files and re-verifies the SHA-256 hash on every operation (confirmed in `loadTemplateFromState`, `cmdTransition`, `cmdRewind`, `cmdValidate`). A template that moves or changes content breaks active workflows. This is the real reason the plugin-then-copy model is necessary, but it's buried in Decision Drivers rather than stated upfront as a problem constraint. Evaluating the alternatives without it makes "project-scoped only" look weaker than it is, and a reader won't understand why the chosen option requires the extra copy step.

Advisory. The design proceeds without fixing this, but readers miss the structural reason for the architecture.

---

## 2. Missing Alternatives

Two alternatives are missing that would be genuinely viable.

**Missing: koto-skills as a plain git repository (no plugin layer).** The design considers "project-scoped only (no plugin)" and rejects it because users must find and copy files manually. A third option exists: publish `koto-skills` as a git repository that users add as a git submodule or shallow-clone into `.claude/skills/`. This is how many Claude Code skill sets are distributed without the plugin machinery. The trade-off is manual update friction vs. plugin system dependency. The design's own Uncertainties section acknowledges the plugin system is new (v1.0.33+, late 2025) -- if that immaturity is a real risk, the git-repository path is the obvious lower-dependency fallback. Its rejection rationale should be explicit, even if the conclusion is "submodule setup is too much friction."

**Missing: built-in starter template in the koto binary.** The design rules out "Template search paths or built-in embedding" as out of scope (line 74) but doesn't present it as a considered-and-rejected alternative with reasoning. A `koto init --template quick-task` that expands a built-in starter would eliminate the reference distribution problem entirely. The rejection is defensible -- keeps the binary minimal, decouples template evolution from koto releases -- but without stating it as a rejected option, a reader doesn't know if it was considered and rejected or simply not considered.

---

## 3. Rejection Rationales

The two documented alternatives have rationales of different quality.

**"Project-scoped only (no plugin)" rejection** is concrete and accurate. Users must find files, copy them, and manually track updates. Fine.

**"`koto generate` as the primary distribution mechanism" rejection** has a circular element. It says `koto generate` "solves the wrong problem" because "the hard part isn't generating the SKILL.md -- it's getting the skill to the user." This is true for reference skill distribution. But `koto generate` was also a candidate for reducing the manual authoring burden for custom skills. The rejection conflates two different problems: (a) reference skill distribution, correctly rejected, and (b) custom skill scaffolding, deferred without explaining why the manual path is acceptable now. The Consequences section then lists "Manual SKILL.md authoring for custom templates requires reading the template to extract evidence keys" as a cost -- a cost that wasn't weighed in the Considered Options section because the rejection addressed problem (a), not problem (b).

---

## 4. Uncertainties

The Uncertainties section is honest but incomplete on two points.

**Documented uncertainties** (plugin system maturity, skill directory path resolution, cross-platform plugin equivalents) are real and accurately described.

**Missing: Stop hook fails silently if koto is not on PATH in the hook environment.** The hook is:

```bash
koto workflows 2>/dev/null | grep -q '"path"' && echo 'Active koto workflow detected...'
```

Claude Code hooks may execute in a restricted shell environment where PATH differs from the user's interactive shell. If `koto` isn't found, `2>/dev/null` suppresses the error and the hook silently does nothing. This is a more likely failure than the "malicious koto binary" documented in Security, and should be named as an uncertainty.

**Missing: Stop hook fails silently with non-default `--state-dir`.** `koto workflows` without `--state-dir` defaults to scanning `"wip/"` (confirmed, `cmdWorkflows` line 481 of `main.go`). If a project initialized workflows with a custom `--state-dir`, the hook scans the wrong directory and finds nothing. The hook only works correctly when workflows use the default state directory. This is an undocumented constraint that affects any user who customizes the state directory.

**Claimable as correct: Stop hook grep target.** The hook greps for `"path"` in the JSON output. `koto workflows` outputs a JSON array of `discover.Workflow` structs (confirmed in `pkg/discover/discover.go` lines 18-24 and `cmdWorkflows` lines 475-492). The `Workflow` struct has `json:"path"` on its `Path` field. A non-empty array always contains the string `"path"`. The grep is correct.

---

## 5. Does the Chosen Option Solve the Stated Problem?

Yes, with one structural gap.

The agent-driven flow works: plugin installs skills, skill instructions tell the agent to copy the template to a project-local path, `koto init --template <path>` processes it. The template locality problem (absolute paths, hash verification) is addressed by the copy step. The author-driven flow requires no changes.

The structural gap: the design creates `koto-skills` as a new artifact the koto project must maintain, but provides no guidance on how SKILL.md content stays synchronized with koto CLI changes. The SKILL.md documents evidence keys, execution loops, and JSON response shapes. If koto CLI behavior changes -- for example, if `koto next` output shape changes, or `koto transition` grows an `--evidence` flag -- the SKILL.md silently becomes stale. The design frames decoupling skill updates from koto releases as a positive (reference skills can evolve independently), but doesn't establish what connects them when koto changes behavior the skill depends on.

This is advisory: the design is correct as described, the maintenance coupling is just undocumented.

---

## Summary

| Finding | Severity | Location in design |
|---------|----------|--------------------|
| Absolute-path / hash constraint missing from problem statement | Advisory | Problem Statement |
| git-repository distribution alternative not considered | Advisory | Considered Options |
| Built-in template embedding not considered or explicitly rejected | Advisory | Considered Options |
| `koto generate` rejection conflates reference distribution and custom scaffolding | Advisory | Considered Options |
| Stop hook fails silently if koto not on PATH in hook env | Advisory | Uncertainties (missing) |
| Stop hook fails silently with non-default `--state-dir` | Advisory | Uncertainties (missing) |
| SKILL.md / koto CLI change synchronization not addressed | Advisory | Consequences |

No blocking issues. The chosen option fits the architecture: no koto code changes, no new CLI surface, no parallel patterns introduced. All findings are advisory -- they affect the written design's completeness, not the design itself.
