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

### From the environment

Every session setting except `marker_retain_days` can also come from an environment variable, named `KOTO_` plus the setting's dotted name, upper-cased, with dots turned into underscores. A variable wins over both files, setting by setting, so you can put one workspace's sessions in the bucket by setting its processes' environment and leave every config file alone:

```bash
export KOTO_SESSION_BACKEND=cloud
export KOTO_SESSION_CLOUD_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
export KOTO_SESSION_CLOUD_BUCKET=my-koto-sessions
export KOTO_SESSION_CLOUD_REGION=auto
export KOTO_SESSION_CLOUD_PATH_STYLE=false   # optional
```

This is the safe way to switch one workspace on a host that runs other sessions: koto re-reads `$HOME/.koto/config.toml` on every command, so changing that file moves every running session on the host to the new backend at once.

Values are trimmed, and an empty or whitespace-only variable counts as unset, so the setting falls through to the files. `KOTO_SESSION_BACKEND` must be `local` or `cloud`, `KOTO_SESSION_CLOUD_PATH_STYLE` `true` or `false`, in lower case, and `KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS` a whole number of seconds. Any other value never falls back to a file or to the local backend: every command that opens a session refuses with an error naming the variable, while `koto config list` and `koto config get` still show it so you can see what's wrong.

The commands a session runs (gates, actions) get the `KOTO_SESSION_*` variables that set its configuration, so a `koto` command a gate runs reaches the same backend.

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

**koto's own variables:**

```bash
export KOTO_SESSION_CLOUD_ACCESS_KEY=<your-access-key>
export KOTO_SESSION_CLOUD_SECRET_KEY=<your-secret-key>
```

Under these names the bucket key isn't picked up by every AWS tool the session runs. They're a pair: when either is set, both credentials come from them and nothing else is consulted, not the `AWS_*` pair and not the user file. If only one is set, every command that uses the cloud backend refuses, naming the missing one, before it sends anything. And while they're in use, the endpoint and the bucket can't come from the project file: a committed `.koto/config.toml` doesn't get to choose where your key is sent. Set those in the environment (`KOTO_SESSION_CLOUD_ENDPOINT`, `KOTO_SESSION_CLOUD_BUCKET`) or the user file.

**AWS variables (CI/CD):**

```bash
export AWS_ACCESS_KEY_ID=<your-access-key>
export AWS_SECRET_ACCESS_KEY=<your-secret-key>
```

These work as they always have when the koto pair isn't set.

**User config (persistent on your machine):**

```bash
koto config set --user session.cloud.access_key <your-access-key>
koto config set --user session.cloud.secret_key <your-secret-key>
```

The `--user` flag is required for credentials. They're blocked from project config to prevent accidental commits to git. Env vars take precedence over user config. `koto config list` redacts credential values in output, and koto replaces any credential it knows of with `[REDACTED:<source>]` in the output of the commands a session runs.

## 4. Use koto normally

There are no sync commands. These commands read or write the session's copy in the bucket as part of their normal work:

- `koto init`, `koto next`, `koto status`
- `koto context add`, `koto context get`, `koto context list`, `koto context exists`, `koto context remove`
- `koto session list`, `koto session rebind`, `koto session cleanup`, `koto session resolve`, `koto session import`

That list isn't exhaustive: any command that reads a session's state through the cloud backend checks it against the bucket first. If the bucket is unreachable, a command keeps working on the local copy and prints a warning to stderr. Once the bucket has answered that its copy of the state file, the context manifest or a key changed, though, a failure to fetch that copy fails the command instead: a 404 for a key the manifest names, say. The local copy may be behind the bucket's then, so a read never answers from it silently. Run the command again once the bucket serves the object.

### What a read costs

A read checks a session with one listing of its objects in the bucket, which says whether it was imported elsewhere and whether its state file and its context manifest changed since this host last pulled or pushed them. Only what changed is downloaded, so a read whose local copy is current costs that one request. Inside a tick, a session the tick's commands already checked isn't checked again: the tick is the window a command relies on, so its later reads of that session make no request. That includes the tick's own session after its commands wrote to it, since those writes wait in the tick's batch and the tick checks the bucket again before it pushes them. A change another host makes during the tick is read by the next tick.

Outside a tick, every koto command is its own process, so a script that calls `koto` many times would check the same session once per call. Instead, a check one koto process on this host made stands in for the next ones for `session.cloud.check_window_secs` seconds (default 5; `KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS` sets it from the environment). A write from this host ends the window at once, so this host always reads its own writes. The cost is the window itself: a write another host makes to a session can go unseen by this host's reads for up to that many seconds, and so can another host's import of the session. Writes don't share checks: each `koto context add` or `remove`, and each push of a session's state on its own, checks the bucket itself, so nothing this host writes reaches a session imported elsewhere. Set it to `0` to check in every process, as before, when sessions are written from more than one host at once and a read must see the other host's write immediately.

koto keeps what it last knew of the bucket's copy in host-local files beside the session in the local store (`cloud-sync.json`, `cloud-manifest.json`, `cloud-tick.id`, and `cloud-check.json` for the shared check). Nothing uploads them.

A `koto context get` or `koto context exists` records the read for the run journal. A read made by the session's own tick goes into its log, as before. A read made from anywhere else, another session's tick or your shell, goes into `context-reads.jsonl` beside the session on this host and is never pushed, so reading a session never writes to it.

A read of a key another host changed downloads the key into this host's copy of the session first, and records that pull as a `context_added` whose writer is `sync`. The pull follows the read: from the session's own tick it goes into the log and up with the tick's push; from anywhere else it goes into `context-reads.jsonl` beside the read, and the session's log and its copy in the bucket are left as they were. The next read finds this host's copy current and costs one listing.

### Commands a `koto next` runs

A `koto context` call made from a template's command (a default action or a gate) works on the session that `koto next` is advancing, and it doesn't push to the bucket itself. It writes the local copy and leaves the push to the tick, which pushes everything its commands wrote once, before it returns. A call costs a request or two instead of a full round of pulls and pushes, so an action can make many of them inside its time limit.

Two checks still run: whether the session was imported elsewhere, which refuses, once per tick, and on the first write of the tick, whether another host's writes conflict with this one's, which also refuses. The tick checks both again before it pushes. The tick's own session is pulled from the bucket once, when the tick starts, not again by each command.

While the tick's commands have writes it hasn't pushed, the bucket holds a `batch-open.json` object beside the session's state file, and `koto session import` refuses the session (`import_source_unpushed`). A tick that was killed before it pushed leaves the writes on its host. The next koto command on the session there, `koto status <name>` for instance, pushes them and removes the object. The same command removes an object left by a `koto context` call killed while it was putting the object in the bucket, before it had written anything.

A `koto context add` or `koto context remove` for any other session, or made outside a tick, pushes at once, as before.

### Where a session lives in the bucket

A session's remote objects sit under a prefix derived from the workspace that created it: the first 16 hex digits of the SHA-256 of the canonical path of the directory koto ran in. A session named `review` created in `/srv/ws-a` lives under `<hash-of-/srv/ws-a>/review/`, and the commands above find it there when you run them from `/srv/ws-a`.

So the prefix is the session's identity on its host. Running `koto next review` on another machine, or from another directory, doesn't continue it. To continue a session in another workspace or on another host, import it (next section). To only read it, name its workspace with `--workspace` (below).

Every workspace on a host shares one local session store, so a command naming `review` from another directory would sync that shared copy against the other directory's prefix, where an older session called `review` may be waiting. These commands refuse instead, before they make any request: every `koto context` subcommand, `koto status`, `koto decisions record` and `list`, `koto overrides record` and `list`, `koto rewind`, `koto cancel` and `koto session resolve`. They exit 2 with an error starting `session_bound_elsewhere` that names the directory the session is bound to:

```
session_bound_elsewhere: session 'review' is bound to /srv/ws-a, not to the directory this command ran in; under the cloud backend a session is read only from its own workspace, so run the command from /srv/ws-a
```

Run the command from that directory, or, to only read the session from where you are, name its workspace with `--workspace` (see "Reading a session in another workspace" below), which reads the bucket's copy without touching this host's. A command counts as run from the session's own workspace when its directory is the one the session was created in, which the session records with its origin, or the session's execution anchor. The two differ for a session started with `koto init --execution-dir`, and either one works. A session created by an older koto, with no such record, counts only its anchor. A subdirectory of either is another workspace. A session with no local copy on this host is read as before. The local backend answers a name the same from every directory and never refuses.

koto's own reads of a session bound to another directory, such as an agent session's anchor or another member of a hierarchy, use this host's copy and take nothing from the current directory's prefix. `koto next` keeps its own check: it refuses a directory outside the session's execution anchor.

`koto session rebind` is a different thing. It's for a checkout that moved on the same host: stand in the new location and rebind the session's execution anchor there. Don't use it to carry a session to another machine.

Under the cloud backend, rebind pushes the rewritten header to the bucket along with its `execution_anchor_rebound` event. If that push fails, the rebind still succeeds locally and prints a warning to stderr; the session's next successful push carries the new anchor.

Run the rebind from the new location (`koto session rebind <name>`), or from the session's own workspace with `--to <new location>`. From any other directory it refuses with `session_bound_elsewhere`, naming the directory the session is bound to and the exact command to run, because its push would land under a prefix neither workspace reads. Running it from the new location works even when every directory the session records is gone, as for a session whose old checkout was deleted.

A rebind also fences the anchor it leaves. It raises the session's `origin.anchor_epoch` and lists the old anchor in `origin.retired_anchors`, and from then on a write to the session (an event, a header rewrite, a context key) refuses with `session_rebound` when it runs from the old anchor, or when its command read the session before the rebind. A process still working from the old checkout gets that error instead of writing a second history of the session. A rebind back to a retired anchor takes it off the list. A session with no origin record has no fence. An older koto ignores both fields, and reads such a header as before.

### Reading a session in another workspace

A finished session is often the record of what a run did, and a later reader, say on a fresh checkout on another machine that shares the bucket, wants to read it without importing it. Name the workspace before the verb:

```bash
koto --workspace /srv/ws-a status review
koto --workspace /srv/ws-a context get review summary.md
koto --workspace 3f9a1c0d2b4e6f81 context list review
```

`--workspace` takes the workspace's absolute path as it was on the host that created the session, the same path `koto session import --from` takes, or its id: the 16 lowercase hex digits of its prefix in the bucket. Four verbs accept it: `koto status`, `koto context get`, `koto context exists` and `koto context list`. They read the session where it is and write nothing: no object in the bucket, no `context_read` record, and no copy in this host's session store. Because they never touch this host's shared copy, they don't refuse with `session_bound_elsewhere`. What a read downloads is kept in a temporary directory that koto removes before it exits. This host doesn't become the session's host, and the source isn't marked migrated.

Every other verb refuses the option with `workspace_read_only` (exit 2) before it reads or writes anything, so a reader can't advance, change or remove another workspace's session by mistake. The option goes before the verb; after it, the verb's own arguments are parsed and `--workspace` is an unexpected argument (`koto session anchor open` has an unrelated `--workspace` of its own).

The errors a read through `--workspace` can print, each as JSON with a `code`:

| `code` | Exit | Meaning |
|--------|------|---------|
| `session_not_found` | 2 | The workspace has sessions in the bucket, but not this one |
| `workspace_not_found` | 2 | The bucket holds no session under the workspace's prefix |
| `workspace_unreadable` | 3 | Listing or reading the bucket failed; nothing is reported as empty |
| `invalid_workspace` | 2 | The value is neither a 16-digit lowercase hex id nor an absolute path |
| `invalid_session` | 2 | The session name isn't one koto could have created |
| `workspace_needs_cloud` | 2 | The session backend here is local, so there's no bucket to read |
| `workspace_read_only` | 2 | The verb writes or removes |

A session that was imported elsewhere refuses with `session_migrated`, naming where it went, as it does in its own workspace.

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

If a different session called `review` already exists on this host or under this workspace's prefix, give the import a new name. The source's own local copy doesn't count: on the host that created the session, the import takes the name over (see "Importing on the same host" below).

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
| `keys` | How many context keys came across, after the keys the import leaves behind (see "What the import settles in koto's own keys" below); the `koto/imported-from` record the import writes isn't counted |
| `template` | Where the compiled template came from: `local-cache`, `bucket` (under `--trust-template`), or `unchanged` when a re-run found the session already built |
| `marked` | The source now carries its migration marker |

The new session holds the source's events plus a `session_imported` event, every context key, and the compiled template. It's pushed under this workspace's prefix, and only then does the import leave a `migrated.json` marker beside the source.

One key is the exception. `workflows/publish-location`, the Claude Code `/workflows` folder koto renders the session into, names a folder on the host that wrote it, so it never goes to the bucket and the import doesn't carry it. A manifest pushed by koto 0.19.0 or earlier can list it without its object; the import skips that entry too. On its first commit on the new host, the session publishes its own location from `KOTO_WORKFLOWS_DIR` or the Claude Code session there.

A child session isn't imported on its own, and importing a parent doesn't bring its children: the import copies only the named session's own objects. Neither is an anchor's close session (a session whose header has `kind: "anchor-close"`): the import refuses one with `import_source_is_anchor_close`.

### What the import settles in koto's own keys

The keys under `koto/` are koto's records about the session (see "The reserved `koto/` area" in the [CLI usage guide](cli-usage.md#the-reserved-koto-area)). The import treats the bucket as untrusted for them:

- It leaves behind the observed marker (`koto/anchor/observed`), any close record (`koto/anchor-close`), every `koto/links/closes/*` link and any `koto/imported-from` the source carried. An imported anchor therefore has no observed marker: it closes once `retention.idle_period` has passed since its last activity, which is the `session_imported` event the import appends.
- It checks every link it keeps (`koto/links/<kind>/<peer>`): a known kind, a session name as the peer, and a body of at most 4 KiB naming that same peer. A link that fails is dropped, with a warning on stderr naming the key and why.
- It settles each `koto/links/held-by/<anchor>` link by the anchor it names, using the `KOTO_ANCHOR` the import runs with as the importing agent session's anchor. The link is kept when that name resolves in this host's store to an anchor that is open and not migrated; the anchor's recorded workspace is a hint only and never decides it. Otherwise it is rewritten to `koto/links/held-by/<your anchor>`, its body carrying `"was"` with the old name, or, when the import runs with no anchor (or `KOTO_ANCHOR` names no anchor here), dropped. Dispatched-by links are carried unchanged.
- It writes `koto/imported-from`, `{"from_session": "<source name>", "from_session_id": "<source id>"}`, into every copy, replacing anything the source held there. That record is what lets the retention rule treat the source as migrated and resolve links naming it to the copy.

The copy's manifest is written from what it then holds, so later verification sees the changed keys. The source gets nothing beyond the migration marker.

An anchor's template is embedded in koto and never reaches the template cache, so an anchor imports with this build's embedded template when its hash matches, with no `koto template compile` first.

So import an anchor before the sessions it holds when you move them together: a session imported after its anchor, while that anchor is still open, keeps its held-by link naming it. Import from your own agent session's shell (with `KOTO_ANCHOR` set) to have that session hold what you import, or from a bare shell to let the retention rule release a finished session once `retention.keep` has passed since it finished.

### What happens to the old copy

From then on the source refuses. `koto next`, `koto status`, and the `koto context` commands on the old host exit 2 with `session_migrated` and a message naming where the session went:

```
session_migrated: session 'review' was migrated to 'review' in /srv/ws-b; continue it there
```

Nothing else under the source's prefix is written, and nothing is deleted. The source's state file and context keys stay in the bucket until someone runs `koto session cleanup review` in the source workspace, which removes them and the local copy but keeps `migrated.json`. Cleanup, a terminal tick and `koto session prune` without `--migrated-from` never delete a `migrated.json`, so any copy of the session still left in that workspace keeps refusing.

An imported session is an ordinary session in its new workspace, so it can be imported again from there to a third one.

### Importing on the same host

Workspaces on one host share one local session store, so on the host that created the session its old copy already holds the name. Run the import without `--as` and it takes the name over, so every caller that looks the session up by name finds the new copy:

```bash
cd /srv/ws-b
koto session import review --from /srv/ws-a
```

It prints the same JSON as an import from another host, with `name` set to `review`. Before it writes anything, it checks the old copy and refuses, leaving it untouched, when:

- another process holds the session's lock (`import_source_running`): stop it first;
- the local copy holds events or context the bucket lacks (`import_source_unpushed`): run `koto session resolve review --keep local` in the source workspace, then import again;
- a local session names it as its parent (`import_source_has_children`).

A local copy behind the bucket is fine: the import takes the bucket's events, as it does from another host.

The import builds the new session in `~/.koto/sessions/.takeover-review/`, pushes it under this workspace's prefix, writes the marker on the source, and only then swaps the two directories in one atomic call. After the swap `.takeover-review/` holds the old copy, which is deleted once the import has checked it's still the copy it read. A copy that changed in between is moved aside to `~/.koto/sessions/.kept-takeover-review-<time>/`, with a warning naming it, so nothing it holds is lost and the name stays usable. While `.takeover-review/` exists, commands on `review` refuse and name the import to run.

If the import stops at any step, run the same command again: a stop before the marker starts over, and a stop after it finishes the swap with the session id the marker recorded. A re-run never overwrites a copy under this workspace's prefix that has moved past its `session_imported` event; it refuses with `import_source_migrated` instead.

The swap needs a filesystem that can exchange two directories atomically (`renameat2` on Linux, `renamex_np` on macOS) with the store on one device. Where that isn't available, the import refuses before writing anything and names `--as`; an import under a new name works everywhere. Running the import from the source workspace itself, or with `--as`, behaves as it always has.

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

Remove the old host's local copy first. Once the marker and the old copy in the bucket are gone, nothing is left to refuse for it: a local copy still on the old host reads as an ordinary session there, and the next command that syncs it pushes it back under the old workspace's prefix. Run `koto session cleanup <name>` in the old workspace on that host before you prune (cleanup leaves the marker in place), or retire the host. A pin on the old copy doesn't stop this: the import carried the pin to the moved copy, so the old copy's pin keeps nothing, cleanup removes it, and `koto session unpin` refuses it as a migrated source. An import an older release ran may not have carried the pin, so check that the moved copy is pinned (`koto session list --pinned` in the new workspace) before removing the old one. Any other pinned session refuses cleanup until `koto session unpin <name>` releases it. `--source-retired` says you have done one of these.

### Import errors

A refusal or failure prints `{"error": {"code": "...", "message": "..."}}`. Exit 2 means you need to change something before running it again; exit 1 is a failure where running it again can help.

| Code | Exit | What it means | What to do |
|------|------|---------------|------------|
| `import_requires_cloud` | 2 | The backend here is local, so there's no bucket to read | Set `session.backend` to `cloud`, pointing at the source's bucket |
| `import_source_not_found` | 2 | The source workspace's prefix holds no state file for that name, or `--from` isn't absolute | Check the session name and pass the source workspace's canonical absolute path as it is on its own host; check both hosts use the same bucket |
| `import_source_migrated` | 2 | The source already carries a migration marker | Continue the session where the message says it went, or import from that workspace instead |
| `import_source_is_child` | 2 | The named session is a child of another session | Import the session at the top of its tree |
| `import_source_is_anchor_close` | 2 | The named session is an anchor's close session (`kind: "anchor-close"`), which records a close on its own host | Import the anchor or the sessions it holds instead; a close session is never imported |
| `import_name_taken` | 2 | A different session already has the name here or under this workspace's prefix | Pass `--as <new-name>`, or remove the session holding the name if it isn't wanted |
| `import_template_unavailable` | 2 | No usable compiled template with the session's hash: none in this host's cache, or under `--trust-template` none in the bucket or one that doesn't match | Run `koto template compile` here on the session's template from a matching checkout, then import again without `--trust-template` |
| `import_source_unreadable` | 1 | A source object couldn't be fetched, or failed validation (for example, a state log this koto can't carry intact) | If the bucket was unreachable, run it again; if the message names a validation failure, the source needs repair on its host |
| `import_push_failed` | 1 | Building the session here, pushing it, or moving it into place failed. Before the source is marked, what the run pushed was taken back; once it's marked (a same-host import whose swap failed), everything is kept and the message says so | Fix what the message names (usually connectivity or local disk) and run the same import again; after the marker, that finishes it. When the message names `--as` (a same-host import this host's store can't swap), import under another name instead |
| `import_source_running` | 2 | Same host only: another process holds the source session's lock | Stop whatever is advancing the session, then import again |
| `import_source_unpushed` | 2 | The source's host holds writes the bucket lacks. On the same host: the source's local copy holds events or context keys the bucket lacks. From any host: the bucket holds the source's `batch-open.json`, left by a `koto next` that stopped before it pushed its commands' writes | Same host: run `koto session resolve <name> --keep local` in the source workspace. For `batch-open.json`: run any koto command on the session in the source workspace on its host, such as `koto status <name>`. Then import again |
| `import_source_has_children` | 2 | Same host only: a local session names the source as its parent | Finish or clean up the child it names, or import under `--as` |
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

`--keep local` uploads in one pass: it reads the bucket's manifest once and uploads only the context keys whose content the bucket doesn't already hold, several at a time, then the manifest, the state file and the version. The response's `elapsed_ms` says how long the session took.

The state log is checked the same way on every sync. A pull replaces this host's log with the bucket's copy only when the copy holds every local event; a copy that lacks some, such as another directory's older copy, leaves the local log as it is. A push goes up only over the copy this host last synced (an `If-Match` on its ETag, or `If-None-Match: *` for a first push), and when that copy moved, only if the push holds every event the copy has. When the two sides each hold an event the other lacks, the command refuses and overwrites nothing:

```
session conflict: session 'review' has diverged between this host's log (14 events, 5210 bytes, last seq 14) and the bucket's copy at 3f9a1c0d2b4e6f81/review/koto-review.state.jsonl (14 events, 5198 bytes, last seq 14); the pull was refused. Nothing was overwritten. ...
```

Resolve it with `koto session resolve` as above, run from the session's directory. A push that refuses this way after an append leaves the event in this host's log, and the command exits non-zero.

## Config reference

| Key | Variable | Description | Default | Project config |
|-----|----------|-------------|---------|---------------|
| `session.backend` | `KOTO_SESSION_BACKEND` | Storage backend | `local` | Yes |
| `session.cloud.endpoint` | `KOTO_SESSION_CLOUD_ENDPOINT` | S3-compatible endpoint URL | (none) | Yes |
| `session.cloud.bucket` | `KOTO_SESSION_CLOUD_BUCKET` | Bucket name | `koto-sessions` (used when the key is unset; `koto config get` shows it as unset) | Yes |
| `session.cloud.region` | `KOTO_SESSION_CLOUD_REGION` | Region | (none) | Yes |
| `session.cloud.path_style` | `KOTO_SESSION_CLOUD_PATH_STYLE` | Address the bucket in the path (`true`) rather than as a subdomain; needed for an IP-address endpoint | `false` | Yes |
| `session.cloud.marker_retain_days` | (none) | Days a migration marker whose target is present is kept before `koto session prune --migrated-from` may remove it without `--source-retired` | `30` | No (user only: it guards a deletion, so a cloned repository's config can't shorten it) |
| `session.cloud.check_window_secs` | `KOTO_SESSION_CLOUD_CHECK_WINDOW_SECS` | Seconds this host's koto processes share one check of a session outside a tick; `0` turns the sharing off. Another host's write can go unseen for up to this long (see [What a read costs](#what-a-read-costs)) | `5` | No (user/env only: a cloned repository's config can't widen it) |
| `session.cloud.access_key` | `KOTO_SESSION_CLOUD_ACCESS_KEY`, else `AWS_ACCESS_KEY_ID` | Access key ID | (none) | No (user/env only) |
| `session.cloud.secret_key` | `KOTO_SESSION_CLOUD_SECRET_KEY`, else `AWS_SECRET_ACCESS_KEY` | Secret access key | (none) | No (user/env only) |

Precedence for each setting: its variable, then project config, then user config, then the default. For the credentials: the koto pair as a whole, else the `AWS_*` pair, else user config.

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

State pushes are conditional writes (`If-Match`, `If-None-Match`), which AWS S3, Cloudflare R2 and MinIO support. A provider that answers them with `501 Not Implemented` gets the push without the check and a warning on stderr, as before conditional pushes.

## Verifying sync

Check your resolved config:

```bash
koto config list
```

After the tables it lists where each session setting came from, as comment lines, so the output is still valid TOML:

```toml
# origin: session.backend = env KOTO_SESSION_BACKEND
# origin: session.cloud.endpoint = user
# origin: session.cloud.bucket = project
# origin: session.cloud.region = unset
# origin: session.cloud.path_style = default
# origin: session.cloud.access_key = env KOTO_SESSION_CLOUD_ACCESS_KEY
# origin: session.cloud.secret_key = env KOTO_SESSION_CLOUD_SECRET_KEY
# origin: session.cloud.marker_retain_days = default
```

`env` names the variable, `project` and `user` the config file, `default` means koto's built-in value applies, and `unset` means nothing set it and there's no default. `koto config list --json` has the same under a top-level `origins` object: `{"session.backend": {"source": "env", "variable": "KOTO_SESSION_BACKEND"}, ...}`. A file counts as an origin only when it names the key.

This shows all settings with credential values redacted. If `session.backend` shows `cloud` and the endpoint and bucket are set, sync is active.

To verify a round-trip, init a workflow and check your bucket for uploaded files:

```bash
koto init sync-test --template <template>
echo "test" | koto context add sync-test hello.txt
# Check your bucket: you should see files under <prefix>/sync-test/
koto session cleanup sync-test
```
