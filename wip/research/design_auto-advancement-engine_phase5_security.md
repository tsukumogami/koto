# Security Review: auto-advancement-engine

## Dimension Analysis

### Download Verification

**Applies:** No

The auto-advancement engine does not download external artifacts. It operates
entirely on local state files (JSONL event logs) and locally compiled templates.
The integration runner closure is stubbed to return `IntegrationUnavailable` in
this design; when a real integration runner is built (separate issue), that
design will need its own download verification review. Nothing in the current
design fetches URLs, pulls registry data, or retrieves remote content.

### Execution Isolation

**Applies:** Yes

**Risk 1: Gate commands run with full user privileges.**
Gate evaluation (`src/gate.rs`) spawns arbitrary shell commands via `sh -c` with
the user's full permissions. The only isolation is process group separation
(`setpgid(0, 0)`) and a timeout (default 30 seconds). There is no filesystem
sandboxing, no network restriction, and no capability dropping. This is an
existing risk, not introduced by this design, but the auto-advancement loop
amplifies it: a single `koto next` call can now trigger multiple gate
evaluations across multiple states in one invocation, where previously each
gate evaluation required a separate CLI call.

**Severity:** Medium. The threat model assumes trusted templates (plugin-
installed or PR-reviewed), and the upstream design explicitly documents this
assumption. The amplification is linear in the number of auto-advanced states,
not exponential.

**Mitigation:** The upstream design's trust model is sound for the current use
case. The design should document that auto-advancement does not change the gate
trust boundary -- the same template author controls which commands run. If koto
ever loads templates from untrusted sources, gate commands need sandboxing
before auto-advancement makes the problem worse.

**Risk 2: Signal handling window between gate completion and shutdown check.**
The design checks the `AtomicBool` shutdown flag between loop iterations, but a
gate evaluation can block for up to 30 seconds. The design acknowledges this in
"Consequences > Negative" and considers it acceptable. From a security
perspective, this is a denial-of-shutdown concern, not a privilege escalation.
An attacker who controls the template already controls the gate commands, so
delaying shutdown provides no additional attack surface.

**Severity:** Low.

**Risk 3: Advisory flock is not mandatory.**
The concurrent access protection uses advisory file locking (`flock`). Advisory
locks are cooperative -- any process that doesn't call `flock` can still read
and write the state file. This is standard for CLI tools and matches the threat
model (protecting against accidental concurrent `koto next` calls, not
adversarial processes).

**Severity:** Low. This is a correctness mechanism, not a security boundary.

### Supply Chain Risks

**Applies:** No

This design introduces one new dependency: `signal-hook` for SIGTERM/SIGINT
handling. `signal-hook` is a well-established, widely-used Rust crate (the
standard approach for signal handling in the Rust ecosystem). It does not pull
in networking, code generation, or build scripts that execute arbitrary code.
The risk profile is equivalent to other foundational crates already in the
dependency tree (`serde`, `clap`).

The integration runner is a closure interface with a stub implementation. No
external integration binaries are resolved, downloaded, or executed in this
design. The future integration config system (deferred to a separate issue) is
where supply chain concerns for integration runners will surface.

### User Data Exposure

**Applies:** Yes

**Risk 1: Evidence and integration output persisted in plaintext.**
The event log stores `evidence_submitted` fields and `integration_invoked`
output as plaintext JSON. The upstream design already identifies this risk and
requires 0600 file permissions. The existing `persistence.rs` implementation
correctly applies `mode(0o600)` on file creation (lines 15-18 for headers,
lines 59-62 for events). The auto-advancement engine does not change this --
it uses the same `append_event` function via the injected closure.

**Severity:** Low (already mitigated by existing implementation).

**Risk 2: Interpolation injection from evidence into directives.**
The upstream design explicitly flags this: if integration output or evidence
values are interpolated into directive text shown to agents, unsanitized
content could inject instructions. The auto-advancement design does not
specify escaping rules for directive rendering, which the upstream design
requested ("The tactical sub-design for the auto-advancement engine should
specify the escaping rules for directive rendering").

**Severity:** Medium. This is a gap -- the upstream design explicitly asked
this sub-design to address it, and it doesn't.

**Mitigation:** The design should add a section specifying that evidence values
and integration output are never interpolated into directive text by the
engine. The engine returns structured `StopReason` variants with typed fields;
the handler maps these to `NextResponse` JSON. Interpolation, if any, happens
in the controller layer (template directive rendering), which is outside the
scope of this design. The design should state this boundary explicitly and
note that the controller's interpolation logic must escape or quote evidence
values before embedding them in directive strings.

## Recommended Outcome

**OPTION 2 - Document considerations:**

The design should add a Security Considerations section addressing two points:

1. **Gate evaluation amplification.** Auto-advancement can trigger multiple gate
   evaluations per `koto next` invocation. This does not change the trust
   boundary (same template author controls commands), but implementers should be
   aware that the blast radius of a malicious gate command is now wider per CLI
   call. If template loading from untrusted sources is ever added, gate
   sandboxing must be implemented before auto-advancement is enabled for those
   templates.

2. **Directive interpolation escaping.** The upstream design requested that this
   sub-design specify escaping rules for directive rendering. The engine layer
   (`advance_until_stop`) does not perform interpolation -- it returns typed
   `StopReason` variants. Directive interpolation is the controller's
   responsibility (existing `pkg/controller/` code). Evidence values and
   integration output must be treated as untrusted strings in any interpolation
   context. The engine's contract is: `StopReason` fields contain raw,
   unescaped data; the consumer must escape before embedding in rendered text.

The existing state file permission model (0600) and process group isolation for
gates are sufficient for the auto-advancement loop. No new file access patterns
or privilege requirements are introduced.

## Summary

The auto-advancement engine is primarily a control flow mechanism over existing
I/O operations (gate evaluation, event appending) and introduces no new attack
surface beyond amplification of existing gate execution. Two items need
documenting: the amplified gate execution window per CLI call, and the
interpolation escaping boundary that the upstream design explicitly delegated to
this sub-design. Neither requires design changes -- both are documentation
gaps that should be closed before implementation.
