# Decision 8: Content storage model

## Question

How does koto store submitted context internally?

## Chosen option

**Files in session directory with manifest**

Store each key as a file inside the session directory (e.g., `~/.koto/sessions/<repo-id>/<name>/ctx/<key>`). A manifest JSON tracks metadata (creation time, size, hash).

## Evaluation

### Option 1: Files in session directory with manifest

**Strengths:**

- Consistent with LocalBackend's existing pattern of session directories as bundles of files. The `create`, `session_dir`, `cleanup`, and `list` methods already treat directories as the unit of storage. Adding a `ctx/` subdirectory fits naturally.
- Zero new dependencies. Uses `std::fs` only, matching the decision driver from the design doc.
- Cloud sync works naturally: files sync to S3 as individual objects. R8 in the PRD specifically notes "files sync more naturally than databases."
- Fully debuggable. Developers can `ls` and `cat` files in the session directory to inspect stored content. This matters during development and troubleshooting.
- Per-key advisory flock is straightforward: lock the individual file being written.
- The manifest is a single JSON file (`ctx/manifest.json`) that tracks metadata without requiring the content files to carry it. On `koto context list`, read the manifest. On `koto context add`, update the manifest atomically alongside the content write.
- `koto context get` can serve content directly from the filesystem, enabling `--to-file` to be a simple copy operation with no serialization overhead.

**Weaknesses:**

- Keys that contain path separators (e.g., `research/lead-1.md`) need sanitization or a mapping layer. The manifest can map logical keys to safe filenames.
- Two writes per `add` (content file + manifest update) need to be ordered correctly to avoid inconsistent state. Write content first, then manifest -- on crash, orphaned content without a manifest entry is harmless and detectable.

### Option 2: JSONL append log per key

**Strengths:**

- Consistent with the JSONL state file format used by the engine's `persistence.rs`. Same `BufWriter` + `sync_data()` pattern.
- Enables history/audit: each write is a new JSONL entry with timestamp.

**Weaknesses:**

- Content is embedded in JSON, requiring escaping. A 20KB markdown file becomes a JSON string literal with escaped newlines, nearly doubling size and making files unreadable with `cat`. This directly conflicts with the debuggability constraint.
- `koto context get` must parse the JSONL file and extract the last entry's content field, then unescape it. For large content, this is wasteful.
- History/audit is not a PRD requirement. The PRD explicitly scopes to replace-only semantics. Storing history adds complexity without a current use case.
- Cloud sync of JSONL files that grow over time is less natural than syncing discrete files that are replaced atomically.
- Per-key locking still needs one file per key, so there's no file-count reduction versus Option 1.

### Option 3: SQLite database

**Strengths:**

- Fast queries, transactions, and concurrent access built in.

**Weaknesses:**

- Adds a binary dependency (`rusqlite` or equivalent). This directly violates the "zero new dependencies" decision driver.
- Content is opaque -- developers can't inspect it without SQLite tools. Violates the debuggability constraint.
- Cloud sync of a single SQLite file is problematic. S3 sync would need to upload the entire database on every context submission, and concurrent access from multiple machines risks corruption. This conflicts with R8.
- SQLite's locking model is database-wide, not per-key, which is a worse fit for the concurrent multi-agent write pattern described in R5.

## Rationale

Files in a session directory are the simplest model that satisfies all constraints. The existing codebase already treats session directories as bundles of files (state file lives in the session directory). Adding a `ctx/` subdirectory with one file per key and a manifest for metadata extends this pattern without introducing new abstractions. The alternatives either sacrifice debuggability (SQLite), add unnecessary complexity for features not in scope (JSONL history), or introduce dependencies (SQLite).

The consistency argument for JSONL -- that it matches the state file format -- is weaker than it appears. State files are append-only event logs where the format serves a purpose (replay, crash recovery). Context storage is replace-only by design (PRD explicitly scopes this). Using an append-only format for replace-only data creates overhead without benefit.

## Assumptions

- Keys will be short strings (under 255 characters) that can be mapped to safe filenames via the manifest.
- Content sizes range from a few bytes to ~20KB based on current wip/ artifact patterns. No content will exceed a few hundred KB.
- The manifest file won't become a bottleneck because concurrent writes target different keys, and the manifest update is a small atomic write.

## Open questions

None. The constraints strongly favor this option.
