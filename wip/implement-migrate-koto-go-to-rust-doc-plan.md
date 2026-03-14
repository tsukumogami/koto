# Documentation Plan: migrate-koto-go-to-rust

Generated from: docs/plans/PLAN-migrate-koto-go-to-rust.md
Issues analyzed: 5
Total entries: 7

---

## doc-1: README.md
**Section**: Install
**Prerequisite issues**: #1
**Update type**: modify
**Status**: pending
**Details**: Replace Go install instructions with Rust/cargo equivalents. Remove `go install github.com/tsukumogami/koto/cmd/koto@latest` and `go build -o koto ./cmd/koto`. Add `cargo install --git https://github.com/tsukumogami/koto` (or equivalent) and `cargo build --release` for building from source.

---

## doc-2: README.md
**Section**: Quick start
**Prerequisite issues**: #1, #4
**Update type**: modify
**Status**: updated
**Details**: Rewrite the quick start sequence to reflect the new command surface. Remove steps for `koto transition`, `koto status`, and `koto query` (all removed). Update `koto init` signature (no `--var` flag, positional name argument). Update `koto next` output shape to `{"state":"...","directive":"...","transitions":["..."]}`. Update `koto version` output from plain text to JSON `{"version":"...","commit":"...","built_at":"..."}`. Note that `koto transition` is removed and workflows cannot be advanced until a later release.

---

## doc-3: README.md
**Section**: Documentation
**Prerequisite issues**: #1
**Update type**: modify
**Status**: pending
**Details**: Remove the link to `docs/guides/library-usage.md` (Go library guide) from the Documentation section, as the Go library no longer exists after the Rust migration. Update any other Go-specific references in that section.

---

## doc-4: docs/guides/cli-usage.md
**Section**: (multiple sections)
**Prerequisite issues**: #2, #3, #4
**Update type**: modify
**Status**: updated
**Details**: Major rewrite to match the new Rust command surface. Specific changes:
- Remove the `transition`, `status`, `query`, `cancel`, and `validate` command sections entirely (all removed in skeleton scope).
- Update "State file resolution" section: state files are now `koto-<name>.state.jsonl` (JSONL format), discovered by glob in the current directory; there is no `--state` or `--state-dir` flag in the skeleton.
- Update `koto init`: new signature is `koto init <name> --template <path>`; no `--var`, no `--state-dir`; output is `{"name":"<name>","state":"<initial>"}`.
- Update `koto next`: new signature is `koto next <name>`; new output shape `{"state":"<s>","directive":"<text>","transitions":["..."]}`.
- Update `koto rewind`: new signature is `koto rewind <name>` (no `--to` flag); appends a rewind event to the JSONL file pointing to the previous state; exits non-zero if already at the initial state.
- Update `koto workflows`: now returns a JSON array of workflow name strings (not objects with path/state metadata).
- Update `koto template compile`: remove `--output` flag; output is the compiled JSON file path on success; uses SHA256-based cache.
- Update `koto template validate`: takes a compiled JSON path; exits 0 on valid, non-zero with JSON error on invalid schema.
- Update `koto version`: output is now JSON `{"version":"...","commit":"...","built_at":"..."}` not plain text.
- Update "Typical agent workflow" section to remove `koto transition` and reflect that workflow advancement is not available in this release.

---

## doc-5: docs/guides/library-usage.md
**Section**: (new file)
**Prerequisite issues**: #1
**Update type**: modify
**Status**: pending
**Details**: The Go library guide documents packages (`pkg/engine`, `pkg/template`, `pkg/controller`, `pkg/discover`) that no longer exist after the Rust migration. Replace the file content with a short notice explaining that koto is now a Rust binary with no importable library interface, and that library-style integration is not available in this release. Alternatively, if the file is better removed than replaced, coordinate with the orchestrator — do not silently delete.

---

## doc-6: docs/reference/error-codes.md
**Section**: (multiple sections)
**Prerequisite issues**: #3, #4
**Update type**: modify
**Status**: updated
**Details**: Remove the Go-specific `*engine.TransitionError` struct definition and the Go code example at the top of the file. The Rust implementation uses `thiserror`-derived types, but callers only see JSON output — the language-specific struct is no longer relevant to document. Review each error code for accuracy against the new command surface: `version_conflict` may no longer apply (JSONL append-only log has no version counter), `terminal_state` behavior changes (the skeleton has no `transition` command to trigger it), and `template_mismatch` logic may differ. Remove or update codes that no longer apply. Add any new codes introduced by the Rust engine errors if they differ from the current set.

---

## doc-7: docs/guides/cli-usage.md
**Section**: State file resolution
**Prerequisite issues**: #3
**Update type**: modify
**Status**: updated
**Details**: This entry can be updated independently of the full CLI guide rewrite (doc-4). Update the state file resolution section alone once Issue 3 is complete: rename `.state.json` → `.state.jsonl`, describe the JSONL append-only format (one event per line, current state = last event's `state` field), and remove references to `--state` and `--state-dir` flags that no longer exist in the skeleton.
