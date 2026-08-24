# Changelog

All notable changes to the koto crate are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project's pre-1.0 versioning treats MINOR as MAJOR per the Cargo
0.x semver convention (`0.10.0` is breaking-change-eligible relative
to `0.9.x`).

## [Unreleased]

### Added

- **`koto session rebind` moves a session whose checkout moved.** Execution
  anchoring shipped in 0.12.0 with the enforcement but not the repair: both
  refusals told the user to run `koto session rebind <session> --to <dir>`,
  and no such subcommand existed, so a developer whose checkout genuinely
  moved was refused and then sent to a command that did nothing. The verb now
  exists. `koto session rebind <name> [--to <dir>]` canonicalizes the target,
  defaulting to the directory it runs in, writes it to the session header,
  and appends an `execution_anchor_rebound` event carrying the directory the
  session left and the one it landed on, so a move is audited rather than
  silent. It is the only verb that changes an anchor, it works on a session
  created by another session exactly as on any other, and rebinding a child
  does not move its parent. Rebinding to the directory a session is already
  bound to appends nothing and reports `"rebound": false`; a target that does
  not resolve, names a file, or belongs to an unknown session is refused with
  the anchor left alone.

- **`koto session recover` brings back sessions the layout migration had to
  set aside.** The migration from the old per-repository layout flattens
  `sessions/<repo-id>/<name>/` into `sessions/<name>/`. A name reused across
  repositories has several sources and one destination, so only one of them
  can keep the name; the rest are moved into
  `sessions/.migration-conflicts/<repo-id>/<name>/`, where they are preserved
  and unreachable — not listed, not resumable, not nameable. One install
  reported a thousand sessions in that state. `koto session recover` reports
  what is in the quarantine, and `--apply` moves it back into the session
  list in one run; `--session <name>` narrows it to one workflow. A session
  returns as `r<repo-id>-<name>`, which is unique per originating repository
  and, because a session's parent is the dotted prefix of its own name, keeps
  a parent and its children pointing at each other. Recovery moves and never
  deletes, never writes over an existing session, and is safe to re-run. The
  migration now closes with one line naming the command.

### Fixed

- **A nested `koto next` no longer leaves the outer tick reporting a
  state the session has left.** A `koto next` run from inside a command
  koto itself was executing — a state's `default_action` or a command
  gate — performed a real transition, in the reported case advancing the
  session all the way to its terminal state. The tick that spawned it
  then finished against the snapshot it started with and answered
  `advanced: false` on the original state, so the caller was told the
  workflow was still waiting on a session that had already ended. The
  answer was wrong rather than missing, and nothing surfaced an error.
  A tick now exports `KOTO_TICK_SESSION` naming the session it is
  advancing before it runs anything, and a `koto next` that finds the
  marker set refuses with the new `nested_invocation` error code (exit
  2), naming the tick in flight and the call to remove. The refusal is
  scoped to the process tree rather than to a session name, so a tick on
  a second workflow from inside a command is refused too — a chain that
  ticks back into the outer session through another one lands on the
  same defect. It covers `koto next` only: `koto context` reads and
  writes from inside a command are a supported pattern and keep working.
  The marker is inherited and carries no liveness — a command that
  detaches survives the process-group kill at timeout and keeps it — so
  the refusal message names the escape hatch, `KOTO_TICK_SESSION= koto
  next <name>`, for a process that outlived its tick.
  Closes koto#208.

- **`details` suppression now keys on delivery, not visit count.** koto
  previously decided whether a `koto next` response carried a phase's
  long-form `details` by counting entries into the phase's state, which
  came apart from the thing that actually mattered in both directions: a
  non-advancing tick (e.g. re-evaluating the same failing gate) re-sent
  `details` forever, while a `koto rewind` back into a phase the agent
  was told to redo withheld them because rewind counts as a re-entry, not
  a fresh visit. The directed-transition path (`koto next --to`) applied
  no suppression rule at all, so it could disagree with the
  natural-advancement path on the same phase. The new rule records the
  fact of delivery directly: an `InstructionsDelivered` event is
  appended whenever a response carries a phase's `details`, and the
  suppression predicate
  (`instructions_delivered_this_window`) asks whether a delivery has
  happened since the most recent arrival at the phase. A shared
  combinator (`NextResponse::with_details_suppressed_unless_full`) now
  applies that one rule at both response-construction sites, so the
  directed path and the natural-advancement path can no longer disagree.
  `--full` still forces `details` through, and now also records a
  delivery, so the next plain tick doesn't re-deliver.

- **A self-loop no longer re-sends a phase's `details`.** A phase
  transitioning to itself, and a `koto next --to <phase>` issued while
  the workflow already occupies that phase, are laps around a loop the
  agent is already in rather than arrivals at it — the agent still holds
  the procedure, so it is not re-sent. Only arrival from a different
  phase delivers, along with any `koto rewind` into the phase, which
  means redo this rather than continue. This is koto#90's acceptance
  criterion 3, which the previous rule overrode without citing it. On a
  long loop the per-lap cost is now the directive alone; an agent that
  loses the procedure mid-loop recovers it with `koto status <name>`,
  which every response for a phase that declares instructions names in its
  `directive`, whether or not that response carried them.
  The boundary that decides delivery is now separate from the epoch the
  dashboard's blocked classification reads, so that badge is unchanged.

### Added

- `koto status <name>` now returns the current phase's `directive`,
  `details`, and `expects` when the workflow is not at a terminal state —
  a read-only retrieval, substituted through the same pipeline `koto
  next` uses, that returns the phase's full instructions regardless of
  the delivery rule above and appends nothing (no delivery record, no
  lock). This is the recovery path for an agent that has lost track of a
  phase's instructions, or that never received them because they were
  suppressed. `directive`, `details`, and `expects` are absent together
  at a terminal phase; `details` is additionally absent when the phase
  declares none.
- `koto status <name>` gains a `template_hash_mismatch` key
  (`{"recorded", "actual"}`) when the compiled template read from disk no
  longer matches the hash recorded in the session header. Unlike `koto
  next`, which fails closed on the same mismatch, `koto status` reports
  it instead of failing, since this command is often the only recovery
  path an agent has left.
- Every `koto next` response whose current phase declares instructions
  now carries a short koto-authored pointer to `koto status` in its
  `directive`, so an agent that has lost everything else still learns
  the retrieval exists. The pointer appears whether or not `details` was
  actually included on that particular response — that's the case it
  matters most for — and is spliced in before any leg-abandonment
  notice.

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
