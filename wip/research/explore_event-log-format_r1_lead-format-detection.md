# Research: Old Format Detection and Test Migration

## Summary

Three state file formats must be distinguished: the Go mutable JSON format (has
`CurrentState` or `current_state` field), the #45 simple JSONL format (no header,
no seq), and the new #46 JSONL-with-header format (`schema_version` on first line).
Detection is done by parsing the first line: presence of `schema_version` = new
format; presence of `current_state` = old Go format; anything else = #45 legacy.
Test migration is moderate in scope: the 15 integration tests use `koto` CLI calls
(not direct persistence writes), so tests automatically pick up the new format once
the CLI is updated. The 6 unit tests in `persistence.rs` that call `make_event`
directly will need rewriting with the new event shape.

## Current format (from #45)

Simple JSONL — one JSON object per line, no header:

```json
{"type":"init","state":"gather","timestamp":"2026-01-01T00:00:00Z","template":"/cache/abc.json","template_hash":"abc123"}
{"type":"rewind","state":"gather","timestamp":"2026-01-01T00:00:00Z"}
```

`Event` struct fields: `event_type` (serialized as `"type"`), `state`, `timestamp`,
`template?`, `template_hash?`. No `seq`. No typed payloads.

## Old Go format

Mutable JSON object (single object, not JSONL):

```json
{
  "current_state": "gather",
  "schema_version": 3,
  "workflow": { "name": "my-wf", "version": "1.0" },
  "variables": {},
  "evidence": { "key": "value" },
  "history": [
    { "from": "", "to": "gather", "timestamp": "...", "type": "init" }
  ]
}
```

Key marker: top-level `current_state` field (lowercase or camelCase depending on Go
version). The entire file is one JSON object, not newline-delimited.

## Detection logic

Detection must happen in `read_events` before attempting JSONL parsing. Read first line:

```
First line of file
├── Is valid JSON object?
│   ├── Has "schema_version" key  → New #46 format with header → parse normally
│   ├── Has "current_state" or "CurrentState" key  → Old Go format → error
│   ├── Has "type" and "state" (no "seq")  → #45 simple JSONL (first line is event)
│   │   → error with migration message
│   └── Has "type" and "state" and "seq"  → Already new format (schema_version=1 means
│       first line was header, this is first event) → parse normally
└── Not valid JSON → Corrupted file → error
```

The #46 format is unambiguous: the first line ALWAYS has `schema_version` and no
`seq` field. It's a header, not an event.

The #45 format: first line is an event with `type` field but no `seq`. If we see
`type` in the first line, it's a #45 legacy file (no header).

## Error messages for legacy formats

**Old Go format:**
```
error: state file uses an outdated format (Go implementation)

The file 'koto-<name>.state.jsonl' was written by the Go version of koto and
cannot be read by this version. The Go format stored mutable state; the new
format is an append-only event log.

To reset: delete the state file and re-initialize.
  rm koto-<name>.state.jsonl && koto init <name> --template <path>
```
Exit code: 3

**#45 simple JSONL format:**
```
error: state file uses the legacy event format (koto v0.x)

The file 'koto-<name>.state.jsonl' was written by an earlier version of koto
that used a simplified event schema without sequence numbers or typed payloads.
This version requires the full event log format.

To reset: delete the state file and re-initialize.
  rm koto-<name>.state.jsonl && koto init <name> --template <path>
```
Exit code: 3

Note: "migration tool" is NOT recommended for the error message. A `koto migrate`
command adds scope without clear value — workflows are short-lived; re-init is the
practical recovery path.

## Test migration scope

**Integration tests** (`tests/integration_test.rs`): 15 tests, all use `koto` CLI
via `assert_cmd`. These tests call `koto init`, `koto next`, etc. — they don't write
raw JSONL. Once the CLI is updated to write the new format, integration tests
automatically use it. **Migration effort: minimal.** Tests that assert on specific
JSON output fields (`state`, `directive`, `transitions`) will need updating if
the output schema changes, but the event log format change is internal.

**Unit tests** (`src/engine/persistence.rs`): 8 unit tests that use `make_event`
helper and `append_event`/`read_events` directly. These will need:
- `make_event` helper updated to include `seq` field and typed payloads
- Tests that write raw JSONL strings (e.g., `read_events_skips_malformed_lines`)
  need their literal JSON updated to include `seq`
- New tests for: header line parsing, seq gap detection, old format rejection,
  epoch boundary derivation

**Tricky case — `read_events_skips_malformed_lines` test:** This test writes raw
JSONL strings directly to a file:
```rust
writeln!(f, r#"{"type":"init","state":"gather","timestamp":"..."}"#)
```
With the new format, these raw strings need `seq` fields and the file needs a
header line. The test will need a full rewrite, or a new `write_raw_test_file()`
helper that constructs valid new-format files for testing.

## Recommended approach for format detection in persistence.rs

```rust
pub fn read_events(path: &Path) -> anyhow::Result<Vec<Event>> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines().enumerate();

    // First line must be the header
    let (_, first_line) = lines.next()
        .ok_or_else(|| anyhow::anyhow!("state file is empty"))??;

    let first_json: serde_json::Value = serde_json::from_str(&first_line)
        .map_err(|_| anyhow::anyhow!("state file is corrupted: first line is not valid JSON"))?;

    // Format detection
    if first_json.get("current_state").is_some() || first_json.get("CurrentState").is_some() {
        anyhow::bail!("state file uses an outdated format (Go implementation). \
            Delete and re-initialize: rm {} && koto init ...", path.display());
    }
    if first_json.get("type").is_some() {
        anyhow::bail!("state file uses the legacy event format (koto v0.x). \
            Delete and re-initialize: rm {} && koto init ...", path.display());
    }
    if first_json.get("schema_version").is_none() {
        anyhow::bail!("state file has unrecognized format: missing schema_version header");
    }

    // Parse header
    let header: StateFileHeader = serde_json::from_value(first_json)?;
    if header.schema_version != 1 {
        anyhow::bail!("state file uses schema_version {}; only version 1 is supported",
            header.schema_version);
    }

    // Parse events
    let mut events = Vec::new();
    let mut expected_seq = 1u64;
    // ... (gap detection logic per seq-gap-semantics research)

    Ok(events)
}
```
