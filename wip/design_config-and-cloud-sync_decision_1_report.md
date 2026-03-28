# Decision 1: Config file format, locations, and precedence

## Decision

**Option 1: TOML with dotted key paths** -- user config at `~/.koto/config.toml`, project config at `.koto/config.toml`, dotted keys mapping to TOML tables.

## Evaluation

### Option 1: TOML with dotted key paths (chosen)

This is the only option that satisfies the PRD. R11 explicitly specifies TOML format, dotted key paths (`session.backend`, `session.cloud.endpoint`), two config locations (project and user), and precedence order (project > user > default). The PRD also specifies `koto config set` writes to user config by default with `--project` for project config.

Beyond PRD compliance, this option aligns with tsuku's existing config system. tsuku uses `~/.tsuku/config.toml` with BurntSushi/toml, dotted keys for nested config (`llm.enabled`, `llm.providers`), and a `Get`/`Set` interface that maps dotted key strings to struct fields. koto's implementation can follow the same pattern with Go's `BurntSushi/toml` or the equivalent pure-Go parser, keeping the developer experience consistent across the org's tools.

The two-file model (user + project) matches git's global/local config. Project config at `.koto/config.toml` is committed to git, letting teams share backend settings (e.g., a shared S3 bucket). User config holds personal defaults and credentials. Environment variables override both for secrets (`AWS_ACCESS_KEY_ID` over `session.cloud.access_key`), which the PRD calls out explicitly to avoid committing credentials.

**Strengths:**
- Direct PRD compliance (R11 specifies this exact approach)
- Consistent with tsuku's config system (same format, same CLI pattern)
- Familiar git-style precedence model
- TOML handles nesting naturally (`[session.cloud]` table)
- Human-readable and hand-editable
- Comments supported (useful for documenting project config in repos)

**Weaknesses:**
- Slightly more complex to implement than flat key-value (need TOML parser, table merging)
- Two config files require a merge strategy (but this is well-understood from git)

### Option 2: Flat env-file style (rejected)

No nesting support. The PRD's config keys are hierarchical (`session.cloud.endpoint`, `session.cloud.bucket`, `session.cloud.region`). Flattening to `SESSION_CLOUD_ENDPOINT` loses the grouping that TOML tables provide. Also directly contradicts R11's TOML specification. Comments would need a custom parser. The format doesn't support typed values (everything is a string), though that's less of an issue for koto's current config surface.

### Option 3: JSON config (rejected)

JSON lacks comments, which matters for project config committed to git -- teams need to document why a particular backend or bucket is configured. JSON is harder to hand-edit (trailing commas, quoting requirements). The PRD specifies TOML. While JSON nesting works well, the ergonomic downsides and PRD non-compliance rule it out.

### Option 4: TOML with separate section files (rejected)

Splitting config into `config.d/session.toml`, `config.d/cloud.toml` adds discovery complexity without clear benefit for koto's config surface area. koto's config is small: session backend, cloud credentials, maybe a few more keys. A single file per level (user, project) is simpler to reason about, simpler to implement `koto config set` against, and simpler to document. The `config.d/` pattern makes sense for systems with many independent plugins contributing config (like systemd), but koto's config is centrally defined.

## Implementation notes

The config struct should use Go structs with `toml` tags, matching tsuku's approach. The merge strategy is straightforward: load default values, overlay user config, overlay project config. For `koto config set`, the target file (user or project) is loaded, modified, and written back atomically (temp file + rename, as tsuku does). The `--project` flag controls which file is written.

Key differences from tsuku's config: koto needs two-file merging (tsuku only has user config), and koto's project config must handle the case where `.koto/config.toml` doesn't exist yet (`koto config set --project` should create it).

Credentials (`session.cloud.access_key`, `session.cloud.secret_key`) should be stored in user config only, never project config. The `--project` flag should reject secret keys. Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) take precedence over config file credentials per R11.
