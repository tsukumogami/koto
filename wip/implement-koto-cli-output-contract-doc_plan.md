# Documentation Plan: koto-cli-output-contract

Generated from: docs/plans/PLAN-koto-cli-output-contract.md
Issues analyzed: 4
Total entries: 3

---

## doc-1: docs/guides/cli-usage.md
**Section**: next
**Prerequisite issues**: #1, #4
**Update type**: modify
**Status**: pending
**Details**: Rewrite the `next` command section to document `--with-data <json>` and `--to <target>` flags, the five JSON response variants (EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable, Terminal), mutual exclusivity of flags, payload size limit (1MB), and the new field presence table. Update the "Typical agent workflow" section to show evidence submission and directed transitions instead of the placeholder loop. Remove both "Note" callouts about transitions being unavailable.

---

## doc-2: docs/reference/error-codes.md
**Section**: next
**Prerequisite issues**: #1, #4
**Update type**: modify
**Status**: pending
**Details**: Add the six structured domain error codes for `koto next` (gate_blocked, invalid_submission, precondition_failed, integration_unavailable, terminal_state, workflow_not_initialized) with the new `{"error": {"code": "...", "message": "...", "details": [...]}}` JSON shape. Document the exit code mapping (0 success, 1 transient, 2 caller error, 3 config error). Explain the two error paths: pre-dispatch I/O errors keep the existing flat format, domain errors use the new structured format.

---

## doc-3: README.md
**Section**: Quick start, Agent integration
**Prerequisite issues**: #1, #4
**Update type**: modify
**Status**: pending
**Details**: Update the `koto next` output example in section 3 ("Get the current directive") to show the new response format with `action`, `advanced`, `expects`, and `error` fields. Remove the "Note" callout about `koto transition` being unavailable. Update the agent integration loop (steps 1-4) to mention evidence submission via `--with-data` and directed transitions via `--to`.
