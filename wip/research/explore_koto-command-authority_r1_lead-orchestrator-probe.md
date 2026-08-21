# Lead: How does the authority-granting template actually reach a user? (orchestrator probe)

A small factual check run alongside the template-boundary lead, which recommended
deciding whether plugin-level trust belongs in shirabe's release process rather
than in koto. This establishes what that release process does today.

## Findings

### The binary is checksummed. The templates are not.

`shirabe/.github/workflows/release-binaries.yml:93-112` assembles a `dist/`
directory and runs `( cd dist && sha256sum shirabe-* > checksums.txt )`,
publishing `checksums.txt` as a release asset. That covers the compiled
`shirabe` CLI binaries and nothing else.

The skills — and therefore the koto templates at
`skills/execute/koto-templates/execute.md` and
`skills/work-on/koto-templates/work-on.md` — do not travel through that path at
all. They ship as a Claude Code plugin.

### The plugin ships from the repo directory with no integrity metadata

`.claude-plugin/plugin.json` declares `"skills": "./skills/"` and a version
string. `.claude-plugin/marketplace.json` declares one plugin entry with
`"source": "./"`. Neither carries a hash, a manifest of file digests, a
signature, or any per-skill integrity field. Installation is a directory copy
from the repository at whatever revision the marketplace resolves.

### So the asymmetry is exact

The artifact that merely *runs* validation logic — the `shirabe` binary — is
published with SHA-256 checksums a user can verify. The artifact that *grants
command authority* — the template — is published with none. Under the author's
ruling, invoking a koto-backed skill authorizes every command the template bakes
in, which makes the template the higher-value artifact of the two.

### koto already has the missing half

Per the template-boundary lead, koto computes a SHA-256 of the compiled template
and fail-closed re-verifies it on nearly every session-mutating command
(`src/cli/mod.rs:3210-3226`, `4750-4767`; `src/cli/overrides.rs:178-199`). The
hash exists, is already trusted for enforcement, and is already surfaced in the
session record. What is missing is anything that decides *which* hash is
acceptable before `koto init` accepts a path — and, on the shirabe side, anything
that records the expected hash at release time.

## Implications

- A plugin-level trust story does not need new cryptography on either side. It
  needs shirabe's release to record the template hashes it ships, and koto to
  accept an expected hash at init time. Both halves are small; the design
  question is where the expected value is stored and who compares it.
- This makes the template-boundary lead's third recommendation concrete rather
  than open-ended: the release process already produces a checksums file, and
  the gap is that its scope stops at the binary.
- It also bounds the problem usefully. Templates never arrive over a network at
  runtime, so the trust decision happens once, at install or release time, not
  on every `koto next`.

## Surprises

- The release workflow already does exactly the right thing for the wrong
  artifact. Adding the templates to the existing `sha256sum` line is a smaller
  change than anything else this exploration is likely to recommend.

## Open Questions

- Does the Claude Code plugin mechanism expose any integrity or pinning surface
  of its own that a manifest could hook into, or would shirabe be inventing one?
- Should the expected hash live in the plugin manifest, in a separate manifest
  file, or be passed to `koto init` by the skill that owns the template?

## Summary

shirabe's release pipeline publishes SHA-256 checksums for the compiled binary
and nothing for the skills, so the templates — the artifact that actually grants
command authority under the ruling — travel as a plain directory copy with no
integrity metadata at all. koto already computes and fail-closed enforces a
compiled-template hash, so both halves of a pinning story mostly exist and are
simply not connected. The smallest useful version is extending the release's
existing `sha256sum` step to cover templates and giving `koto init` a way to
require an expected hash.
