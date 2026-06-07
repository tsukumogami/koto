---
schema: prd/v1
status: Accepted
problem: |
  koto already versions its on-disk contract internally --
  `CURRENT_SCHEMA_VERSION` gates the session-log format, log headers
  carry `schema_version`, and `STABILITY.md` documents the bump protocol
  -- but none of it is queryable at the boundary. `koto version` reports
  the build, which moves every release regardless of the contract. So a
  tool that embeds koto reverse-engineers compatibility (parsing a log
  header, reading STABILITY.md, or guessing from the release version) or
  over-pins, and breaks silently when koto changes.
goals: |
  Publish koto's contract-compatibility version on a queryable,
  machine-readable surface a dependent tool can read and pin against,
  derived from the compatibility koto already tracks internally and
  documented so a consumer knows what holds while it is unchanged and
  what it learns when it moves.
---

# PRD: A version-compatibility surface for koto

## Status

Accepted

Requirements for exposing koto's contract-compatibility version to the
tools that embed it, derived from the accepted
`docs/briefs/BRIEF-config-version-compat.md`. This builds on the
compatibility koto already tracks (`CURRENT_SCHEMA_VERSION`,
`schema_version` log headers, `STABILITY.md`); it does not introduce a
parallel versioning scheme. Mechanism choices (the exact surface, the
value's shape, how it stays honest) are left to the design.

## Problem Statement

koto is a dependency for other tools in the toolkit: they import its
library, drive its CLI, and read the session event logs it writes, and
they must keep working as koto evolves.

koto is not silent about compatibility. A schema version
(`CURRENT_SCHEMA_VERSION`) gates the session-log format, every log
header records the schema version it was written at, and `STABILITY.md`
documents the protocol by which that version bumps and which surfaces
are frozen. The problem is that this information is not reachable where
a consumer needs it. The schema version is an internal constant and a
number stamped inside log files; the contract is prose in a document.
There is no surface a dependent tool can read at the boundary -- cheaply,
programmatically -- that answers "what contract version is this koto?"
`koto version` reports the build version, which moves on every release
whether the contract changed or not, so it cannot serve as the pin.

The result: a consumer reverse-engineers koto's compatibility (parsing a
log header, reading the stability doc, or inferring from the release
number) and that inference rots silently the day koto changes, with the
break surfacing at runtime far from its cause; or it over-pins to an
exact build and churns on every release. The compatibility koto already
tracks internally never reaches the consumers that need it as a value
they can pin against.

## Goals

- A dependent tool can read koto's contract-compatibility version from a
  stable, machine-readable surface at the boundary, without parsing a
  session log or running a workflow.
- That version reflects koto's *contract*, not its build: it holds
  steady across releases that change nothing a consumer depends on, and
  moves when the contract changes -- so a real incompatibility is
  detectable deliberately.
- The published version is derived from the compatibility koto already
  tracks, so there is a single source of truth, not a parallel scheme
  that can drift.
- A consumer knows what the version promises: the contract is documented
  alongside the value.

## User Stories

Use-case form (this is a developer-tooling integration feature):

- As an author of a tool that embeds koto, I read koto's published
  compatibility version at setup and pin or validate against it, so I
  depend on a stated contract instead of guessing from the release
  number or parsing a log header.
- As that author, when koto ships a release that changed nothing my
  contract covers, the compatibility version holds steady and my tool
  keeps working without a churned pin.
- As that author, when koto changes its contract incompatibly, the
  compatibility version moves and my tool detects the mismatch at the
  boundary with a clear signal, instead of failing deep at runtime.
- As a koto maintainer, the published compatibility version stays
  consistent with the schema version and stability protocol koto already
  maintains, so I am not keeping two version schemes honest by hand.

## Requirements

Functional:

- **R1 -- Queryable at the boundary.** koto SHALL expose its
  contract-compatibility version on a surface a dependent tool can read
  programmatically without parsing a session-log header or executing a
  workflow.
- **R2 -- Machine-readable and stable.** The surface SHALL present the
  version in a stable, parseable form a consumer can read and compare
  across koto builds.
- **R3 -- Contract-versioned, not build-versioned.** The compatibility
  version SHALL be distinct from koto's build/release version: it SHALL
  hold steady across releases that do not change the consumer contract
  and SHALL move when the contract changes, per koto's documented
  bump protocol.
- **R4 -- Single source of truth.** The published version SHALL be
  derived from or identical to the compatibility koto already tracks
  (`CURRENT_SCHEMA_VERSION` and the `STABILITY.md` protocol), not a
  separate, independently-maintained version that can drift from it.
- **R5 -- Documented meaning.** koto SHALL document what the
  compatibility version promises -- what a consumer can rely on while it
  is unchanged, and what it learns when it moves -- extending the
  existing stability contract rather than restating it.
- **R6 -- Present on every build.** Every koto build SHALL carry an
  accurate compatibility version on the surface.

Non-functional:

- **R7 -- Additive and compatible.** Exposing the surface SHALL NOT
  break existing consumers of `koto version` output or change the
  session-log format; existing fields and behavior SHALL be preserved.
- **R8 -- Cheap to read.** Reading the compatibility version SHALL not
  require koto to initialize a workflow, touch session storage, or
  perform network or credential operations.

## Acceptance Criteria

- [ ] A consumer reads koto's compatibility version from a documented,
      machine-readable surface without parsing a log or running a
      workflow.
- [ ] The compatibility version is distinct from the build version and
      changes only according to the documented contract-bump rules.
- [ ] The published value is consistent with `CURRENT_SCHEMA_VERSION`
      and the `STABILITY.md` protocol (a single source of truth, not a
      second scheme).
- [ ] `STABILITY.md` (or the documented contract) states what the
      compatibility version promises and when it moves.
- [ ] Existing `koto version` output and the session-log format are
      unchanged for current consumers (additive only).
- [ ] The existing koto test suite passes; stability/forward-compat
      checks are unbroken.

## Out of Scope

- **The project-config trust model.** Whether a checked-in
  `.koto/config.toml` can silently change koto's behavior (the key
  allowlist versus a per-directory opt-in) is the sibling residual of
  the same config work and is scoped separately.
- **The consumer side.** How a dependent tool stores its pin, compares
  versions, and enforces the result is that tool's own work.
- **Rebuilding the config system or the version command.** The config
  substrate and `koto version` already exist; this extends them.
- **Broadening the contract.** Defining new frozen surfaces beyond what
  `STABILITY.md` already governs is not in scope unless the design finds
  it unavoidable.
- **Cross-machine storage and cloud sync as features.**

## Decisions and Trade-offs

- **Build on `CURRENT_SCHEMA_VERSION`, do not invent a parallel
  version.** koto already maintains a schema version and a bump protocol;
  a second, independently-authored compatibility number would drift from
  it and double the maintenance. The published version is the existing
  one, surfaced -- the design decides whether it is exactly
  `CURRENT_SCHEMA_VERSION` or a contract version computed from it.
- **Contract version, not build version.** The build version
  (`koto version`) moves every release and cannot serve as a pin; the
  value of this feature is precisely a version that moves only on
  contract change. R3 fixes that property; the design picks how it maps
  to the bump protocol.
- **Additive surface.** Exposing the version through the existing
  `koto version` output (which already has a `--json` mode) keeps the
  change additive and avoids a new top-level surface; the design
  confirms the exact location, but R7 fixes that current consumers must
  not break.
