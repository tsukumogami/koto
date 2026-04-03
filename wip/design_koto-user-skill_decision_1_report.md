<!-- decision:start id="koto-user-skill-responsibility-split" status="assumed" -->
### Decision: Responsibility Split Between SKILL.md and Reference Files

**Context**

The koto-user skill needs to guide agents running koto-backed workflows. The existing `AGENTS.md` (550 lines) covers the domain as a monolith — useful, but structured for reference rather than runtime use. The new skill splits this content across SKILL.md and three reference files: command-reference.md, response-shapes.md, and error-handling.md.

The central design tension: a thin SKILL.md keeps the core file short but forces agents to follow links for common operations. A balanced SKILL.md puts the happy-path essentials inline, reserving depth for references. A fat SKILL.md essentially reproduces AGENTS.md and defeats the split.

The binding constraint is that reference files are link-only — agents follow links when they need depth, not automatically. This means anything the agent needs during a typical session loop (init → next → dispatch → submit evidence → repeat) must be in SKILL.md. The `expects` schema in an `evidence_required` response tells the agent what to submit, but not how to submit it — that's a CLI concern that belongs inline.

**Assumptions**

- Agents reading SKILL.md will not auto-load reference files; they follow links only when explicitly needed.
- The typical session reaches `evidence_required` on nearly every state that requires agent work, making the evidence submission pattern the most frequently needed piece of information.
- The three evidence_required sub-cases (clean evidence, gates blocking with accepts, gates blocking without accepts) are the trickiest part of runtime dispatch and cannot be deferred to references without harming correctness.

**Chosen: Balanced SKILL.md**

SKILL.md covers:
- The complete runtime lifecycle (init → next → evidence/override/wait → repeat → done)
- The 6-value action dispatch table with one-liner descriptions for each action value
- The three evidence_required sub-cases with their distinguishing signals (blocking_conditions empty vs. populated, vs. gate_blocked action)
- The evidence submission pattern inline: `koto next <name> --with-data '<json>'` with a one-line example showing that JSON keys match `expects.fields`
- The override flow (two-step: `koto overrides record` then re-query) — covered inline because it's the recovery path for blocked gates
- Links to the three reference files with a one-line description of when to follow each

command-reference.md covers:
- Every koto CLI subcommand relevant to workflow runners (exhaustive): `koto init`, `koto next` (all flags: `--with-data`, `--to`, `--full`), `koto overrides record/list`, `koto decisions record/list`, `koto rewind`, `koto cancel`, `koto workflows`, `koto template compile`
- Full flag descriptions, argument types, and return shapes for each command

response-shapes.md covers:
- At least one annotated JSON example per action value (all 6 actions)
- Field-by-field annotations explaining what each field means and how to use it
- The `details` field visibility rules (first visit vs. repeat vs. `--full`)
- The `advanced` field semantics

error-handling.md covers:
- Exit codes 0-3 with their error code mappings
- The `agent_actionable: false` scenario (agent cannot fix this; report to user)
- Per-error-code guidance: what happened, what to do next

**Rationale**

The key invariant is that an agent running a typical workflow session should not need to open any reference file. The happy path — `evidence_required` → read `expects` → submit via `--with-data` — is fully executable with SKILL.md alone. The evidence submission pattern (one-liner showing `--with-data` with JSON matching `expects.fields`) is short enough to include inline without making SKILL.md long.

The three evidence_required sub-cases must live in SKILL.md, not response-shapes.md, because they determine which action path the agent takes — getting them wrong causes the agent to submit evidence when it should fix gates first. This is correctness-critical, not just depth.

command-reference.md is the right home for exhaustive flag documentation because agents only need it when using less-common commands (`--to`, `--full`, `koto rewind`) or when they want to verify exact syntax. It's not needed on every loop iteration.

response-shapes.md serves agents who encounter an unfamiliar action value or need to verify field names. Since all 6 action values are described in the SKILL.md dispatch table, agents follow the link only when the one-liner isn't enough.

**Alternatives Considered**

- **Thin SKILL.md**: SKILL.md covers lifecycle and dispatch table only; evidence submission format lives in response-shapes.md. Rejected because the evidence submission pattern (`--with-data` with JSON matching `expects.fields`) is needed on almost every state transition. Forcing agents to open response-shapes.md for this violates the "minimize files read" constraint.
- **Fat SKILL.md**: SKILL.md reproduces AGENTS.md content (all response shapes with full annotated JSON, all error codes, full command reference). Rejected because it recreates the monolith problem — long files make it harder for agents to locate the specific information they need, and the reference files become redundant.

**Consequences**

- Agents running typical workflows read only SKILL.md. Reference files are consulted for depth (unfamiliar commands, schema verification, error diagnosis).
- SKILL.md will be longer than a purely thin approach, but the extra lines are the evidence submission pattern and three sub-cases — content that pays for itself by avoiding reference-chasing on every loop iteration.
- command-reference.md becomes the authoritative CLI reference; it must be kept in sync with koto CLI changes.
- The dispatch table in SKILL.md duplicates action value names that also appear in response-shapes.md — this is intentional. SKILL.md gives the behavioral summary; response-shapes.md gives the full schema. Both are needed and serve different reading contexts.
<!-- decision:end -->
