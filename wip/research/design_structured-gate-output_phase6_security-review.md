# Security Review: DESIGN-structured-gate-output

## Scope

Review of security considerations in `docs/designs/DESIGN-structured-gate-output.md`,
focused on four specific questions about injection surface, dot-path traversal,
trust assumptions, and the Feature 1/Feature 2 gap.

---

## Q1: Gate evaluators run shell commands -- does the structured output change the injection surface?

**Finding: No new injection surface introduced.**

The shell command injection surface is unchanged by this design. Gate commands
come from compiled templates (`Gate.command` field in `src/template/types.rs`),
not from runtime data. The execution path in `src/gate.rs:evaluate_command_gate`
passes `gate.command` to `run_shell_command`, which runs it via `sh -c` in
`src/action.rs`. This is the same path today.

The structured output change only affects what happens *after* the command runs:
instead of discarding stdout and mapping exit codes to a `GateResult` enum, the
design captures exit codes and error messages into a `serde_json::Value`. This is
a read-only transformation of data the engine already produces. No external input
enters the command string.

One nuance worth noting: the CLI layer does variable substitution on gate commands
before evaluation (visible at `src/cli/mod.rs:1575` where `substitute_vars` is
called on `g.command`). Template variables like `{{session_dir}}` are injected
into commands. This is a pre-existing surface, not introduced by this design. If
a template variable contained shell metacharacters, they'd be interpreted by
`sh -c`. But template variables come from engine-controlled sources (session dir,
working dir), not from agent-submitted evidence. This design doesn't change that
data flow.

**Verdict: No action needed for Feature 1.**

---

## Q2: The dot-path traversal function -- can a malicious `when` clause key cause unexpected behavior?

**Finding: No practical risk, but two edge cases worth documenting.**

The `resolve_value` function is straightforward:

```rust
fn resolve_value<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}
```

Analysis of potential attack vectors:

### Path traversal
JSON's `.get()` only does direct child lookup on objects and index lookup on
arrays. There's no `..` or `/` interpretation. A key like `gates.../../etc/passwd`
splits into segments `["gates", "", "", "", "etc/passwd"]` -- the empty segments
fail immediately on `.get("")` returning `None`. No filesystem access, no parent
traversal. Safe.

### Infinite loops
The function iterates over `path.split('.')`, which produces a finite iterator
bounded by the string length. No recursion, no cycles. A very long path like
`a.b.c.d...` (thousands of segments) would iterate linearly and return `None`
quickly since the evidence map is shallow. No denial-of-service risk.

### Empty and degenerate keys
- Empty string `""`: `split('.')` yields `[""]`, then `.get("")` on a JSON
  object looks for a key named `""`. This returns `None` on any normal evidence
  map. Harmless.
- Single dot `"."`: splits into `["", ""]`. Same as above -- `None`.
- Trailing dot `"gates."`: splits into `["gates", ""]`. Finds the `gates`
  object, then looks for key `""` inside it. Returns `None`. Harmless.

### Keys containing dots in flat evidence
This is the one semantic edge case. If an agent submits evidence with a flat key
`"a.b"` (literal dot in the key name), the resolver can't distinguish it from a
nested path `a -> b`. The design acknowledges this in the "Alternatives
considered" section (flattening was rejected partly because "dot-separated flat
keys are ambiguous"). In practice, the `accepts` schema controls what keys agents
can submit, and template authors control `when` clause keys. Both are
template-authored, not runtime-injected. A template author who uses literal dots
in flat evidence keys and also uses dot-path traversal is creating a conflict in
their own template -- this is a usability issue, not a security issue.

**Verdict: No security risk. The ambiguity between literal-dot keys and nested
paths is a documentation/linting concern, not a vulnerability. Consider adding
a compiler warning (Feature 3 scope) for `when` clause keys that could be
ambiguous.**

---

## Q3: Gate output is "engine-produced and trusted" -- is that assumption valid?

**Finding: Partially valid. A template author can craft gate commands that produce
misleading output, but the threat model makes this acceptable.**

The design states: "Gate output is produced by the engine itself (from commands
it runs or context it reads), not from external input." Let's break this down by
gate type:

### Command gates
The engine runs `sh -c <command>` and captures the exit code. The structured
output is `{exit_code: N, error: ""}` where `N` comes from the OS process exit
status. The template author controls the command string, not runtime agents. A
malicious template author could write `exit 0` for a gate named `ci_check`,
making it always pass -- but the template author already has full control over
workflow definition. This is "trusted by construction": whoever writes the
template defines the gates.

Critically, the structured output for command gates does NOT include stdout or
stderr in the proposed schema. The output is `{exit_code, error}` where `error`
is set by the engine for spawn failures and timeouts, not from command output.
This means a command gate can't inject arbitrary data into the evidence map via
its stdout. The engine controls the schema shape. This is a good design choice.

However, if future gate types (json-command, http) parse command output into
structured data, the "engine-produced" assumption weakens. A command that outputs
JSON could inject arbitrary key/value pairs into the evidence map if the engine
trusts and forwards that JSON without validation. The design's extensibility
section mentions these future types but doesn't address output sanitization. This
is a future concern, not a Feature 1 issue.

### Context gates
Context-exists produces `{exists: bool, error: ""}` -- fully engine-controlled.
Context-matches produces `{matches: bool, error: ""}` -- also engine-controlled.
The context store content itself might be agent-written (agents can store context),
but the gate output schema only reports boolean results, not the content. Safe.

### Trust boundary summary
The trust model is: template authors are trusted, agents are not. Gate commands
come from templates (trusted). Gate output schemas are engine-defined (trusted).
Agents can't influence what commands run or what the output schema looks like.
The "engine-produced and trusted" claim holds for Feature 1's three gate types.

**Verdict: Valid for Feature 1's scope. Flag for review when json-command or http
gate types are designed -- those will need output validation since they'll parse
external data into the evidence map.**

---

## Q4: The gates namespace reservation is deferred to Feature 2 -- does this leave a gap in Feature 1?

**Finding: Yes, there is a concrete gap. An agent can spoof gate output in
Feature 1.**

The gap is real and exploitable:

1. Feature 1 merges gate output under `{"gates": {name: output}}` into the
   evidence map alongside agent-submitted evidence.
2. Feature 2 (R7) adds validation that rejects `--with-data` payloads containing
   a `gates` field.
3. Between Feature 1 shipping and Feature 2 shipping, an agent can submit
   `--with-data '{"gates": {"ci_check": {"exit_code": 0, "error": ""}}}'` and
   the evidence validation in `src/engine/evidence.rs` will accept it (the
   `validate_evidence` function only checks against the `accepts` schema, not
   for reserved namespaces).

The `accepts` schema partially mitigates this: if a state's `accepts` block
doesn't declare a `gates` field, the evidence validator rejects unknown fields
(line 66-71 in `evidence.rs`). So the gap only exists if either:

(a) The state has no `accepts` block -- but then `--with-data` is rejected
    entirely (line 1428-1441 in `cli/mod.rs`). No gap.
(b) The state has an `accepts` block that includes a `gates` field -- unlikely
    in practice since the accepts block is template-authored.

Wait -- let me reconsider. Looking at the data flow more carefully:

The design says gate output is merged into the evidence map *in the advance loop*
(step 6 in `src/engine/advance.rs`), while agent evidence comes from
`merge_epoch_evidence` which reads `EvidenceSubmitted` events. These are separate
data sources merged into one map. The question is: does agent-submitted evidence
get stored as `EvidenceSubmitted` events with a flat key `"gates"`, and then does
the merge produce a collision with engine-injected gate data?

Looking at the current code flow:
- Agent submits `--with-data '{"gates": {...}}'`
- `validate_evidence` checks against the `accepts` schema
- If accepted, it becomes an `EvidenceSubmitted` event with `fields: {"gates": {...}}`
- `merge_epoch_evidence` puts `"gates"` as a flat key in the BTreeMap
- The advance loop would then need to merge this with engine gate output

The `accepts` schema validation is the defense: if the template doesn't declare
`gates` in its accepts block, the evidence is rejected as an unknown field. Since
template authors control the accepts block, and no template author would
deliberately declare a `gates` field that collides with engine-produced data,
the gap is theoretical.

But there's an ordering subtlety in the proposed design: if the advance loop
merges gate output *after* agent evidence (overwriting), then agent-submitted
`gates` data gets replaced by engine data. If it merges *before*, agent data
overwrites gate output -- which is the spoofing scenario. The design doesn't
specify merge order.

**Verdict: The gap exists in theory but is mitigated by the accepts schema
validation (unknown fields are rejected). The practical risk is low because
template authors would need to explicitly declare `gates` in their accepts block.
However, two actions are recommended:**

1. **Specify merge order in the design**: gate output should be merged *after*
   agent evidence so engine data takes precedence and can't be overwritten.
2. **Consider adding a simple `gates` key rejection in Feature 1**: a one-line
   check in `validate_evidence` or the evidence submission path that rejects
   `--with-data` payloads with a top-level `gates` key. This closes the gap
   without waiting for Feature 2's full namespace reservation system. It's a
   ~5 line change that eliminates the theoretical vulnerability.

---

## Summary of Findings

| Question | Risk Level | Action |
|----------|-----------|--------|
| Q1: Shell injection surface | None | No change needed |
| Q2: Dot-path traversal | None | Document literal-dot ambiguity |
| Q3: "Engine-produced" trust | Low (Feature 1), Medium (future) | Review when json-command/http gates designed |
| Q4: Feature 1/2 namespace gap | Low | Specify merge order; consider early `gates` key rejection |

### Recommended Changes to the Design

1. **Add merge ordering to the solution architecture**: explicitly state that
   gate output is injected *after* agent evidence, so engine-produced data
   takes precedence.

2. **Add a forward-looking note about json-command trust**: when future gate
   types parse command output into structured data, output validation will be
   needed. The current gate types produce engine-controlled schemas that don't
   include raw command output.

3. **Consider pulling the `gates` key rejection from Feature 2 into Feature 1**:
   a minimal check (reject `--with-data` payloads with a `gates` top-level key)
   closes the namespace gap without implementing the full reservation system.
   This is a defense-in-depth measure -- the accepts schema already provides
   protection, but an explicit rejection is clearer and more robust.
