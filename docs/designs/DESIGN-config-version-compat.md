---
schema: design/v1
status: Accepted
upstream: docs/prds/PRD-config-version-compat.md
problem: |
  koto already tracks its contract compatibility internally
  (`CURRENT_SCHEMA_VERSION` gates the session-log format, `STABILITY.md`
  documents the bump protocol) but exposes nothing a dependent tool can
  read at the boundary to pin against. `koto version` reports the build,
  which moves every release. PRD-config-version-compat requires a
  queryable, machine-readable, contract-versioned surface derived from
  the version koto already maintains -- a single source of truth,
  additive, and cheap to read.
decision: |
  Publish the existing `CURRENT_SCHEMA_VERSION` as a `schema_version`
  field on the `BuildInfo` struct that `koto version --json` already
  serializes, and document in `STABILITY.md` that this field is koto's
  compatibility surface and what pinning to it promises. No new version
  scheme, no new command, no new file: the compatibility koto already
  tracks is surfaced through the version command that already exists.
rationale: |
  `CURRENT_SCHEMA_VERSION` is already the single source of truth for
  contract compatibility and is already governed by the STABILITY.md
  bump protocol, so surfacing it (rather than authoring a parallel
  number) satisfies the single-source-of-truth requirement and adds no
  maintenance. `koto version --json` is already the machine-readable
  boundary surface, computed from static build info with no workflow,
  storage, or network access -- so adding a field is additive (existing
  json fields and the text output are untouched) and cheap to read,
  meeting R7 and R8 directly.
---

# DESIGN: A version-compatibility surface for koto

## Status

Accepted

Design for PRD-config-version-compat. Settles the deferred mechanism
forks: what value koto publishes, where the surface lives, and how it
stays a single source of truth.

## Context and Problem Statement

`koto version` (`src/cli/mod.rs`) prints build identity from
`buildinfo::build_info()` -- a `BuildInfo { version, commit, built_at }`
struct (`src/buildinfo.rs`). It has two modes: a human line
(`koto <version> (<commit> <built_at>)`) and, with `--json`, the
serde-serialized struct.

Separately, `src/engine/types.rs` defines
`pub const CURRENT_SCHEMA_VERSION: u32 = 1`, which gates the session-log
format (readers reject logs written at a higher schema version), and
`docs/STABILITY.md` documents the protocol that bumps it (patch: +0;
minor: +0 or +1; major: +1 with a deprecation window) and the frozen
surfaces it governs.

The two never meet. A consumer can read the build version (which moves
every release) or parse a log header (which requires a log to exist),
but cannot ask koto for the contract version directly. The PRD requires
exposing that contract version at the boundary, derived from what koto
already tracks.

## Decision Drivers

- **R4 -- single source of truth.** The published value must not be a
  parallel version that can drift from `CURRENT_SCHEMA_VERSION`.
- **R7 -- additive.** Existing `koto version` consumers (text and json)
  must not break.
- **R8 -- cheap to read.** No workflow init, session storage, network,
  or credentials when reading the value.
- **R3 -- contract-versioned.** The value must move on contract change,
  not on every build.
- **Smallest surface that works.** The feature is a single value; it
  should not add a command, a file, or a config key when an existing
  surface serves.

## Considered Options

### Fork A -- What value is published

- **A1 (chosen): publish `CURRENT_SCHEMA_VERSION` itself.** It is
  already the contract-compatibility version, already governed by the
  STABILITY.md bump protocol. Surfacing it is the single source of
  truth by construction.
- A2: author a new, separate "contract version" distinct from the schema
  version. Rejected: a second number maintained by hand drifts from the
  schema version (violates R4) and doubles the release discipline, for
  no gain the schema version does not already provide.

### Fork B -- Where the surface lives

- **B1 (chosen): a `schema_version` field on `BuildInfo`, emitted by
  `koto version --json`.** `koto version --json` is already the
  machine-readable boundary surface, already computed from static build
  info (R8), and adding a serde field is additive (R7). A consumer reads
  `koto version --json` and takes `.schema_version`.
- B2: a new `koto compat` subcommand. Rejected: a new top-level command
  for one integer when `koto version --json` already exists and is where
  a consumer looks for "what koto is this."
- B3: a config-surface key. Rejected: compatibility is a property of the
  build/contract, not user configuration; it does not belong in the
  defaults/user/project cascade.
- B4: the session-log header. Rejected: that already carries
  `schema_version`, but it requires a log to exist and be parsed -- the
  exact boundary problem this feature removes.

### Fork C -- The human (text) output

- **C (chosen): leave the text output unchanged; add the value to
  `--json` only.** The text line is a stable surface some consumers
  scrape; appending to it risks breaking strict parsers (R7). The
  machine surface is `--json`, which is where a programmatic consumer
  should read. (A later additive text line is possible but is not
  required and is held out to keep R7 unambiguous.)

## Decision Outcome

1. Add `schema_version: u32` to `BuildInfo` (`src/buildinfo.rs`),
   populated from `engine::types::CURRENT_SCHEMA_VERSION`.
2. `koto version --json` emits it automatically (it serializes the whole
   struct -- no handler change). The text output is untouched.
3. Document in `STABILITY.md` that `koto version --json`'s
   `schema_version` is koto's compatibility surface: a consumer pins
   against it, it holds across releases that do not bump the schema
   version, and it moves per the existing bump protocol.

## Solution Architecture

**`src/buildinfo.rs`.**
```
struct BuildInfo {
    version: &'static str,     // unchanged: crate/build version
    commit: &'static str,      // unchanged
    built_at: &'static str,    // unchanged
    schema_version: u32,       // NEW: = engine::types::CURRENT_SCHEMA_VERSION
}
```
`build_info()` sets `schema_version` from the constant. The field is
last so the existing json object gains a key without disturbing the
others. (`BuildInfo` derives `Serialize`; the new field rides the same
derive.)

**`koto version --json` output** becomes
`{"version":"...","commit":"...","built_at":"...","schema_version":1}`.
A consumer reads `.schema_version` and pins against it. No code change
in the `Command::Version` handler (`src/cli/mod.rs`) -- it already
serializes the struct.

**`docs/STABILITY.md`** gains a short "Compatibility version surface"
section: names `koto version --json`'s `schema_version` as the value to
pin against, states what holds while it is unchanged and what a bump
signals, and cross-references the existing `CURRENT_SCHEMA_VERSION` bump
protocol so the two are never separately maintained.

## Implementation Approach

Ordered, each step testable:

1. **Add the field.** Add `schema_version: u32` to `BuildInfo`, populate
   from `CURRENT_SCHEMA_VERSION` in `build_info()`. Unit test:
   `build_info().schema_version == CURRENT_SCHEMA_VERSION`.
2. **Lock the json contract.** Test that `koto version --json` parses as
   JSON, still carries `version` / `commit` / `built_at`, and now carries
   `schema_version` equal to `CURRENT_SCHEMA_VERSION`; and that the text
   `koto version` output is unchanged.
3. **Document the surface.** Add the `STABILITY.md` section and a
   one-line pointer from wherever `CURRENT_SCHEMA_VERSION` is defined,
   so a maintainer bumping the constant sees that it is published.

## Security Considerations

No new input, network, credential, or storage surface. The value is a
compile-time constant emitted through a command that already runs with
no side effects. It exposes only the schema version, which is already
public in every session-log header and in `STABILITY.md`; surfacing it
through `koto version --json` reveals nothing not already observable.

## Consequences

**Positive.** Single source of truth (the published value *is*
`CURRENT_SCHEMA_VERSION`); additive (existing json fields and the text
line are untouched); cheap (static, no IO); no new command, file, or
config key. Consumers get a stable, machine-readable pin.

**Negative / follow-ups.** The surface publishes only the schema/log
contract version; if koto later wants to version its CLI or library
contract independently of the log schema, that is a separate value and a
later decision (the STABILITY.md "future surface" note already
anticipates broader contracts). The text output deliberately does not
carry the value, so a text-only consumer must move to `--json`.
