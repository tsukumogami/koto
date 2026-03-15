# Error Code Reference

Every error from the koto CLI is a JSON object with an `error` field and a `command` field:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

The `error` field is a human-readable message. The `command` field identifies which subcommand produced the error. Both fields are always present.

## Error conditions by command

### init

**Workflow already exists** — a `koto-<name>.state.jsonl` file exists in the current directory:

```json
{"error":"workflow 'my-workflow' already exists","command":"init"}
```

Rename the workflow or delete the existing state file.

**Invalid template** — the template file can't be compiled:

```json
{"error":"failed to parse template: missing required field 'initial_state'","command":"init"}
```

Run `koto template compile <path>` to see the full compilation error.

---

### next

**Workflow not found** — no state file for the given name in the current directory:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

Run `koto workflows` to list active workflows.

**Incompatible state file format (exit code 3)** -- the state file uses an older format that's no longer supported. Two cases:

Old Go format (has `current_state` field):

```json
{"error":"incompatible state file format: state file uses old Go format; delete and re-initialize with 'koto init'","command":"next"}
```

Older JSONL format from #45 (has `type` but no `schema_version`):

```json
{"error":"incompatible state file format: state file uses an older format; delete and re-initialize with 'koto init'","command":"next"}
```

Delete the state file and run `koto init` again to create a new one in the current format.

**Corrupt state file (exit code 3)** -- the state file exists but can't be parsed. This covers empty files, invalid JSON, unrecognized formats, and sequence number gaps:

```json
{"error":"state file corrupted: sequence gap at line 4: expected seq 3, got 5","command":"next"}
```

Inspect the file directly. The first line should be a header with `schema_version`, and each subsequent line should be a valid event with a monotonic `seq` number. A truncated final line (e.g., from a crash) is recovered automatically -- only interior corruption triggers this error.

**No events in state file** -- the state file has a header but no event lines:

```json
{"error":"state file has no events","command":"next"}
```

---

### rewind

**Incompatible or corrupt state file (exit code 3)** -- same format detection errors as `next` apply to `rewind`. See the `next` section above for details.

**Already at initial state** -- only one state-changing event exists, so there's nothing to rewind to:

```json
{"error":"already at initial state, cannot rewind","command":"rewind"}
```

**Workflow not found:**

```json
{"error":"workflow 'my-workflow' not found","command":"rewind"}
```

---

### template compile

**Compilation failed** — invalid YAML, missing required fields, or unknown gate type:

```json
{"error":"missing required field 'initial_state'","command":"template compile"}
```

---

### template validate

**Schema invalid** — the compiled JSON doesn't match the expected schema:

```json
{"error":"invalid JSON: missing field `format_version`","command":"template validate"}
```
