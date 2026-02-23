# Documentation Plan: koto-installation

Generated from: docs/designs/DESIGN-koto-installation.md
Issues analyzed: 4
Total entries: 1

---

## doc-1: README.md
**Section**: Install
**Prerequisite issues**: #27
**Update type**: modify
**Status**: pending
**Details**: Replace the current Install section (which only shows `go install` and build-from-source) with the install script as the primary method (`curl -fsSL ... | sh`). Keep `go install` and build-from-source as secondary options. Add `tsuku install koto` as an alternative for tsuku users. Issue #27's acceptance criteria explicitly require this README update. Issue #28 (tsuku recipe) adds a one-liner mention in the same section but doesn't need its own entry -- the tsuku recipe can be documented at the same time since it's a single line addition.
