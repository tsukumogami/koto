---
schema: design/v1
---

# DESIGN: An external event sink for koto's session records

## Status

Proposed

## Context and Problem Statement

koto writes one run-journal line per session event to `~/.koto/_run_journal.jsonl`,
and a separate host job later copies that file and forwards it. That hop is
being removed. koto will hand its records to an external event sink, a separate
executable at a fixed path, at defined points in each command
(tsukumogami/koto#544). The journal keeps running unchanged in this release;
release B removes it (tsukumogami/koto#545).

The sink's contract is fixed upstream and isn't reopened here: discovery at
`$HOME/.local/libexec/fleet/event-sink` with owner, mode, symlink and
parent-directory checks; `FLEET_EVENT_SINK=off` as the only switch; the call
`event-sink send --scope koto-journal [--carry N]` with NDJSON on stdin; exit
codes 0, 1, 2, 3 and 4; at most 200 records a call; one 3.0 s deadline per
flush with a process-group kill; a circuit breaker on exit 3, a kill or exit 2;
and five flush points. What this design settles is how koto does it: where a
journal write becomes an enqueue, the exact code site of each flush point, how
the deadline and kill are built, the carry sidecar and its lock, the exit-hook
list and the `exit(code)` helper, the stamps on the queued copy, the module
moves, `koto session identity`, the `run_journal.enabled` key, the stub-sink
test harness, and which parts ship in release A and which in release B.

Today every record goes through one function, `run_journal::write`
(`write_within` for the `response` record), called from five hooks:
`after_init`, `after_commit`, `terminal`, `response` and `imported`. Commands
leave through about two dozen direct `std::process::exit` calls and one
`OnceLock` exit flush (`EXIT_FLUSH` in `src/cli/mod.rs`) that `koto next` and
the `--workspace` read path each set for their own cleanup.

## Decision Drivers

- The sink contract is fixed: discovery, call shape, exit codes, the 200-record
  chunk, the 3.0 s deadline and the circuit breaker are designed against, not
  revisited.
- Telemetry never fails, retries or blocks a command past the 3.0 s bound, and
  no flush sits inside an action's 30 s timer.
- The journal stays byte-for-byte as today in release A; the queued copy alone
  carries the new stamps.
- Inert until install: with no sink at the fixed path koto makes no call and
  does no extra work beyond one discovery check per process. Release A is cut
  before 2026-11-09 and installed on hosts only after 2026-11-09T02:00Z.
- `context_reads` records never reach the sink.
- The release date is tight, so the build reuses what koto has (the record
  builders, the process-group kill in `src/action.rs`, the existing session
  sidecar) rather than adding new machinery.
- Tests never reach a real sink: every test that exercises sending does so
  through a stub at the fixed path under a scratch HOME.
