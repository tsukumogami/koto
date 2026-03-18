# Phase 3 Research: Integration Runner

## Questions Investigated
- What does the `IntegrationInvoked` event payload look like? What fields does it carry?
- What does the `NextResponse::Integration` variant carry? How does it differ from `IntegrationUnavailable`?
- How should integrations be discovered/configured? (user config file? template field? environment?)
- What should the integration runner's interface look like?
- What happens when integration output needs to be fed into transition resolution?
- How does the upstream design specify integration behavior?

## Findings

### IntegrationInvoked Event Payload

Defined in `src/engine/types.rs` (lines 45-49):

```rust
IntegrationInvoked {
    state: String,
    integration: String,
    output: serde_json::Value,
}
```

Three fields: the state name where the integration was invoked, the integration name (matching the template's `integration` field string), and the output as arbitrary JSON. The event type name serializes as `"integration_invoked"`. Full round-trip deserialization is implemented via `IntegrationInvokedPayload` helper struct.

This event type exists in the taxonomy but is never constructed anywhere in the codebase -- no code path creates an `EventPayload::IntegrationInvoked`.

### NextResponse::Integration vs IntegrationUnavailable

Both are defined in `src/cli/next_types.rs`.

**Integration** (lines 28-34): Returned when an integration ran successfully.
```rust
Integration {
    state: String,
    directive: String,
    advanced: bool,
    expects: Option<ExpectsSchema>,
    integration: IntegrationOutput,  // { name: String, output: serde_json::Value }
}
```

**IntegrationUnavailable** (lines 35-41): Returned when an integration is declared but cannot run.
```rust
IntegrationUnavailable {
    state: String,
    directive: String,
    advanced: bool,
    expects: Option<ExpectsSchema>,
    integration: IntegrationUnavailableMarker,  // { name: String, available: bool }
}
```

Key differences:
1. `Integration` carries `IntegrationOutput` with a `name` and `output` (the actual result as `serde_json::Value`)
2. `IntegrationUnavailable` carries `IntegrationUnavailableMarker` with a `name` and `available: false` flag
3. Both serialize with `"action": "execute"` -- the agent can distinguish them by checking whether `integration.output` or `integration.available` is present
4. Both carry `expects: Option<ExpectsSchema>` -- meaning a state with an integration can also have an `accepts` block, allowing the agent to submit evidence after receiving integration output
5. `IntegrationUnavailable` exits with code 1 (transient/retryable per `NextErrorCode::IntegrationUnavailable`)

Currently, only `IntegrationUnavailable` is ever returned. The `dispatch_next` function in `src/cli/next.rs` (line 75) unconditionally returns `IntegrationUnavailable` when a state has an integration field, with the TODO comment: `// TODO(#49): Add availability check and Integration branch when the integration runner is implemented.`

### Integration Discovery and Configuration

The template declares an integration as a bare string on a state:

```yaml
states:
  analyze:
    integration: delegate_review
```

This compiles to `TemplateState.integration: Option<String>` (in `src/template/types.rs`, line 43). The compile step just passes it through (`source_state.integration.clone()` in `src/template/compile.rs`, line 179). There is no validation that the integration name resolves to anything -- the compiler accepts any string.

The upstream design (DESIGN-unified-koto-next.md) specifies the discovery model in two places:

1. **Security section**: "Integration names must resolve from a closed set (project configuration or plugin manifest), not from arbitrary strings in template files. A template declaring `integration: some-name` tells koto to route to the configured handler for `some-name`; the actual command or process is defined in user or project configuration, not in the template itself."

2. **Phase 2 deliverables**: "Graceful degradation behavior: a missing integration config entry is not a template load-time error; `koto next` degrades to returning the directive without integration output (PRD R17)"

No configuration file format, lookup path, or registry mechanism exists yet. The design says configuration should be project-scoped or plugin-manifest-scoped, but leaves the exact format to the tactical sub-design.

### Integration Runner Interface

Nothing is implemented. The upstream design's data flow section specifies the advancement loop behavior:

```
if integration configured: invoke runner, append integration_invoked -> stop
```

This means:
- The integration runner is invoked during the advancement loop, not as a separate CLI call
- After invocation, an `integration_invoked` event is appended to the log
- The loop stops after invoking an integration (it's a stopping condition)
- The response includes the integration output for the agent to use

The runner's logical interface, derived from the event payload and response types:
- **Input**: integration name (`String`) + current state context
- **Output**: `serde_json::Value` (arbitrary JSON) or an error indicating unavailability
- **Side effect**: none (the caller appends the event)

### Integration Output and Transition Resolution

The design's data flow shows integration invocation as a stopping condition -- the loop stops, returns the output to the agent, and the agent is expected to submit evidence (via `--with-data`) in a subsequent call if the state has an `accepts` block.

Integration output does NOT directly feed into transition resolution. The flow is:
1. Agent calls `koto next`
2. Engine hits integration state, invokes runner, appends `integration_invoked` event, stops
3. Response includes both `integration.output` and `expects` (if the state has `accepts`)
4. Agent reads integration output, reasons about it, then calls `koto next --with-data` with evidence
5. Evidence is validated against `accepts`, transition `when` conditions are evaluated

This two-step flow means integration output is informational for the agent, not machine-evaluated for routing. The agent interprets the output and translates it into structured evidence that the `when` conditions can match against.

### Upstream Design Specifications

The DESIGN document specifies:
- Integration is one of the six event types in the taxonomy
- Integration runner interface and invocation is a Phase 4 (auto-advancement engine) deliverable
- The `integration` field on a template state is a string tag; routing lives in user config
- Integration output in `integration_invoked` events should be validated against size limits, treated as untrusted, and subject to schema validation
- Interpolation injection risk: if integration output is interpolated into directive text, it must be sanitized/escaped
- Integration unavailability is transient (exit code 1), not a caller error

## Implications for Design

1. **Configuration system needed**: The auto-advancement engine needs a way to resolve integration names to executable runners. The design says "project configuration or plugin manifest" but no format exists. This is the biggest open question for the integration runner -- it's not just a function signature, it's a discovery/config system.

2. **Runner is a closure, not a subprocess (necessarily)**: Since `advance_until_stop()` takes I/O closures, the integration runner fits naturally as one of those closures. The CLI handler would construct the runner closure from configuration and inject it. The engine itself doesn't need to know about config files or subprocess spawning.

3. **Two-phase interaction model**: Integration output doesn't drive transitions directly. The engine stops after invocation, returns output to the agent, and waits for evidence submission. This simplifies the engine -- integration is just another stopping condition, not a transition evaluator.

4. **Event must be appended before returning**: The engine must append `integration_invoked` to the log before returning the response. On subsequent calls, replay should recognize that the integration was already invoked for this state visit (epoch boundary) to avoid re-invocation.

5. **Expects coexists with integration output**: A state can have both `integration` and `accepts`. The response carries both `integration.output` and `expects`. This is already modeled correctly in `NextResponse::Integration`.

6. **Unavailability is the default**: Until the runner is implemented, every integration state returns `IntegrationUnavailable`. The graceful degradation path (return directive + expects without integration output) is the current behavior.

## Surprises

1. **No re-invocation guard**: The design doesn't explicitly address what happens if an agent calls `koto next` again on a state where `integration_invoked` was already appended. The epoch boundary rule scopes evidence but doesn't mention integration events. The engine will need to check whether an `integration_invoked` event already exists in the current epoch for the current state to avoid re-running the integration. Without this, every `koto next` call on an integration state would re-invoke the runner.

2. **Integration output is opaque JSON**: There's no schema for integration output -- it's `serde_json::Value`. The design mentions "subject to schema validation if the integration is expected to return structured data" but provides no mechanism for declaring that schema. Size limits are mentioned but not specified.

3. **Security constraint is strong**: The design explicitly prohibits resolving integration commands from template strings. The template only carries a name; the command must come from user/project config. This means the config system is security-critical, not just convenience.

4. **The `available: false` marker is redundant**: `IntegrationUnavailableMarker` has both `name` and `available: bool`, but `available` is always `false` (if it were `true`, you'd return `Integration` instead). The field exists for JSON clarity -- the agent checks `integration.available` to distinguish the two cases without needing to check for `integration.output`.

## Summary

The integration runner infrastructure is well-scaffolded in types but completely unimplemented. `IntegrationInvoked` events, `NextResponse::Integration`, and `IntegrationUnavailableMarker` all exist with correct serialization, but no code path constructs them beyond the `IntegrationUnavailable` fallback. The biggest open design question is the configuration system for resolving integration names to executable runners -- the upstream design mandates this lives in project config (not templates) for security reasons, but no config format or lookup mechanism exists. The engine-level integration is straightforward: integration is a stopping condition in the advancement loop, output is returned to the agent as informational context alongside `expects`, and the agent submits evidence in a follow-up call. One gap the architect should address: re-invocation prevention -- the engine needs to detect that an `integration_invoked` event already exists in the current epoch to avoid running the integration again on repeated `koto next` calls.
