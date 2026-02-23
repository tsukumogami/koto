# Exploration Summary: koto installation and distribution

## Problem (Phase 1)
koto exists as a Go library with a CLI entrypoint but has no binary distribution, no release automation, and no installation path for end users. Someone who wants to use koto must clone the repo and build from source.

## Decision Drivers (Phase 1)
- Zero runtime dependencies -- koto is a single static binary
- Standard Go open-source distribution channels (go install, Homebrew, GitHub Releases)
- Template search path must not include world-writable directories (security constraint)
- First-run experience should work without configuration
- Built-in templates need a distribution mechanism (embed vs filesystem)
- Version reporting must be available at runtime

## Research Findings (Phase 2)
- GoReleaser is the standard for Go CLI distribution (used by gh, fzf, gum, goreleaser itself)
- Standard pattern: tag push triggers GitHub Actions, GoReleaser builds multi-platform, auto-updates Homebrew tap
- Version embedding via ldflags: `-X main.Version={{.Version}}` etc.
- go:embed works with go install, suitable for built-in templates
- Layered template loading: check project-local, then user config, then embedded defaults
- XDG Base Directory for user config (~/.config/koto/), but koto already uses ~/.koto/ convention
- Existing koto cache uses KOTO_HOME env var for override

## Decision Summary

GoReleaser for release automation, `go:embed` for built-in templates, three-layer search path (`pkg/resolve`) for template resolution. Version infrastructure already exists in `internal/buildinfo/`. Key security additions: override warning for project-local shadows, `--no-local` flag, Cosign signing planned for v0.2.0.

## Current Status
**Phase:** 8 - Review Complete
**Last Updated:** 2026-02-23
