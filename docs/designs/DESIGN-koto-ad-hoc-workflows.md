---
status: Proposed
problem: |
  koto can only run a workflow from a pre-authored, compiled template, so an
  agent facing a novel complex task cannot get koto's ordered, recoverable,
  auditable execution without a human (or a heavyweight durable-authoring step)
  writing the template first. There is no path for an agent to author a
  workflow inline and run it immediately.
decision: |
  Add a single-shot path that accepts a workflow definition on stdin, compiles
  and strictly validates it in-process, and initializes a session in one
  invocation — reusing koto's existing compile pipeline and state machine with
  no new execution semantics. Persist both the compiled artifact (so per-tick
  template-hash verification keeps succeeding) and the human-readable authored
  source (for audit) with the session. Ship a koto-skills skill that teaches
  decomposition and the run path, with a decomposition-quality bar and a
  tier-2 eval.
rationale: |
  The execution engine, audit log, and rewind already exist and are the value;
  the only gap is an authoring/ergonomics seam. A thin stdin entry over the
  existing pipeline keeps the blast radius small and inherits koto's guarantees
  unchanged. Persisting the compiled form is forced by koto re-verifying the
  template hash every tick; persisting the source closes the audit gap that
  compiled-only storage would leave.
---

# DESIGN: koto ad-hoc workflows

## Status

Proposed

## Context and Problem Statement

koto executes workflows defined as templates: a markdown/YAML source is
compiled to content-addressed JSON (`koto template compile`), and `koto init
<name> --template <file>` starts a session from a compiled template on disk.
Every `koto next` tick re-reads the compiled template and re-verifies its
SHA-256 hash, so the compiled artifact is a live, per-session dependency. The
only assisted authoring path today is the `koto-author` skill, which produces a
*durable, reusable* template + paired SKILL.md meant to be committed and run
many times.

This leaves a gap: an agent handed a novel, one-off complex task cannot obtain
koto's ordered/recoverable/auditable execution without first materializing a
template file (by hand or via the heavyweight durable-authoring flow). There is
no way for an agent to decompose the task in front of it into a workflow and run
it immediately, using koto alone.

The technical problem this design solves: provide a single-shot path from an
inline, agent-authored workflow definition to a running koto session, reusing
the existing compile-and-execute machinery, while (a) validating strictly with
agent-actionable errors, (b) keeping the compiled artifact alive for the
session's per-tick hash verification, and (c) preserving the human-readable
authored definition for the audit trail. The capability is paired with a
koto-skills teaching skill (decomposition guidance + quality bar) and a
behavioral eval.

*The requirements for this work are specified in an accepted PRD held in a
private planning tracker; this design restates the problem in implementation
terms and does not depend on that document.*

## Decision Drivers

- **Reuse the existing engine.** No new workflow-execution semantics, and no
  changes to the state-file format or gate model — the capability is a thin
  entry point over the current compile + init + state-machine path.
- **Hash-verification persistence.** koto re-verifies the compiled template's
  SHA-256 hash on every `koto next`; whatever the stdin path produces must
  persist for the session's lifetime or running workflows break mid-flight.
- **Auditability.** The human-readable authored definition must be recoverable
  from the session, not only the compiled JSON.
- **Strict, agent-actionable validation.** Definitions must meet koto's current
  template standard (structured gate routing; no legacy patterns), and errors
  must name the failing element so an agent can self-correct.
- **Single-shot ergonomics.** No scratch file and no separate compile step on
  the authoring path.
- **Teachable and evaluable.** The path must be exercisable by a koto-skills
  skill and verifiable by a tier-2 (execution-based) eval, including a
  decomposition-quality assertion.
- **Keep ephemeral distinct from durable.** The path must not become a backdoor
  substitute for durable, reusable templates authored via `koto-author`.

## Considered Options

### Decision 1: CLI surface for inline definition

How does an agent hand koto a workflow definition and get a session started in
one invocation? A grounding finding reframed the options: koto has **no `-`
stdin sentinel** anywhere in `src/`; its only stdin idiom is flag-presence —
`koto context add` reads stdin by default and `--from-file` overrides
(`src/cli/context.rs`).

Key assumptions: the existing `koto init <name> --template <path>` callers stay
unaffected; the stdin bytes are bridged into the compile pipeline (the bridge
target is settled by Decision 2); persisting the readable source is a separate
concern (Decision 2).

#### Chosen: `koto init --from-stdin`

Keep the `koto init` verb and add an explicit `--from-stdin` flag, mutually
exclusive with `--template`, that reads the definition from standard input.
This matches koto's established flag-presence stdin idiom, is discoverable in
`--help`, keeps the durable-template flag (`--template`) semantically separate
from the ephemeral path, and adds the smallest possible surface over the
existing `init` machinery.

#### Alternatives Considered

- **`koto init <name> --template -` (overload the flag with a `-` sentinel):**
  invents a `-` convention koto never uses and overloads the durable-template
  flag, blurring the ephemeral/durable line the design wants sharp.
- **`koto init <name> -` (positional `-` sentinel):** same invented convention
  plus a positional idiom koto doesn't use; undiscoverable in `--help`.
- **Dedicated verb (`koto compose` / `koto init --inline`):** honors the drivers
  but adds the largest new surface for what is framed as a thin entry point.

### Decision 2 (critical): Where the compiled artifact and source live

Today `koto init` compiles to a content-addressed file at
`~/.cache/koto/<sha256>.json` and records that path plus the SHA-256
`template_hash` in the session (`src/cache.rs`, `src/cli/init_child.rs`). Every
`koto next` re-reads that file and re-verifies the hash
(`src/cli/mod.rs` verification path), failing the workflow if the file is
missing or mismatched. For an inline definition with no durable source file,
the storage location is a correctness question: cache eviction (`rm -rf
~/.cache/koto`) would brick a running ad-hoc workflow.

Key assumptions: `template_hash` is content-addressed over the compiled JSON
bytes, so relocating the file changes only a path string, not the hash;
read-side path resolution against the session directory is additive, not a
state-file-format change; the inline path is scoped separately from the
file-template path.

#### Chosen: store compiled JSON + source in the session directory

For the inline path, write both the compiled JSON and the human-readable source
into the session directory (`~/.koto/sessions/<repo-id>/<name>/`), and record
the compiled artifact as a **session-relative** `template_path` resolved against
the session dir at read time. This makes the compiled artifact durable session
state, so external cache eviction cannot break a run (R6); preserves the
readable source alongside the session for audit (R4); and leaves the
content-addressed hash and the read/verify logic unchanged (R11) — only a path
value differs. The file-template (`--template <path>`) path is untouched and
keeps using the absolute cache path.

#### Alternatives Considered

- **Keep compiled JSON in `~/.cache` + guard against eviction:** every guard
  (pin, refcount, recompile-on-miss) is fragile against a blunt `rm -rf`
  cleanup, can introduce a hash mismatch if compiler output drifts, and
  permanently pollutes the GC-less global cache with single-use entries.
- **Embed the compiled form/source inline in the state-file header or log:**
  satisfies R6/R4 but is a state-file-format change forbidden by R11, forces
  every state-file reader to understand embedded artifacts, and bloats every
  log read.
- **Store an absolute path inside the session dir:** simpler — every existing
  reader works unchanged with zero resolution code. Rejected because the
  session `relocate` operation (`src/session/local.rs`) renames the whole
  session directory; an absolute path would dangle after relocation. A
  session-relative path is relocate-safe, at the cost of the centralized
  resolution in Solution Architecture component 3. (If ad-hoc-session relocate
  proves rare, the absolute-path variant is the obvious simplification to
  revisit.)

Open risk carried to Implementation: on session relocation the event-log
`template_path` is not rewritten while the header is (`src/session/local.rs`) —
storing a session-relative path mitigates this; the implementation must verify
both read sites resolve it.

### Decision 3: Skill structure, quality bar, and invocation

How is the teaching skill organized, how is the decomposition-quality bar
encoded, and how is it invoked? koto-skills today ships `koto-user` (run) and
`koto-author` (durable 8-state authoring); skills activate **via description
frontmatter only** — there is no slash-command mechanism in the plugin.

Key assumptions: the inline entry (Decisions 1/2) exists for the skill to drive;
`template-format` guidance can be referenced cross-skill; CI requires a per-skill
`evals/evals.json` (`check-evals-exist.sh`).

#### Chosen: third sibling skill, inline quality bar, trigger-description invocation

Add a third sibling skill under `plugins/koto-skills/skills/` (registered in
`plugin.json`) that **reuses** rather than duplicates: it hands the run loop to
`koto-user` and consumes the shared `template-format` reference. The
decomposition-quality bar is encoded **inline** in `SKILL.md` as a compact
checklist plus two worked examples (one branching, one linear-with-gates), so it
is reliably read at authoring time and is assertable by the eval. Invocation is
a **trigger description** in the house style (capability + "Use when …" clause)
covering human-directed invocation as the baseline and agent self-activation,
with a steering clause redirecting durable/repeated authoring to `koto-author`.

#### Alternatives Considered

- **Fold into `koto-author` as an ephemeral mode:** violates the ephemeral/
  durable separation and the explicit no-backdoor non-goal.
- **Duplicate template-format + run-loop guidance inline:** contradicts the
  "don't duplicate" driver and creates a drift surface.
- **Separate rubric/reference doc for the quality bar:** on-demand loading risks
  the bar being skipped at decomposition time, weakening the teaching and eval
  signal.
- **Slash command (alone or alongside):** no such mechanism exists in
  koto-skills; diverges from the description-only house style and adds unused
  infrastructure.

### Decision 4: Eval design

How is the behavioral acceptance bar verified? The harness (`scripts/run-evals.sh`,
per-skill `evals.json`, LLM grader) already supports a tier-2 "execute" mode
(`EVAL_SCENARIO` + fixtures/binaries on PATH), but no execution-based eval
exists yet.

Key assumptions: the inline entry lands as a `koto init` variant the eval can
invoke; `koto next` surfaces a machine-detectable terminal signal (confirmed in
`src/cli/mod.rs`); the real koto binary plus shimmed domain tools can sit in
`fixtures/bin`; running the eval on demand (not per-PR) is acceptable.

#### Chosen: one tier-2 hybrid-oracle eval

A single tier-2 execution eval reusing the existing plumbing (zero new harness
code). Fixture: one fixed multi-phase task with a natural verification gate and
no matching template, in a hermetic `fixtures/` tree, entered via a
human-directed skill invocation that the agent then executes. Oracles are
**mechanical-first**: validity, runs-to-terminal, and rewind-works are each
asserted against real koto exit codes and JSON (compile/init exit 0; the `koto
next` loop reaches a terminal `completed`; `koto rewind` exits 0 to a prior
state and `koto next` resumes). The decomposition-quality assertion is **hybrid**:
mechanical structural guardrails (state-count band, ≥1 gate, no monolithic
single state) plus one LLM-judged check that the gate sits at the real
verification boundary. Threshold: N=3 runs; all mechanical/structural assertions
pass every run; the LLM-judged quality assertion passes in ≥2 of 3.

#### Alternatives Considered

- **All-LLM oracles for validity/terminal/rewind:** koto exposes these
  deterministically; LLM grading adds variance for no benefit.
- **Real external tools in the fixture:** breaks hermeticity; the shim-on-PATH
  design exists to avoid it.
- **Single-run all-pass threshold:** too brittle for the LLM-judged assertion.
- **Best-of-N threshold:** masks instability; majority is stricter.
- **Pure mechanical quality metric:** a wrongly-gated but well-sized workflow
  would pass, failing R8's intent.

## Decision Outcome

The four decisions compose into a single thin path. `koto init --from-stdin`
(D1) reads an inline definition and runs it through the **existing** compile +
state-machine pipeline; the compiled artifact and the readable source are
written into the **session directory** with a session-relative `template_path`
(D2), so per-tick hash verification keeps working without touching the
state-file format or the engine. A third koto-skills sibling skill (D3) teaches
an agent to decompose a task, drive the new entry, and stay clear of durable
authoring, and a single tier-2 hybrid eval (D4) verifies the end-to-end
behavior through a human-directed invocation.

Cross-validation reconciliations: (1) D1's incidental assumption that the
compiled output lands in `~/.cache` is superseded by D2 — the stdin bytes are
bridged into the **session directory**, not the global cache; D1's flag-surface
choice is unaffected. (2) The source PRD's invocation requirement named "an
explicit invocation"; since koto-skills has no slash-command mechanism, that
resolves to the skill's **trigger description** firing on a human directive,
which is the verified baseline (D3, D4).

## Solution Architecture

**Components.**

1. **`koto init --from-stdin` (CLI, `src/cli/`).** New flag on the existing
   `init` verb, mutually exclusive with `--template`, and incompatible with
   `--allow-legacy-gates` (rejected on this path). Reads the full definition
   from stdin, writes it to a source file in the session directory, then feeds
   that file through the **existing** path-based compile helper in **strict**
   mode. On compile/validation failure it writes nothing that persists and
   exits non-zero with an error naming the failing element (state / transition
   / gate). ("No scratch file" is the user-facing property — the agent pipes
   bytes and never manages a file; koto's internal source write is the audit
   artifact of Decision 2, not a user-managed temp file.)
2. **Session-local artifact storage (`src/cli/init_child.rs`, `src/cache.rs`,
   `src/session/local.rs`).** The compile path already supports this cheaply:
   `compile_cached_into(source, target_dir, strict)` parameterizes both the
   output directory and strict mode (`src/cache.rs`). For the inline path,
   `target_dir` is the session directory, so the compiled JSON lands there and
   is recorded as a **session-relative** `template_path`; the raw source is
   written beside it under a **fixed** koto-convention filename (the original
   extension, if recorded at all, is metadata — never a path component). The
   content-addressed `template_hash` is unchanged (SHA-256 over compiled bytes,
   computed after validation), so cache-hit integrity is preserved.
3. **Centralized session-relative path resolution (`src/engine/persistence.rs`
   `derive_machine_state`).** `template_path` is read raw by ~10 commands today
   (`next`, `rewind`, `decisions record`, `status`, `cancel`, batch view,
   `overrides`, `workspace`, `dashboard_data`, `retry`) — and `koto next`
   itself does not resolve it currently. Rather than thread resolution through
   every call site, resolve a session-relative `template_path` against the
   session directory **once** in `derive_machine_state`, so all readers inherit
   it and the existing hash re-verification is unchanged. This keeps the "thin
   entry, small blast radius" property honest. Note this is a new resolution
   base, distinct from the existing `resolve_template_path` helper (which
   resolves against the parent source dir / submitter cwd for batch spawns).
4. **`koto-adhoc` skill (`plugins/koto-skills/skills/`).** Teaches
   decomposition (inline quality-bar checklist + two examples), drives
   `koto init --from-stdin` then the `koto-user` run loop, and steers repeated
   authoring to `koto-author`. Registered in `plugin.json`; activates via its
   trigger description.
5. **Tier-2 eval (`evals/` for the skill).** Fixed hermetic fixture + hybrid
   oracles + N=3 threshold, per Decision 4.

**Data flow.**

```
agent ──pipes definition──▶ koto init --from-stdin <name>
                               │ write source to session dir (fixed filename)
                               │ compile_cached_into(source, session_dir, strict) ──fail──▶ exit≠0, named error, nothing persists
                               ▼ ok
   session dir: compiled.json (session-relative template_path) + <fixed-source-filename>
                               ▼
   any reader (next/rewind/status/…) ─▶ derive_machine_state resolves
        session-relative path ─▶ existing hash re-verify ─▶ directive
        … directive/execute/evidence loop … koto rewind (unchanged) … terminal
```

## Implementation Approach

Phased so each step is independently reviewable; each inherits koto's existing
test surface.

1. **Session-local storage + centralized resolution (Decision 2 first — it is
   the correctness core).** Have the inline compile write into the session dir
   via `compile_cached_into(source, session_dir, strict)` with a
   session-relative `template_path`, and resolve that path **once** in
   `derive_machine_state` so all ~10 readers (`next`, `rewind`, `decisions
   record`, `status`, `cancel`, batch view, `overrides`, `workspace`,
   `dashboard_data`, `retry`) inherit it. Verify file-template workflows are
   byte-for-byte unaffected (regression). Confirm the relocation gap (header
   rewritten but event-log `template_path` not, `src/session/local.rs`) is
   closed by the relative path.
2. **`koto init --from-stdin` surface (Decision 1).** Add the flag, the
   stdin→session-dir source write, the strict compile, mutual exclusion with
   `--template`, and rejection of `--allow-legacy-gates`. **Acceptance-level
   guards (promoted from the security review):** (a) the persisted source
   filename is a fixed convention — any recorded extension is metadata, never
   concatenated into a path (prevents `source/../../x` traversal); (b)
   `--from-stdin` rejects `--allow-legacy-gates` so strict cannot be downgraded.
   Error paths: invalid definition → no session, non-zero exit, element-named
   message.
3. **`koto-adhoc` skill (Decision 3).** Author the SKILL.md (inline quality bar
   + two examples), reuse template-format + koto-user references, trigger
   description, koto-author steering clause; register in `plugin.json`.
4. **Tier-2 eval (Decision 4).** Build the hermetic fixture and `evals.json`
   with mechanical + LLM-judged assertions and the N=3 threshold; satisfy the
   per-skill eval CI requirement.

## Security Considerations

**Trust model: unchanged from `--template`.** `koto init --from-stdin` bridges
stdin bytes into the same compiler and state machine that `koto init --template
<file>` uses. koto trusts whoever invokes it: the workflow author is the invoker
(or whoever they chose to pipe input from). Accepting a definition on stdin
crosses no privilege boundary that authoring a `--template` file did not already
cross — both run as the same user, in the same working directory, on the same
engine.

**Command gates execute arbitrary shell — by design, and not new here.** A
workflow's `command` gate runs its command string via `sh -c` with the invoking
user's privileges on each `koto next` tick (`src/gate.rs`, `src/action.rs`). An
inline definition can therefore contain arbitrary commands, exactly as a
`--template` file can today. Strict compilation rejects legacy/malformed gate
*structure*; it does not and is not intended to constrain *what* a `command`
gate runs. Sandboxing or command allowlisting is out of scope for this design:
it would be a new, engine-wide policy applying equally to both the file and
stdin paths, not a property of the stdin entry point. Callers running
agent-authored definitions should rely on their existing agent-sandboxing
posture — koto faithfully executes whatever valid definition it is handed, just
as it does for file templates.

**Filesystem scope is bounded.** The compiled JSON and raw source are written
only under the session directory `~/.koto/sessions/<repo-id>/<name>/`. The
attacker-influenced component, `<name>`, passes `validate_session_id` and
`validate_workflow_name`: neither permits `/` or `..`, and `validate_workflow_name`
additionally rejects `~`, so `<name>` cannot encode path traversal or an
absolute path. `~/.koto` is created `0700` and artifact files `0600` (at
creation). The source filename is a fixed koto convention; any recorded source
extension is stored as metadata, never concatenated into a path (otherwise
`source/../../x`-style traversal would be the naive implementation — see the
Implementation Approach acceptance guard).

**The persisted source is a secret-bearing artifact.** For audit, the inline
path writes the human-readable definition to disk alongside the session — a copy
that, for a file template, would otherwise live only where the author kept it.
Authors must not embed secrets (tokens, keys) directly in `command` gate strings
or variable defaults; reference environment variables or files read at
gate-evaluation time instead (gates run in a shell, so `$VAR` resolves at eval
time without baking the secret into the definition). The session directory's
source and compiled JSON should be handled with the same care as the state file,
especially if a session directory is shared or archived for audit.

**No supply-chain surface.** The path is fully local: it fetches nothing,
resolves no remote definition, and adds no dependencies. The only input is the
bytes the invoker supplied on stdin.

**Resource use (minor, pre-existing).** Like the existing `koto context add`
stdin idiom, the read is unbounded, so a multi-gigabyte pipe could exhaust
memory or fill `~/.koto`. It is self-inflicted and same-user (the invoker is
piping to their own koto), so it carries the same trust model as the rest of
the path; an input size cap is a reasonable future hardening, engine-wide, not a
blocker here.

## Consequences

**Positive.**

- koto gains standalone "structure on demand": an agent can govern a novel
  complex task with no pre-authored template and no human authoring step.
- A thin entry over the existing compiler, state machine, audit log, and
  `rewind` — no new execution semantics, small blast radius.
- Sessions stay auditable (readable source preserved) and recoverable (`rewind`
  works unchanged).
- Session-local storage removes a latent fragility for the inline path: an
  ad-hoc workflow can't be bricked by `~/.cache` eviction mid-run.

**Negative (with mitigations).**

- A second cleartext copy of the definition lives on disk. *Mitigation:* the
  0700 session tree plus explicit guidance against embedding secrets (use
  `$VAR`/file reads at gate-eval time).
- Behavioral divergence: the stdin path is strict while `--template` stays
  permissive. *Mitigation:* documented; the broader consistency cleanup is
  deferred (named in the source PRD).
- This is koto-skills' first tier-2 execution eval, so fixtures/scenarios are
  net-new, and the LLM-judged quality assertion carries some variance.
  *Mitigation:* mechanical-first oracles plus an N=3 majority threshold on the
  one judged assertion.
- The decomposition-quality bar is heuristic; an agent can still author a
  weak-but-valid decomposition. *Mitigation:* the eval guards the floor (valid,
  runs, gate at the real boundary), not perfection; the skill's inline examples
  raise the typical case.
