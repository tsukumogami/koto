# Changelog

All notable changes to the koto crate are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project's pre-1.0 versioning treats MINOR as MAJOR per the Cargo
0.x semver convention (`0.10.0` is breaking-change-eligible relative
to `0.9.x`).

## [0.10.0] - 2026-05-24

### Request-store substrate + first stability lockdown

This release ships the request-store dispatch protocol — a
coordinated set of engine modules that let a coordinator session
discover unassigned child sessions, atomically claim them, dispatch
them to a substrate, wake their requester when they reach terminal
state, and respawn requesters whose substrate transcripts have
expired. It also freezes the first crate surface that external
substrates may import.

The changes span the request-store header fields on
`StateFileHeader`, the discovery scan with per-coordinator cursor,
the claim sidecar, the terminal-index, the audit-event family,
idempotency hashing with three-point fsync discipline, the
wake-candidates pass, F1 cold-restart re-priming, recursion caps,
and the public-surface lockdown.

See `docs/STABILITY.md` (added in this release) for the bump
protocol, the four frozen `SessionBackend` methods, and the
additive-evolution rules that apply to every public type re-exported
under `koto::engine::types::*`.

#### Operator-facing behavior change — auto-cleanup removed (load-bearing)

**koto v0.9.x auto-cleans terminal sessions; this release removes
that default.** Operators must invoke `koto workspace prune` to
reclaim disk space from completed or abandoned workflows. Without
periodic prunes, `~/.koto/sessions/` grows unbounded.

The intentional behavior change is required by the dispatch
protocol itself: the discovery scan and the terminal-index reader
both depend on terminal sessions remaining on disk long enough for
the per-coordinator cursor to advance correctly. The 7-day TTL on
coordinator cursors (`request_store.coord_cursor_ttl_days`) bounds the
horizon during which a terminal session needs to remain visible.

koto's dashboard surfaces stale-tree indicators so operators see at
a glance when prune is needed.

**Operators upgrading from 0.9.x should add `koto workspace prune` to
their periodic-maintenance script.** See the verb's documentation in
`docs/guides/cli-usage.md` for the full flag set.

#### Downstream consumer contract

The crate-surface lockdown in this release establishes the first
durable contract for downstream consumers that import from
`koto::engine::types`. The eight types frozen here — plus the four
`SessionBackend` methods marked `# Stability: Stage 1 — Frozen` —
form the import contract documented in `docs/STABILITY.md`.

External-consumer compile verification ships in this release as the
`koto-stability-tests` crate (workspace-internal, not published).
CI runs `cargo test -p koto-stability-tests` on every PR to catch
accidental breaking changes before release.

### Added

- `koto-stability-tests/` external-consumer fixture crate. Imports
  every promised export from the frozen surface and exercises the
  four frozen `SessionBackend` methods via a trait-object smoke
  test.
- `docs/STABILITY.md` — public stability contract, bump protocol,
  and additive-evolution rules.
- `docs/workspace-layout.md` — workspace dir/file layout
  reference.
- `koto::engine::types::*` re-exports for the eight frozen types:
  `StateFileHeader`, `Event`, `EventPayload`, `SpawnEntrySnapshot`,
  `ChildSnapshot`, `AssignmentClaim`, `derive_state_from_log`,
  `CURRENT_SCHEMA_VERSION`.
- `koto::error::Error` — re-exported `EngineError` alias.
- `StateFileHeader` request-store fields: `needs_agent`, `role`,
  `inputs`, `coordinator_of_record`, `requested_by`,
  `assignment_claim`, `dispatch_epoch`, `respawn_generation`, plus
  four forward-compat reserved fields.
- `Event.idempotency_hash` for retry-safe append discipline.
- `koto workspace prune --root <session> [--dry-run] [--yes] [--force]`
  CLI verb. Reclaims terminal workflow trees with a symlink-refusal
  safety gate and an interactive confirmation prompt.
- `koto next` directive return: `unassigned_children` array
  populated by the per-tick discovery scan.
- `koto next` directive return: every variant (including Terminal
  and Error) carries `unassigned_children` for uniform
  coordinator-side consumer branching.
- Discovery scan with mtime-cursor + tied-boundary seen-set rule
  + 7-day cursor TTL + cursor GC.
- Terminal-index JSONL writer + skip-malformed reader +
  compaction-lease O_EXCL sidecar with stale-recovery.
- Claim sidecar: O_EXCL + happy-path dispatch orchestration +
  four-case drift recovery.
- Audit-event family with reserved `kind` discriminator on
  `EvidenceSubmitted`: `ChildDispatched`, `ChildRedelegated`,
  `RequesterWoken`, `RequesterRespawn`. The `request_store.` prefix is
  reserved for future audit kinds.
- Idempotency-hash short-circuit + 3-point fsync discipline before
  substrate wake-delivery.
- Wake-candidates pass + age-and-activity recovery.
- F1 cold-restart re-priming + F3 fallback +
  `respawn_generation_cap`. The resume-context prompt is a
  fixed-form committed template.
- Epoch-fence validation on child-log writes.
- Recursion-cap enforcement + recursion_caps bench harness.
- Discovery scan bench harness with soft-by-default reporting at
  100/1k/10k/26k workspace sizes.
- `RequestStoreConfig` 5-level precedence cascade + reserved
  `[request_store.recursion]` warn. Eight operator-tunable dimensions:
  `stale_claim_timeout_seconds`,
  `stale_dispatch_timeout_seconds`, `redelegation_cap`,
  `coord_cursor_ttl_days`, `terminal_index_compact_lines`,
  `compact_lock_timeout_seconds`, `directive_batch_size`,
  `respawn_generation_cap`.
- `ValidatedSessionId` / `ValidatedCoordId` newtypes for security
  hardening at every public boundary.
- New typed errors: `EpochFenceViolation`,
  `RedelegationCapExceeded`, `ConcurrentSubmissionConflict`,
  `RecursionCapExceeded`, `ReservedKindCollision`,
  `InvalidSessionId`, `InvalidCoordId`.

### Changed

- **Removed auto-cleanup default** (see "Operator-facing behavior
  change" above). This is the load-bearing operator-facing change
  in this release.
- `StateFileHeader` extended with the request-store fields listed
  above (additive — pre-existing state files round-trip unchanged).
- `Event` extended with `idempotency_hash: Option<String>`
  (additive).
- `NextResponse::Terminal` and `NextResponse::Error` now carry
  `unassigned_children: Vec<UnassignedChild>`. Adds a new key in
  the JSON output; consumers that ignore unknown keys continue to
  work.
- `koto next` startup runs cursor GC, terminal-index compaction
  threshold check, and wake-candidates pass before the per-tick
  advance loop.
- `cargo workspace` layout: koto crate sits alongside
  `koto-stability-tests/` as workspace members.
- Crate version bumped 0.9.1-dev → 0.10.0 per the pre-1.0 semver
  discipline. Breaking changes to the locked surface require a
  6-week deprecation window per `docs/STABILITY.md`; additive
  evolution is permitted in minor releases.

### Stability

- **Stage 1 freeze.** Eight types under `koto::engine::types::*`
  plus `koto::error::Error` and four `SessionBackend` methods
  (`create`, `list`, `read_events`, `init_state_file`) are
  documented as the load-bearing public surface. The
  `# Stability: Stage 1 — Frozen` doc-comment marker identifies
  each. Renames, removals, and signature changes follow the
  deprecation protocol in `docs/STABILITY.md`. Adding new fields
  (to structs), new variants (to enums whose serde uses
  `#[serde(other)]` or accepts unknown keys), and new error
  variants is permitted in minor releases.
