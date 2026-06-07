---
schema: brief/v1
status: Accepted
problem: |
  Tools that embed koto -- importing its library, driving its CLI, or
  reading its session logs -- must stay compatible with it across
  releases. koto exposes no stable surface that says what it is
  compatible with: its config carries runtime tuning and nothing about
  versioning. So a dependent tool hard-codes an assumption about koto's
  behavior and breaks silently when koto moves, or pins to an exact
  build and churns on every release.
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

The problem is that koto gives them nothing stable to build that
dependency on. There is no surface that answers "what is this koto
compatible with?" -- its configuration carries session and
request-store tuning, and says nothing about versioning or
compatibility. A dependent tool is left with two bad options. It can
hard-code an assumption about how koto behaves; that assumption rots
silently the day koto changes, and the breakage shows up at runtime,
far from its cause. Or it can pin to an exact koto build and refuse
anything else; now it churns on every koto release, even the ones that
changed nothing it depends on.

Either way the coupling is implicit and fragile. There is no point of
record where koto states the contract it honors, so a consumer can
neither depend on stability nor detect a real change deliberately. The
compatibility relationship between koto and the tools built on it
exists only as folklore in each consumer's code.

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
- **Where the surface lives.** Whether consumers read it through a koto
  CLI query, a field on the existing config surface, the session-log
  header koto already writes, or a dedicated file -- and whether more
  than one form is warranted for library versus CLI versus log-reading
  consumers.
- **Whether the statement is authored or derived.** Whether the
  compatibility value is set deliberately at release time or computed
  from koto's own version, and how it is kept honest across releases.

## References

- `docs/designs/current/DESIGN-config-and-cloud-sync.md` -- the design
  for the configuration substrate this builds on.
- `src/config/mod.rs`, `src/config/resolve.rs` -- the shipped config
  types and the cascade resolution this surface would extend or sit
  alongside.
