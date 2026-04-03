<!-- decision:start id="agents-md-structure" status="assumed" -->
### Decision: Root AGENTS.md Structure

**Context**

Claude Code auto-loads `AGENTS.md` files from the working directory and its ancestors. A root `AGENTS.md` at `koto/` will be loaded for every session in the repository — writing templates, running workflows, implementing engine features, reviewing PRs. The existing `plugins/koto-skills/AGENTS.md` (550 lines) is only loaded when working inside the plugin directory, making it invisible to most sessions and far too long for root-level auto-loading.

The replacement must fit in 80 lines, name five specific commands, reference both skills, and point to `docs/guides/cli-usage.md`. Its purpose is orientation, not reference. All depth lives in the skill files and the CLI guide.

Three candidate structures were evaluated: (A) command quick-reference table with skill pointers, (B) conceptual prose overview with skill pointers, and (C) minimal prose with skill pointers only.

**Assumptions**

- Agents starting in the koto directory have varying levels of prior koto context — some are cold-starting, others are returning to a session. The file must serve both without over-serving either.
- The koto-user skill (being created alongside this decision) will provide sufficient depth for workflow runners, so the root file doesn't need to duplicate that guidance.
- A one-sentence conceptual framing ("koto is a state machine engine for AI agent workflows") is adequate for cold-start orientation — the command table communicates what koto does by showing what you can do with it.

**Chosen: Alternative A — Command quick-reference table + skill pointers**

A compact table listing the five required commands with one-line descriptions, preceded by a single orientation sentence, followed by skill pointers and the docs link. The structure uses roughly 40-50 lines, staying well within the 80-line budget and leaving room for a short "which skill to use" guide.

The table format satisfies the explicit constraint on command naming while communicating the command vocabulary more precisely than prose. A cold-start agent reading five command names plus one-line descriptions immediately understands koto's execution model — init a workflow, get the next directive, record overrides, roll back, and list active workflows. This is orientation through concrete vocabulary, not abstract explanation.

**Rationale**

Alternative A delivers the densest orientation per line. The 80-line constraint is tight; a command table communicates the "what is koto" question implicitly through its verbs, leaving the remaining budget for skill pointers and the docs link. Alternative B inverts the priority — spending more lines on prose that agents can infer from context, while giving less precision on syntax. Alternative C is too sparse: an agent that doesn't know whether to load koto-author or koto-user has not been oriented, regardless of how many pointers are listed.

The constraint "must name these commands explicitly" already pushes toward a table. A table is the most compact structure that names, describes, and shows the syntax of multiple commands simultaneously.

**Alternatives Considered**

- **Alternative B — Conceptual overview + skill pointers**: Allocates 15-25 lines to prose before reaching commands. The prose is harder to scan than a table and uses more budget to communicate the same information less precisely. An agent that needs deeper conceptual grounding should follow the skill pointer, not read more prose in AGENTS.md. Rejected because orientation through verbs (table) is more efficient than orientation through nouns (prose).

- **Alternative C — Minimal prose + skill pointers only**: Fails the "must name these commands explicitly" constraint cleanly unless commands are listed in prose, at which point it becomes longer than a table anyway. More critically, an agent that doesn't know if it's authoring templates or running workflows won't know which skill to load without at least a brief "which skill" guide. Rejected because pointer-only content doesn't provide enough orientation to route the agent correctly.

**Consequences**

The root file will be roughly 40-50 lines — well inside budget — which allows room for a "which skill to use" heuristic (2-4 lines) that routes agents to koto-author vs koto-user based on their task. The command table doubles as a fast check that koto is installed and the agent is using the right syntax. Sessions that need depth follow the skill pointers or the docs link; they don't re-read AGENTS.md.
<!-- decision:end -->
