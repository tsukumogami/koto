---
schema: brief/v1
status: Accepted
problem: |
  Tools that embed koto -- importing its library, driving its CLI, or
  reading its session logs -- must stay compatible with it across
  releases. koto versions its on-disk contract internally (a schema
  version gates the session-log format and STABILITY.md documents the
  bump protocol), but none of that is queryable at the boundary: a
  consumer cannot ask koto what contract version it honors and pin
  against the answer. `koto version` reports the build, which moves
  every release. So a dependent tool reverse-engineers compatibility or
  over-pins, and breaks silently when koto moves.
outcome: |
  A tool that embeds koto can ask koto what it is compatible with and
  get a stable answer it pins against. When koto changes in a way that
  matters, the dependent tool sees the compatibility signal move and
  reacts deliberately -- a clear failure at the boundary -- instead of
  breaking in the dark at runtime.
---

# BRIEF: A version-compatibility surface for koto

## Status

Accepted

Framing for the residual config work after the general-purpose
configuration substrate already shipped (TOML `KotoConfig`, the
defaults -> user -> project cascade, the `koto config` CLI, a
project-key allowlist, and a programmatic API). This brief is not
about building a config system; it frames one feature: a stable
surface on which koto declares what it is compatible with, for the
tools that embed it. The related project-config trust question is a
separate feature and is held out of scope below.

## Problem Statement

koto is not a standalone end product; other tools in the toolkit build
on it. They import its library, drive its CLI, and read the session
event logs it writes, and they must keep working as koto evolves
across releases.

koto is not silent about compatibility -- it just does not expose it
where a consumer can use it. Internally a schema version
(`CURRENT_SCHEMA_VERSION`) gates the session-log format, every log
header carries the schema version it was written at, and `STABILITY.md`
documents the protocol by which that version bumps and which surfaces
are frozen. But all of that lives inside koto: the schema version is a
constant and a number stamped in log files, and the contract is prose
in a doc. There is no surface a dependent tool can read at the boundary
that answers "what contract version is this koto?" -- `koto version`
reports the build version, which moves on every release whether the
contract changed or not.

So a dependent tool is left with two bad options. It can
reverse-engineer compatibility -- parse a log header, read
`STABILITY.md`, or guess from the release version -- and that inference
rots silently the day koto changes, with the breakage showing up at
runtime far from its cause. Or it can pin to an exact koto build and
churn on every release, even the ones that changed nothing it depends
on. The compatibility koto already tracks internally never reaches the
consumers that need it as a value they can pin against.

## User Outcome

A tool that embeds koto can ask koto what it is compatible with and get
a stable, reliable answer.

At setup the tool reads koto's published compatibility surface and pins
or validates against it -- it depends on a stated contract instead of
guessing from koto's version number or its observed behavior. When koto
later ships a change that matters to that contract, the surface moves,
and the dependent tool sees it: it fails or warns at the boundary, on
purpose, with a clear signal pointing at the compatibility mismatch.
When koto ships a release that changed nothing the contract covers, the
surface holds steady and the consumer keeps working without churn.

The tool author stops reverse-engineering koto's compatibility from
release notes and version strings, and starts relying on koto to state
it. The coupling between koto and the tools built on it becomes
explicit, legible, and safe to depend on.

## User Journeys

### A dependent tool pins to koto's compatibility

A tool that embeds koto -- shirabe, for instance -- needs to know
whether the koto it found on the system is one it supports. At setup it
reads koto's published compatibility surface, learns the contract this
koto honors, and pins or validates against it. It proceeds only against
a koto it knows it is compatible with, rather than assuming and hoping.

### A koto upgrade surfaces an incompatibility deliberately

A developer upgrades koto under a tool that depends on it. The tool
re-reads koto's compatibility surface, sees that the contract it pinned
to has moved, and stops with a clear message about the mismatch --
rather than running against an incompatible koto and failing somewhere
deep and confusing later. The breakage is caught at the boundary, where
it is cheap to understand.

### A koto release states what it is compatible with

The person cutting a koto release publishes the compatibility metadata
as part of that release, so every koto build carries an accurate
statement of the contract it honors. Consumers downstream read a value
that the release deliberately set, not one inferred after the fact.

## Scope Boundary

**IN:**

- A stable surface on which koto publishes its version-compatibility
  metadata, in a form an embedding tool can read and pin against.
- Defining what koto's compatibility statement means at that surface --
  what a consumer is promised when the value holds and what it learns
  when the value moves.
- Building strictly on the configuration substrate and release process
  that already exist.

**OUT:**

- **The project-config trust model.** Whether a checked-in
  `.koto/config.toml` can silently change koto's behavior -- the key
  allowlist versus an explicit per-directory trust opt-in -- is a
  separate residual of the same config work. It shares no mechanism
  with the compatibility surface (it governs config koto *accepts
  inward*, not what koto *publishes outward*) and deserves its own
  framing.
- The consumer side of compatibility. koto publishes the surface; how a
  dependent tool stores its pin and enforces it is that tool's own
  work.
- Rebuilding the configuration system. The TOML format, the cascade,
  the `koto config` CLI, the allowlist, and the programmatic API
  already exist and are not in scope to replace.
- Cross-machine session storage and cloud sync as features. koto's
  config serves them, but that functionality is separate work.

## Open Questions

- **What koto publishes.** Whether the compatibility statement is a
  single version, a supported range, a separate contract or schema
  version that moves independently of koto's release version, or a
  capability set -- and how it maps to the behavior consumers actually
  depend on. Deferred to the PRD and its design.
- **Where the surface lives.** Whether consumers read it through the
  existing `koto version` output (which already has a `--json` mode), a
  new CLI query, a field on the config surface, the session-log header
  koto already writes, or a dedicated file -- and whether more than one
  form is warranted for library versus CLI versus log-reading consumers.
- **Whether the statement is authored or derived.** Whether the
  compatibility value is set deliberately at release time or computed
  from koto's own version, and how it is kept honest across releases.

## References

- `docs/STABILITY.md` -- koto's existing stability contract and the
  `CURRENT_SCHEMA_VERSION` bump protocol this surface would publish.
- `src/engine/types.rs` -- `CURRENT_SCHEMA_VERSION` and the
  `schema_version` log-header field that already version the on-disk
  contract internally.
- `src/cli/mod.rs` -- the existing `koto version` command (with a
  `--json` mode) that reports the build version today.
- `docs/designs/current/DESIGN-config-and-cloud-sync.md` -- the design
  for the configuration substrate this work sits alongside.
