# Security Review: unified-koto-next (Phase 6 — Maintainer Reviewer)

## Scope

This review covers the Security Considerations section of
`docs/designs/DESIGN-unified-koto-next.md` and the phase 5 security analysis. It
evaluates five questions: uncovered attack vectors, sufficiency of mitigations, validity
of "not applicable" claims, implementer clarity, and residual risk communication.

The review reads against the current `pkg/engine/engine.go` implementation, which
provides the ground truth for what is already in place versus what the design is adding.

---

## Question 1: Attack vectors not considered

### 1.1 Temp file permissions — Blocking gap

`atomicWrite()` creates state files using `os.CreateTemp()`, which inherits the process
umask. On a shared system with a permissive umask (e.g., 0022), the temp file is created
world-readable before the rename completes. The design's "state files must be created with
0600 permissions" mitigation does not address this window. `os.CreateTemp` produces mode
0600 only if the umask is 0177; a typical 0022 umask produces 0644.

The design must require that `atomicWrite` sets explicit file permissions with `os.Chmod`
or opens the temp file with `os.OpenFile(path, flags, 0600)` after creation. The
mitigation in the design says 0600 should be used but gives no guidance to the implementer
on how to achieve it for the temp-then-rename path.

### 1.2 Evidence values passed to directive interpolation — Not considered

`controller.go:77-81` shows that `c.eng.Evidence()` is merged into the interpolation
context and passed to `template.Interpolate(section, ctx)`. The section content is then
returned to the caller (the agent) as a directive string.

The design's User Data Exposure section notes that directive templates might interpolate
evidence values into text forwarded to external services. But this is already happening in
the current codebase — it is not a future risk. The design adds `--with-data` as a primary
evidence input mechanism, increasing the likelihood that callers will pass structured data
(including tokens or partial secrets) that ends up verbatim in directive text shown to LLMs
or logged to stdout.

The security section should state this as a current behavior, not a future possibility, and
require that template authors explicitly opt in to evidence interpolation in directives
rather than having it on by default.

### 1.3 `--to` flag bypasses gates on a looping workflow — Not considered

The design says directed transitions (`--to`) always skip gate evaluation. In a looping
workflow a state can be reached from multiple paths. An operator calling `koto next --to
<state>` can skip a gate that was specifically placed on a re-entry transition to verify
evidence from the previous loop iteration. The design documents `HistoryEntry.Directed =
true` for auditability, but the audit trail is only useful if someone reads it. An operator
who doesn't know that a gate was bypassed may believe the workflow completed a full
verified loop when it did not.

The design should document this explicitly: directed transitions are unconditional,
including on looping workflows. If a gate must not be bypassable, the template author
should not rely solely on per-transition gates — integrate a state-level invariant check or
separate the re-entry path into a distinct state.

### 1.4 Cycle detection scope — Partially unaddressed

The visited-set is scoped to one `Advance()` call. This prevents infinite loops within a
single invocation. However, nothing in the design or security section addresses a template
where two states have `Processing` fields that each call integrations which invoke `koto
next` themselves — creating cross-process reentrancy. This is the integration runner
equivalent of a recursive call. It's not a high-probability scenario, but it involves the
new `IntegrationRunner` surface introduced by this design and should be acknowledged.

---

## Question 2: Are the identified mitigations sufficient?

### 2.1 Command gate environment isolation — Insufficient as written

The design says "inherited environment must be stripped when invoking gate commands —
retain PATH and HOME only." The current `evaluateCommandGate` in `engine.go` does **not**
do this. `cmd.Env` is never set, which means the command inherits the full process
environment. This is an existing gap, but the design extends gate reach (per-transition
gates), increasing the blast radius.

The mitigation as written in the design ("must be stripped") is correct guidance, but it
is not implemented. An implementer reading only the design's security section would believe
this is already enforced. The design must distinguish between "this is a requirement for
the implementation" and "this is already enforced by the codebase."

Marking this as an implementer action item (see Question 4) is necessary.

### 2.2 IntegrationRunner allowlist — Deferred without a plan

Both the design and the phase 5 analysis say the `IntegrationRunner` implementation "must
resolve names through a configured allowlist." There is no `IntegrationRunner` implementation
yet. The design introduces the interface but leaves the allowlist mechanism entirely
unspecified: what the registry looks like, where it is configured, whether it is file-based
or compiled in.

An implementer writing the first `IntegrationRunner` concrete type has no guidance on what
"configured allowlist" means. Without a specified contract, the first implementation may
hard-code integration names directly in a `switch` statement, which is functionally an
allowlist but is not extensible and will not scale. The design should specify the minimum
contract for the initial implementation (e.g., map from name to absolute binary path,
loaded from config at controller construction).

### 2.3 SHA-256 compiled template cache — Correctly scoped

The design states the cache "ensures integrity within a session but does not verify
template authorship." This is accurate. The cache prevents a template from changing
mid-run; it does not prevent running a malicious template that was correctly compiled. The
framing is correct and the limitation is called out. No issue here.

---

## Question 3: "Not applicable" justifications

### 3.1 Download Verification — Correctly "not applicable"

This design adds no download path. All state operations are local. The justification holds.

### 3.2 Execution Isolation is marked applicable — Correct

No issue with the classification.

### 3.3 Supply Chain Risks — Marked "partially applicable"

The classification is correct: templates are the supply chain artifact. However, the
design's treatment elides a newly introduced surface. The `Processing` field in a template
is a new mechanism that names an integration to invoke. This is distinct from command
gates. The supply chain section discusses command gates but does not explicitly call out
the `Processing` field as a separate template-authored execution vector. An implementer
who reads the supply chain section will think about reviewing gate command strings; they
may not think about reviewing `processing:` values as a distinct attack surface.

---

## Question 4: Implementer clarity gaps

### 4.1 The "state files at 0600" requirement lacks a how

The security section says state files must be created at 0600. The `atomicWrite` function
uses `os.CreateTemp` which does not guarantee 0600 under typical umasks. The implementer
needs to know they must call `f.Chmod(0600)` (or use `os.OpenFile` with explicit mode)
on the temp file before the rename, not just set permissions on the final path. This is
not stated anywhere in the design or the phase 5 analysis.

### 4.2 Evidence clearing is described as "atomic with the transition commit" but the mechanism is not specified for the new flow

The design says "evidence is cleared atomically with each transition commit." In the new
evidence-clearing flow (design Phase 1: "Reset `State.Evidence = make(map[string]string)`
... Commit via `persist()` — single atomic write"), this is satisfied by the rename
semantics of `atomicWrite`. But the design's security section does not explain this
connection. An implementer who changes the persistence mechanism (e.g., moves to a
database) could break the atomicity guarantee without realizing it matters for security
(stale evidence from a prior loop iteration being evaluated by a gate after a crash
recovery).

The security section should state: the atomicity of evidence clearing relies on
`atomicWrite`'s rename semantics; any persistence change must preserve this property.

### 4.3 `--with-data` path canonicalization is mentioned but not specified

The design says "the file path must be canonicalized to prevent traversal attacks." It
does not specify: canonicalize against what base directory? The CLI caller provides a
path; the process CWD is used for resolution. An implementer might call `filepath.Abs()`
and consider that sufficient, but `filepath.Abs` does not resolve symlinks. The actual
requirement should be `filepath.EvalSymlinks` after `filepath.Abs`, with the result
checked against an allowed prefix. As written, the guidance is incomplete.

### 4.4 Command gate environment isolation is a requirement, not an existing behavior

As noted in 2.1, the current implementation does not strip environment variables. The
design's security section says "inherited environment must be stripped," but the
implementation does not do this yet. The design should mark this as a Phase 1 deliverable
(or a standalone fix before this design merges), not leave it as ambient guidance that
could be read as "already handled."

---

## Question 5: Residual risk communication

The design's residual risk framing has two problems.

**Problem A: The design conflates "mitigations are documented" with "mitigations are
implemented."** The security section for command gate isolation reads as a specification of
how things work, not as a to-do list. The current code does not strip environment
variables. An implementer reading the design without the code will believe the environment
is already sanitized. The design should distinguish explicitly between behaviors that are
already enforced (symlink check in `atomicWrite`, version conflict detection, timeout
enforcement) and behaviors that must be implemented as part of this design (env stripping,
0600 permissions on temp files, allowlist enforcement in `IntegrationRunner`).

**Problem B: Template trust model — the risk is higher than communicated.** The design
says "operators should apply the same review processes to templates as to application
code." This is appropriate guidance for internal use. But koto is a public open-source
project. The long-term distribution model includes third-party templates. "Future work:
ECDSA template signing" is listed as a consequence mitigation, not a security section
item. There is no statement of what the residual risk is until signing is implemented: any
user who executes a third-party template without reviewing it grants that template
arbitrary command execution rights on their machine, with a broad inherited environment.
This needs to be stated plainly in the security section as the current threat model, not
implied by noting that signing is future work.

---

## Summary of Findings

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| 1.1 | Temp file created world-readable under common umasks; 0600 guidance doesn't address this | Blocking | Design Security / atomicWrite implementation |
| 2.1 / 4.4 | Command gate env isolation is a design requirement, not an existing behavior; current code inherits full env | Blocking | Design Security / `evaluateCommandGate` |
| 4.3 | `--with-data` path canonicalization guidance is incomplete (`filepath.Abs` != symlink-safe) | Blocking | Design Security |
| 1.2 | Evidence-in-directive interpolation is current behavior, not a future risk; should require explicit opt-in | Advisory | `controller.go:77-81`, Design Security |
| 1.3 | `--to` gate bypass on looping workflows is not documented as a template design constraint | Advisory | Design Security |
| 2.2 | IntegrationRunner allowlist is required but unspecified; first implementer has no contract to follow | Advisory | Design Architecture / Security |
| 3.3 | `processing:` field as a supply chain vector is not called out separately from command gates | Advisory | Design Security |
| 5B | Residual risk from unauthenticated third-party templates is understated; signing is deferred with no interim guidance | Advisory | Design Security / Consequences |
| 4.2 | Atomicity of evidence clearing is implicitly tied to rename semantics; persistence changes could break it silently | Advisory | Design Security |

---

## Recommended Design Changes

1. **Add an explicit implementation checklist to the Security Considerations section.**
   Separate "already enforced" items (symlink check, version conflict, timeout) from "must
   be implemented" items (env stripping, 0600 temp file, `--with-data` path validation,
   IntegrationRunner allowlist). This prevents implementers from skipping required work
   because the design reads as descriptive rather than prescriptive.

2. **Replace the temp file 0600 guidance with a specific implementation requirement.** The
   guidance must specify that `os.CreateTemp` is insufficient and that the temp file must
   be opened with mode 0600 using `os.OpenFile` (or `Chmod` called immediately after
   `CreateTemp`).

3. **Specify the minimum contract for the initial `IntegrationRunner` implementation.**
   At minimum: a map from integration name to absolute binary path, configured at
   controller construction, with an explicit error for unmapped names.

4. **Update the template trust model statement** to say plainly: until template signing
   is implemented, executing any koto template is equivalent to executing the gate commands
   and integration CLIs it declares. This is the current threat model, not a future
   concern.
