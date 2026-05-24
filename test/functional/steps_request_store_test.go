package functional

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/cucumber/godog"
)

// InitializeRequestStoreScenario registers the extra step definitions
// used by the request-store feature files. The base set of steps lives
// in steps_test.go; these supplement it with the JSON-array-shape and
// symlink-fixture assertions the request-store scenarios need.
func InitializeRequestStoreScenario(ctx *godog.ScenarioContext) {
	ctx.Step(`^the JSON output field "([^"]*)" has length (\d+)$`, theJSONOutputFieldHasLength)
	ctx.Step(`^the JSON output field "([^"]*)" does not have element (\d+)$`, theJSONOutputFieldDoesNotHaveElement)
	ctx.Step(`^the session directory "([^"]*)" is a symlink to "([^"]*)"$`, theSessionDirIsSymlinkTo)
	ctx.Step(`^the state file header for "([^"]*)" sets:$`, theStateFileHeaderSets)
}

// theJSONOutputFieldHasLength asserts that the JSON value at the given
// dotted path is an array of exactly `expected` elements.
func theJSONOutputFieldHasLength(field string, expected int) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return fmt.Errorf("JSON field %q not found in output:\n%s", field, sc.stdout)
	}
	arr, ok := val.([]interface{})
	if !ok {
		return fmt.Errorf("JSON field %q is not an array: %T (value: %v)", field, val, val)
	}
	if len(arr) != expected {
		return fmt.Errorf("JSON field %q: expected length %d, got %d\nfull output:\n%s",
			field, expected, len(arr), sc.stdout)
	}
	return nil
}

// theJSONOutputFieldDoesNotHaveElement asserts that the JSON value at
// the given path either does not exist OR is an array shorter than or
// equal to the given index. Useful for "the array does not include a
// duplicate at index N" style checks where the array may legitimately
// have fewer elements.
func theJSONOutputFieldDoesNotHaveElement(field string, idx int) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return nil
	}
	arr, ok := val.([]interface{})
	if !ok {
		return fmt.Errorf("JSON field %q is not an array: %T", field, val)
	}
	if idx < len(arr) {
		return fmt.Errorf("JSON field %q unexpectedly has element %d (array length %d):\n%s",
			field, idx, len(arr), sc.stdout)
	}
	return nil
}

// theSessionDirIsSymlinkTo creates a symlink at
// <tempDir>/.koto/sessions/<linkName> pointing at the existing session
// directory <tempDir>/.koto/sessions/<targetName>. Used by the workspace
// prune symlink-refusal scenario; symlinks in the live workspace are a
// workspace-escape vector the verb must reject before any I/O.
func theSessionDirIsSymlinkTo(linkName, targetName string) error {
	sessionsDir := filepath.Join(sc.tempDir, ".koto", "sessions")
	if err := os.MkdirAll(sessionsDir, 0755); err != nil {
		return fmt.Errorf("mkdir sessions dir: %w", err)
	}
	target := filepath.Join(sessionsDir, targetName)
	link := filepath.Join(sessionsDir, linkName)
	if _, err := os.Lstat(target); err != nil {
		return fmt.Errorf("symlink target %q does not exist: %w", target, err)
	}
	if err := os.Symlink(target, link); err != nil {
		return fmt.Errorf("creating symlink %q -> %q: %w", link, target, err)
	}
	return nil
}

// theStateFileHeaderSets reads the first line of
// <tempDir>/.koto/sessions/<name>/koto-<name>.state.jsonl, parses it as
// the header JSON object, merges the given `key: value` pairs (one per
// line) into it, and writes it back. Used to flip an existing session's
// header into the request-store dispatched-child shape so the per-write
// epoch fence (and the `unassigned_children` discovery scan) can be
// exercised end-to-end without requiring the request-store substrate to
// produce the header itself.
//
// Each docstring line is `key: value` with `value` parsed as JSON. The
// helper accepts strings, booleans, and integers — sufficient for the
// header fields the request-store fence keys off (needs_agent, role,
// coordinator_of_record, requested_by, dispatch_epoch).
func theStateFileHeaderSets(name string, body *godog.DocString) error {
	stateFile := filepath.Join(sc.tempDir, ".koto", "sessions", name,
		fmt.Sprintf("koto-%s.state.jsonl", name))
	content, err := os.ReadFile(stateFile)
	if err != nil {
		return fmt.Errorf("read state file %q: %w", stateFile, err)
	}
	lines := strings.SplitN(string(content), "\n", 2)
	if len(lines) == 0 {
		return fmt.Errorf("state file %q is empty", stateFile)
	}
	headerLine := lines[0]
	var header map[string]interface{}
	if err := json.Unmarshal([]byte(headerLine), &header); err != nil {
		return fmt.Errorf("parse header %q: %w", headerLine, err)
	}
	for _, raw := range strings.Split(body.Content, "\n") {
		raw = strings.TrimSpace(raw)
		if raw == "" {
			continue
		}
		colon := strings.Index(raw, ":")
		if colon < 0 {
			return fmt.Errorf("malformed header override line %q (want `key: value`)", raw)
		}
		key := strings.TrimSpace(raw[:colon])
		value := strings.TrimSpace(raw[colon+1:])
		var v interface{}
		if err := json.Unmarshal([]byte(value), &v); err != nil {
			return fmt.Errorf("override %q: value %q is not valid JSON: %w", key, value, err)
		}
		header[key] = v
	}
	rewritten, err := json.Marshal(header)
	if err != nil {
		return fmt.Errorf("re-encode header: %w", err)
	}
	out := string(rewritten)
	if len(lines) > 1 {
		out += "\n" + lines[1]
	}
	if err := os.WriteFile(stateFile, []byte(out), 0644); err != nil {
		return fmt.Errorf("write state file %q: %w", stateFile, err)
	}
	return nil
}
