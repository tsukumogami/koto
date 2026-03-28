# Cloud sync setup

koto can sync sessions to any S3-compatible backend so you can resume workflows on a different machine. Sync is invisible -- existing commands handle it automatically.

## 1. Install koto

```bash
curl -fsSL https://raw.githubusercontent.com/tsukumogami/koto/main/install.sh | bash
```

Or build from source:

```bash
cargo install koto
```

Cloud sync is included in the default binary. No feature flags needed.

## 2. Configure the backend

```bash
koto config set session.backend cloud
koto config set session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
koto config set session.cloud.bucket my-koto-sessions
koto config set session.cloud.region auto
```

For team-shared settings (endpoint, bucket, region), use `--project` to write to `.koto/config.toml` (committed to git):

```bash
koto config set --project session.backend cloud
koto config set --project session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
koto config set --project session.cloud.bucket my-koto-sessions
koto config set --project session.cloud.region auto
```

## 3. Set credentials

Credentials go in environment variables (recommended for CI) or user config (for developer machines). They're never allowed in project config.

**Environment variables (CI/CD):**

```bash
export AWS_ACCESS_KEY_ID=<your-access-key>
export AWS_SECRET_ACCESS_KEY=<your-secret-key>
```

**User config (persistent on your machine):**

```bash
koto config set --user session.cloud.access_key <your-access-key>
koto config set --user session.cloud.secret_key <your-secret-key>
```

The `--user` flag is required for credentials — they're blocked from project config
to prevent accidental commits to git. Env vars take precedence over user config.
`koto config list` redacts credential values in output.

## 4. Use koto normally

No new commands needed. `koto init`, `koto next`, and `koto context add` sync to the cloud automatically. If the cloud is unreachable, operations succeed locally and retry on the next command.

```bash
# On machine A
koto init my-workflow --template review.md
echo "findings" | koto context add my-workflow research.md

# On machine B (same config + credentials)
koto next my-workflow  # downloads session from cloud, picks up where A left off
```

## 5. Handle conflicts (rare)

If two machines advance the same workflow without syncing, koto detects the conflict:

```
session conflict: local version 7 (machine a1b2c3), remote version 6 (machine d4e5f6)
```

Resolve by picking a side:

```bash
koto session resolve --keep local   # force-upload your version
koto session resolve --keep remote  # download the other machine's version
```

## Config reference

| Key | Description | Default | Project config |
|-----|-------------|---------|---------------|
| `session.backend` | Storage backend | `local` | Yes |
| `session.cloud.endpoint` | S3-compatible endpoint URL | (none) | Yes |
| `session.cloud.bucket` | Bucket name | (none) | Yes |
| `session.cloud.region` | Region | (none) | Yes |
| `session.cloud.access_key` | Access key ID | (none) | No (user/env only) |
| `session.cloud.secret_key` | Secret access key | (none) | No (user/env only) |

## Supported providers

Any S3-compatible storage works:

| Provider | Endpoint format |
|----------|----------------|
| AWS S3 | `https://s3.<region>.amazonaws.com` |
| Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` |
| MinIO | `http://localhost:9000` (with `allow_insecure = true`) |
| DigitalOcean Spaces | `https://<region>.digitaloceanspaces.com` |
| Backblaze B2 | `https://s3.<region>.backblazeb2.com` |

Set `session.cloud.endpoint` to your provider's S3-compatible URL.

## Verifying sync

Check your resolved config:

```bash
koto config list
```

This shows all settings with credential values redacted. If `session.backend` shows `cloud` and the endpoint/bucket are set, sync is active.

To verify a round-trip, init a workflow and check your bucket for uploaded files:

```bash
koto init sync-test --template <template>
echo "test" | koto context add sync-test hello.txt
# Check your S3 bucket -- you should see files under <repo-id>/sync-test/
koto session cleanup sync-test
```
