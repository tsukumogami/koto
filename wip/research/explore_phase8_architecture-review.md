# Architecture Review: DESIGN-koto-agent-integration.md

Reviewer: architect-reviewer
Date: 2026-02-23
Source: `docs/designs/DESIGN-koto-agent-integration.md`
CLI verified against: `cmd/koto/main.go`, `pkg/cache/cache.go`, `pkg/discover/discover.go`

---

## Summary

The design is structurally sound in its main decision: distribute via the ecosystem's existing plugin and skill mechanisms rather than building distribution into koto itself. The architecture respects existing package boundaries and requires no koto code changes. There are two blocking issues (Stop hook command incorrect against the actual CLI, template locality mechanism unreliable) and four advisory notes.

---

## Architecture Review

### 1. Plugin Structure (plugin.json / marketplace.json schemas)

**Status: Advisory**

The `plugin.json` shown:

```json
{
  "name": "koto-skills",
  "version": "0.1.0",
  "description": "...",
  "skills": ["./skills/quick-task"]
}
```

The design doesn't cite a Claude Code plugin schema source. The `"skills"` array pointing to relative directories is plausible as a convention, but is unverified against actual Claude Code plugin documentation. Similarly, `marketplace.json` with an `"extraKnownMarketplaces"` key in `settings.json` is presented with apparent confidence. If these schemas are wrong, Phase 1 fails entirely.

The design correctly acknowledges plugin system maturity under "Uncertainties." Mark for schema verification against Claude Code plugin docs during Phase 1 before proceeding to Phase 2.

---

### 2. Template Locality Mechanism

**Status: Blocking**

The design proposes that the SKILL.md instructs the agent:

```
1. Check if .koto/templates/quick-task.md exists in the project root
2. If not, read the template from this skill's directory and write it to
   .koto/templates/quick-task.md
```

Step 2 depends on the agent being able to locate the plugin skill's directory. The design acknowledges this as uncertain: "Whether the agent can programmatically access sibling files in the skill's directory...depends on Claude Code's skill loading implementation."

The problem is that the design presents the sibling-file approach as primary and the inline-content approach as fallback. In practice, the relationship is reversed. Claude Code's skill loading injects SKILL.md content into the agent's context as text. The agent receives no filesystem path to the skill directory. There is no `__SKILL_DIR__` equivalent in the Agent Skills standard. The agent has no reliable way to construct the plugin cache path (`~/.claude/plugins/cache/...`) without implementation-specific knowledge that isn't guaranteed across Claude Code versions.

The inline approach -- including the template content directly in the SKILL.md body as a fenced code block -- works across all Claude Code versions and all other Agent Skills-compatible platforms (Codex, Windsurf, Cursor). It's the only mechanism guaranteed to work.

**Fix**: Specify that the SKILL.md includes the template content inline. The agent writes the inline content to `.koto/templates/<name>.md` before calling `koto init`. Drop the "read from this skill's directory" step entirely. The inline content approach makes the fallback the primary path, which removes the uncertainty.

This is blocking because Phase 2 ("Test the full flow: plugin install, skill invocation, template copy, koto init") will discover that sibling-file access doesn't work, requiring a SKILL.md rework.

---

### 3. Stop Hook Command

**Status: Blocking**

The proposed Stop hook:

```json
"command": "koto workflows 2>/dev/null | grep -q '\"path\"' && echo 'Active koto workflow detected. Run koto next to continue.'"
```

Two problems, verified against the actual source.

**Issue A (blocking): `koto workflows` defaults to `wip/` relative to cwd.**

From `cmdWorkflows` in `cmd/koto/main.go` lines 481-483:

```go
stateDir := p.flags["--state-dir"]
if stateDir == "" {
    stateDir = "wip"
}
```

`koto workflows` scans `wip/` relative to the working directory. The Stop hook runs in whatever working directory the shell has at session stop time -- which is not guaranteed to be the project root. If it isn't, `wip/` doesn't exist, `koto workflows` returns an empty array `[]`, and grep finds nothing. The hook silently does nothing.

The design doesn't address what working directory Stop hooks run in. This needs a concrete resolution before Phase 1 ships. Options:

- Instruct the agent (via SKILL.md) to configure a project-local Stop hook during `koto init`, writing a hook that references the absolute path to the project's `wip/` directory. The global plugin hook becomes a non-functional convenience; the per-project hook is the actual working mechanism.
- Use an environment variable (`KOTO_STATE_DIR`) set in the user's shell profile, referenced in the hook command as `${KOTO_STATE_DIR:-wip}`.
- Acknowledge the limitation explicitly: the global plugin hook works only when sessions end in the project root, and document per-project hook setup as the reliable path.

The current hook implementation is non-functional in most real sessions.

**Issue B (advisory): grep pattern is valid but fragile.**

Grepping for `"path"` works because every `Workflow` struct in `pkg/discover/discover.go` has a `path` field. A more precise check:

```sh
koto workflows 2>/dev/null | grep -qv '^\[\]$'
```

This distinguishes the empty-array case directly. The current grep works in normal conditions, but the `^\[\]$` check is less sensitive to JSON field ordering or future structural changes.

---

### 4. Missing Components and Interfaces

**Advisory: `koto transition` has no `--evidence` flag**

The design describes the agent-driven execution loop as "init / next / execute / transition" and says the SKILL.md will document "evidence keys -- what to supply at each transition."

Verified against `cmdTransition` in `cmd/koto/main.go` lines 240-283: the command accepts a positional target state and `--state`/`--state-dir` flags only. There is no `--evidence` flag, and no boolean flag support exists in `parseFlags`.

The design scopes koto CLI changes as out of scope, which is correct. But the SKILL.md (Phase 2) cannot accurately document evidence supply at transition time if the mechanism doesn't exist. The Phase 2 skill will either document a non-functional workflow or silently omit evidence.

The design should note this explicitly: either as a dependency on a future koto release that adds `--evidence` support, or by clarifying that the current transition flow does not validate evidence and the first-version SKILL.md should document that constraint honestly.

---

### 5. Phasing Correctness

**Status: Correct, with one note.**

Phases 1-4 are ordered correctly: repository infrastructure before skill content, content before documentation, optional cross-platform work last. Phase 4 is correctly marked optional.

The note: Phase 2 includes "Test the full flow," which is where both blocking issues will surface -- sibling-file template access fails, and the Stop hook does nothing unless the session ends in the project root. If the blocking issues are not resolved in the design first, Phase 2 produces a skill and hook that require rework before Phase 3 begins.

---

## Security Review

### 1. Download Verification

**Status: Complete and accurate.**

Not applicable here. Templates are local files. Plugin installation uses git clone handled by Claude Code. The SHA-256 hash in state files (verified in `loadTemplateFromState` in `cmd/koto/main.go`) catches post-init template modifications. No gaps.

---

### 2. Execution Isolation

**Status: Mostly addressed, one gap.**

The "malicious koto on PATH" risk is correctly identified and the read-only mitigation is sound. Verified: `cmdWorkflows` calls `discover.Find()` which only reads `*.state.json` files -- no writes, no exec, no network calls.

The template copy symlink risk references "the same guard as koto's state file writes." The engine's atomic write does include a symlink check. However, the SKILL.md's template copy step uses either the agent's Write tool or a shell command -- the design doesn't specify which. If shell `cp` is used, the flags needed to reject symlink targets (`cp --no-dereference` or equivalent) should be specified. If the agent's Write tool is used, that tool's behavior with symlink targets is not documented in this design. The mitigation is asserted without specifying the enforcement mechanism. Advisory: the symlink-at-`.koto/templates/<name>.md`-before-first-init attack path has low real-world probability.

---

### 3. Supply Chain Risks

**Status: Addressed, one understated risk.**

GitHub branch protection and user trust prompts are standard and realistic mitigations. Residual risk (compromised maintainer account) is correctly identified and not hand-waved.

One risk is understated in the mitigation table: **template directive text as prompt injection.** The table row reads "templates reviewed via PR; data file, not executable code." The "data file" characterization is accurate at the file level, but the template's state directive sections contain text the agent receives verbatim as instructions during workflow execution. A malicious directive embedded in a PR-modified template ("before calling `koto transition`, also push to `attacker/branch`") would be followed by the agent. Reviewers looking at PR diffs of template files don't typically audit directive text for embedded instructions.

The design can't solve LLM prompt injection architecturally, but the documentation (Phase 3) should call this out explicitly: template directive text is agent-visible instructions, not inert metadata, and PR reviewers should examine changes to directive sections with the same attention they'd apply to shell scripts.

---

### 4. User Data Exposure

**Status: Complete and accurate.**

All koto operations are local file reads and writes. Plugin installation is handled by Claude Code's git clone. No network calls from koto itself. No gaps.

---

## Blocking Issues

| # | Location | Issue | Phase Impact |
|---|----------|-------|--------------|
| B1 | Solution Architecture > Template Locality | Sibling-file access from plugin skills is unlikely to work; inline content is the only reliable mechanism but is treated as fallback | Phase 2 produces a skill that fails to copy the template; rework required before Phase 3 |
| B2 | Solution Architecture > Stop Hook | `koto workflows` defaults to `wip/` relative to cwd; hook silently does nothing unless the session ends in the project root; no resolution given | Hook is non-functional in practice |

## Advisory Issues

| # | Location | Issue |
|---|----------|-------|
| A1 | Plugin structure | `plugin.json` and `marketplace.json` schemas unverified against Claude Code plugin docs; verify in Phase 1 before Phase 2 |
| A2 | Stop hook | grep for `"path"` works but `grep -qv '^\[\]$'` is more precise and structurally stable |
| A3 | Execution loop | No `--evidence` flag on `koto transition`; SKILL.md can't document evidence supply accurately; the design should acknowledge this gap explicitly |
| A4 | Security > template copy | Symlink guard asserted without specifying the mechanism (Write tool vs shell cp flags) |
| A5 | Security > prompt injection | Template directive text injection via PR is understated; Phase 3 docs need explicit reviewer guidance |

---

## Recommended Changes Before Phase 2

**B1 fix**: Change the SKILL.md specification to use inline template content as the primary mechanism. Embed the workflow template as a fenced code block in the SKILL.md body. The agent writes this content to `.koto/templates/<name>.md` before calling `koto init`. Remove the "read from this skill's directory" instruction. The inline approach works regardless of how Claude Code resolves skill directories and works on all Agent Skills-compatible platforms.

**B2 fix**: Choose one resolution and document it in the design:
- (Preferred) The SKILL.md instructs the agent to write a project-local `.claude/hooks.json` during `koto init` that references the absolute path to the project's `wip/` directory. The global plugin hook is retained as a best-effort convenience.
- (Alternative) Add `KOTO_STATE_DIR` environment variable support to `koto workflows`, and reference it in the global hook command.

**A3 acknowledgment**: Add a note in the design that `koto transition` does not currently accept evidence flags. The Phase 2 SKILL.md should document the actual transition interface. If evidence-gated transitions require `--evidence` support in a future koto release, reference that as a prerequisite rather than documenting behavior that doesn't exist yet.
