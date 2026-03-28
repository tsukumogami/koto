# Decision 3: Credential handling and env var overrides

## Question

How does koto handle S3 credentials securely?

## Chosen: Option 1 -- Env var override with config-file blocklist

Credentials (`session.cloud.access_key`, `session.cloud.secret_key`) can be set in
user config (`~/.koto/config.toml`) but are blocked from project config
(`.koto/config.toml`). Env vars `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`
override config values. Resolution order: env var > user config > error.

## Rationale

Three factors drive this choice:

**S3-compatible stores need explicit credentials.** The PRD requires support for
Cloudflare R2, MinIO, and other S3-compatible stores -- not just AWS. These providers
use access_key/secret_key pairs. AWS-specific credential chains (instance profiles,
SSO, credential_process) don't apply to R2 or MinIO. A generic access_key/secret_key
model covers all targets uniformly.

**Developer ergonomics on local machines.** Env-vars-only (Option 2) forces developers
to manage credentials outside koto -- shell profiles, direnv, or manual exports before
every session. Storing credentials in `~/.koto/config.toml` (permissions 0600 inside a
0700 directory) matches what `~/.aws/credentials` already does and lets `koto config
set session.cloud.access_key <value>` work as a one-time setup step.

**No async runtime needed for credential resolution.** koto is currently a synchronous
binary with no tokio dependency. Delegating to the AWS SDK (Option 3) would pull in
`aws-config` and `aws-credential-types`, requiring an async runtime just to resolve
credentials. The S3 client will eventually need async for HTTP, but credential
resolution should stay decoupled from that. Reading a TOML file and checking env vars
is trivial and keeps the credential path synchronous.

## Security analysis

| Vector | Mitigation |
|--------|------------|
| Credentials committed to git via project config | `koto config set --project session.cloud.access_key` returns a hard error. The blocklist is a static list of sensitive key prefixes checked at write time. |
| User config file readable by other users | `~/.koto/` is created with 0700 permissions (LocalBackend already sets this). Config file inherits directory permissions. Same security model as `~/.aws/credentials`. |
| Env vars visible in /proc | Standard CI/CD pattern. Same exposure as AWS_ACCESS_KEY_ID everywhere else. No worse than what every other tool accepts. |
| Credentials in memory | Short-lived: read once during S3 client initialization, then held in the client struct for the duration of the command. No persistence beyond process lifetime. |

## Blocklist implementation

A static array of key prefixes that `config set --project` rejects:

```rust
const SENSITIVE_PREFIXES: &[&str] = &[
    "session.cloud.access_key",
    "session.cloud.secret_key",
];
```

`koto config set --project` checks the key against this list before writing. User
config (`koto config set` without `--project`) is unrestricted.

## Credential resolution pseudocode

```
fn resolve_credentials(config: &Config) -> Result<Option<Credentials>> {
    let access_key = env::var("AWS_ACCESS_KEY_ID")
        .ok()
        .or_else(|| config.get("session.cloud.access_key"));
    let secret_key = env::var("AWS_SECRET_ACCESS_KEY")
        .ok()
        .or_else(|| config.get("session.cloud.secret_key"));

    match (access_key, secret_key) {
        (Some(ak), Some(sk)) => Ok(Some(Credentials { access_key: ak, secret_key: sk })),
        (None, None) => Ok(None),  // no credentials configured
        _ => Err(anyhow!("partial credentials: both access_key and secret_key must be set")),
    }
}
```

## Rejected options

### Option 2: Env vars only, no credential config

Simpler but worse developer experience. Forces every developer to manage credentials
outside koto. The "simplicity" benefit is marginal -- Option 1 adds ~20 lines of
blocklist checking code. The flexibility cost is real: developers who use koto
across multiple projects with different S3 backends would need per-directory env var
management (direnv or similar), adding an external tool dependency that contradicts
koto's self-contained philosophy.

### Option 3: Credential delegation to AWS SDK

Would pull in the full `aws-config` crate and require tokio as a runtime dependency
for credential resolution. The AWS SDK's credential chain (instance profiles, SSO,
credential_process, etc.) is designed for AWS -- not S3-compatible stores. R2 and
MinIO users would still fall back to access_key/secret_key via env vars or
`~/.aws/credentials`, gaining nothing from the SDK's chain. The dependency cost is
high (aws-config pulls in ~40 transitive crates) for minimal benefit.

If koto later needs AWS-native features (IAM roles, STS assume-role), the credential
resolution function can be extended to optionally use the AWS SDK when the `cloud`
feature flag is enabled. Option 1 doesn't close this door.

### Option 4: Encrypted credential store

Overkill for this use case. Encrypted storage requires either a master password
(interactive prompt, breaks CI/CD -- violates PRD constraints) or system keyring
integration (platform-specific, adds native dependencies, breaks koto's no-system-deps
philosophy). The threat model doesn't justify it: `~/.koto/config.toml` with 0600
permissions in a 0700 directory provides the same protection as `~/.aws/credentials`,
which the industry has accepted as sufficient for decades.

## Assumptions

1. The `~/.koto/` directory will be created with 0700 permissions by the config
   subsystem, matching what LocalBackend already does for session directories.
2. S3-compatible stores (R2, MinIO) will accept the same access_key/secret_key
   credential format that AWS S3 uses.
3. The S3 client crate chosen in Decision 6 will accept credentials as explicit
   strings (not requiring the AWS SDK credential provider chain).
4. CI/CD environments will provide credentials via env vars, which is standard
   practice.
