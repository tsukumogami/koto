# Exploration Findings: local-dashboard

## Core Question

What are the requirements for a locally-running dashboard that gives koto users visibility into workflow sessions? The scope spans rendering approach, session hierarchy display, live-update behavior, and invocation model — sufficient detail for a PRD that implementers can build against without guesswork.

## Round 1

### Key Insights

- **TUI (ratatui) is the clear MVP rendering choice.** koto is purely synchronous; ratatui adds one pure-Rust dependency and reuses all existing state functions. Embedded web requires tokio (first async code in the project). Native desktop has no distribution story. *(rendering-approach)*
- **Parent-pointer hierarchy: wide, not deep.** `parent_workflow` in each child's header is the only link; a tree requires scanning all sessions and grouping by parent. Max depth ~3 levels; dominant pattern is 1 parent + up to 1000 sibling tasks with DAG dependencies (`waits_on`). *(hierarchy-view, hierarchy-complexity)*
- **Session files are at stable, predictable paths.** `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`, fsynced on every write. Safe for tailing. No watch/serve infrastructure exists today. *(live-updates, invocation-discovery)*
- **Gate evaluations require Tier 2 scanning.** F2 has `gate_evaluated` events (Tier 2) but no summary field. "Last gate result" must be computed by scanning events per state. Achievable, but adds client-side logic. *(hierarchy-view)*
- **No `workflow_completed` event — terminal state is indirect.** Dashboards must load the compiled template to compare `transitioned.to` against terminal states. Known F2 gap. *(live-updates)*
- **Demand is organizationally valid.** F3 is formally sequenced as precondition for F5/F6. No external user requests. Infrastructure work, same as F2. *(adversarial-demand)*
- **Zero existing UI infrastructure.** No dashboard, watch, serve, or daemon in koto today. Entirely greenfield. *(rendering-approach, invocation-discovery)*

### Tensions

- **Tier 2 gate display in MVP vs. deferred.** Issue explicitly lists "gate evaluations" as display target; F2 has the data but it requires Tier 2 scanning. In scope is achievable; deferring narrows MVP significantly.
- **TUI at large batch widths.** 1000-sibling batches need aggregated summary rows + drill-down. PRD must specify this display pattern.
- **Terminal state detection.** No `workflow_completed` event; dashboards need template coupling or a timeout heuristic. PRD must choose.

### Gaps

- Update latency expectation not yet specified (drives polling vs. inotify).
- Single-session vs. repo-wide vs. always-on daemon not yet decided.
- Daemon/service architecture not yet investigated.

### User Focus

User selected "Explore invocation model and scope" and raised daemon mode (`systemd start/stop`) as an architectural question. This is the key open question for Round 2.

## Decision: Crystallize

## Round 2

### Key Insights

- **Daemon mode is architecturally viable but not needed for F3.** koto sessions are typically short (2-30 minutes for simple/medium workflows). Even batch runs lasting hours are agent-driven — no automatic background scheduling requires always-on monitoring. JSONL logs accumulate from `koto init` regardless of dashboard state, so a mid-session launch still shows full history. *(session-lifecycle)*
- **Ad-hoc TUI is the correct MVP invocation model.** git instaweb is the closest analogue: data always accumulates; the dashboard is a temporary viewing layer. Daemon mode is V2 — right for batch supervisor use cases (100+ children, hour-long runs) when formally scoped. *(comparable-tools)*
- **Daemon doesn't require tokio.** A daemon can be implemented with `std::thread` and synchronous Unix socket I/O — no async runtime required. The architecture stays simple; daemon mode is not a dependency escalation. *(daemon-vs-adhoc)*
- **Signal handling is already production-grade.** koto has `signal-hook` + `AtomicBool` shutdown flag in the advance loop. Daemon mode reuses this infrastructure. *(daemon-vs-adhoc)*
- **No automatic batch scheduling.** The parent agent must explicitly re-tick to advance child workflows. "Always-on dashboard" for batch supervision is compelling but not yet a formally defined use case. *(session-lifecycle)*

### Decisions Made This Round

- Daemon mode deferred to V2. PRD should note compatibility intent (command interface should not preclude future daemon integration).
- Ad-hoc TUI (`koto dashboard [<name>]`) is the F3 invocation model.
- Single-repo scope (current working directory) as the default session discovery context.

## Accumulated Understanding

koto is a purely synchronous CLI with sessions stored at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`, fsynced on every write. Sessions are typically short (2-30 minutes) though batches can run for hours. The JSONL log accumulates from `koto init` regardless of dashboard state — "always-on" is a convenience, not a data-completeness requirement.

The F3 MVP is an ad-hoc TUI command (`koto dashboard`): launches ratatui, discovers sessions for the current repo, shows hierarchy (root → batch coordinator → sibling tasks), polls for updates at ~500ms, exits on user command. Gate display (Tier 2 scanning) is in scope per the issue's requirements. Daemon mode is explicitly deferred to V2 with the command interface designed to accommodate it later.

The VISION goal — "first tangible observability experience" that makes F5/F6's value legible — is achievable with this scope. A user watching a batch coordinator's 100 children progress in a TUI understands exactly what remote dashboard access would give them.
