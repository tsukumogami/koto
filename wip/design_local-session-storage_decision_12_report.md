# Decision 12: Content key design

## Question

What's the key format for content stored in koto? How do keys map from current wip/ naming conventions?

## Chosen: Option 1 -- Hierarchical path keys

**Confidence: high**

## Rationale

Keys are path-like strings using forward slashes as namespace separators: `research/lead-1.md`, `scope.md`, `findings.md`. The slash character creates a natural hierarchy that maps directly to the `ctx/` subdirectory structure chosen in Decision 8 (files in session directory with manifest).

Three properties make this the right choice:

**Natural mapping from current wip/ conventions.** The existing naming pattern is `wip/<command>_<topic>_<artifact>` with a `research/` subdirectory for deep research outputs. Since koto sessions already scope by topic (session ID = workflow name), the `<command>_<topic>_` prefix becomes redundant. The key is the portion after topic scoping:

| Current wip/ path | koto key |
|---|---|
| `wip/explore_<topic>_scope.md` | `scope.md` |
| `wip/explore_<topic>_findings.md` | `findings.md` |
| `wip/explore_<topic>_decisions.md` | `decisions.md` |
| `wip/explore_<topic>_crystallize.md` | `crystallize.md` |
| `wip/research/explore_<topic>_r1_lead-resume-logic.md` | `research/r1/lead-resume-logic.md` |
| `wip/research/explore_<topic>_r1_lead-concurrency.md` | `research/r1/lead-concurrency.md` |
| `wip/design_<topic>_coordination.json` | `coordination.json` |
| `wip/design_<topic>_decision_8_report.md` | `decisions/d8-report.md` |
| `wip/issue_<N>_baseline.md` | `baseline.md` |
| `wip/issue_<N>_plan.md` | `plan.md` |
| `wip/implement-<topic>-state.json` | `state.json` |

The command prefix (`explore_`, `design_`, `plan_`, `issue_`) drops out because the session's workflow template already carries that context. The topic drops out because the session ID is the topic. What remains is the artifact's structural role within the workflow.

**Prefix-based listing.** `koto context list --prefix research/r1/` returns all lead outputs for round 1. This satisfies the constraint that skills need pattern-based listing (e.g., "all research files for round 1") without building a query language. Prefix matching on path-like strings is the simplest filtering model that covers the existing patterns.

**Direct filesystem mapping.** Decision 8 chose files in a session directory with a manifest. Hierarchical keys map to subdirectories: key `research/r1/lead-cli-ux.md` becomes file `ctx/research/r1/lead-cli-ux.md`. The manifest maps logical keys to these paths, handling any edge cases, but the common case is a direct 1:1 correspondence. Developers debugging sessions can `ls ctx/research/r1/` and see exactly what they'd expect.

### Key validation rules

Keys must be safe for filesystem storage while remaining human-readable:

- **Allowed characters:** `[a-zA-Z0-9._-/]`
- **Must not start or end with `/`** (no leading or trailing slashes)
- **No consecutive slashes** (`//` rejected)
- **No `.` or `..` path components** (prevents path traversal)
- **Maximum length:** 255 characters (filesystem name limit applies to each path component, not the full key, but 255 total is a reasonable upper bound)
- **Each component between slashes** must match `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` (start with alphanumeric, same spirit as session ID validation from the design doc)

Validation runs in `koto context add` at the submission boundary. `get` and `exists` don't re-validate since keys that reach them were validated at write time.

## Evaluation

### Option 1: Hierarchical path keys

**Strengths:**
- Maps naturally from current wip/ conventions after dropping the topic and command prefixes that koto's session model handles
- Prefix-based listing covers the "all research for round N" query pattern without a query language
- Direct filesystem mapping to the `ctx/` subdirectory, consistent with Decision 8
- Human-readable in `koto context list` output and in direct filesystem inspection
- Validation rules are straightforward and consistent with session ID validation

**Weaknesses:**
- Keys with slashes need `create_dir_all` when storing as files (trivial cost, but an extra step vs flat keys)
- Potential for inconsistent nesting depth across skills if conventions aren't documented (mitigated by template-level conventions)

### Option 2: Flat string keys

**Strengths:**
- Simplest implementation -- keys map directly to filenames in a single directory
- Current wip/ filenames could become keys with minimal transformation

**Weaknesses:**
- Retains the `explore_<topic>_` prefix noise since there's no structural hierarchy to absorb the grouping. Keys like `explore_content-ownership_r1_lead-resume-logic.md` are long and redundant when the session already scopes by topic.
- No prefix-based listing. Filtering "all round 1 research" requires substring matching or regex on flat keys, which is more fragile than prefix matching.
- A single directory with many files becomes unwieldy for sessions with deep research (10+ lead outputs per round, multiple rounds).
- The manifest still needs to exist (Decision 8), so "simpler implementation" only saves `create_dir_all` calls.

### Option 3: Structured metadata keys

**Strengths:**
- Rich querying: "all keys where namespace=research AND round=1" is a structured filter
- Explicit metadata makes conventions machine-enforceable

**Weaknesses:**
- Requires a schema for key metadata that every skill must conform to. Current skills have heterogeneous naming patterns (explore uses rounds, implement uses issue numbers, design uses decision IDs). A single schema either overspecifies or has too many optional fields.
- Complex API surface: `koto context add --namespace research --round 1 --name lead-cli-ux.md` vs `koto context add --key research/r1/lead-cli-ux.md`. The structured version is harder to type, harder to compose in shell, and harder to extend.
- The querying advantage over prefix matching is marginal for current patterns. "All round 1 research" is `--prefix research/r1/` with hierarchical keys or `--namespace research --round 1` with structured keys. Same result, more API surface.
- Overkill for current needs, as the constraints note. Building rich querying now means designing schemas for patterns that don't yet exist.

## Assumptions

- Skills will adopt a convention where keys reflect the artifact's structural role (scope, findings, plan, etc.) rather than repeating command and topic information already captured by the session.
- The number of unique keys per session will stay under a few hundred based on current wip/ artifact patterns. No need for indexed lookup or database-style querying.
- Key conventions will be documented per-template or per-skill, not enforced by koto's core. Koto validates format (characters, path safety) but not semantics.

## Rejected

### Option 2: Flat string keys
Retains redundant naming noise, lacks natural grouping for prefix-based queries, and doesn't simplify implementation meaningfully given that Decision 8 already requires a manifest.

### Option 3: Structured metadata keys
Over-engineers the querying model for patterns that prefix matching handles adequately. Forces a schema on heterogeneous skill naming patterns without a concrete benefit today.
