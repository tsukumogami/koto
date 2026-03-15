# Documentation Plan: event-log-format

Generated from: docs/plans/PLAN-event-log-format.md
Issues analyzed: 4
Total entries: 4

---

## doc-1: README.md
**Section**: (multiple sections)
**Prerequisite issues**: Issue 3
**Update type**: modify
**Status**: done
**Details**: Update "Quick start" section 2 (`koto init`) output example to reflect the new format (header + events instead of simple JSON). Update "Key concepts" section: state files now have a header line followed by typed events with sequence numbers; current state is derived by log replay (not from the last event's `state` field). Update `koto workflows` mention if present to note it returns objects, not strings.

---

## doc-2: docs/guides/cli-usage.md
**Section**: (multiple sections)
**Prerequisite issues**: Issue 3
**Update type**: modify
**Status**: done
**Details**: Update "State file resolution" section to describe the new format: header line with schema_version/workflow/template_hash/created_at, followed by typed events with seq numbers. Change "current state is the `state` field of the last event" to "current state is derived by replaying the log." Update `init` command section: output unchanged but note that state file now starts with a header line plus `workflow_initialized` and initial `transitioned` events (3 lines). Update `rewind` command section: rewind now appends a `rewound` event with `from`/`to` payload. Update `workflows` command section: output changes from string array `["name"]` to object array `[{"name":"...","created_at":"...","template_hash":"..."}]`.

---

## doc-3: docs/reference/error-codes.md
**Section**: next, init
**Prerequisite issues**: Issue 3
**Update type**: modify
**Status**: done
**Details**: Add new error conditions for format detection: old Go format state file rejected (exit code 3), old #45 simple JSONL format rejected (exit code 3), and sequence gap corruption (exit code 3 with `state_file_corrupted` error). Update the existing "Corrupt state file" entry under `next` to mention sequence gap detection and truncated final line recovery. These are the three-tier format detection errors from the design.

---

## doc-4: docs/guides/cli-usage.md
**Section**: Typical agent workflow
**Prerequisite issues**: Issue 3
**Update type**: modify
**Status**: skipped
**Details**: The workflow loop example references `.directive` and `.transitions` from `koto next` output, which doesn't change. However, the note at the bottom about `koto transition` not being available should remain as-is since transitions are still deferred. No content change needed unless Issue 3 alters the `koto next` JSON output shape -- verify after implementation and skip if unchanged.
**Skip reason**: Verified that `koto next` JSON output shape is unchanged (still `state`, `directive`, `transitions`). No doc update needed.
