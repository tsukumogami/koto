# Lead: Go implementation audit

## Findings

### CLI Surface (cmd/koto/)

Nine subcommands, all output JSON, all flags in `--key value` format:

| Command | Purpose |
|---------|---------|
| `version` | Print build version info |
| `init` | Initialize a workflow from a template |
| `transition` | Advance state to a named target |
| `next` | Get current directive |
| `query` | Inspect full workflow state as JSON |
| `status` | Check workflow status |
| `rewind` | Roll back to a previous state |
| `cancel` | Delete state file (abandon workflow) |
| `template validate` / `template compile` | Validate and compile templates |
| `workflows` | List active workflows |

### State File Format

Files named `koto-<name>.state.json`. JSON with:

```json
{
  "SchemaVersion": 2,
  "Workflow": {
    "Name": "...",
    "TemplateHash": "...",
    "TemplatePath": "...",
    "CreatedAt": "..."
  },
  "Version": 3,
  "CurrentState": "analyze",
  "Variables": {},
  "Evidence": { "key": "value" },
  "History": [
    { "From": "init", "To": "gather", "Timestamp": "...", "Type": "transition", "Evidence": {} }
  ]
}
```

Key behaviors:
- Atomic writes: write to temp file, fsync, rename (prevents partial writes)
- Version counter incremented on each write; concurrent modification detected by comparing Version on read vs. write
- SchemaVersion 1 and 2 both supported (legacy)

### Template Format

Two formats:
1. **Legacy**: Markdown with YAML header (deprecated but still parsed)
2. **Modern compiled**: FormatVersion=1 JSON produced by `koto template compile`

Compiled template JSON structure:
```json
{
  "FormatVersion": 1,
  "Name": "workflow-name",
  "States": {
    "state-name": {
      "Directive": "Do this thing",
      "Transitions": ["next-state"],
      "Terminal": false,
      "Gates": [
        { "Type": "field_not_empty", "Field": "result" },
        { "Type": "field_equals", "Field": "status", "Value": "done" },
        { "Type": "command", "Command": "./check.sh", "Timeout": 30 }
      ]
    }
  }
}
```

Template hashing: SHA256 of compiled JSON (modern) or SHA256 of raw source (legacy).
Template hash stored in state file; mismatch detected at runtime.

Variable interpolation: `{{KEY}}` placeholders in Directive strings, substituted from Variables map.

### pkg/engine/

Core state machine logic:
- Transition validation (is target a valid outgoing transition from current state?)
- Gate evaluation: AND logic across all gates; field gates check Evidence map, command gates execute shell with timeout (30s default), process groups for clean kill
- Evidence accumulation into state.Evidence map
- Rewind: rolls back History to a prior state (cannot rewind to terminal states)
- Version conflict detection on persist

### pkg/template/

- YAML parsing via `gopkg.in/yaml.v3`
- Template compilation (source → FormatVersion=1 JSON)
- Template loading from compiled JSON
- Cache integration for compiled templates

### pkg/cache/

Compiled template cache: SHA256(compiled JSON) → cached file on disk. Avoids re-compilation on repeated use.

### pkg/controller/

Orchestrates engine + template loading; the layer `cmd/koto/` calls into.

### pkg/discover/

File globbing to find `koto-*.state.json` files in the current directory (for `koto workflows`).

### Non-trivial porting requirements

1. **Atomic file I/O**: write-to-temp-then-fsync-then-rename pattern must be replicated exactly
2. **Version conflict detection**: Version field compared on read vs. write; concurrent writers get an error
3. **YAML parsing**: `gopkg.in/yaml.v3` — needs `serde_yaml` or similar in Rust
4. **Dual hash support**: legacy templates hashed from raw source; modern from compiled JSON
5. **Command gates with process groups**: shell execution with timeout and clean kill of child processes
6. **Variable interpolation**: `{{KEY}}` substitution in directive strings
7. **File globbing**: `koto-*.state.json` discovery

## Implications

The functional scope for the Rust rewrite is well-defined: 9 commands, one state file format (JSON today, JSONL after the event-sourced refactor), one compiled template format, and a handful of gate types. The atomic write + version conflict pattern is the most correctness-sensitive piece. Shell command execution with timeout/process-group kill needs careful handling in Rust (tokio::process or std::process with thread-based timeout).

## Surprises

- More commands than expected: `cancel`, `workflows`, `query` are not mentioned in the issue body's "What stays the same" list but exist in the codebase
- SchemaVersion 1 and 2 both exist — but since this is pre-release and the event-sourced refactor will replace the format entirely, the Rust rewrite can target SchemaVersion 2 only (or skip versioning entirely since v3/JSONL is coming immediately after)
- `koto transition` is listed in issue #45 "What stays the same" but `DESIGN-unified-koto-next.md` explicitly removes it — the Rust rewrite should preserve it for now since the event-sourced changes come later

## Open Questions

- Should the Rust rewrite include `cancel` and `workflows` subcommands given they're not mentioned in the issue? Likely yes — preserve all current behavior.
- The event-sourced refactor replaces the state file format entirely — should the Rust rewrite bother implementing SchemaVersion 1 legacy format detection, or just SchemaVersion 2?

## Summary

The koto Go codebase implements 9 CLI subcommands, a JSON state file with atomic write + version-conflict detection, and a compiled template format with three gate types (field check, field equals, command). The most complex pieces to port are atomic file I/O with version conflict detection and command gate execution with process-group-based timeout/kill. The Rust rewrite scope is larger than the issue body implies — cancel and workflows subcommands exist and should be included.
