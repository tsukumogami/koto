# Lead: How should koto next accept state-dependent data?

## Findings

### The tension

Purpose-specific flags (`--transition`, `--response-file`, `--evidence-file`) are self-documenting and shell-completable, but they grow with every new capability. A generic mechanism (`--data <file>` or stdin with JSON) keeps the interface stable but shifts validation to runtime and obscures from the caller what's expected.

### HATEOAS: the state tells you what it needs

The most relevant pattern from REST API design is HATEOAS (Hypermedia As The Engine Of Application State): the current state's response includes links/schemas describing what actions are available and what they expect. The client never hard-codes "what to call next" — it reads the current response and follows what it says.

Applied to koto: `koto next` (read) returns the current directive plus an `expects` field describing what input the current state accepts. The agent doesn't need to know in advance whether to use `--transition` or `--evidence-file` — it reads `expects` from the previous `koto next` call and knows.

```json
{
  "op": "read",
  "state": "implement",
  "directive": "Implement the feature described in the spec.",
  "expects": {
    "type": "transition",
    "targets": ["review", "blocked"]
  }
}
```

```json
{
  "op": "read",
  "state": "audit",
  "directive": "Run a security audit and submit findings.",
  "expects": {
    "type": "evidence",
    "schema": {
      "findings": "string",
      "severity": "enum[low,medium,high,critical]"
    }
  }
}
```

### Generic vs. typed flags: the real trade-off

**Typed flags** (`--transition review`):
- Self-documenting: `koto next --help` shows what's possible
- Shell-completable
- Grow with capabilities: adding evidence submission requires `--evidence-file`
- Hard-code knowledge of action types into the CLI interface

**Generic input** (`--data /tmp/input.json` or stdin):
- Interface stays constant as capabilities grow
- Validation entirely at runtime (koto reads the file and validates against current state)
- Agent must read `expects` from prior output to know what JSON shape to produce
- Less discoverable for humans, but agents don't use `--help`

**Hybrid approach** (recommended by research):

Use a single generic submission flag (`--submit <file>`) where the file contains a typed JSON envelope:

```json
{
  "type": "transition",
  "target": "review"
}
```

```json
{
  "type": "evidence",
  "findings": "No critical issues found.",
  "severity": "low"
}
```

The `type` field in the submitted JSON lets koto validate against the current state's expectations without needing per-type CLI flags. The agent constructs this JSON based on the `expects` field from the prior `koto next` read.

### How well-known CLIs handle this

**git interactive rebase**: Uses a file-based protocol — the rebase instructions file is the "data submission". The command reads the file to know what to do. Simple but brittle (plain text format).

**Terraform plan/apply**: Plan file is a typed artifact; `apply` validates it against current state. The plan file format is structured JSON internally.

**JSON Schema in API responses**: Industry standard for communicating "what I accept" — embed a JSON Schema fragment in the response, and the client validates locally before submitting. This is what the `expects` field in koto's output would do.

**clig.dev guidelines**: Recommend against interactive prompts in automatable CLIs. For machine-to-machine communication, prefer JSON in / JSON out with well-defined schemas.

### Implications for `koto next` interface

The concrete recommendation emerging from research:

- `koto next` (no args): read current state, return directive + `expects`
- `koto next --submit <file>`: submit data from file, validate against current `expects`, advance state
- `koto next --submit -`: read submission from stdin

The `expects` schema in the read response is what lets agents know what to put in the submission file. No per-capability flags needed. The interface is stable as new state types are added — only the `expects` schemas change.

## Implications

koto's output schema needs an `expects` field on every read response. This is the mechanism that makes generic data submission work without coupling the CLI to the set of known action types. Agents treat `koto next` (no args) as a schema discovery call before constructing their submission.

## Surprises

The HATEOAS pattern from REST APIs turns out to be directly applicable to CLI design. The insight is that the same principle — "the current response tells you what's valid next" — works whether the client is a browser following hyperlinks or an agent parsing JSON. koto's design already follows this spirit (the directive tells the agent what to do), it just needs to extend it to input discovery.

## Open Questions

- Should `expects` be a full JSON Schema fragment or a simpler custom format? Full JSON Schema enables local validation but adds complexity; a simple custom format is easier to implement.
- Should the submission envelope include a `state` field to prevent stale submissions (submitting data intended for a state the workflow has since advanced past)?

## Summary

The HATEOAS pattern — where the current state's response describes what input it accepts — resolves the generic-vs-typed tension: `koto next` (read) returns an `expects` schema, and `koto next --submit <file>` accepts a typed JSON envelope validated against that schema, keeping the CLI interface stable as new action types are added. This mirrors how AI agents themselves work (tool schemas are discovered from responses, not hardcoded), making it natural for agent consumers. The key open question is whether `expects` should be a full JSON Schema fragment or a lightweight custom format.
