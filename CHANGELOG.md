# Changelog

All notable changes to the koto crate are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project's pre-1.0 versioning treats MINOR as MAJOR per the Cargo
0.x semver convention (`0.10.0` is breaking-change-eligible relative
to `0.9.x`).

## [0.10.0] - 2026-05-24

### KT1 release — request-store + stability lockdown

This release ships the **KT1 dispatch protocol** (per
`docs/designs/DESIGN-koto-request-store.md`) and freezes the first
crate surface bunki BK2 and other external substrates may import.
Twenty implementation issues land in this release, spanning the
request-store header fields, discovery scan, claim sidecar,
terminal-index, audit-event family, idempotency + fsync discipline,
wake-candidates pass, F1 cold-restart re-priming, recursion caps,
and the Stage 1 stability lockdown.

See `docs/STABILITY.md` (added in this release) for the bump
protocol, the four frozen `SessionBackend` methods, and the
additive-evolution rules that apply to every public type re-exported
under `koto::engine::types::*`.

#### Operator-facing behavior change — auto-cleanup removed (load-bearing)

**koto v0.9.x auto-cleans terminal sessions; KT1 removes this
default.** Operators must invoke `koto workspace prune` to reclaim
disk space from completed or abandoned workflows. Without periodic
prunes, `~/.koto/sessions/` grows unbounded.

KT12's dashboard surfaces stale-tree indicators (PRD R27) so
operators see at a glance when prune is needed.

This is the intentional behavior change documented in the design's
Cross-Product Coordination > "Auto-cleanup behavior change" section
(design lines 1904-1911). The change is load-bearing for the KT1
dispatch protocol: the discovery scan and the terminal-index reader
both depend on terminal sessions remaining on disk long enough for
the per-coordinator cursor (Issue 7) to advance correctly. The 7-day
TTL on coordinator cursors (`kt1.coord_cursor_ttl_days`) bounds the
horizon during which a terminal session needs to remain visible.

**Operators upgrading from 0.9.x should add `koto workspace prune` to
their periodic-maintenance script.** See the verb's documentation in
`docs/guides/cli-usage.md` for the full flag set.

#### Bunki BK2 cross-product coordination

KT1's crate-surface lockdown is the gate at which the bunki BK2
substrate begins its production rollout against
`koto::engine::types`. The eight types frozen by Issue 19 — plus
the four `SessionBackend` methods marked `# Stability: Stage 1 —
Frozen` — form bunki's import contract.

External-consumer compile verification ships in this release as the
`koto-stability-tests` crate (workspace-internal, not published).
CI runs `cargo test -p koto-stability-tests` on every PR.

### Added

- `koto-stability-tests/` external-consumer fixture crate. Imports
  every promised export from Decision 5 and exercises the four
  frozen `SessionBackend` methods via a trait-object smoke test.
- `docs/STABILITY.md` — public stability contract, bump protocol,
  and additive-evolution rules.
- `docs/workspace-layout.md` — workspace dir/file layout
  reference.
- `koto::engine::types::*` re-exports for the eight Stage 1 frozen
  types: `StateFileHeader`, `Event`, `EventPayload`,
  `SpawnEntrySnapshot`, `ChildSnapshot`, `AssignmentClaim`,
  `derive_state_from_log`, `CURRENT_SCHEMA_VERSION`.
- `koto::error::Error` — re-exported `EngineError` alias.
- `StateFileHeader` KT1 request-store fields: `needs_agent`,
  `role`, `inputs`, `coordinator_of_record`, `requested_by`,
  `assignment_claim`, `dispatch_epoch`, `respawn_generation`, plus
  four forward-compat reserved fields.
- `Event.idempotency_hash` for retry-safe append discipline (Issue
  12 / R17).
- `koto workspace prune --root <session> [--dry-run] [--yes] [--force]`
  CLI verb (Issue 6). Reclaims terminal workflow trees with a
  symlink-refusal safety gate and an interactive confirmation
  prompt.
- `koto next` directive return: `unassigned_children` array
  populated by the per-tick discovery scan (Issues 5, 7).
- `koto next` directive return: every variant (including Terminal
  and Error) carries `unassigned_children` for uniform
  coordinator-side consumer branching (Task #18).
- Discovery scan with mtime-cursor + tied-boundary seen-set rule
  + 7-day cursor TTL + cursor GC (Issue 7).
- Terminal-index JSONL writer + skip-malformed reader +
  compaction-lease O_EXCL sidecar with stale-recovery (Issues 8,
  9).
- Claim sidecar: O_EXCL + happy-path dispatch orchestration +
  four-case drift recovery (Issue 11).
- Audit-event family with reserved `kind` discriminator on
  `EvidenceSubmitted`: `ChildDispatched`, `ChildRedelegated`,
  `RequesterWoken`, `RequesterRespawn` (Issues 14, 15, 16). The
  `kt1.` prefix is reserved for future audit kinds.
- Idempotency-hash short-circuit + 3-point fsync discipline before
  substrate wake-delivery (Issue 12 / R17, R19).
- Wake-candidates pass + age-and-activity recovery
  (Issue 15 / R19, R30).
- F1 cold-restart re-priming + F3 fallback +
  `respawn_generation_cap` (Issue 16 / R31, R32). Resume-context
  prompt is a fixed-form committed template (Decision 5 lines
  724-732).
- Epoch-fence validation on child-log writes (Issue 13 / R43).
- Recursion-cap enforcement + recursion_caps bench harness (Issue
  17 / AD3.3).
- Discovery scan bench harness with soft-by-default reporting at
  100/1k/10k/26k workspace sizes (Issue 10 / R20).
- `Kt1Config` 5-level precedence cascade + reserved
  `[kt1.recursion]` warn (Issue 18). Eight operator-tunable
  dimensions: `stale_claim_timeout_seconds`,
  `stale_dispatch_timeout_seconds`, `redelegation_cap`,
  `coord_cursor_ttl_days`, `terminal_index_compact_lines`,
  `compact_lock_timeout_seconds`, `directive_batch_size`,
  `respawn_generation_cap`.
- `ValidatedSessionId` / `ValidatedCoordId` newtypes for security
  hardening at every public boundary (Issue 3).
- New typed errors: `EpochFenceViolation`,
  `RedelegationCapExceeded`, `ConcurrentSubmissionConflict`,
  `RecursionCapExceeded`, `ReservedKindCollision`,
  `InvalidSessionId`, `InvalidCoordId`.

### Changed

- **Removed auto-cleanup default** (see "Operator-facing behavior
  change" above). This is the load-bearing operator-facing change
  in this release.
- `StateFileHeader` extended with KT1 fields (additive — pre-KT1
  state files round-trip unchanged).
- `Event` extended with `idempotency_hash: Option<String>`
  (additive).
- `NextResponse::Terminal` and `NextResponse::Error` now carry
  `unassigned_children: Vec<UnassignedChild>` (Task #18). Adds a
  new key in the JSON output; consumers that ignore unknown keys
  continue to work.
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
