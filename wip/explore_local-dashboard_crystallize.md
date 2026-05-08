# Crystallize Decision: local-dashboard

## Chosen Type

PRD

## Rationale

The exploration established what the feature should do (single coherent feature emerged, requirements were entirely unclear before exploration), but no written requirements contract exists. The issue is literally titled "docs(prd): local dashboard" with the goal "Write the PRD for F3." Requirements emerged during exploration — rendering approach, invocation model, gate display scope, daemon vs. ad-hoc — none of these were given as input. A PRD must capture them before design work begins.

Design Doc was the alternative but was demoted for having the anti-signal "what to build is still unclear": the WHAT (full requirements) hasn't been documented yet. Design Doc follows PRD, not precedes it.

## Signal Evidence

### Signals Present (PRD)

- **Single coherent feature emerged from exploration**: F3 Local Dashboard is a single bounded feature with clear scope — TUI-based session viewer for koto workflow hierarchies.
- **Requirements were unclear or contested**: Rendering approach, invocation model (ad-hoc vs. daemon), gate display scope, live update mechanism — all were open questions that exploration resolved.
- **Core question is "what should we build and why?"**: Issue #366 is explicitly a PRD authorship task. The exploration produced the raw material; the PRD captures it.
- **User stories or acceptance criteria are missing**: No PRD, no design doc, no ACs exist for the local dashboard. Only the issue's high-level goal and the exploration's findings.

### Anti-Signals Checked (PRD)

- **Requirements were provided as input to the exploration**: Not present. Requirements emerged from the exploration itself (rendering approach, hierarchy view design, gate display, daemon decision all discovered in Rounds 1-2).
- **Multiple independent features**: Not present. Single coherent feature.

## Alternatives Considered

- **Design Doc**: 5 signals but demoted. Anti-signal "what to build is still unclear (route to PRD first)" fires — the requirements haven't been written down yet. Design follows PRD.
- **Plan**: −2. No upstream artifact exists; approach was still being debated in Round 1.
- **No Artifact**: −3. Multiple decisions were made during exploration that must be captured permanently; others will need documentation to build from.

## Key Decisions to Carry Into PRD

From exploration rounds:
1. Rendering: Terminal UI (ratatui), not web server or native desktop
2. Invocation: Ad-hoc `koto dashboard [<name>]`, not daemon — JSONL logs always accumulate so mid-session launch gives full history
3. Live updates: File polling (~500ms), inotify as optional V2 optimization
4. Gate display: In scope — Tier 2 `gate_evaluated` events, client-side last-result computation per state
5. Session discovery: Scan `~/.koto/sessions/<repo-id>/` for current repo
6. Daemon mode: Explicitly deferred V2; command interface must not preclude future daemon integration
7. Target use case: Monitor hours-long orchestration pipelines (the full explore→prd→design→plan→work-on sequence when it becomes koto-managed)
