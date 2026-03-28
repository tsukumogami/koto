# koto

Workflow orchestration engine for AI coding agents. koto enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

Agents call `koto next` to get their current directive and do the work. If something goes wrong, `koto rewind` rolls back to the previous state without losing the audit trail.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/tsukumogami/koto/main/install.sh | bash
```

The script detects your platform, downloads the latest release, verifies the checksum, and adds koto to your PATH. Set `KOTO_INSTALL_DIR` to change the install location (defaults to `~/.koto`).

Or download a binary directly from the [GitHub Releases page](https://github.com/tsukumogami/koto/releases).

Or build from source with Rust:

```bash
git clone https://github.com/tsukumogami/koto.git
cd koto
cargo build --release
# binary is at target/release/koto
```

## Quick start

### 1. Create a workflow template

Templates are markdown files with YAML front-matter. Each `##` heading defines a state, and `**Transitions**` lines define which states can follow.

```markdown
---
name: review
version: "1.0"
description: Code review workflow
variables:
  PR_URL: ""
---

## assess

Review the PR at {{PR_URL}} and summarize the changes.

**Transitions**: [feedback]

## feedback

Leave review comments on the PR.

**Transitions**: [done]

## done

Review complete.
```

States without transitions are terminal -- the workflow ends there.

### 2. Initialize a workflow

```bash
koto init review --template review.md
```

```json
{"name":"review","state":"assess"}
```

This creates a session directory at `~/.koto/sessions/<repo-id>/review/` and writes a state file inside it. The state file starts with three lines: a header (schema version, workflow name, template hash, timestamp), a `workflow_initialized` event, and an initial `transitioned` event.

### 3. Get the current directive

```bash
koto next review
```

```json
{
  "action": "execute",
  "state": "assess",
  "directive": "Review the PR at {{PR_URL}} and summarize the changes.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "summary": {"type": "string", "required": true}
    }
  },
  "error": null
}
```

The `action` field is `"execute"` while work remains and `"done"` at the terminal state. The `expects` object tells the agent what evidence to submit. The `advanced` flag is `true` when the call itself caused a state change (via `--with-data` or `--to`).

## Key concepts

**Templates** define the workflow: states, transitions between them, and directive text for each state. Variables (`{{KEY}}`) are interpolated into directives at runtime. The runtime also injects `{{SESSION_DIR}}`, which resolves to the session's absolute path so templates can reference session-local files. Use `koto template compile` to validate templates during development and see the compiled JSON output.

**Sessions** are stored at `~/.koto/sessions/<repo-id>/<name>/`, keeping state files out of your working directory. Each session holds a state file and any artifacts the workflow produces. When a workflow reaches its terminal state, `koto next` automatically cleans up the session directory (pass `--no-cleanup` to keep it). Use `koto session dir <name>` to get the path, `koto session list` to see all sessions, or `koto session cleanup <name>` to remove one manually.

**Content ownership**: Agents submit workflow artifacts through `koto context add` rather than writing files directly. This gives koto full visibility into what was produced and enables content-aware gates (`context-exists`, `context-matches`) that check content state without shell commands. Use `koto context get` to retrieve content and `koto context list` to see what's been submitted.

**State files** (`koto-<name>.state.jsonl`) live inside session directories and use an event log format. The first line is a header with the schema version, workflow name, template hash, and creation timestamp. Subsequent lines are typed events with monotonic sequence numbers and type-specific payloads. The current state is derived by replaying the log -- specifically, the `to` field of the last state-changing event.

**Template integrity**: The template's SHA-256 hash is locked at init time and stored in the first event. If the compiled template changes, `next` will fail. To update the template, reinitialize the workflow.

**Cloud sync**: Sessions default to local storage, but koto can sync them to any S3-compatible backend (AWS S3, Cloudflare R2, MinIO). Set `session.backend` to `"cloud"` via `koto config set`, configure your endpoint and credentials, and existing commands handle sync transparently. Install with `cargo install koto --features cloud` to enable it. See the [CLI usage guide](docs/guides/cli-usage.md) for setup details.

**Configuration**: koto merges config from two layers: project config (`.koto/config.toml`, shared via version control) and user config (`~/.koto/config.toml`, machine-specific). Credentials are restricted to user config and environment variables -- they can't be set in project config. Use `koto config list` to see the resolved values.

## Cloud sync setup

koto can sync sessions to any S3-compatible backend so you can resume workflows on a different machine. Sync is invisible -- existing commands handle it automatically.

### 1. Install with cloud support

```bash
cargo install koto --features cloud
```

Or build from source:

```bash
cargo build --release --features cloud
```

The default install (without `--features cloud`) has zero S3 dependencies.

### 2. Configure the backend

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

### 3. Set credentials

Credentials go in environment variables (recommended for CI) or user config (for developer machines). They're never allowed in project config.

**Environment variables (CI/CD):**

```bash
export AWS_ACCESS_KEY_ID=<your-access-key>
export AWS_SECRET_ACCESS_KEY=<your-secret-key>
```

**User config (persistent on your machine):**

```bash
koto config set session.cloud.access_key <your-access-key>
koto config set session.cloud.secret_key <your-secret-key>
```

Env vars take precedence over user config. `koto config list` redacts credential values in output.

### 4. Use koto normally

No new commands needed. `koto init`, `koto next`, and `koto context add` sync to the cloud automatically. If the cloud is unreachable, operations succeed locally and retry on the next command.

```bash
# On machine A
koto init my-workflow --template review.md
echo "findings" | koto context add my-workflow research.md

# On machine B (same config + credentials)
koto next my-workflow  # downloads session from cloud, picks up where A left off
```

### 5. Handle conflicts (rare)

If two machines advance the same workflow without syncing, koto detects the conflict:

```
session conflict: local version 7 (machine a1b2c3), remote version 6 (machine d4e5f6)
```

Resolve by picking a side:

```bash
koto session resolve --keep local   # force-upload your version
koto session resolve --keep remote  # download the other machine's version
```

### Supported providers

Any S3-compatible storage works: AWS S3, Cloudflare R2, MinIO, DigitalOcean Spaces, Backblaze B2. Set the `session.cloud.endpoint` to your provider's S3-compatible URL.

## Agent integration

AI coding agents can run koto workflows through the Claude Code plugin. Install it with two commands:

```
/plugin marketplace add tsukumogami/koto
/plugin install koto-skills@koto
```

The plugin ships with **hello-koto**, a minimal two-state skill that walks through the full loop: template setup, variable interpolation, command gates, and state transitions. Run `/hello-koto Hasami` to try it.

Once a skill is installed, the agent follows a simple cycle:

1. `koto init` -- start the workflow from a template
2. `koto next` -- get the current directive and `expects` schema
3. Execute the work described in the directive
4. `koto next --with-data '{...}'` -- submit evidence matching the schema, or `koto next --to <state>` for a directed transition
5. Repeat from step 2 until `action` is `"done"`

The plugin also includes a Stop hook that detects active workflows when a session ends, so the agent can resume where it left off.

Skills use the [Agent Skills](https://agentskills.io) open standard, which means they work across Claude Code, Codex, Cursor, Windsurf, and other platforms that support it. For project-specific workflows, write a SKILL.md alongside your template in `.claude/skills/<name>/` and commit both to version control.

## Documentation

- [CLI usage guide](docs/guides/cli-usage.md) -- all subcommands with examples, including template authoring tools
- [Error code reference](docs/reference/error-codes.md) -- structured error codes and handling
- [Custom skill authoring](docs/guides/custom-skill-authoring.md) -- creating workflow skills for AI agents

## License

See [LICENSE](LICENSE).
