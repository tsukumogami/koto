# Research: Header Line Schema and koto workflows

## Summary

The upstream design DESIGN-unified-koto-next.md specifies a header line schema for the new JSONL state files (`{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123","created_at":"..."}`) that contains workflow initialization metadata. Issue #46 (implement event log format) requires `koto workflows` to read header lines to return metadata beyond bare workflow names. Currently, `koto workflows` only globs for `koto-*.state.jsonl` files and returns sorted names as a JSON array of strings — it does not read headers or state file contents at all. The header schema needs to be finalized with all required fields, and `koto workflows` must be enhanced to parse headers and optionally return enriched metadata.

## Current koto workflows Behavior

**Current implementation:**
- `src/discover.rs` exports `find_workflows(dir: &Path) -> Vec<String>`
- Reads directory, matches files by glob pattern (`koto-*.state.jsonl`)
- Extracts workflow name by stripping `koto-` prefix and `.state.jsonl` suffix
- Returns sorted Vec of names

**Current CLI handler** (lines 284-296 in `src/cli/mod.rs`):
- Calls `find_workflows(&current_dir)`
- Serializes names as JSON array: `["my-workflow","task-42"]`
- Returns to stdout

**What it returns now:**
```json
["my-workflow","task-42"]
```

**What it does not do:**
- Does not read state file contents
- Does not parse the first line (header)
- Does not derive current state
- Does not return timestamps, hashes, or template paths
- Does not validate state files

## Header Schema Design (from DESIGN-unified-koto-next.md)

The upstream design specifies this as the state file format (lines 228-234):

```
{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123","created_at":"..."}
{"seq":1,"timestamp":"...","type":"workflow_initialized","payload":{"variables":{}}}
{"seq":2,"timestamp":"...","type":"transitioned","payload":{"from":null,"to":"gather_info","condition_type":"auto"}}
...
```

**Header line (first line):**
- JSONL format (one JSON object, not array)
- Must contain: `schema_version`, `workflow`, `template_hash`, `created_at`
- Is NOT a regular event (has no `seq`, `timestamp`, `type`, `payload`)
- Format detection requirement (line 247-250): old state files (mutable JSON) contain `CurrentState` field; new JSONL files do not

**Analysis of each candidate field:**

| Field | Purpose | Presence | Type | Notes |
|-------|---------|----------|------|-------|
| `schema_version` | State file format version | Required | number | Enables future format changes without breaking; currently 1 |
| `workflow` | Workflow name | Required | string | Must match the state filename prefix (koto-${workflow}.state.jsonl) |
| `template_hash` | SHA256 hash of compiled template | Required | string | Allows verification that events were created with this template version; matches `init` event's `template_hash` |
| `created_at` | Workflow creation timestamp | Required | string | ISO 8601 format (RFC 3339); useful for sorting workflows by age |

These are the only fields shown in the design example. The design does not mention:
- Current state (derived from last transitioned event)
- Template path (only available via replay of init event)
- Variable count or summary
- Last modified timestamp (derivable from last event's timestamp)

## Header vs workflow_initialized Event: Redundancy Analysis

**Current field mapping** (from integration test, line 138-144 in cli/mod.rs):

When `koto init` creates a workflow:
```rust
let event = Event {
    event_type: "init".to_string(),
    state: initial_state.clone(),
    timestamp: now_iso8601(),
    template: Some(cache_path_str),
    template_hash: Some(hash),
};
```

When serialized to JSONL, this becomes an event like:
```json
{"type":"init","state":"gather","timestamp":"2026-01-01T00:00:00Z","template":"/cache/abc.json","template_hash":"abc123"}
```

**Design shows this as `workflow_initialized` event** (DESIGN line 215):
```
| workflow_initialized | koto init | workflow, template_hash, variables |
```

**Redundancy analysis:**

| Field | Header | workflow_initialized Event | When Diverges | Why Separate |
|-------|--------|---------------------------|---------------|-----------   |
| `schema_version` | Required | None | Never | Header is format metadata, events are data |
| `workflow` | Required | Required (in payload) | Never | Header names the log; events redundantly declare it |
| `template_hash` | Required | Required (in payload) | If template changes | Design requires hash in header to verify log integrity on load; event carries it for audit trail |
| `created_at` | Required | None (init.timestamp exists) | Never | Header captures workflow creation instant; init event timestamp is recorded separately for audit |
| Template path | Not in header | In workflow_initialized event | N/A | Path is only needed when replaying events; header is lightweight metadata for discovery |

**Rationale for structure:**

- **Header fields are immutable:** Once written, they're read-only metadata about the workflow itself (when created, with what schema, what template)
- **Event fields are audit trail:** Each event carries timestamp and type-specific data; events are the authoritative log
- **Redundancy is intentional:** `template_hash` appears in both because header uses it for fast integrity checks before replay; events include it for the audit trail
- **Performance driver:** `koto workflows` reads only the first line (header) to get schema_version and template_hash without replaying the entire log
- **No template_path in header:** The path is transient (cached in `~/.cache/koto/`); header should not encode filesystem layout; it lives only in init event

## Recommended Header Schema

**Complete JSONL header schema:**

```json
{
  "schema_version": 1,
  "workflow": "my-workflow",
  "template_hash": "abc123def456...",
  "created_at": "2026-03-15T14:30:00Z"
}
```

**Field specifications:**

| Field | Type | Example | Constraints | Why Required |
|-------|------|---------|-----------|-------------|
| `schema_version` | Integer (1) | `1` | Immutable; must equal 1 for current format | Enables breaking changes in future formats without data loss |
| `workflow` | String | `"my-workflow"` | Must match `[a-z0-9-]+` and state filename | Allows validation that file matches its intended name; prevents misnamed files being misinterpreted |
| `template_hash` | String | `"sha256_hex_64_chars"` | 64-char hex SHA-256 digest | Fast integrity check before replay; detects stale workflows when template changes |
| `created_at` | String | `"2026-03-15T14:30:00Z"` | RFC 3339 / ISO 8601 UTC | Enables sorting workflows by age; audit trail anchor point |

**Rationale for "required" status:**
- All four fields are required to prevent partial headers and format ambiguity
- Format detection relies on these fields being present (not JSON with `CurrentState` field)
- No optional fields — simpler to parse, no null-handling needed

## Recommended koto workflows Output Change (if any)

**Current output (status quo for #45, the foundation):**
```json
["my-workflow","task-42"]
```

**Issue #46 requirement:** `koto workflows` should read the header line for workflow metadata

**Three options:**

### Option A: Keep current output (names only)
- No change to `koto workflows` command
- Rationale: Simplicity; agents don't need metadata to call `koto next <name>`
- Trade-off: Loss of optimization opportunity; agents must call `koto next` to get state

**Recommendation: NOT this — Issue #46 explicitly says to read headers**

### Option B: Return array of metadata objects (breaking change)
```json
[
  {
    "name": "my-workflow",
    "current_state": "gather_info",
    "created_at": "2026-03-15T14:30:00Z",
    "template_hash": "abc123"
  },
  {
    "name": "task-42",
    "current_state": "complete",
    "created_at": "2026-03-14T09:00:00Z",
    "template_hash": "def456"
  }
]
```

**Fields to include:**
- `name`: Workflow name (from filename)
- `current_state`: Derived from last event in log (requires replay)
- `created_at`: From header (O(1) read)
- `template_hash`: From header (O(1) read)

**Considerations:**
- Breaking change to CLI contract (current output is array of strings; new is array of objects)
- Agents need to parse new format; tools like `jq` need adjustment
- But #45 established that koto has no released users, so breaking changes acceptable during foundation phase
- Agents calling `koto workflows` typically want current_state to display status without calling `koto next`

**Recommendation: YES — this aligns with Issue #46's intent to return metadata**

### Option C: Return both
```json
{
  "workflows": ["my-workflow","task-42"],
  "metadata": {
    "my-workflow": {
      "current_state": "gather_info",
      "created_at": "2026-03-15T14:30:00Z",
      "template_hash": "abc123"
    },
    "task-42": {
      "current_state": "complete",
      "created_at": "2026-03-14T09:00:00Z",
      "template_hash": "def456"
    }
  }
}
```

**Trade-off:**
- Not breaking; tools expecting `jq '.[0]'` still work (returns array, not object)
- But output is verbose and agents must handle nested structure
- Redundancy: names appear in both array and metadata keys

**Recommendation: NO — overly complex for a discovery command**

## Implementation Approach for koto workflows with Headers

**Recommended implementation (Option B output schema):**

```rust
pub fn find_workflows_with_metadata(dir: &Path) -> anyhow::Result<Vec<WorkflowMetadata>> {
    let mut workflows = Vec::new();
    
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        
        // Check filename matches pattern
        let file_name = entry.file_name();
        let name_str = match file_name.to_str() {
            Some(n) if n.starts_with("koto-") && n.ends_with(".state.jsonl") => {
                let inner = &n[5..n.len()-11];
                if inner.is_empty() { continue; }
                inner.to_string()
            },
            _ => continue,
        };
        
        // Read first line (header)
        let header_line = match read_first_line(&path) {
            Ok(Some(line)) => line,
            _ => continue, // Skip if unreadable
        };
        
        // Parse header JSON
        let header: HeaderLine = match serde_json::from_str(&header_line) {
            Ok(h) => h,
            Err(_) => continue, // Skip if not valid JSON
        };
        
        // Validate schema_version
        if header.schema_version != 1 {
            continue; // Skip future format versions
        }
        
        // Derive current_state from log
        let current_state = derive_current_state_from_log(&path)?;
        
        workflows.push(WorkflowMetadata {
            name: name_str,
            current_state,
            created_at: header.created_at,
            template_hash: header.template_hash,
        });
    }
    
    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(workflows)
}
```

**Data structures needed:**

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct HeaderLine {
    pub schema_version: u32,
    pub workflow: String,
    pub template_hash: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowMetadata {
    pub name: String,
    pub current_state: String,
    pub created_at: String,
    pub template_hash: String,
}
```

**Performance note:**
- Reading header (first line) is O(1) per file
- Deriving current_state requires reading last line of each state file (O(file size))
- For long-running workflows with thousands of events, this becomes slow
- Mitigation: Issue #46 should consider caching `current_state` in header, or defer state derivation to a lazy-load flag

## Testing Considerations for Header Implementation

**Test cases needed for Issue #46:**

1. **Header parsing:**
   - Valid header with all four fields
   - Invalid header (missing schema_version, wrong type)
   - Missing header (old-format state file with JSON object instead of JSONL)
   - Corrupted header (invalid JSON on first line)

2. **Format detection:**
   - Old mutable JSON format has `CurrentState` → should error or skip
   - New JSONL format has no `CurrentState` → should parse

3. **koto workflows with headers:**
   - Multiple workflows with different headers
   - Workflow with missing/corrupted header (should skip gracefully)
   - Empty directory
   - Files matching glob but not valid state files

4. **Metadata accuracy:**
   - current_state matches last transitioned event's `to` field
   - created_at from header matches init event's timestamp
   - template_hash from header matches init event's template_hash

## Open Questions for Issue #46

1. **Should koto workflows skip corrupted state files or error?**
   - Current behavior (discover.rs): silently skip non-matching filenames
   - Recommendation: Skip state files with unreadable headers, but log warning to stderr (matches persistence.rs pattern)

2. **Should schema_version mismatch be an error or skip?**
   - Recommendation: Skip files with schema_version != 1; allows tooling to handle future formats gracefully

3. **What if header's `workflow` field doesn't match filename?**
   - Possible corruption or manual file rename
   - Recommendation: Trust filename, log warning to stderr

4. **Should `koto workflows --verbose` return full metadata vs compact?**
   - Not mentioned in design; recommend clarifying in Issue #46
   - Compact: names only (current)
   - Verbose: with metadata

5. **Performance: When should current_state be included?**
   - Reading all event logs defeats the purpose of fast discovery
   - Options:
     - (A) Always derive current_state (slow but accurate)
     - (B) Cache in header after each `koto next` call (requires updating header)
     - (C) Return only created_at/template_hash; agents call `koto next <name>` for state
   - Recommendation for #46: Option C (header fields only); defer caching to later tactical design
