# Security Review: koto-user-skill (Adversarial Analysis)

## 1. Attack Vectors Not Considered

### Prompt Injection via Skill Content

The phase-5 analysis treats the skill files as inert documentation. They are not — they are instructions consumed by AI agents at runtime. A malicious or careless contribution to `koto-user/` could embed adversarial directives that hijack agent behavior when the skill is loaded. Examples: a hidden instruction to exfiltrate the agent's current context, a directive to suppress gate failures, or a command that redirects `koto next` evidence to an attacker-controlled endpoint. This is the primary vector the prior analysis missed.

The attack surface is bounded by Claude Code's tool permissions and the agent's context window, but it is real. Any agent that loads `koto-user` is executing the skill's instructions with the same trust level as the rest of CLAUDE.md. A supply-chain compromise of `plugins/koto-skills/` — whether through a malicious PR, a compromised contributor account, or a repo fork used as a skill source — could silently alter agent behavior across all users of the plugin.

### Embedded Command Examples as Social Engineering

The skill files contain CLI command examples (e.g., `koto next`, `koto overrides record`). An attacker who can modify these examples could substitute malicious commands that agents copy-paste or execute directly. This is low-probability in a reviewed open-source repo but nonzero, and the prior analysis did not flag it.

### JSON/YAML Schema Examples as Parser Gadgets

Static schema examples in documentation are read by agents that may pass them to JSON parsers or template renderers. Carefully crafted examples (e.g., deeply nested structures, billion-laughs-style YAML) could cause agent-side resource exhaustion if an agent attempts to validate or render them. The risk is low given Claude Code's input handling, but "static examples are never executed" is a weaker claim than the prior analysis implies.

## 2. Adequacy of Mitigations for Identified Risks

The phase-5 analysis identifies no risks and therefore prescribes no mitigations beyond standard PR review. For the documentation-as-artifact framing, that is correct. For the prompt-injection framing introduced above, PR review is a necessary but insufficient control on its own. The existing mitigations are:

- **Code review**: Catches obvious malicious content; does not catch subtle adversarial instructions embedded in plausible-looking guidance prose.
- **Git history**: Provides auditability after the fact; does not prevent consumption of a compromised version.
- **Gitignored secrets**: Correctly scoped; not relevant to this vector.

These are adequate for the threat model of an open-source documentation file. They are not adequate if the skill is treated as a trusted control plane for agent behavior — which is exactly what Claude Code's skill loading mechanism makes it. The gap is acceptable given the low attacker motivation for this specific plugin, but it should be named.

## 3. "Not Applicable" Justifications Review

### External Artifact Handling — Correctly N/A

The design itself does not fetch or execute external content. The N/A holds.

### Permission Scope — Mostly N/A, One Nuance

The `plugin.json` edit adds a new entry to the `skills` array. The prior analysis correctly notes this carries no broader permission surface than existing entries. However, `plugin.json` is a trust anchor: it controls which skills agents load. A modification to this file is meaningfully different from a change to a passive markdown file. The N/A is defensible but understates the sensitivity of `plugin.json` as a file type.

### Supply Chain or Dependency Trust — N/A is Too Narrow

The analysis correctly notes no new package dependencies. It does not consider the skill files themselves as a supply-chain artifact. If `plugins/koto-skills/` is forked or mirrored and used as an alternate skill source, a compromised fork carries the same prompt-injection risk without going through this repo's review process. The N/A is accurate for this repo's own CI/CD chain, but the supply-chain framing should extend to downstream consumers of the plugin.

### Data Exposure — Correctly N/A

No credentials, keys, or host-specific data are in scope. The N/A holds.

## 4. Residual Risk and Escalation

**Escalation not required.** The residual risk is:

- **Prompt injection via skill content**: Low probability (requires PR approval by a maintainer or account compromise), bounded impact (limited to agent behavior within Claude Code's tool permissions). Standard PR review with human sign-off is appropriate. No additional control is warranted at this stage.
- **`plugin.json` as trust anchor**: Low risk for this specific change (one string added to an array). Worth noting as a general practice: changes to `plugin.json` deserve slightly more scrutiny than prose edits, since they affect which skills load.

Neither risk warrants blocking the design or requiring additional security controls. Document both in the PR description so reviewers apply appropriate attention to the skill content and the manifest entry.

## Verdict

The phase-5 "N/A with justification" conclusion is correct for a traditional documentation change. The one gap is that skill files are agent instructions, not passive documentation — prompt injection is a real (if low-probability) vector that the prior analysis did not name. Mitigations are sufficient given the threat model; no escalation is needed.
