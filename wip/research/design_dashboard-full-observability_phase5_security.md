# Security Review: dashboard-full-observability

## Dimension Analysis

### External Artifact Handling
**Applies:** No

The design introduces no new external artifact surface. The dashboard reads only local JSONL state files written by the koto engine itself. The `IntentUpdated` event appended by `koto session update --intent` writes a plain string provided by the local user on the command line — no network fetch, no downloaded binary, no external URL resolution. `EvidenceSubmitted.summary` is an optional string field added to an existing event type and sourced the same way all other evidence fields are: from local agent output.

The migration helper (D4) traverses `~/.koto/sessions/` to find and rename directories matching the old 16-hex-char subdirectory pattern. It touches only files already owned by koto on the local filesystem; it does not download or execute anything.

Template file reads in `is_terminal_state` (existing code, unmodified by this design) read compiled JSON templates from a path stored in the state file header. That path is set at `koto init` time, not via any network request, so it remains local-read-only with no new vector introduced here.

### Permission Scope
**Applies:** Yes — low severity, no escalation risk, one consideration worth documenting.

**Current scope (unchanged by most of this design):** Read/write access to `~/.koto/sessions/<session>/` and the compiled template cache. No network, no elevated privileges, no sudo.

**What D1 changes:** Removing the repo-id hash from `LocalBackend::new()` relocates session files from `~/.koto/sessions/<16-hex-id>/<name>/` to `~/.koto/sessions/<name>/`. The flattened namespace means any koto invocation from any working directory now reads and writes the same directory tree. Before this change, a session named `my-workflow` in project A and a session with the same name in project B were naturally isolated by their distinct repo-id subdirectories.

**Risk:** If two projects happen to use the same session name, their state files now share a namespace under `~/.koto/sessions/`. The design acknowledges this via the migration collision warning ("collisions are left in place with a warning"). This is a correctness concern — not a privilege escalation — because both projects already belong to the same user. It could, however, allow one project's dashboard to display or interact with another project's sessions, which is an unintended data commingling.

**Mitigation already present:** `validate_session_id` enforces a strict allowlist (`^[a-zA-Z][a-zA-Z0-9._~-]*$`) on all session IDs before any filesystem path is constructed, blocking path traversal. The `.koto` root is created with mode 0700, individual state files with mode 0600 (both existing protections, unmodified). `flock`-based exclusive locking protects concurrent writes.

**Gap to document:** The design should note that users who rely on same-named sessions across multiple projects will experience collisions after migration, and should recommend workflow naming conventions (e.g., project-prefixed names) as a mitigation.

**What D2 changes:** `koto session update --intent` uses `O_APPEND` to append an `IntentUpdated` JSONL line. `O_APPEND` guarantees atomicity for writes up to `PIPE_BUF` bytes on POSIX systems (typically 4096 bytes). Intent strings are short in practice, but the design does not bound their length. A very long intent string (>4096 bytes) could produce a non-atomic append that interleaves with a concurrent engine write, corrupting the JSONL file.

**Recommendation:** Add a length cap on the intent string (e.g., 1024 characters) before appending, consistent with how the engine's own event payloads are naturally bounded.

### Supply Chain or Dependency Trust
**Applies:** No

This design adds no new crate dependencies. All changed modules (`src/session/local.rs`, `src/event/mod.rs`, `src/cli/dashboard*.rs`) use types and helpers already present in the codebase. The migration helper uses only `std::fs`. The dashboard rendering changes use the existing `ratatui` dependency. No new build-time scripts, proc macros, or external registries are introduced.

### Data Exposure
**Applies:** Yes — low severity, no transmission, but two surface expansions worth noting.

**Session scope expansion (D1):** Before this change, `koto dashboard` showed only sessions scoped to the current repo's hashed ID. After D1, it shows all sessions across all projects. A developer running the dashboard from any directory will now see session names, states, intent strings, template names, and gate history for every koto-managed workflow on their machine. This is intentional (the "global scope as F5 foundation" decision driver), but it's worth making explicit in user-facing documentation: the dashboard provides a global view, not a per-project one.

**Intent field (D2):** `IntentUpdated` stores a free-text string written by the user or an agent. It will appear in the dashboard's Summary tab and in the JSONL state file, which is readable by any process running as the same user. This is no worse than the existing state file content (which already contains template paths, state names, gate outputs, and evidence fields), but users should be aware that intent strings they type or that agents generate may contain sensitive context (e.g., partial filenames, feature descriptions). No encryption or redaction is proposed, which is appropriate given the existing model, but should be noted in documentation.

**`EvidenceSubmitted.summary`:** An optional plain-text summary field on an existing event type. Same exposure posture as other evidence fields already in the file. No new risk introduced.

**No network transmission:** The dashboard is a read-only TUI consuming local files. No telemetry, no remote write, no data leaves the machine through any mechanism introduced by this design.

## Recommended Outcome

**OPTION 2 - Document considerations:**

The design is sound and introduces no material security risks. Two items warrant a short Security Considerations section in the design document rather than blocking changes:

---

### Security Considerations

**Session namespace collision after migration**

Removing the repo-id scoping (D1) merges all project sessions into a single flat namespace. Two projects using the same session name will collide; the migration helper warns on collision and leaves the existing session in place, but the new session is not created. Users should adopt project-prefixed session names (e.g., `myproject-feature-branch`) to avoid collisions. The dashboard's global view is intentional; document it so users are not surprised by sessions from other projects appearing in the list.

**Intent string length and O_APPEND atomicity**

`koto session update --intent` uses `O_APPEND` to avoid a read-modify-write race with a running engine. POSIX guarantees `O_APPEND` writes are atomic only up to `PIPE_BUF` bytes (typically 4096). Intent strings are expected to be short, but the design does not enforce a bound. Impose a maximum length (1024 characters is reasonable) before the append so that no write can exceed the atomicity guarantee, and return a clear error if the limit is exceeded.

**Dashboard visibility is user-scoped**

State files are owned by the invoking user (mode 0600, `.koto` root mode 0700). The dashboard exposes session names, states, gate outcomes, evidence fields, and intent strings — all already present in the state file. No data leaves the machine. Users should be aware that intent strings may contain context they consider sensitive, as the state file is not encrypted.

---

## Summary

This design introduces no external artifact fetching, no privilege escalation, no new dependencies, and no network data transmission. The two items that need attention are: (1) the O_APPEND atomicity boundary for long intent strings, which warrants a length cap of ~1024 characters in the CLI handler before the append; and (2) the session namespace collision risk from flattening the directory layout, which should be documented as a naming-convention recommendation rather than a code change. Neither finding blocks the design.
