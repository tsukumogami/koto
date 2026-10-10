# /brief Discovery: session-view

## Problem Candidate
A person responsible for a koto session — an operator checking on a long-running
workflow, a teammate picking up someone else's run, a maintainer debugging a
stuck one — has no way to read what the session actually holds. The session's
working state lives in its context keys, but every existing surface stops short
of content: `koto context list` prints only key names, `koto context get`
dumps one key's raw bytes (unusable for binary or large values and only one key
at a time), and `koto status` and the dashboard show the state machine's
position and directive but nothing of what the session has accumulated. To
answer "what is this session working with, and how much of it?" a person today
has to script their own loop over keys and hope none of the values wrecks their
terminal.

## Outcome Candidate
A person opens the session in a surface koto already ships — the local
dashboard, or the workflow view inside their coding session — and reads the
session's working state the way they'd read a status page: where it stands
(state and directive), where it runs (execution anchor, store origin), and
everything it holds (every context key with its size and its content shown
legibly — large or binary values as size, type and a bounded excerpt, never a
byte dump). When something is absent, unreadable, or lives on a migrated
session, the view says so plainly instead of failing or showing a hole.

## Grounding Anchor
conversation only (the /explore handoff at wip/scope_session-view_handoff.md
carries the exploration's findings and the maintainers' recorded constraints of
2026-10-10: two existing surfaces only, no new verb/subcommand/surface)

## Journey Sketch
- An operator running several koto-backed workflows opens the dashboard,
  focuses one session, and reads its full working state: keys, sizes, content.
- An agent-session owner glances at the Claude Code workflow view and sees, per
  session, a compact summary of what it holds (key names, sizes, counts,
  anchor, store kind) alongside the phases it already shows.
- A teammate inspecting a session from a script runs the dashboard's existing
  --once mode with a detail flag and gets a parseable, bounded rendering of one
  session's contents.
- A person pointed at a migrated session is told its newer name and workspace
  instead of seeing empty or erroring output.

## Open Questions for Drafting
- Exact wording for "remote": the session's store origin (local vs cloud and
  its base), since no git remote is recorded anywhere.
- The excerpt bound, binary criterion and size-unit style are design-level
  parameters; the brief should state the legibility requirement without
  pinning numbers.
