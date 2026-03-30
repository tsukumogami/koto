# Security Review: koto-next-output-contract

## Dimension Analysis

### External Artifact Handling

**Applies:** No

This design does not introduce any new external artifact handling. The changes are entirely within the serialization, error classification, and response field population layers. The inputs to `koto next` remain the same: a local JSONL state file and a locally cached compiled template. The `<!-- details -->` marker is parsed from template files that have already been loaded and compiled -- no new download, fetch, or external input processing is added.

The one area that touches external execution -- gate commands (e.g., `./check-ci.sh`) -- is pre-existing behavior. The design threads gate results through `StopReason::EvidenceRequired` but does not change how gates are evaluated or what commands are executed. The `blocking_conditions` field surfaces data that was already computed; it doesn't introduce new command execution paths.

### Permission Scope

**Applies:** No

The design does not change the filesystem, network, or process permissions required by `koto next`. Specifically:

- **Filesystem**: The state file (`.jsonl`) is already read with `read_events` and written with `append_event`. The new `derive_visit_counts` function operates on the same `&[Event]` slice already loaded in memory -- no additional file access. State files are created with mode 0600 (owner-only), and this doesn't change.
- **Network**: No network access is introduced. Gate commands that happen to make network calls are pre-existing behavior.
- **Process**: No new subprocesses are spawned. The `--full` CLI flag is a read-side override that skips the visit count check during response serialization -- it doesn't escalate privileges.
- **Template compilation**: The `<!-- details -->` splitting happens in `extract_directives`, which operates on an in-memory string. No new file reads.

### Supply Chain or Dependency Trust

**Applies:** No

The design is purely internal refactoring of serialization and error classification. No new crate dependencies are added. The changes use existing standard library types (`HashMap`, `BTreeMap`, `Option`) and existing project types (`GateResult`, `Event`, `EventPayload`). The `serde` and `serde_json` crates already in use handle all serialization.

Template files are authored by the user or fetched from a local cache that was populated during `koto init`. The `<!-- details -->` marker is an HTML comment parsed by the existing compiler -- it doesn't pull in any new parsing libraries or external content.

### Data Exposure

**Applies:** Yes -- low severity, informational only.

**Risk**: The `blocking_conditions` field on `EvidenceRequired` responses exposes gate evaluation results that were previously discarded. This includes gate names, types, and status strings. For `command`-type gates, the gate name and pass/fail status are surfaced. This is intentional (the whole point of the design), but it does increase the information available in `koto next` JSON output.

**Assessment**: Low severity. The data exposed is:
1. Gate names and types -- these are already defined in the template file, which the caller loaded.
2. Gate pass/fail status -- this was already visible via the `GateBlocked` response variant for states without `accepts` blocks. The change makes it visible for the `EvidenceRequired` variant too.
3. The `details` field contains template author content (instructions for AI agents). This is not user data or secrets -- it's the same content that was previously returned as part of `directive` on every call.

**Mitigations already in place**:
- State files are created with 0600 permissions (owner-only read/write).
- `koto next` output goes to stdout of the calling process. No new transmission channels are created.
- Template content is authored by the workflow creator, not sourced from untrusted input.

**Additional consideration**: The `derive_visit_counts` function scans the full event log, which contains state names and transition history. This data stays in memory and flows to the response serializer only as a count (integer), not as the raw event data. No new data leaks from the event log to stdout beyond what's already present.

No mitigations are needed. The data exposure is by design and appropriately scoped.

## Recommended Outcome

**OPTION 3: N/A with justification**

All four security dimensions are either not applicable or applicable at informational-only severity with no action needed. The design is a serialization-layer refactoring that does not introduce new external inputs, change permission boundaries, add dependencies, or expose sensitive data. The one applicable dimension (data exposure) is the explicit goal of the design -- surfacing gate results that were previously computed and discarded -- and the exposed data is template-authored content, not user secrets.

## Summary

This design carries no meaningful security risk. It restructures how `koto next` serializes responses and classifies errors, operating entirely on data already loaded in memory from local files with owner-only permissions. The new `blocking_conditions` and `details` fields expose template-authored content and gate evaluation results that were previously computed but discarded -- this is the design's intended behavior, not an unintended leak. No new external inputs, dependencies, network access, or privilege changes are introduced.
