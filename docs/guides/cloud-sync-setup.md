# Cloud sync setup

koto can sync sessions to any S3-compatible bucket, so a session's state and context survive the machine it runs on, and a session can be moved to another host with `koto session import`. Sync rides on the commands you already run; there's no separate sync step.

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

`koto config set` writes to the project config, `.koto/config.toml` in the current directory, unless you pass `--user`. That's the right place for settings a team shares, since the file can be committed:

```bash
koto config set session.backend cloud
koto config set session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
koto config set session.cloud.bucket my-koto-sessions
koto config set session.cloud.region auto
```

To keep the settings to your own machine instead, add `--user`, which writes `$HOME/.koto/config.toml`:

```bash
koto config set --user session.backend cloud
koto config set --user session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
```

Project config wins over user config where both set a key.

### Endpoints given as an IP address, and self-hosted stores

By default koto addresses the bucket as a virtual host, `<bucket>.<endpoint-host>`. That can't work when the endpoint's host is an IP address, and a self-hosted store behind a hostname often doesn't serve bucket subdomains either. Set `session.cloud.path_style` to `true` and koto puts the bucket in the path instead, `<endpoint>/<bucket>/<key>`:

```bash
koto config set session.cloud.endpoint http://10.0.0.5:9000
koto config set session.cloud.bucket koto-sessions
koto config set session.cloud.region us-east-1
koto config set session.cloud.path_style true
```

A plain `http://` endpoint needs nothing extra; koto uses the scheme you give it. Leaving `path_style` unset is the same as `false`.

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

The `--user` flag is required for credentials. They're blocked from project config to prevent accidental commits to git. Env vars take precedence over user config. `koto config list` redacts credential values in output.

## 4. Use koto normally

There are no sync commands. These commands read or write the session's copy in the bucket as part of their normal work:

- `koto init`, `koto next`, `koto status`
- `koto context add`, `koto context get`, `koto context list`, `koto context exists`, `koto context remove`
- `koto session list`, `koto session rebind`, `koto session cleanup`, `koto session resolve`, `koto session import`

That list isn't exhaustive: any command that reads a session's state through the cloud backend pulls it from the bucket first. If the bucket is unreachable, a command keeps working on the local copy and prints a warning to stderr.

### Where a session lives in the bucket

A session's remote objects sit under a prefix derived from the workspace that created it: the first 16 hex digits of the SHA-256 of the canonical path of the directory koto ran in. A session named `review` created in `/srv/ws-a` lives under `<hash-of-/srv/ws-a>/review/`, and the commands above find it there when you run them from `/srv/ws-a`.

So the prefix is the session's identity on its host. Running `koto next review` on another machine, or from another directory, doesn't continue it. To continue a session in another workspace or on another host, import it (next section).

`koto session rebind` is a different thing. It's for a checkout that moved on the same host: stand in the new location and rebind the session's execution anchor there. Don't use it to carry a session to another machine.

Under the cloud backend, rebind pushes the rewritten header to the bucket along with its `execution_anchor_rebound` event. If that push fails, the rebind still succeeds locally and prints a warning to stderr; the session's next successful push carries the new anchor.

## 5. Moving a session to another host

`koto session import` reads a session from another workspace's prefix in the bucket and creates it as a new local session in the directory you run it from. Both hosts need the cloud backend pointed at the same bucket.

### Stop the source first

Import a stopped session only: if the source keeps advancing after the import, the session forks into two copies that nothing reconciles. Before you import:

1. Stop every process that advances the session on the source host: the agent, any loop ticking `koto next`, anything writing context.
2. Make sure its last write reached the bucket. If any command on the source host printed a cloud sync warning, or you aren't sure, run this on the source host, from the source workspace:

   ```bash
   koto session resolve review --keep local
   ```

   `--keep local` uploads the whole local session, state file and context, to the bucket.

koto can't tell whether a source is still moving. Anything the old host writes after the import has read the source never reaches the new copy.

### Have the template on the new host

The imported session needs the compiled template it recorded, matched by hash. By default koto takes it from the new host's own template cache, so compile the session's template there first, from a checkout that matches the one the session started with:

```bash
koto template compile path/to/review.md
```

Or pass `--trust-template` to the import, which takes the compiled template the source pushed to the bucket at `koto init`, after checking that its hash matches the session's. That template holds every command the session runs, so pass it only when you trust everyone who can write to the bucket. A session created by a koto that didn't push its template has no copy in the bucket; compile locally for those.

### Run the import

On the new host, `cd` to the directory the session should run in and run:

```bash
koto session import review --from /srv/ws-a
```

`--from` is the absolute path of the source workspace as it was on the host that created the session, symlinks resolved. A relative path is refused. To find it, run `pwd -P` in the source workspace; `koto session list` there shows the session's name. The current directory becomes the new session's execution anchor and its workspace, so run the session's later commands from there.

If a session called `review` already exists on this host or under this workspace's prefix, give the import a new name:

```bash
koto session import review --from /srv/ws-a --as review-b
```

On success it prints one JSON object:

```json
{"name":"review","imported":true,"from":{"workspace":"/srv/ws-a","session":"review"},"keys":4,"template":"local-cache","marked":true}
```

| Field | Meaning |
|-------|---------|
| `name` | The session's name here (the `--as` name when given) |
| `from.workspace`, `from.session` | Where it came from |
| `keys` | How many context keys came across |
| `template` | Where the compiled template came from: `local-cache`, `bucket` (under `--trust-template`), or `unchanged` when a re-run found the session already built |
| `marked` | The source now carries its migration marker |

The new session holds the source's events plus a `session_imported` event, every context key, and the compiled template. It's pushed under this workspace's prefix, and only then does the import leave a `migrated.json` marker beside the source.

A child session isn't imported on its own, and importing a parent doesn't bring its children: the import copies only the named session's own objects.

### What happens to the old copy

From then on the source refuses. `koto next`, `koto status`, and the `koto context` commands on the old host exit 2 with `session_migrated` and a message naming where the session went:

```
session_migrated: session 'review' was migrated to 'review' in /srv/ws-b; continue it there
```

Nothing else under the source's prefix is written, and nothing is deleted. The source's state file and context keys stay in the bucket until someone runs `koto session cleanup review` in the source workspace, which removes them and the local copy but keeps `migrated.json`. Cleanup, a terminal tick and `koto session prune` without `--migrated-from` never delete a `migrated.json`, so any copy of the session still left in that workspace keeps refusing.

An imported session is an ordinary session in its new workspace, so it can be imported again from there to a third one.

### Removing the old copy and its marker

Markers pile up under the old workspace's prefix, each beside whatever of the old copy is still there. Once the old host can't come back and advance its copy, remove them with `koto session prune --migrated-from`, run from any workspace on the same bucket:

```bash
koto session prune --migrated-from /srv/ws-a                  # preview
koto session prune --migrated-from /srv/ws-a --apply --plan <token>
```

Pass the old workspace's absolute path as it was on its own host, the same path `koto session import --from` took. The preview lists every marker under that workspace's prefix with the session it names, where the session went, the marker's age (from its `migrated_at`), what the target holds now, and whether it would be removed and why. The target status comes from a listing of the target's prefix and a read of its state file, checked against the old copy's own state file:

| `target_status` | What the bucket holds for the target |
|-----------------|--------------------------------------|
| `present` | A state file whose header carries the session id the marker names, and whose whole log reads with no error and begins with every event of the old copy's log, unchanged |
| `replaced` | A state file for another session under the same name |
| `migrated` | A marker of its own: the session moved on again |
| `absent` | No state file |
| `unverified` | The right session's state file, but its log doesn't read to the end (a line that doesn't parse, the last one included, or a gap in its sequence), it ends before the old copy's does, it holds a different event at some point the old copy's log reaches, or the old copy can't be listed or its log can't be read |
| `unknown` | The listing or the read failed |

A marker is removable only when its target is `present`, and then only once it is at least `session.cloud.marker_retain_days` days old (default 30), or when you pass `--source-retired` to say the old host is gone for good. Setting `marker_retain_days` to 0 makes every marker whose target is `present` removable at once. Any other status keeps the marker at any age, with or without the flag, because the old copy beside it may be the only one left. A marker whose body doesn't parse, has no `migrated_at`, or whose target doesn't name a session and a workspace prefix is kept as `unreadable`. Every entry carries `removable`, a `reason` (the target status, `unreadable`, `age`, or `source_retired`) and a `detail` sentence, so the preview shows which markers `--source-retired` adds. An entry whose old copy could be listed and its log read also carries `objects`, the number of objects its removal deletes, the marker included.

Removing a marker deletes everything under `<prefix>/<name>/` in the old workspace's prefix, the marker last. The marker isn't removed on its own: without it, the old copy would read as a live session again. An old copy whose state file is already gone, as a removal that stopped part way leaves it, has no events left to compare, so a retry only needs a target whose log reads cleanly. Apply reads each marker again and judges it again, so a marker rewritten since the preview, or whose target changed, is skipped with the reason. Nothing locks the bucket between that last judgement and the deletes, so a target that changes in that short window isn't seen. If a delete fails, that entry stops before its marker goes, so the old copy still refuses; it is listed under `failed`, the run exits 3, and running it again finishes the job.

The plan token and stored plan work as they do for finished sessions (see the [CLI usage guide](cli-usage.md#session-prune)): an unchanged token applies, one whose set changed exits 4 with `prune_plan_stale`, and the stored plan records whether `--source-retired` was given. `--apply` without `--plan` removes what is removable now. Marker removal never touches the local session store: a local copy of the old session on the host you run it from stays, and the only local write is the stored plan.

Remove the old host's local copy first. Once the marker and the old copy in the bucket are gone, nothing is left to refuse for it: a local copy still on the old host reads as an ordinary session there, and the next command that syncs it pushes it back under the old workspace's prefix. Run `koto session cleanup <name>` in the old workspace on that host before you prune (cleanup leaves the marker in place), or retire the host. `--source-retired` says you have done one of these.

### Import errors

A refusal or failure prints `{"error": {"code": "...", "message": "..."}}`. Exit 2 means you need to change something before running it again; exit 1 is a failure where running it again can help.

| Code | Exit | What it means | What to do |
|------|------|---------------|------------|
| `import_requires_cloud` | 2 | The backend here is local, so there's no bucket to read | Set `session.backend` to `cloud`, pointing at the source's bucket |
| `import_source_not_found` | 2 | The source workspace's prefix holds no state file for that name, or `--from` isn't absolute | Check the session name and pass the source workspace's canonical absolute path as it is on its own host; check both hosts use the same bucket |
| `import_source_migrated` | 2 | The source already carries a migration marker | Continue the session where the message says it went, or import from that workspace instead |
| `import_source_is_child` | 2 | The named session is a child of another session | Import the session at the top of its tree |
| `import_name_taken` | 2 | A different session already has the name here or under this workspace's prefix | Pass `--as <new-name>`, or remove the session holding the name if it isn't wanted |
| `import_template_unavailable` | 2 | No usable compiled template with the session's hash: none in this host's cache, or under `--trust-template` none in the bucket or one that doesn't match | Run `koto template compile` here on the session's template from a matching checkout, then import again without `--trust-template` |
| `import_source_unreadable` | 1 | A source object couldn't be fetched, or failed validation (for example, a state log this koto can't carry intact) | If the bucket was unreachable, run it again; if the message names a validation failure, the source needs repair on its host |
| `import_push_failed` | 1 | Building the session here, pushing it, or moving it into place failed; what the run pushed was taken back | Fix what the message names (usually connectivity or local disk) and run the same import again |
| `import_unmarked` | 1 | The session was imported and pushed, but the marker on the source couldn't be written, so the old copy won't refuse yet | Run the same import again; it writes only the marker. Keep the source stopped until it succeeds |

## 6. Handle conflicts (rare)

If the local copy and the bucket's copy of the same session both advanced since they last agreed, koto detects the conflict:

```
session conflict: local version 7 (machine a1b2c3), remote version 6 (machine d4e5f6)
```

Resolve by picking a side:

```bash
koto session resolve <name> --keep local   # force-upload your version
koto session resolve <name> --keep remote  # download the bucket's version
```

## Config reference

| Key | Description | Default | Project config |
|-----|-------------|---------|---------------|
| `session.backend` | Storage backend | `local` | Yes |
| `session.cloud.endpoint` | S3-compatible endpoint URL | (none) | Yes |
| `session.cloud.bucket` | Bucket name | `koto-sessions` (used when the key is unset; `koto config get` shows it as unset) | Yes |
| `session.cloud.region` | Region | (none) | Yes |
| `session.cloud.path_style` | Address the bucket in the path (`true`) rather than as a subdomain; needed for an IP-address endpoint | `false` | Yes |
| `session.cloud.marker_retain_days` | Days a migration marker whose target is present is kept before `koto session prune --migrated-from` may remove it without `--source-retired` | `30` | No (user only: it guards a deletion, so a cloned repository's config can't shorten it) |
| `session.cloud.access_key` | Access key ID | (none) | No (user/env only) |
| `session.cloud.secret_key` | Secret access key | (none) | No (user/env only) |

## Supported providers

Any S3-compatible storage works:

| Provider | Endpoint format | Notes |
|----------|----------------|-------|
| AWS S3 | `https://s3.<region>.amazonaws.com` | |
| Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` | |
| MinIO | `http://10.0.0.5:9000` | Set `session.cloud.path_style` to `true` |
| DigitalOcean Spaces | `https://<region>.digitaloceanspaces.com` | |
| Backblaze B2 | `https://s3.<region>.backblazeb2.com` | |

Set `session.cloud.endpoint` to your provider's S3-compatible URL.

## Verifying sync

Check your resolved config:

```bash
koto config list
```

This shows all settings with credential values redacted. If `session.backend` shows `cloud` and the endpoint and bucket are set, sync is active.

To verify a round-trip, init a workflow and check your bucket for uploaded files:

```bash
koto init sync-test --template <template>
echo "test" | koto context add sync-test hello.txt
# Check your bucket: you should see files under <prefix>/sync-test/
koto session cleanup sync-test
```
