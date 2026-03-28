# Security Review: DESIGN-config-and-cloud-sync

**Reviewer role**: Security
**Design**: `docs/designs/DESIGN-config-and-cloud-sync.md`
**Outcome**: OPTION 2 -- Document considerations (security section exists, gaps identified below)

---

## 1. Credential storage and leakage

**What the design does well.** The credential blocklist preventing `session.cloud.access_key` / `session.cloud.secret_key` from being written to project config is the right structural control. Write-time enforcement is stronger than read-time filtering because it prevents the data from reaching disk in the wrong location.

**Gap: blocklist is prefix-based but the prefix list is narrow.** The design mentions a "static array of key prefixes" but only names the two credential keys explicitly. If a future key like `session.cloud.session_token` (STS temporary credentials) is added, the blocklist must be updated manually. Consider blocklisting the entire `session.cloud` subtree for project config and explicitly allowlisting the non-secret keys (`endpoint`, `bucket`, `region`). An allowlist for project config is safer than a denylist for credentials.

**Gap: `koto config list` output.** The `list` subcommand "dumps resolved config as TOML" including `--json`. If credentials are resolved (from user config or env vars), does `list` output them? A terminal session can be recorded, shared, or logged. The design should specify whether `list` redacts credential values (e.g., showing `session.cloud.access_key = <set>` instead of the raw value). This applies to both TOML and JSON output modes.

**Gap: config file permissions on creation.** The design says `~/.koto/config.toml` inherits 0700 from `~/.koto/`. But `ensure_koto_root()` in `local.rs` only runs on `SessionBackend::create()`. If a user runs `koto config set` before ever creating a session, the `~/.koto/` directory may be created by the config module without the 0700 permission enforcement. The config module must independently ensure 0700 on `~/.koto/` when writing `config.toml`, not rely on LocalBackend having run first.

## 2. S3 transport security

**What the design does well.** HTTPS by default via rust-s3 for AWS endpoints is correct.

**Gap: custom endpoint HTTPS enforcement.** The design says "users may need to configure trust" for custom endpoints but doesn't state whether plaintext HTTP endpoints are allowed. If a user sets `session.cloud.endpoint = http://minio.local:9000`, does koto connect over HTTP? The design should take a position: either reject non-HTTPS custom endpoints by default (with an explicit opt-out like `session.cloud.allow_insecure = true`), or document that custom endpoints allow HTTP. Without this, the default posture is ambiguous.

**Observation (low severity).** The 5-second manifest TTL cache means a stale manifest could cause a re-upload of already-synced content. This is a consistency concern, not a security one, but worth noting that the cache doesn't create a window where stale security-relevant data (like a revoked session) persists.

## 3. Config file permissions

**What the design does well.** The 0700 permission model on `~/.koto/` and the split between user config (local-only, restricted) and project config (git-committed, no secrets) is sound.

**Gap: project config `.koto/config.toml` permissions.** The design says `koto config set --project` creates `.koto/` on first use. What permissions does this directory get? Since it's committed to git, the permissions are determined by the repo's umask. This is fine for non-secret data, but the design should confirm that the credential blocklist runs *before* any file I/O, so a race or early-exit bug can't write a credential to the project config file.

## 4. Bucket access model

**What the design does well.** The design explicitly acknowledges the shared-bucket model and correctly frames it as mirroring the local `~/.koto/` model.

**Consideration: cross-project access.** A single set of S3 credentials grants access to all repo-id prefixes in the bucket. This means any koto-enabled project on a developer's machine can read/write any other project's sessions. For most use cases this is acceptable (same trust boundary as the local filesystem). However, the design should note that in CI/CD environments, a shared bucket with shared credentials means any CI job can access any project's workflow state. If this is the intended model, document it explicitly. If not, consider whether per-project credential scoping (via IAM policy on the prefix) should be a documented recommendation.

## 5. Version counter

**What the design does well.** The design correctly states the version counter is a consistency mechanism, not a security boundary. This is the right framing.

**Consideration: version.json is writable by anyone with bucket access.** An actor who can PUT to the bucket can set `version` to `u64::MAX`, effectively preventing further increments (overflow) or forcing perpetual conflict states. This is a DoS vector, not a confidentiality issue. Since the threat model already assumes bucket access equals full trust, this is consistent. No design change needed, but the security section could note the DoS vector for completeness.

## 6. Feature flag gating

**What the design does well.** The `#[cfg(feature = "cloud")]` gating on `CloudBackend` and the `#[cfg(not(feature = "cloud"))]` error arm in `build_backend()` are the correct pattern. Cloud code literally doesn't compile into the default binary.

**No gaps identified.** The compile-time gating is the strongest form of feature isolation. There's no runtime flag that could be bypassed.

---

## Summary of recommendations

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1 | Credential blocklist should be an allowlist for project config | Medium | Update design: allowlist non-secret keys for project config |
| 2 | `koto config list` may leak credentials to terminal | Medium | Specify redaction behavior in design |
| 3 | Config module must enforce 0700 on `~/.koto/` independently | Medium | Add to implementation notes |
| 4 | Custom endpoint HTTP/HTTPS policy undefined | Medium | Take explicit position in design |
| 5 | Cross-project bucket access in CI/CD | Low | Document recommendation for prefix-scoped IAM |
| 6 | Version counter overflow DoS | Low | Optional: note in security section |

None of these are blocking the design's fundamental approach. The architecture (blocklist, user/project split, feature flag gating, local-first with sync layer) is sound. The gaps are in edge-case specification, not structural flaws.
