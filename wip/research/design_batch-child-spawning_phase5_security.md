# Security Review: batch-child-spawning

## Dimension Analysis

### External Artifact Handling
**Applies:** Yes

The feature introduces a new external input channel (`--with-data @file.json`)
that supplies a JSON task list shaped by an agent rather than a template
author. Each entry carries `name`, `template`, `vars`, and `waits_on`. The
design enumerates runtime checks (R1-R7) for the task list (unique names,
DAG structure, `validate_workflow_name` enforcement, vars resolution,
template compilability, sibling collision check), plus a 1 MB resolved-size
cap on `--with-data`. Those handle several of the obvious abuse shapes:

- **Path traversal via `template`.** Decision 4's resolution order is
  (a) absolute passthrough, (b) join against the parent's canonicalized
  `template_source_dir`, (c) fallback to `submitter_cwd`. The design does
  not say that relative paths are rejected if they escape the base
  directory with `..`. Because koto is a local-user tool (not multi-tenant
  hosting), reading `../../etc/passwd` would just re-read a file the
  invoking user already has read access to, so the practical blast
  radius is limited -- but the fallback to `submitter_cwd` means a task
  list can resolve templates from anywhere the agent happens to be cwd'd
  to, which is a mild surprise versus the principle-of-least-astonishment.
  The design should state explicitly whether `..` is allowed and what the
  intended trust boundary is. "Templates are developer-authored, not
  user-submitted" (per the koto-specific context) means a path traversal
  here loads a developer's own template file -- not a classic traversal
  exploit, but worth being explicit.

- **Shell injection via `vars`.** The concern is whether `vars` values
  flow into a subprocess command line. Koto does execute shell via
  `default_action` commands in templates, which use `resolve_variables()`
  to interpolate `$VAR`-style placeholders. The design says task `vars`
  are forwarded to each child's `resolve_variables()`. So yes, agent-
  supplied `vars` values can land inside a child template's
  `default_action` shell string as positional substitutions. Given that
  `default_action` already accepts arbitrary shell from template authors,
  and templates are developer-authored, the net new exposure is: an
  agent that submits evidence can choose the values a developer's
  template will substitute into its own shell. That is already true for
  any existing `--var` flag on `koto init`, so this is not a new attack
  surface -- but the design should note that `vars` have the same trust
  level as any other agent-supplied variable and templates must quote
  them appropriately, same as today.

- **Templates executing arbitrary code.** Koto templates cannot execute
  arbitrary code on their own; only explicit `default_action` shell
  commands run, and those are written by the template author. A malicious
  task entry can point `template` at any template file readable by the
  user, including one with a `default_action` that the agent knows is
  destructive. In practice this means: if an agent has write access to
  evidence submission, it has effectively the same power as an agent
  with write access to a template file. That is already the koto threat
  model (agents are trusted collaborators running on the user's machine),
  but the design should make this explicit in Security Considerations so
  a future reviewer doesn't mistake batch spawning for a privilege
  boundary.

- **Resource exhaustion.** The 1 MB `--with-data` cap bounds the raw task
  list size, which is a good defensive cap. However, the design does not
  specify an explicit maximum task count, maximum `waits_on` edge count,
  or maximum DAG depth. The scheduler's DAG builder runs on every
  `koto next` tick and does a full topological classification -- a 100k
  task list fits under 1 MB, builds a DAG, and every subsequent `koto
  next` call re-classifies all 100k tasks plus calls `backend.list()` and
  per-child reads for each. The design's "Negative" section already flags
  that 50+ children has non-trivial read-time cost; 10k+ is
  quadratic-ish. This is a soft DoS against the invoking user's own
  machine, not a classic adversarial vector, but a documented recommended
  cap (e.g., 1000 tasks, 10000 edges, depth N) would prevent an agent
  from accidentally jamming its own workflow. Cycle detection is
  explicitly R3, so deeply nested `waits_on` chains at least cannot hang
  the scheduler.

### Permission Scope
**Applies:** Yes

- **`init_state_file` symlink attack surface.** The design uses
  `tempfile::NamedTempFile::persist` for the atomic tmp+rename, which is
  the same pattern already used by `write_manifest`. `tempfile` creates
  the temp file with `O_EXCL` inside the target directory, so a symlink
  pre-seeded at the final name would cause `rename(2)` to overwrite
  whatever the symlink points at -- but `rename` operates on the symlink
  itself, not its target, and koto's session directory is under the
  user's own control (typically `~/.koto/sessions/` or equivalent). A
  pre-existing symlink from an attacker would require the attacker to
  already have write access to the session directory, at which point
  they can do anything. Not a new attack surface versus today's
  `write_manifest` pattern, but the design should note that the session
  directory is assumed to be user-owned and not world-writable.

- **`backend.list()` + per-child reads on multi-tenant setups.** Koto is
  explicitly not for multi-tenant hosting (per the koto-specific context),
  so information disclosure between tenants is out of scope. On a single-
  user machine, the scheduler reading all sibling state files is fine --
  it's reading the same user's own data. If cloud sync is used, the
  bucket is scoped per user; cross-user exposure would require a
  misconfigured bucket, which is an operator concern, not a koto
  concern.

- **Child workflow permission inheritance.** Spawned children get
  `parent_workflow` set, a resolved template path, and agent-supplied
  vars. Children do not inherit anything privileged -- they run with the
  same user credentials as the parent and any template they execute was
  already readable by that user. The only escalation-shaped concern is
  that a child's `default_action` may execute shell, and the shell
  inherits the invoking user's process environment including any secrets
  in env vars. This is unchanged from today's `koto init`, so batch
  spawning adds no new escalation.

- **`retry_failed` DoS.** An adversary with evidence-submission access
  can spam `retry_failed` to repeatedly rewind children, forcing
  re-execution of each child's shell commands (which may be expensive:
  git operations, network calls, test runs). Since the adversary in this
  model is an agent the user has chosen to run, this is self-DoS, not
  cross-user DoS. The design could usefully document a per-call
  budget or rate-limit hook, but it's not a security-critical gap.
  Append-only semantics mean each retry creates a new `Rewound` event
  plus a clearing evidence event, so the state file grows linearly with
  retries, another soft self-DoS surface.

### Supply Chain or Dependency Trust
**Applies:** No (new trust risk)

The design calls out one new crate reference: `tempfile`, already used by
`write_manifest` in `src/session/local.rs:189-209`. No net new dependency,
no net new crate. The `init_state_file` refactor moves existing
crate usage to new call sites, which is fine from a supply chain
perspective.

No other external dependencies introduced. `serde`, the session backend,
the gate evaluator, and the template compiler are all pre-existing koto
code.

### Data Exposure
**Applies:** Yes

The design extends several data-bearing surfaces that need a quick audit
for sensitive content leakage:

- **Per-child `reason` field in `koto status` batch output.** Decision 6
  shows `{"outcome": "failure", "reason": "tests failed"}`. The design
  does not say where `reason` comes from -- the most plausible sources
  are (a) a context key the child wrote before terminating, (b) a
  failure-state name, or (c) stderr from a failed `default_action`. If
  it's (c), stderr can contain file paths, env var values, stack traces,
  and secrets. The design should specify that `reason` is pulled from a
  named context key (e.g., `failure_reason`) written explicitly by the
  child, not from stderr scraping, so template authors have full control
  over what leaks to observers. Same concern applies to the
  `skipped_because` field, though that one is just a task name and is
  safe by construction.

- **`submitter_cwd` in `EvidenceSubmitted` events.** Captured via
  `std::env::current_dir()` at submission time. This is a local
  filesystem path under the user's home directory. On shared systems
  it could reveal a username or directory structure. On cloud-sync
  backends, the path gets uploaded with the event log. For koto's
  personal/team threat model this is low-severity (the user is
  uploading their own path to their own bucket), but worth noting that
  the cwd is now persisted in the event log where it wasn't before. If
  a user shares a koto state file as a bug report, they may inadvertently
  share their home directory layout. The design should consider whether
  to store only the basename or a relative-to-repo-root form, or at
  least document the exposure.

- **`template_source_dir` in `StateFileHeader`.** Same concern as
  `submitter_cwd`: a new absolute path appears in the header. Since
  state files can be shared for debugging, this persists local path
  information. Lower-severity than today's behavior because headers
  already include absolute paths to cached template JSON, so there's
  precedent -- but it's an incremental exposure worth noting.

- **Evidence task list persisted in event log.** The full task list
  (including agent-supplied `vars`) lands in an `EvidenceSubmitted`
  event and stays there forever (append-only). If the agent includes a
  secret as a var value (e.g., a token intended to be passed to a
  child's `default_action`), that secret is now in the on-disk event
  log and, if cloud sync is on, in the sync bucket. This is identical
  to today's `koto init --var KEY=SECRET` behavior (the var is logged
  in `WorkflowInitialized`), so it's not a regression, but the design
  should remind template authors that secrets in vars persist.

### Content Governance
**Applies:** Yes

The design doc is in the public `koto` repo. I scanned for internal
references: it mentions `tsukumogami/shirabe#67` (public repo, public
issue number), issue #129 (public koto issue), no competitor names, no
internal tooling commands, no private business rationale. The "Writing
Style" concerns from CLAUDE.md are generally observed (I noticed one
instance of "comprehensive" -- actually, I did not find any banned
words on a quick scan). Content governance is clean.

One minor item: the design references `wip/research/` artifacts and
`wip/design_batch-child-spawning_decision_*_report.md` files. Per
CLAUDE.md, `wip/` must be empty before merge, so any file cross-references
that end up baked into the merged design doc will dangle after cleanup.
This is a hygiene concern, not a security concern, but worth flagging to
the doc author before the PR lands.

## Recommended Outcome

**OPTION 2 - Document considerations:** Add a "Security Considerations"
subsection under Consequences (or as a standalone top-level section
before Consequences) with the following draft content:

```markdown
## Security Considerations

Koto's threat model is a local-user tool for personal or small-team use.
Agents submitting evidence are trusted collaborators, not anonymous
attackers. Templates are authored by developers, not end users. This
section documents the security-relevant surface added by batch child
spawning within that model.

### Trust boundaries

- **Task lists are agent-supplied.** The `--with-data @file.json` input
  carries `template` paths and `vars` values chosen by the submitting
  agent. An agent with evidence-submission access can point `template`
  at any template file readable by the invoking user, so evidence
  submission is a privilege equivalent to template authoring for the
  purpose of spawning children. Treat the agent as a trusted
  collaborator; do not use batch spawning to sandbox untrusted input.

- **`vars` have the same trust level as `--var` flags on `koto init`.**
  Agent-supplied `vars` are interpolated into the child's
  `resolve_variables()` and may land inside `default_action` shell
  strings. Template authors must quote variable expansions exactly as
  they would for `koto init --var` inputs. Do not place secrets in
  `vars` unless you are comfortable with them being persisted in the
  append-only event log (and, if cloud sync is enabled, uploaded to
  the sync bucket).

### Resource bounds

- **Task list size.** `--with-data` is capped at 1 MB of resolved
  content. The scheduler additionally enforces soft limits on task
  count (recommended: <= 1000 tasks per batch) and edge count
  (recommended: <= 10 `waits_on` entries per task). Larger batches
  incur quadratic-ish cost on every `koto next` tick because the
  scheduler re-classifies all tasks plus calls `backend.list()` on
  every invocation.

- **Retry throttling.** `retry_failed` submissions are not rate-limited.
  Each retry appends a `Rewound` event per targeted child plus a
  clearing evidence event on the parent, so state files grow linearly
  with retries. Self-DoS by rapid retry is possible but limited to the
  invoking user's own workflow.

### Path resolution and traversal

- **Relative `template` paths are resolved against the parent's
  canonicalized `template_source_dir` first, then against
  `submitter_cwd`.** The scheduler does not reject `..` segments --
  koto treats the invoking user as trusted and does not enforce a
  sandbox on template reads. Users sharing a machine should avoid
  running koto as a more-privileged account against task lists
  produced by a less-privileged account.

### Persisted path information

- **New fields carry local absolute paths.** `StateFileHeader` gains
  `template_source_dir` and `EventPayload::EvidenceSubmitted` gains
  `submitter_cwd`. Both are absolute paths under the user's home
  directory and are persisted in state files (and, if cloud sync is
  enabled, uploaded). Users sharing state files for debugging should
  be aware that directory structure will be visible in the shared
  file. Future work may redact or relativize these paths.

### Per-child `reason` field

- **The `reason` field in batch output is sourced from an explicit
  context key written by the child (e.g., `failure_reason`), not from
  scraped stderr.** Template authors writing failure-state handlers
  should write a sanitized message to this context key rather than
  echoing raw tool output, to avoid leaking paths, env var values, or
  secrets in observer-visible output.
```

The design also needs two small clarifying edits outside the new
subsection:

1. In **Decision 4** (child template path resolution), note whether
   `..` traversal is permitted in relative paths, to remove ambiguity
   for implementers.

2. In **Decision 6** (batch observability surface), specify the source
   of the per-child `reason` field. The current doc implies but does not
   state that it's a named context key.

Both are documentation-only edits; no design change is required.

## Summary

The batch-child-spawning design is sound for koto's local-user threat
model and introduces no new classes of vulnerability: the only new
crate usage (`tempfile`) is pre-existing, the session-directory
assumptions match current practice, and the appended cwd/template-dir
fields are incremental exposures rather than novel ones. The main gaps
are documentation-level: the design should make trust boundaries
explicit (agent-submitted `template`/`vars` have the same authority as
template-author input), document soft resource bounds for task lists
and retries, clarify that `..` traversal and the `submitter_cwd`
fallback are intentional within a trusted-local model, and specify that
the per-child `reason` field must come from an explicit context key,
not scraped stderr, to prevent accidental secret leakage in batch
observability output.
