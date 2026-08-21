# Lead: Does Claude Code's plugin mechanism expose an integrity or pinning surface?

Answered by a Claude Code documentation specialist agent, commissioned because
the template-boundary lead's recommendation turned on whether shirabe would hook
into an existing mechanism or invent one. Each answer was required to be marked
confirmed-from-docs, confirmed-from-behavior, or not-available, with an explicit
"this does not exist" rather than an adjacent description.

## Findings

### A pinning surface exists, and it is stronger than expected

For git-backed sources, an individual plugin entry in a marketplace supports both
`ref` (branch or tag) and `sha` (a 40-character commit SHA). When both are
present, `sha` wins. A user can lock a plugin to an exact commit, permanently.
Confirmed from docs.

For archive sources there is a `sha256` field carrying a 64-hex digest. A
mismatch refuses the install outright with an integrity-check failure. Confirmed
from docs.

### What it does not cover

- **The marketplace catalog itself pins only to `ref`** — a branch or tag, never
  a commit. The catalog listing plugins is mutable even when the plugins it lists
  are pinned.
- **No content hashes or signatures for git sources.** The `sha` is a revision
  pointer, not a digest of what was installed. `plugin.json` and
  `marketplace.json` carry no signature or hash fields of their own.
- **No install-time or update-time hook.** Nothing runs that a plugin author
  could use to verify content before it is used. `disableCommandPluginSources`
  and `allowManagedHooksOnly` are org-wide blocks on plugin *types*, not a
  verification step. Not available.
- **No load-time verification.** Once installed, plugin files are trusted
  implicitly. Reserved-name checks are name-based allowlisting, not content
  verification. Confirmed from docs.
- **No documented convention for shipping a checksum file alongside a release.**

## Implications

The template trust question splits in two, and only one half needs building.

**"Which revision of the templates am I running?"** is answerable today. Pinning
the shirabe plugin entry to a `sha` locks the whole plugin — templates included —
to an exact commit. That is a real, supported control costing a documentation
line rather than an engineering project, and it is the half most people mean when
they ask about supply chain.

**"Is the template koto is about to run the one that was reviewed?"** is not
answerable today and is the half worth building. The plugin `sha` binds an
install to a revision; it does not bind a specific template file at the moment
`koto init` accepts a path, which is where authority is actually granted. That
gap is what koto's existing hash machinery could close with an `--expect-hash`
argument, or what shirabe's `assert-child-template.sh` could close by growing
from an existence check into a content check.

The two compose rather than compete: pin the plugin to a revision so the file on
disk is known, and check the compiled hash at init so nothing between install and
run substituted something else.

## Surprises

- The archive path already has the control this exploration was about to
  recommend inventing — a required digest that refuses the install on mismatch.
  It simply does not extend to git sources, which is how shirabe ships.
- The absence of any install-time hook matters more than the absence of
  signatures. A hook would let a plugin verify itself; without one, verification
  happens either at pin time (the user's choice) or at run time (koto's job).

## Open Questions

- Would distributing shirabe as an archive with a `sha256`, rather than as a git
  source, be a reasonable trade? It buys a real digest check and costs the
  convenience of tracking a branch.
- Does pinning to a `sha` interact badly with how this workspace expects plugin
  updates to arrive? Worth checking before recommending it as practice.

## Summary

Claude Code supports pinning a git-backed plugin to an exact commit via a `sha`
field that takes precedence over `ref`, and archive sources carry a real `sha256`
digest check — so the "which revision am I running" half of template trust is
solvable today with configuration rather than engineering. What does not exist is
any content hash for git sources, any install-time or update-time hook, and any
load-time verification: once installed, plugin files are trusted implicitly. The
half worth building is binding a specific template to a reviewed hash at the
moment `koto init` accepts it, which composes with revision pinning rather than
duplicating it.
