# Security Review: DESIGN-gate-backward-compat

**Document reviewed:** `docs/designs/DESIGN-gate-backward-compat.md`
**Review date:** 2026-04-02
**Reviewer role:** Security researcher

---

## Overview

This design introduces two isolated changes to the koto workflow engine:

1. A new `legacy_gates: true` frontmatter field that compiler and engine read to
   conditionally suppress errors and alter evidence injection behavior.
2. A guarded merge step in the advance loop that excludes gate output from the
   resolver's evidence map for legacy-mode states.

Both changes are internal to koto's own processing pipeline. The design does not
introduce new network communication, external service calls, or new user-facing
input surfaces beyond the YAML frontmatter field already accepted by the template
compiler.

---

## Dimension 1: External artifact handling

**Applies:** Partially — the design processes a new YAML frontmatter field.

**Assessment:** The `legacy_gates` field is parsed from YAML frontmatter as an
`Option<bool>`. YAML boolean parsing is well-bounded: the field either deserializes
to `true`, `false`, or absent (treated as `false` via `#[serde(default)]`). There
is no string interpolation, no shell expansion, and no path construction from this
value. The compiler and engine use the field only as a branch condition — they read
a boolean and execute one of two static code paths.

No external URLs, binaries, or scripts are fetched or executed as part of this
feature. Gate evaluators already exist in the engine and are not modified here.
The design explicitly notes that `koto init` does not run strict gate validation;
this means the new field is never used as an input to a validation step that could
be weaponized by a malformed template during init.

**Severity:** Negligible. A boolean field parsed by serde has no meaningful attack
surface beyond what any other frontmatter field already presents.

**Mitigations already present:** serde's typed deserialization rejects anything
that cannot represent a boolean. No additional validation is required.

**Remaining gap:** None identified.

---

## Dimension 2: Permission scope

**Applies:** No meaningful new permissions introduced.

**Assessment:** The design makes no filesystem, network, or subprocess changes
beyond what the existing compiler and engine already perform. Specifically:

- The compiler reads template files and writes compiled output — unchanged behavior.
- The engine reads state files and writes transitions atomically — unchanged behavior.
- The `koto init` warning writes to stderr only. Stderr output requires no elevated
  permissions and carries no user data.
- `has_gates_routing` is hoisted earlier in the advance loop but does not gate or
  unlock any I/O operation. It conditions a map insertion — no file handle, socket,
  or subprocess is involved.

There is no privilege escalation risk. The `legacy_gates` flag cannot be used by a
template to request broader permissions, invoke system calls, or expand the engine's
ambient authority.

**Severity:** None.

**Mitigations:** Not applicable.

**Remaining gap:** None.

---

## Dimension 3: Supply chain or dependency trust

**Applies:** No new dependencies introduced.

**Assessment:** The design adds no new crates, external libraries, or build-time
dependencies. Both changes land in existing source files (`compile.rs`, `types.rs`,
`mod.rs`, `advance.rs`) using types and patterns already present in the codebase
(`Option<bool>`, `serde(default)`, boolean conditionals, `eprintln!`).

Template authors supply the `legacy_gates` field in their own template files. The
trust model here is the same as for all other frontmatter fields: templates are
authored by the team or by open-source contributors who submit them through the
normal review process. There is no automated ingestion of templates from untrusted
sources as part of this feature.

The design notes explicitly that `koto template compile` runs in a CI context
controlled by the template author, not on untrusted runtime input. This is the
correct trust boundary for a compile-time field.

**Severity:** None.

**Mitigations:** Not applicable.

**Remaining gap:** One observation worth tracking (not a blocker): if koto ever
introduces a template registry that fetches and compiles remote templates
automatically, every frontmatter field — including `legacy_gates` — should be
reviewed for whether a malicious template could use it to alter engine behavior in
a way the user did not intend. That risk does not exist today because template
ingestion is always a human-in-the-loop operation.

---

## Dimension 4: Data exposure

**Applies:** Partially — the evidence exclusion change affects what data the
resolver receives.

**Assessment:** This dimension covers two sub-questions: what data the feature
accesses, and whether any data could be exposed to unintended parties.

**Data accessed:** The `legacy_gates` boolean is the only new piece of data read by
this feature. It is not logged, transmitted, or stored beyond its in-memory use
during compilation and engine execution. The `koto init` warning message is
hardcoded — it contains no template metadata, user identity, file paths, or
environment variables.

**Data exposure direction:** The evidence exclusion change (Phase 3) moves in the
direction of reducing data visibility, not expanding it. For legacy states,
`gates.*` keys are withheld from the evidence map passed to `resolve_transition`.
This means the resolver in legacy mode sees less data than it did before — gate
output that was previously injected (but ignored) is now excluded entirely. There
is no mechanism by which this change could cause gate output to reach an external
party or be logged in a place it was not logged before.

A subtlety worth confirming: the design states that gate output is still available
for `GateEvaluated` events and the `GateBlocked` stop reason's `failed_gates`
field, since those paths use `gate_results` directly rather than the merged
evidence map. Reviewers should verify that `GateEvaluated` events do not serialize
and transmit gate output to any telemetry endpoint without user consent. This is
not introduced by this design — it is an existing behavior — but it becomes
relevant if gate output carries sensitive values from the workflow environment.

**Severity:** Low, contingent on existing telemetry posture. The design itself
introduces no new exposure.

**Mitigations already present:** The evidence exclusion change reduces the data
surface for legacy states. No new transmission paths are added.

**Remaining gap:** Confirm that `GateEvaluated` event emission does not forward
gate output to external telemetry without user consent. This is a pre-existing
concern, not introduced here, but worth documenting.

---

## Summary and recommended outcome

### Recommended Outcome: Option 1 — Approve as designed

The design is sound from a security standpoint. Neither the frontmatter field nor
the evidence exclusion introduces new attack surface, expands permissions, adds
dependencies, or exposes user data. The one pre-existing concern — whether
`GateEvaluated` events forward gate output to external telemetry — is worth a
one-time audit but is not a blocker for this design and was not introduced here.

The explicit opt-in model (`legacy_gates: true` in frontmatter) is strictly better
than the alternatives considered (CLI flags, auto-detection with warnings) from a
security and auditability standpoint: the legacy posture of a template is visible
in the template itself, reviewable in pull requests, and not influenced by CI
invocation flags that could be silently broadened.

No mitigations are required before landing. The pre-existing telemetry audit
recommendation can be tracked as a separate, independent concern.
