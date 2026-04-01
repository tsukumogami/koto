# Security Review: Gate Override Mechanism Design

**Document reviewed:** `docs/designs/DESIGN-gate-override-mechanism.md`
**Source files consulted:** `src/cli/mod.rs`, `src/engine/advance.rs`, `src/engine/persistence.rs`
**Date:** 2026-04-01

---

## Findings by Threat Area

### 1. Input validation

**Gap identified.** The design states that `--rationale` and `--with-data` are both subject to the
1MB size limit (R12), and the CLI handler for `handle_overrides_record` lists "validate rationale
length <= 1MB" as step 4 in the processing sequence. However, the design does not specify what
happens when `--with-data` is provided: is it counted separately toward 1MB, or combined with
`--rationale`? The existing `handle_next` handler enforces `MAX_WITH_DATA_BYTES` on the raw string
length of `--with-data` alone, before parsing; the design implies the same limit applies here but
does not spell out whether `--rationale` has its own independent 1MB check or shares a combined
budget with `--with-data`. For `koto decisions record`, the entire `--with-data` payload is capped
at 1MB (covering both `choice` and `rationale` in one JSON object). The new `overrides record`
command splits these into separate flags, creating ambiguity about enforcement order and totals.
Beyond size, there is no bounded character set or structural constraint on `--rationale`; a
null byte or control character would be stored verbatim. This is low-risk given the rationale is
only re-serialized as a JSON string field, but the design should explicitly state whether any
character normalization or escaping happens at persistence time.

### 2. Injection

**Adequate.** The design is explicit that `--rationale` is stored verbatim in the event log and
"never executed or evaluated as code." The advance loop reads overrides via `derive_overrides`,
which returns `GateOverrideRecorded` events. The `override_applied` value from the event is
injected directly into `gate_evidence_map` as a `serde_json::Value`, and the `rationale` field
never enters the evidence map or transition routing logic. There is no downstream `eval`, shell
expansion, or template substitution operating on these fields. The `--with-data` value, once it
passes schema validation, is stored as a typed `serde_json::Value` and flows only through the
evidence injection path, which treats it as data. No injection path exists in the current design.

### 3. Namespace collision

**Gap identified.** The design reserves the `gates` top-level key in agent evidence submissions
by checking `obj.contains_key(GATES_EVIDENCE_NAMESPACE)` in `handle_next`. This check is a
shallow key presence test on the top-level object. It does not prevent an agent from using a
key like `"gates.ci_check"` as a flat top-level field (dot-literal key), which could collide
with the dot-path traversal logic in transition `when` clauses if the resolver interprets
`"gates.ci_check"` as a path. Whether this is a real risk depends on the implementation of the
dot-path helper (Issue #3/`src/engine/substitute.rs`), but the design does not address flat
dot-literal key collisions. Additionally, the design calls out that `koto context set` is
excluded because context and evidence are "structurally separate namespaces." This is correct
for the existing context store, but the design should confirm whether any other evidence
namespaces (e.g., `_koto`, `meta`) are or should be reserved, since the namespace reservation
discussion focuses exclusively on `gates`.

### 4. Audit trail integrity

**Gap identified.** The design specifies that `actual_output` is read from the most recent
`GateEvaluated` event for the named gate in the current epoch, so an agent cannot fabricate the
gate's historical output. However, the design permits calling `koto overrides record` for a gate
that was never evaluated in the current epoch: "If no `GateEvaluated` event exists for the named
gate, `actual_output` is `null` and the override is still recorded." This means an agent can
record an override for a gate that does not exist in the current state's template, a gate in a
different state entirely, or a hypothetical gate name. The resulting `GateOverrideRecorded` event
would have a `gate` field that does not match any gate the advance loop consults, so the override
has no effect on routing -- `derive_overrides` filters by state as well as epoch. But the event
still appears in `koto overrides list` output, which could mislead a human reviewer into thinking
a real gate was overridden. The design should specify that `handle_overrides_record` validates
the named gate exists in the current state's template before appending the event.

### 5. Event log tampering

**Adequate.** The design relies on the existing append-only JSONL state file. Override events
are appended using the same `append_event` path as all other events, which assigns monotonically
increasing sequence numbers and calls `sync_data()` after each write. The persistence layer
validates sequence continuity on read (gaps produce a `state_file_corrupted` error). Rewind
starts a new epoch, making prior overrides invisible to `derive_overrides` without deleting them
-- they remain in `derive_overrides_all`. There is no replay or deduplication vulnerability
because `derive_overrides` returns all `GateOverrideRecorded` events in the epoch scoped to the
current state: multiple records for the same gate are all returned, and the advance loop applies
the last one (or all of them -- the design does not specify deduplication behavior when multiple
overrides exist for the same gate in the same epoch, but the worst case is the last one wins,
which is the expected behavior for idempotent overrides). The design does not introduce any new
integrity surface beyond what already exists for decisions.

### 6. Cross-epoch leakage

**Adequate.** The `derive_overrides` function mirrors `derive_decisions` exactly. Both functions
find the epoch boundary by scanning backwards for the most recent `Transitioned`,
`DirectedTransition`, or `Rewound` event whose `to` field matches the current state, then return
only events after that boundary that carry the matching state field. This correctly isolates each
epoch: an override recorded in epoch N becomes invisible to `derive_overrides` once the workflow
transitions to a new state (epoch N+1), because the epoch boundary index advances. Rewind also
creates a new epoch boundary, clearing any overrides from the rewound epoch. The
`derive_overrides_all` function intentionally spans epochs for audit purposes (`koto overrides
list`) and is not used by the advance loop. No leakage path exists in the described design.

### 7. Schema validation bypass

**Gap identified.** The design specifies that `--with-data` provided to `koto overrides record`
is validated against the gate type's schema (R5). However, the design does not describe what
that schema validation actually checks. Built-in defaults for the three known gate types are
`{"exit_code": int, "error": string}` for `command` and `{"exists": bool, "error": string}` /
`{"matches": bool, "error": string}` for context gates. If the schema validator only checks that
required keys are present without enforcing types (e.g., `exit_code` must be an integer, not a
string or a deeply nested object), an agent could supply `{"exit_code": {"$ref": "..."}, "error":
""}` that passes key-presence validation. The downstream effect is that `when` clauses evaluating
`gates.ci_check.exit_code == 0` would fail to match unexpectedly, stalling the workflow. The
existing `validate_evidence` in `src/engine/evidence.rs` does enforce field types (string,
number, boolean, enum), but the design does not confirm that override schema validation reuses
this same mechanism or implements equivalent type enforcement. This should be explicitly stated.
Additionally, for `override_default` values declared in the template (not supplied via
`--with-data`), there is no mention of schema validation at template compile time -- a template
author who declares a structurally invalid `override_default` would not discover the error until
`koto overrides record` is called.

### 8. Size limit enforcement

**Gap identified.** The design states that "rationale and `--with-data` payloads are subject to
the same 1MB size limit as other `--with-data` payloads." In the existing `handle_decisions_record`
handler, the single `--with-data` string (containing both `choice` and `rationale` as JSON
fields) is checked against `MAX_WITH_DATA_BYTES` as one unit. In the new `overrides record`
command, `--rationale` and `--with-data` are separate CLI flags, each potentially up to 1MB.
The design's `handle_overrides_record` steps list "validate rationale length <= 1MB" and
"parse and validate `--with-data` JSON if provided" as separate steps, implying independent limits.
If both are enforced independently, a single `koto overrides record` call could write up to ~2MB
to the state file for one event (1MB rationale + 1MB override value + field names and metadata).
The design should clarify whether the limits are independent or combined, and if independent,
whether the combined write size is acceptable given the existing mitigation note that gate counts
per state are typically 1-5. This is not a critical vulnerability but a design precision gap
that could cause uneven behavior compared to other commands.

---

## Summary

No critical vulnerabilities were found. The design correctly handles injection, cross-epoch
leakage, and event log integrity. Four gaps were identified that should be addressed before
implementation:

1. **Phantom gate overrides** (Threat 4): `handle_overrides_record` should validate that the
   named gate exists in the current state's template before appending a `GateOverrideRecorded`
   event. Recording overrides for nonexistent or wrong-state gates is harmless to routing but
   pollutes the audit trail visible in `koto overrides list`.

2. **Schema validation for `--with-data` and `override_default`** (Threat 7): The design should
   confirm that override value schema validation enforces field types (not just key presence), and
   should specify that `override_default` values declared in templates are validated at compile
   time rather than deferred to override-record time.

3. **Combined size limit ambiguity** (Threat 8 / Threat 1): The design should explicitly state
   whether `--rationale` and `--with-data` limits are enforced independently (up to 2MB per
   event) or combined (shared 1MB budget). The current phrasing implies independent limits,
   which diverges from the `decisions record` pattern.

4. **Namespace dot-literal collision** (Threat 3): The design should address whether a flat
   key named `"gates.something"` in agent evidence could collide with dot-path traversal in
   `when` clause evaluation, or confirm that the dot-path resolver treats `"gates"` as a
   top-level object key only.
