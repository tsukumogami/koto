# Techwriter Validation: koto dashboard docs (issue 6)

## Doc plan entries reviewed

The doc plan at `wip/implement-doc_local-dashboard_doc_plan.md` specified two entries:

| ID | File | Section | Status before this pass |
|----|------|---------|------------------------|
| doc-1 | `docs/guides/cli-usage.md` | `### dashboard` (new section under Commands) | Missing — no dashboard section existed |
| doc-2 | `README.md` | **Live dashboard** bullet in Key concepts | Missing — no dashboard entry existed |

## What was missing

Neither file mentioned `koto dashboard` in any form. The cli-usage.md Commands section jumped from `config` / cloud sync to `version` with no dashboard entry. The README Key concepts section had entries for batch child spawning and cloud sync but nothing for the dashboard.

## Changes made

### docs/guides/cli-usage.md

Added `### dashboard` section immediately before `### version`. The section covers:

- Command signature: `koto dashboard [<name>] [--once] [--interval <ms>]`
- Positional argument (`<name>` optional, filters to one session)
- Flags: `--once`, `--interval <ms>`
- TUI navigation table (j/k, Enter, Escape, r, q)
- `--once` output format: tab-separated `<name>\t<current_state>\t<elapsed>\t<status_bucket>` with the five bucket values
- Note that `--once` exits 0 even when the session directory is empty
- Example invocations covering both interactive and scripting modes
- Note that `dashboard` outputs to the terminal, not JSON, distinguishing it from other commands

### README.md

Added **Live dashboard** paragraph to the Key concepts section, after the Batch child spawning entry. Describes: TUI showing sessions as a tree with state/elapsed/task counts, j/k/Enter/q navigation, and the `--once` scripting mode. Two to three sentences, matching the style of adjacent entries.

## Verification

Both files were read before editing and confirmed to have no existing dashboard content. The edits are consistent with the `--name` flag being implemented as a positional argument (per the doc plan: "positional argument (`<name>` optional, filters to a single session)").
