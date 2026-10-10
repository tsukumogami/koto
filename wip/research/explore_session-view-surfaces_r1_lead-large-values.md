# Lead: How do the surfaces handle large or binary values, and what truncation/excerpt precedents exist?

## Findings

### Truncation and size-limit precedents

The cut is always at a UTF-8 boundary and always flagged:

- **Gate and action capture:** `src/action.rs:20-28` sets
  `MAX_ACTION_OUTPUT_BYTES` (64 KiB per stream). The result carries
  `stdout_truncated`, `stderr_truncated` and `truncated`
  (`src/action.rs:239-243`). `docs/reference/session-feed.md` documents them.
- **Session-log cut:** the session feed adds a second 4,096-byte cut. Byte
  bounds are measured on redacted UTF-8 text before JSON escaping, and a cut
  never splits a character.
- **Findings caps:** `src/findings.rs:33-53` bounds each finding's rule id
  (128 bytes), path (512), rule reference (512) and message (1,000). At most
  50 findings, with `findings_truncated` past that. `TRUNCATION_NOTE_LINE` is
  `"... [output truncated]"`. `redact.rs` has the cut helpers: `cut_bytes`
  (`:184`) and `safe_cut_len`.
- **Input caps:** `MAX_WITH_DATA_BYTES = 1_048_576` (`src/cli/mod.rs:50`) caps
  `--with-data`, `--inputs` and rationale; rejecting is chosen over truncating
  (`error-codes.md`). `MAX_VARS_FILE_BYTES` is 64 KiB (`init_entry.rs:43`).
  Wake files truncate at 32 KiB (`cli-usage.md`).
- **Context values have no size limit.** `koto context add` reads the whole
  file or stdin into memory (`src/cli/context.rs:36-52`). The manifest entry
  stores `size` and `hash` (`src/session/context.rs:9`), and the
  `ContextAdded` event carries `key`, `hash`, `size` and `writer` — so a view
  can show size and hash without reading content.
- **No excerpt rule for directives:** no truncation of directive or details
  text in `koto next` or status output.

### Dashboard (`src/cli/dashboard*.rs`)

- **`--once` feed:** `sanitize_field` (`dashboard.rs:75`) replaces `\t`, `\n`
  and `\r` with a space, so appended fields can't break the tab-separated
  contract. Applied to state, intent, template and idle; NOT applied to the id
  column, and it does not strip other control characters.
- **TUI evidence:** `EVIDENCE_DISPLAY_CAP = 3`, with a "↓ N more" line
  (`dashboard_render.rs:25,308-320`). Each entry is the full JSON of its
  fields, no per-value truncation.
- **History:** a `ContextAdded` event renders as only `context: <key>`
  (`dashboard_data.rs:857`). Size and content are not shown.
- **Layout:** width breakpoints below 40, below 80 and 80+
  (`dashboard_render.rs:57-72`). Ratatui's `Paragraph` wraps; no koto helper
  truncates to a column width.

### Size formatting

No KB/MiB formatting helper exists in `src/`. Nearest precedents are
`format_elapsed` (`dashboard.rs:82`, "1m5s" style) and error messages printing
raw byte counts (`init_entry.rs:220`).

### UTF-8 and binary handling

- `decode_capture` (`src/redact.rs:417-430`) drops a trailing partial sequence
  and lossy-replaces invalid bytes ("a command emitting binary still yields
  readable output").
- `${context.<key>}` substitution treats non-UTF-8 content as missing and
  resolves to the empty string (`custom-skill-authoring.md`).
- `koto context get` writes raw bytes without a UTF-8 check; `context list`
  prints only a JSON array of keys.
- No other binary detection (no NUL scanning). The session-feed design says of
  `stdout` only "may be large; consumers displaying this should truncate or
  paginate" (`DESIGN-session-feed-data-contract.md:758`).

### Terminal safety

- `DESIGN-local-dashboard.md:691-695`: ratatui renders to a cell buffer and
  does not interpret escapes in content.
- `DESIGN-session-migration.md:600-603`: bucket strings reach output only
  inside JSON-encoded strings.
- `src/discover.rs:65` cuts names to 50 bytes in error messages "to avoid
  terminal issues" — slicing at a byte offset could panic on a multibyte
  character (a counter-example to `safe_cut_len`).
- **No escaping of control characters in content.** The `--once` path writes
  strings directly, so ESC bytes would pass through. The ratatui guarantee is
  the only protection, and only on the TUI path.

### Binary fixtures

`tests/support/migration_carrier.rs:275-284` defines `carrier_keys()`: five
keys, one `data/blob.bin` (4,096 bytes of `(i*31)%256`), others text of several
sizes, one under a namespace. `tests/session_migration_test.rs` runs them
through the carrier and compares SHA-256. Reusable fixtures for the view's
tests already exist.

## Implications

- Bounded views of large values already exist for gates and actions: a byte
  cap, a cut at a character boundary and a boolean flag. A key excerpt can
  follow that pattern.
- Size comes for free from the manifest; `context list` currently prints none.
- A new size formatter is needed; binary detection is also new (try
  `from_utf8` on the first N bytes or look for NUL, fall back to type+size).
- Excerpts in `--once` need a stronger sanitizer than `sanitize_field` (ESC
  and other control characters pass through today). In the TUI, ratatui's
  buffer is the safeguard.
- An excerpt needs a character-boundary-safe cut; `redact::safe_cut_len` is
  the model.

## Surprises

- The 1 MiB cap applies only to JSON flag input; context values are unbounded
  and the dashboard never reads them.
- The dashboard surfaces context only as key names in history.
- The migration design's "JSON-encoded so safe" argument does not cover the
  tab-separated `--once` feed.

## Open Questions

- What is the excerpt size, and is it in bytes or characters?
- Should `--once` emit a tab-separated column for keys, or a separate mode?
- What counts as binary: invalid UTF-8, NUL, or control-character density?
- Which unit style for sizes (`4.0 KiB`) and how large values are labelled.
- Whether to show the hash or a file path for the "or a path" case, and what
  the path is for the cloud backend.
- Whether the sanitizer should also cover the id column and ESC characters.

## Summary

koto has clear precedents for bounded text (64 KiB capture with `*_truncated`
flags, character-boundary cuts in `redact.rs`, lossy decode in
`decode_capture`, and `size`/`hash` already in the context manifest) but no
size formatter, no binary/NUL detection, and no control-character sanitizer
beyond tab/newline in `--once`; context values themselves have no cap. The
view should reuse the cut-and-flag pattern, read sizes from the manifest, and
add a small shared excerpt function covering UTF-8 check, boundary-safe cut,
control-character escaping and size formatting. The biggest open question is
the excerpt bound and the binary criterion, which nothing in the repo decides.
