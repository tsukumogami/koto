# Lead: What query interface lets a parent read child workflow data without coupling?

## Findings

### Current query surface

koto has no dedicated "query" subcommand. The agent's window into workflow state comes through several commands:

1. **`koto next <name>`** -- the primary interaction point. Returns a `NextResponse` JSON with the current state name, directive, details, expects schema, blocking conditions, and whether the engine advanced. Defined in `src/cli/next_types.rs` as an enum with six variants: `EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`, `Terminal`, `ActionRequiresConfirmation`. This is the agent's main loop driver, not a read-only query.

2. **`koto workflows`** -- lists all active workflows as a JSON array of `WorkflowMetadata` objects: `{name, created_at, template_hash}`. Implemented in `src/discover.rs` via `find_workflows_with_metadata()`. Returns only header-level metadata; no current state or evidence.

3. **`koto decisions list <name>`** -- returns decisions recorded in the current state epoch. Uses `derive_decisions()` from `src/engine/persistence.rs`, which replays the JSONL event log and returns `DecisionRecorded` events after the most recent state-change boundary.

4. **`koto session dir <name>`** / **`koto session list`** -- session directory paths and listing. Low-level plumbing.

5. **`koto context get <session> <key>`** -- retrieves stored context artifacts. The `ContextStore` trait (`src/session/context.rs`) supports hierarchical keys, add/get/exists/remove/list_keys. This is a general key-value store scoped to a session.

6. **State file internals** -- `src/engine/persistence.rs` provides `derive_state_from_log()`, `derive_evidence()`, `derive_machine_state()`, `derive_decisions()`, and `derive_overrides()`. These all replay the JSONL event log (`koto-<name>.state.jsonl`). They are library functions, not exposed as CLI subcommands.

### Key structural facts

- **No parent-child relationship exists today.** There's no `parent` field in `StateFileHeader`, no child tracking in events, and no cross-workflow query capability.
- **Each workflow is fully isolated.** The `SessionBackend` trait operates on a single session ID. `read_events()` reads one workflow's log. There's no join or cross-reference.
- **Evidence is ephemeral to the current state epoch.** `derive_evidence()` only returns evidence submitted after the most recent state-change event for the current state. Once the workflow transitions, prior evidence is still in the log but not surfaced by the derive functions.
- **The context store is per-session.** `ContextStore` methods take `(session, key)`. There's no cross-session context access.
- **Workflow names are validated strictly** (`src/discover.rs:validate_workflow_name`): alphanumeric start, then `[a-zA-Z0-9._-]`. This allows naming conventions like `explore.child-1` to encode hierarchy.

### Option analysis

#### Option A: Extend existing query (`koto query <parent> --children`)

Would require:
- A new `query` subcommand (none exists today)
- A parent-child relationship model (header field or naming convention)
- Cross-session reads within a single CLI invocation

**Data accessible:** Parent state + child summaries (state, evidence, decisions). Would need to call `derive_state_from_log` and `derive_evidence` for each child session.

**Coupling:** Moderate. Parent template doesn't need to know child templates, but the query response shape embeds child state names that come from child templates.

**Agent usage:** Single command to get a dashboard view. Good for "check all children" pattern.

**Assessment:** Packs a lot into one command. The response shape would be large and variable depending on child count. Hard to paginate or filter.

#### Option B: Separate children command (`koto children <parent>`, then `koto query <child>`)

Would require:
- A `children` subcommand that lists child workflows for a parent
- Some mechanism to establish the parent-child link (naming convention or header metadata)
- Individual `koto query <child>` (or existing `koto next --no-advance` equivalent) for details

**Data accessible:** Same as Option A, but retrieved in two steps.

**Coupling:** Low. Parent just discovers child names and queries each one independently. Child templates are fully opaque to the parent.

**Agent usage:** Two-step: list children, then query each. More commands but each is simpler and composable. Agent can query only the children it cares about.

**Assessment:** Fits koto's existing pattern of small, composable commands. The `koto workflows` command already returns a list; `koto children <parent>` would be a filtered version. Individual child queries use the same machinery as any workflow query.

#### Option C: Tree query (`koto query <parent> --tree`)

Would require:
- Everything from Option A, plus recursive child-of-child resolution
- A potentially unbounded response for deep hierarchies

**Data accessible:** Full hierarchy state in one call.

**Coupling:** High. Response shape depends on the full tree structure. Any template change anywhere affects the output.

**Agent usage:** One-shot view of everything. But agents operate with limited context windows; a tree dump for 5+ children with evidence would easily exceed useful size.

**Assessment:** Premature. The use case describes one level of hierarchy (parent spawns children). Building for arbitrary depth adds complexity without demonstrated need. YAGNI.

#### Option D: Evidence bridge (child completion auto-submits to parent)

Would require:
- A mechanism for child workflows to know their parent's name and current state
- Auto-submission of a summary as parent evidence upon child terminal state
- New evidence event type or annotation marking it as "from child"

**Data accessible:** Only what the child's terminal summary includes. Parent gets a curated view, not raw child state.

**Coupling:** Tight coupling at the data contract level. The child template's terminal state must produce evidence matching what the parent expects. Parent and child templates must agree on the summary schema.

**Agent usage:** Zero-query for the common case: parent just sees child results arrive as evidence. But if the parent needs to check child progress before completion, this approach is insufficient alone.

**Assessment:** Attractive for the "children complete and report back" case, but insufficient for the "parent checks on in-progress children" case. Works well as a complement to Option B, not a replacement.

### How each option handles the exploration use case

The stated use case: parent exploration spawns child experiments, children complete and report summaries, parent reads child evidence when it needs details.

- **Option A:** Parent runs `koto query explore --children` and gets all child state + evidence. One command, but potentially massive output.
- **Option B:** Parent runs `koto children explore` to list children, then `koto next <child>` or a new `koto query <child>` for any child it wants detail on. Matches the "read evidence when needed" pattern naturally.
- **Option C:** Overkill for one-level hierarchy.
- **Option D:** Children auto-submit summaries as parent evidence. Parent gets results without querying. But "needs details about what children did" requires falling back to B-style individual queries anyway.

### Recommended approach: Option B + Option D as complementary layers

**B provides the query path:** `koto children <parent>` lists children with their current state. For details, the agent uses `koto context get <child> <key>` or reads child evidence through a new `koto query <child>` subcommand.

**D provides the push path:** When a child hits its terminal state, the orchestrating agent (not koto itself) submits a summary to the parent via `koto next <parent> --with-data '{"child": "name", "outcome": "..."}'`. This keeps koto out of the cross-workflow data flow -- the agent does the stitching.

This separation means koto only needs two new things:
1. A way to establish parent-child links (naming convention or `--parent` flag on init)
2. A `koto children <parent>` command to discover the link

## Implications

The query interface decision directly shapes how much complexity enters koto's core. The current codebase has zero cross-workflow awareness -- every function in `persistence.rs` and `session/mod.rs` operates on a single workflow. Adding cross-workflow queries means either:

- **Naming convention approach:** No schema changes. Children named `<parent>.<child-suffix>` can be discovered by prefix-filtering `koto workflows` output. This can ship without any koto code changes -- a `koto workflows | jq 'map(select(.name | startswith("explore.")))'` does it today.

- **Metadata approach:** Add `parent` to `StateFileHeader` (set via `koto init --parent <name>`). More explicit but requires schema version bump and new header parsing.

The naming convention approach is sufficient for the initial use case and requires no koto changes. A `koto children` convenience command could be added later when the pattern proves out.

For the agent, the practical interface is: name children with a parent prefix, filter `koto workflows` output, and use `koto context get <child> <key>` to read child artifacts. The agent already has all the tools to stitch cross-workflow data.

## Surprises

1. **There is no `koto query` command.** The CLAUDE.md mentions `koto query` as a key command, but it doesn't exist in the CLI enum. The closest thing is `koto next` (which has side effects) and `koto workflows` (which only returns metadata). The absence of a read-only query means agents can't inspect a workflow's current state without potentially advancing it. This is a gap independent of hierarchical workflows.

2. **`koto context get` is already a cross-workflow query primitive.** Since the agent knows session names, it can read any workflow's context store. If children store their results in context keys, the parent agent can read them directly via `koto context get <child-session> results.json`. No new interface needed for the data transfer.

3. **Evidence is scoped to the current state epoch and lost on transition.** `derive_evidence()` only returns evidence for the current state. This means a child workflow's evidence from intermediate states is not accessible through the derive functions -- you'd need to replay the full JSONL log. For the parent reading a completed child, the evidence from the child's final state may already be gone once the child is cleaned up.

4. **Workflow naming rules are permissive enough for hierarchy encoding.** Dots and hyphens are allowed, so `explore.r1.cli-experiment` is a valid name. This enables convention-based hierarchy without code changes.

## Open Questions

1. **Should there be a read-only query command?** `koto next` has side effects (gate evaluation, default action execution, state advancement). A `koto status <name>` that returns current state, evidence, and decisions without advancing would be useful for both parent-child queries and general observability. Is this a prerequisite for hierarchical workflows?

2. **Who manages child lifecycle cleanup?** When a child workflow completes, its session directory and state file remain until `koto session cleanup <name>` is called. If the parent agent manages child lifecycle, it needs to track which children to clean up. If koto manages it, the parent-child relationship must be explicit.

3. **How does the parent learn a child completed?** Polling `koto workflows` or `koto next <child>` is one approach. An alternative: the child agent reports back to the parent agent through whatever spawning mechanism the parent used (e.g., Claude Code subagent return value). koto doesn't need to be the notification channel.

4. **Should child evidence be preserved after cleanup?** If the parent needs child evidence after the child workflow is cleaned up, someone must copy it first. The context store (`koto context add`) could serve as the archive, but whose responsibility is it?

5. **What about the `koto query` mentioned in CLAUDE.md?** The local CLAUDE.md lists `koto query` as a key command, but it doesn't exist. Is this planned but unimplemented, or is it a documentation error? Its described behavior ("inspect full workflow state as JSON") would be exactly the read-only query primitive needed here.

## Summary

koto currently has no cross-workflow query capability and no parent-child relationship model; every function operates on a single isolated workflow. The lowest-friction interface combines convention-based naming (children use `<parent>.<suffix>`) with the existing `koto context get` primitive for cross-workflow data reads, requiring zero koto code changes for a first iteration. The biggest open question is whether koto needs a read-only `koto status` or `koto query` command -- `koto next` has side effects that make it unsuitable for observational queries, and the CLAUDE.md already documents a `koto query` command that doesn't exist in the codebase.
