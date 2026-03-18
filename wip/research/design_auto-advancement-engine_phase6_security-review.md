# Security Review: Auto-Advancement Engine Design

## Scope

Review of `docs/designs/DESIGN-auto-advancement-engine.md` security considerations,
cross-referenced against the upstream design (`DESIGN-unified-koto-next.md`), existing
gate evaluator (`src/gate.rs`), and persistence layer (`src/engine/persistence.rs`).

## Summary

The design's security section correctly identifies the main risk categories and its
"not applicable" justifications are legitimate. The mitigations for identified risks
are adequate for the current trust model (templates from trusted sources only). Two
gaps warrant attention: one is a real attack vector not considered, the other is
residual risk from an identified concern that lacks a concrete mitigation timeline.

---

## 1. Attack Vectors Not Considered

### 1.1 Event log size exhaustion via advancement loops (NEW)

**Risk**: The advancement loop chains through states, appending an event per transition.
A malicious or buggy template with a large number of states (each with passing gates
and unconditional transitions) could generate hundreds of events in a single `koto next`
invocation before hitting the visited-state cycle detector. Each event triggers a full
file re-read via `read_last_seq` (see `src/engine/persistence.rs:44`), creating
O(n^2) I/O. Combined with fsync per event, this could cause sustained disk pressure.

The cycle detector bounds this at `|states|`, so infinite loops are prevented. But
a template with 500 states chaining linearly would produce 500 events, 500 file
re-reads, and 500 fsyncs in a single invocation.

**Severity**: Low. Templates come from trusted sources. This is a resource exhaustion
concern, not a privilege escalation. The design already acknowledges the `append_event`
performance issue in Consequences > Negative and proposes a mitigation (passing expected
seq as parameter). However, the security section should note that the advancement loop
amplifies this from a linear concern (single-shot dispatch) to a quadratic one.

**Recommendation**: Add a configurable maximum chain length (e.g., 100 transitions
per invocation) as a safety bound. This is defense-in-depth against template bugs, not
a security boundary.

### 1.2 Gate command re-evaluation amplification is under-specified (PARTIAL)

The design notes that auto-advancement "can trigger multiple gate evaluations per
`koto next` invocation" and calls this a widened blast radius. This is correct. What's
missing is quantification: the loop evaluates gates at *every* state that has them,
and each gate spawns a subprocess. A template with 10 auto-advanceable states each
having 3 gates produces 30 subprocess spawns in a single `koto next` call.

The gate evaluator (`src/gate.rs:17`) has a 30-second default timeout per gate, and
gates within a state are evaluated without short-circuiting (all gates run regardless
of individual results). The worst case for a single invocation: `|states| * |gates_per_state|
* 30s` of subprocess execution time.

**Severity**: Low under current trust model. Medium if templates from semi-trusted
sources are ever supported.

**Recommendation**: The design should specify whether the signal handler's `AtomicBool`
check happens *between* individual gate evaluations within a state, or only between
loop iterations (i.e., between states). The current design text says "between iterations"
which means a state with 5 gates each timing out at 30s would take 150s to respond
to SIGTERM. The gate evaluator would need a shutdown flag parameter to check between
gates.

---

## 2. Mitigations for Identified Risks

### 2.1 Gate evaluation amplification

**Design says**: Existing constraint, not new. Template author controls commands.
Sandboxing needed before untrusted templates.

**Assessment**: Adequate for current trust model. The gate evaluator's process group
isolation (`setpgid(0,0)` at `src/gate.rs:74`) and timeout-based SIGKILL
(`src/gate.rs:103-105`) are solid. The process group kill prevents orphaned child
processes from surviving gate timeout. The zombie reap (`child.wait()` at line 107)
prevents PID table exhaustion.

### 2.2 Directive interpolation escaping

**Design says**: Engine returns structured `StopReason` with raw values. Any layer
that interpolates must treat values as untrusted.

**Assessment**: Adequate. The boundary is clearly drawn. The engine's contract is
data-out, not rendered-text-out. The responsibility statement is explicit: "must treat
those values as untrusted strings and escape them appropriately." This pushes the
escaping obligation to the consumer, which is the correct boundary since the engine
doesn't know the rendering context (Markdown, shell, JSON).

The upstream design (`DESIGN-unified-koto-next.md`, Security Considerations) asked this
sub-design to "specify the escaping rules for directive rendering." The sub-design
responds by saying the engine doesn't render directives, so escaping isn't its concern.
This is architecturally correct -- the engine shouldn't own rendering -- but it means
the escaping rules still need to be specified *somewhere*. The CLI handler currently
serializes `NextResponse` to JSON, which inherently escapes strings. If a future
controller layer renders Markdown or shell commands from evidence values, the escaping
gap re-opens.

**Recommendation**: File a follow-up issue to specify escaping rules in the controller/
template rendering layer. The engine design correctly defers this, but the obligation
shouldn't be lost.

### 2.3 User data exposure

**Design says**: Evidence persisted as plaintext JSON. Existing `append_event` applies
0600 permissions. Engine uses same persistence path.

**Assessment**: Adequate. Verified in code: `src/engine/persistence.rs:16-18` and
`59-62` both set mode 0o600 via `OpenOptionsExt`. The auto-advancement engine injects
`append_event` as a closure, so it inherits these permissions without introducing a
new file access pattern.

---

## 3. "Not Applicable" Justifications

### 3.1 Download verification: "Not applicable"

**Assessment**: Correct. The advancement engine operates on local state files and
compiled templates loaded from local cache. No network I/O occurs. The design correctly
notes that the deferred integration runner config system will need its own review.

---

## 4. Residual Risk

### 4.1 Advisory flock is advisory-only

The design uses `flock` for concurrent access protection. Advisory locks are not
enforced by the kernel -- any process that doesn't call `flock` can read/write the
state file freely. This is standard for CLI tools (git uses the same pattern for
index.lock), but worth noting: a separate tool or script that modifies the state
file directly bypasses the lock entirely. The sequence gap detection in `read_events`
provides post-hoc detection of corruption, which is the correct defense-in-depth layer.

**Assessment**: Acceptable residual risk. No escalation needed.

### 4.2 Template hash verification scope

The design retains SHA-256 hash verification tying the event log to a specific template
version. This prevents template tampering between `koto init` and subsequent `koto next`
calls. However, the hash is of the *compiled* template JSON, not the source YAML. If the
compilation pipeline has a bug that produces different output from the same source, the
hash wouldn't catch it. This is outside the scope of the advancement engine design but
is residual risk in the overall system.

**Assessment**: Acceptable. The compilation pipeline is a separate concern.

### 4.3 Interpolation injection remains unresolved in the system

As noted in 2.2, the engine correctly avoids interpolation, but no component currently
owns the escaping contract. Evidence values and integration output flow through the
system as raw JSON and are returned to agents as raw JSON (which is safe because JSON
serialization handles escaping). But if any future rendering layer interpolates these
values into non-JSON contexts (Markdown directives, shell commands), injection is
possible. The upstream design flagged this; the sub-design deferred it; nobody has
picked it up yet.

**Assessment**: Medium residual risk. Should be tracked as a follow-up issue before
the integration runner (which produces subprocess output that flows into evidence)
is implemented. The risk compounds when integration output -- which comes from
arbitrary subprocesses -- is stored in the event log and later consumed by a
rendering layer.

---

## 5. Findings Summary

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1 | Loop amplifies append_event O(n) re-read to O(n^2) | Low | Add max chain length bound; note in design |
| 2 | Signal handler granularity unclear for multi-gate states | Low | Specify whether shutdown check occurs between gates |
| 3 | Interpolation escaping obligation unassigned in system | Medium | File follow-up issue before integration runner work |
| 4 | Advisory flock is advisory-only | Accepted | No action (standard pattern, seq gap detection covers it) |
| 5 | All "not applicable" justifications valid | N/A | No action |
