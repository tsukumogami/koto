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

**Corrupt state file** — the JSONL file exists but all lines are malformed or it's empty:

```json
{"error":"corrupt state file","command":"next"}
```

Inspect the file directly. Each line should be a valid JSON event object.

---

### rewind

**Already at initial state** — only one event exists, so there's nothing to rewind to:

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
