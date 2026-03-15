# Crystallize: event-log-format

## Artifact Type

Design document — `docs/designs/DESIGN-event-log-format.md`

## Rationale

Issue #46 has a `needs-design` label. The requirement is explicit: produce an accepted
design doc before implementation. All six research leads have been investigated and
all key decisions are made. The design doc is written and ready for review.

## Key Decisions Captured

1. Sequence gaps → halt-and-error (not skip)
2. `rewound` events included in state derivation rule (fixes upstream ambiguity)
3. All state-changing events create evidence epochs (fixes upstream ambiguity)
4. Header: four fields only (no template_path, no cached current_state)
5. `koto workflows` returns objects with header metadata (breaking change from #45)
6. `koto query` deferred to post-#49
7. mode 0600 + sync_data() after every write
8. Writer-managed seq assignment

## Handoff

Design doc written. No additional artifact needed — proceeding to PR.
