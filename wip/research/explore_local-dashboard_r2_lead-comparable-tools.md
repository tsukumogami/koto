# Lead: Comparable tool patterns for local observability dashboards

## Findings

### Comparable Tool Survey

**k9s (Kubernetes CLI dashboard):**
- Ad-hoc TUI launched per-use. Single binary, no daemon.
- Works because Kubernetes state is external and always-on — the user's dashboard doesn't need to be running for state to accumulate.
- User launches k9s when they want to look; exits when done. No persistence between sessions.
- Matches "single binary" constraint well.

**Grafana Agent:**
- Long-running daemon with local web UI. Typically managed by systemd or Docker.
- Not a single binary anymore — adds deployment complexity and service management lifecycle.
- Compelling for always-available observability when users can't predict when they'll need visibility.
- Overkill for a single-machine developer tool unless the team explicitly wants the always-on pattern.

**cargo-watch / watchexec:**
- Ad-hoc file watchers. Fire commands on file change.
- Still require pre-launch. Don't solve "available without having started it first."
- Single binary; lightweight. Relevant as a pattern for the file-watching mechanism, not the invocation model.

**git instaweb:**
- Ad-hoc web server launched on demand. Opens browser, serves git web UI until Ctrl+C.
- "Bridging pattern": interactive invocation of always-available data.
- Works because the data (git history) is always present; the user just needs a temporary viewing session.
- Closest comparable to koto's situation: data is always accumulating in JSONL files; the dashboard is a temporary viewing layer.

### Signal Handling in koto

koto already has signal handling infrastructure in the advance loop: an `AtomicBool` shutdown flag checked at each iteration, with `signal-hook` as a dependency. This infrastructure is currently scoped to a single CLI invocation but is architecturally compatible with a longer-lived daemon if needed.

### Key Constraint: Single Binary Distribution

koto distributes as a single binary. A mandatory separate daemon binary (Grafana pattern) would require:
- A separate build target
- A separate install step
- systemd/launchd management documentation
- IPC between the daemon and the CLI

An optional background mode embedded in the same binary (e.g., `koto daemon start`) is feasible — the daemon is just the same binary running with a different subcommand. systemd/launchd service files reference the same binary path.

### The "Available Without Starting" Problem

Long-running AI coding sessions create an asymmetry: users may not think to launch the dashboard before starting a workflow. If a session runs for 2 hours and the user wants to check progress at the 90-minute mark, an ad-hoc tool requires retroactive launch — but the JSONL files already contain the full history from minute 0. The dashboard can always reconstruct state from the log, regardless of when it was started.

This means "available without starting" is less critical for koto than it would be for a streaming-only observability system. The JSONL log is append-only and always-present; launching the dashboard mid-session still shows full history. The argument for always-on is convenience, not data completeness.

## Implications

1. **git instaweb is the closest analogue.** Data always accumulates; the dashboard is a temporary viewer. This favors ad-hoc invocation as the baseline.
2. **Optional daemon mode is low-cost to add later.** The same binary, a new subcommand, a sample systemd unit file in docs/. This is a V2 feature, not required for MVP.
3. **Single binary constraint rules out a mandatory separate daemon binary.** The MVP should be ad-hoc (`koto dashboard`) with daemon mode explicitly deferred to a future iteration if demand emerges.
4. **Existing signal handling infrastructure makes daemon mode feasible when needed.** The advance loop already has shutdown coordination.

## Surprises

Signal handling (`signal-hook`, `AtomicBool` shutdown flag) already exists in the codebase. This was not expected for a stateless CLI and reduces the daemon implementation cost if it's ever needed.

The "available without starting" problem is less severe for koto than it appears — JSONL logs are always written from `koto init` forward, so a dashboard launched mid-session still has full history.

## Open Questions

1. Does the product vision require always-on availability, or is "launch when you need it" sufficient for F3?
2. Should the PRD explicitly defer daemon mode to V2, or leave it open for the design doc?
3. If daemon mode is deferred, should the PRD specify a command interface compatible with future daemon mode (e.g., `koto dashboard` that could later become `koto daemon start`)?

## Summary

Comparable tools show three patterns: ad-hoc TUI (k9s), ad-hoc web server (git instaweb), and always-on daemon (Grafana Agent). koto's JSONL-based session storage makes ad-hoc the natural baseline — data accumulates regardless of dashboard state, so mid-session launch still shows full history. Single-binary constraint rules out a mandatory separate daemon, but optional daemon mode is feasible with the existing signal-handling infrastructure and can be added as V2 if demand emerges.
