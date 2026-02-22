# Validation Report: Issue #4

Tested: 2026-02-22
Binary: built from source (`go build ./cmd/koto/`)
Platform: Linux 6.17.0-14-generic, Go 1.25.7
Isolation: `env -i` with explicit PATH including `$QA_HOME/bin`

---

## Scenario 1: Go module builds and vets cleanly

**Status**: passed

- `go build ./...` exited 0 with no output
- `go vet ./...` exited 0 with no output
- `go list -m all` returned only `github.com/tsukumogami/koto` -- no external dependencies

---

## Scenario 2: Init creates a valid state file

**Status**: passed

Command: `koto init --name test-workflow --template <template> --state-dir <dir>`

State file contents:
```json
{
  "schema_version": 1,
  "workflow": {
    "name": "test-workflow",
    "template_hash": "sha256:a5ef1903f1d63a3232a837c139aa40f3c2ea39580057b73db88af12ea90d5745",
    "template_path": "<absolute-path>/test.md",
    "created_at": "2026-02-22T05:50:50Z"
  },
  "version": 1,
  "current_state": "ready",
  "variables": {},
  "history": []
}
```

All expected fields present: `schema_version: 1`, `version: 1`, `current_state: "ready"` (initial state of stub machine), empty `history` array, `workflow` object with `name`, `template_hash`, `template_path`, and `created_at`.

---

## Scenario 3: Init output is valid JSON with state and path

**Status**: passed

stdout: `{"path":"<state-dir>/koto-test-s3.state.json","state":"ready"}`
Exit code: 0

Valid JSON with both `state` and `path` keys. Note: JSON key order is `path` then `state` (alphabetical from Go's map marshaling), which is fine since JSON object key order is not meaningful.

---

## Scenario 4: Transition advances state and increments version

**Status**: passed

After init (ready) then `koto transition running --state <path>`:

stdout: `{"state":"running","version":2}`
Exit code: 0

State file on disk:
- `current_state`: "running"
- `version`: 2
- `history`: 1 entry with `from: "ready"`, `to: "running"`, `type: "transition"`, and a valid RFC3339 timestamp

---

## Scenario 5: Transition from terminal state returns terminal_state error

**Status**: passed

After advancing ready -> running -> done, then attempting `koto transition running`:

stdout: `{"error":{"code":"terminal_state","message":"cannot transition from terminal state \"done\"","current_state":"done","target_state":"running"}}`
Exit code: 1

Error JSON contains `code: "terminal_state"` and `current_state: "done"` as expected.

---

## Scenario 6: Transition to invalid target returns invalid_transition error

**Status**: passed

From initial state "ready", attempting `koto transition nonexistent-target`:

stdout: `{"error":{"code":"invalid_transition","message":"cannot transition from \"ready\" to \"nonexistent-target\": not in allowed transitions [running]","current_state":"ready","target_state":"nonexistent-target","valid_transitions":["running"]}}`
Exit code: 1

Error JSON contains `code: "invalid_transition"` and `valid_transitions: ["running"]` listing the allowed targets from the current state.

---

## Scenario 7: Next returns execute directive for non-terminal state

**Status**: passed

After init (current state = "ready"):

stdout: `{"action":"execute","state":"ready","directive":"Execute the ready phase of the workflow."}`
Exit code: 0

JSON contains `action: "execute"`, `state: "ready"`, and a non-empty `directive` string.

---

## Scenario 8: Next returns done directive for terminal state

**Status**: passed

After advancing to terminal state "done":

stdout: `{"action":"done","state":"done","message":"workflow complete"}`
Exit code: 0

JSON contains `action: "done"`, `state: "done"`, and a `message` field.

---

## Scenario 9: Stub subcommands return not_implemented

**Status**: passed

All six stub subcommands tested:

| Command | Exit Code | error.code |
|---------|-----------|------------|
| rewind | 1 | not_implemented |
| cancel | 1 | not_implemented |
| query | 1 | not_implemented |
| status | 1 | not_implemented |
| validate | 1 | not_implemented |
| workflows | 1 | not_implemented |

Each command outputs JSON with `error.code: "not_implemented"` and a descriptive message.

---

## Additional Validation

Unit tests (`go test -short ./...`) all pass:
- `cmd/koto`: OK
- `internal/buildinfo`: OK
- `pkg/controller`: OK
- `pkg/engine`: OK

---

## Summary

All 9 scenarios passed. The walking skeleton implementation is fully functional:
- Init, transition, and next commands work correctly via CLI
- Error handling returns structured JSON with appropriate error codes
- State persistence works with atomic writes
- Stub subcommands are properly wired up as placeholders
- No external dependencies beyond Go stdlib
