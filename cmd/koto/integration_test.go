package main_test

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// kotoBinary is the path to the compiled koto binary, set by TestMain.
var kotoBinary string

func TestMain(m *testing.M) {
	// Build the koto binary once for all integration tests.
	tmp, err := os.MkdirTemp("", "koto-integration-*")
	if err != nil {
		fmt.Fprintf(os.Stderr, "create temp dir: %v\n", err)
		os.Exit(1)
	}
	defer os.RemoveAll(tmp)

	kotoBinary = filepath.Join(tmp, "koto")
	cmd := exec.Command("go", "build", "-o", kotoBinary, ".") //nolint:gosec // test-only: path is from os.MkdirTemp
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "build koto binary: %v\n", err)
		os.Exit(1)
	}

	os.Exit(m.Run())
}

const integrationTemplate = `---
name: integration
version: "1.0"
description: Integration test workflow
initial_state: assess
variables:
  TASK:
    default: default-task
states:
  assess:
    transitions: [plan]
  plan:
    transitions: [implement]
  implement:
    transitions: [done]
  done:
    terminal: true
---

## assess

Assess the task: {{TASK}}.

## plan

Create a plan for {{TASK}}.

## implement

Build the solution.

## done

Work is complete.
`

// writeTemplate writes a template file to dir and returns its absolute path.
func writeTemplate(t *testing.T, dir, name, content string) string {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatalf("write template: %v", err)
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		t.Fatalf("abs path: %v", err)
	}
	return abs
}

// runKoto executes the koto binary with the given args and returns stdout,
// stderr, and any error. A non-zero exit code returns an *exec.ExitError.
func runKoto(t *testing.T, args ...string) (stdout, stderr string, err error) {
	t.Helper()
	cmd := exec.Command(kotoBinary, args...) //nolint:gosec // G204: integration test runs built binary with known args
	var outBuf, errBuf []byte

	cmd.Stdout = writerFunc(func(p []byte) (int, error) {
		outBuf = append(outBuf, p...)
		return len(p), nil
	})
	cmd.Stderr = writerFunc(func(p []byte) (int, error) {
		errBuf = append(errBuf, p...)
		return len(p), nil
	})

	err = cmd.Run()
	return string(outBuf), string(errBuf), err
}

// writerFunc adapts a function to the io.Writer interface.
type writerFunc func([]byte) (int, error)

func (f writerFunc) Write(p []byte) (int, error) { return f(p) }

// mustRunKoto runs koto and fails the test if it returns an error.
func mustRunKoto(t *testing.T, args ...string) string {
	t.Helper()
	stdout, stderr, err := runKoto(t, args...)
	if err != nil {
		t.Fatalf("koto %v failed: %v\nstdout: %s\nstderr: %s", args, err, stdout, stderr)
	}
	return stdout
}

// parseJSON unmarshals a JSON string into a map. Fails the test on error.
func parseJSON(t *testing.T, s string) map[string]interface{} {
	t.Helper()
	var m map[string]interface{}
	if err := json.Unmarshal([]byte(s), &m); err != nil {
		t.Fatalf("parse JSON: %v\ninput: %s", err, s)
	}
	return m
}

// jsonStr extracts a string value from a parsed JSON map at the given key.
func jsonStr(t *testing.T, m map[string]interface{}, key string) string {
	t.Helper()
	v, ok := m[key]
	if !ok {
		t.Fatalf("key %q not found in JSON: %v", key, m)
	}
	s, ok := v.(string)
	if !ok {
		t.Fatalf("key %q is not a string: %T", key, v)
	}
	return s
}

// jsonFloat extracts a float64 value from a parsed JSON map at the given key.
func jsonFloat(t *testing.T, m map[string]interface{}, key string) float64 {
	t.Helper()
	v, ok := m[key]
	if !ok {
		t.Fatalf("key %q not found in JSON: %v", key, m)
	}
	f, ok := v.(float64)
	if !ok {
		t.Fatalf("key %q is not a number: %T", key, v)
	}
	return f
}

// errorCode extracts the error code from a koto JSON error response.
// koto errors are printed to stdout as {"error": {"code": "...", ...}}.
func errorCode(t *testing.T, stdout string) string {
	t.Helper()
	m := parseJSON(t, stdout)
	errObj, ok := m["error"]
	if !ok {
		t.Fatalf("no 'error' key in JSON output: %s", stdout)
	}
	errMap, ok := errObj.(map[string]interface{})
	if !ok {
		t.Fatalf("'error' is not an object: %T", errObj)
	}
	code, ok := errMap["code"].(string)
	if !ok {
		t.Fatalf("'code' is not a string in error: %v", errMap)
	}
	return code
}

// initWorkflow runs koto init and returns the state file path.
func initWorkflow(t *testing.T, tmplPath, stateDir, name string, vars ...string) string {
	t.Helper()
	args := []string{"init", "--name", name, "--template", tmplPath, "--state-dir", stateDir}
	for _, v := range vars {
		args = append(args, "--var", v)
	}
	stdout := mustRunKoto(t, args...)
	m := parseJSON(t, stdout)
	return jsonStr(t, m, "path")
}

// TestIntegration_FullLifecycle tests init -> next -> transition through
// all states -> next at terminal (done).
func TestIntegration_FullLifecycle(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "lifecycle", "TASK=build feature")

	// next at initial state: should return execute directive.
	stdout := mustRunKoto(t, "next", "--state", statePath)
	nextResult := parseJSON(t, stdout)
	if action := jsonStr(t, nextResult, "action"); action != "execute" {
		t.Errorf("next action = %q, want %q", action, "execute")
	}
	if state := jsonStr(t, nextResult, "state"); state != "assess" {
		t.Errorf("next state = %q, want %q", state, "assess")
	}

	// transition assess -> plan
	stdout = mustRunKoto(t, "transition", "plan", "--state", statePath)
	transResult := parseJSON(t, stdout)
	if state := jsonStr(t, transResult, "state"); state != "plan" {
		t.Errorf("transition state = %q, want %q", state, "plan")
	}
	if v := jsonFloat(t, transResult, "version"); v != 2 {
		t.Errorf("version = %v, want 2", v)
	}

	// transition plan -> implement
	stdout = mustRunKoto(t, "transition", "implement", "--state", statePath)
	transResult = parseJSON(t, stdout)
	if state := jsonStr(t, transResult, "state"); state != "implement" {
		t.Errorf("transition state = %q, want %q", state, "implement")
	}
	if v := jsonFloat(t, transResult, "version"); v != 3 {
		t.Errorf("version = %v, want 3", v)
	}

	// transition implement -> done (terminal)
	stdout = mustRunKoto(t, "transition", "done", "--state", statePath)
	transResult = parseJSON(t, stdout)
	if state := jsonStr(t, transResult, "state"); state != "done" {
		t.Errorf("transition state = %q, want %q", state, "done")
	}
	if v := jsonFloat(t, transResult, "version"); v != 4 {
		t.Errorf("version = %v, want 4", v)
	}

	// next at terminal: should return done directive.
	stdout = mustRunKoto(t, "next", "--state", statePath)
	nextResult = parseJSON(t, stdout)
	if action := jsonStr(t, nextResult, "action"); action != "done" {
		t.Errorf("terminal next action = %q, want %q", action, "done")
	}

	// query and verify final state.
	stdout = mustRunKoto(t, "query", "--state", statePath)
	queryResult := parseJSON(t, stdout)
	if state := jsonStr(t, queryResult, "current_state"); state != "done" {
		t.Errorf("query current_state = %q, want %q", state, "done")
	}
	if v := jsonFloat(t, queryResult, "version"); v != 4 {
		t.Errorf("query version = %v, want 4", v)
	}
	history, ok := queryResult["history"].([]interface{})
	if !ok {
		t.Fatalf("history is not an array: %T", queryResult["history"])
	}
	if len(history) != 3 {
		t.Errorf("history len = %d, want 3", len(history))
	}
}

// TestIntegration_RewindAndReadvance tests init -> advance -> rewind ->
// advance again, verifying the history includes the rewind entry.
func TestIntegration_RewindAndReadvance(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "rewind-test")

	// advance: assess -> plan -> implement
	mustRunKoto(t, "transition", "plan", "--state", statePath)
	mustRunKoto(t, "transition", "implement", "--state", statePath)

	// rewind to plan
	stdout := mustRunKoto(t, "rewind", "--to", "plan", "--state", statePath)
	rewindResult := parseJSON(t, stdout)
	if state := jsonStr(t, rewindResult, "state"); state != "plan" {
		t.Errorf("rewind state = %q, want %q", state, "plan")
	}

	// verify history has the rewind entry
	stdout = mustRunKoto(t, "query", "--state", statePath)
	queryResult := parseJSON(t, stdout)
	history, ok := queryResult["history"].([]interface{})
	if !ok {
		t.Fatalf("history is not an array: %T", queryResult["history"])
	}
	lastEntry, ok := history[len(history)-1].(map[string]interface{})
	if !ok {
		t.Fatalf("last history entry is not an object: %T", history[len(history)-1])
	}
	typ, ok := lastEntry["type"].(string)
	if !ok {
		t.Fatalf("history entry 'type' is not a string: %T", lastEntry["type"])
	}
	if typ != "rewind" {
		t.Errorf("last history type = %q, want %q", typ, "rewind")
	}
	from, ok := lastEntry["from"].(string)
	if !ok {
		t.Fatalf("history entry 'from' is not a string: %T", lastEntry["from"])
	}
	if from != "implement" {
		t.Errorf("rewind from = %q, want %q", from, "implement")
	}
	to, ok := lastEntry["to"].(string)
	if !ok {
		t.Fatalf("history entry 'to' is not a string: %T", lastEntry["to"])
	}
	if to != "plan" {
		t.Errorf("rewind to = %q, want %q", to, "plan")
	}

	// re-advance: plan -> implement -> done
	mustRunKoto(t, "transition", "implement", "--state", statePath)
	mustRunKoto(t, "transition", "done", "--state", statePath)

	stdout = mustRunKoto(t, "query", "--state", statePath)
	queryResult = parseJSON(t, stdout)
	if state := jsonStr(t, queryResult, "current_state"); state != "done" {
		t.Errorf("final state = %q, want %q", state, "done")
	}
	// history: assess->plan, plan->implement, implement->plan (rewind),
	// plan->implement, implement->done = 5 entries
	history, ok = queryResult["history"].([]interface{})
	if !ok {
		t.Fatalf("history is not an array: %T", queryResult["history"])
	}
	if len(history) != 5 {
		t.Errorf("history len = %d, want 5", len(history))
	}
}

// TestIntegration_Cancel tests init -> cancel -> verify state file deleted.
func TestIntegration_Cancel(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "cancel-test")

	// State file should exist after init.
	if _, err := os.Stat(statePath); err != nil {
		t.Fatalf("state file should exist after init: %v", err)
	}

	// Cancel.
	mustRunKoto(t, "cancel", "--state", statePath)

	// State file should be deleted.
	if _, err := os.Stat(statePath); !os.IsNotExist(err) {
		t.Errorf("state file should be deleted after cancel, stat error: %v", err)
	}

	// Template and other files should be unaffected.
	if _, err := os.Stat(tmplPath); err != nil {
		t.Errorf("template file should still exist after cancel: %v", err)
	}

	// Operations on the deleted state file should fail.
	_, _, err := runKoto(t, "next", "--state", statePath)
	if err == nil {
		t.Error("expected error when operating on deleted state file")
	}
}

// TestIntegration_MultiWorkflow tests init two workflows -> operations
// without --state fail -> with --state succeed -> workflows lists both.
func TestIntegration_MultiWorkflow(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	// Init two workflows.
	statePathA := initWorkflow(t, tmplPath, stateDir, "workflow-a")
	statePathB := initWorkflow(t, tmplPath, stateDir, "workflow-b")

	// Operations without --state should fail with multiple state files.
	stdout, _, err := runKoto(t, "next", "--state-dir", stateDir)
	if err == nil {
		t.Fatal("expected error when multiple state files and no --state")
	}
	// Verify the error output is valid JSON and mentions the problem.
	m := parseJSON(t, stdout)
	errObj, ok := m["error"].(map[string]interface{})
	if !ok {
		t.Fatalf("'error' is not an object: %T", m["error"])
	}
	msg, ok := errObj["message"].(string)
	if !ok {
		t.Fatalf("'message' is not a string: %T", errObj["message"])
	}
	if msg == "" {
		t.Error("error message should not be empty")
	}

	// Operations with --state should succeed on workflow-a.
	stdout = mustRunKoto(t, "next", "--state", statePathA)
	nextResult := parseJSON(t, stdout)
	if state := jsonStr(t, nextResult, "state"); state != "assess" {
		t.Errorf("workflow-a state = %q, want %q", state, "assess")
	}

	// Advance workflow-a only.
	mustRunKoto(t, "transition", "plan", "--state", statePathA)

	// Verify workflow-b is unchanged.
	stdout = mustRunKoto(t, "query", "--state", statePathB)
	queryB := parseJSON(t, stdout)
	if state := jsonStr(t, queryB, "current_state"); state != "assess" {
		t.Errorf("workflow-b should be unchanged at %q, got %q", "assess", state)
	}

	// workflows should list both.
	stdout = mustRunKoto(t, "workflows", "--state-dir", stateDir)
	var workflows []interface{}
	if err := json.Unmarshal([]byte(stdout), &workflows); err != nil {
		t.Fatalf("parse workflows JSON: %v\ninput: %s", err, stdout)
	}
	if len(workflows) != 2 {
		t.Errorf("workflows count = %d, want 2", len(workflows))
	}
}

// TestIntegration_InvalidTransition tests attempting an invalid transition
// and verifying the error JSON with code "invalid_transition".
func TestIntegration_InvalidTransition(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "invalid-trans")

	// From assess, only "plan" is valid. Try "done" which is not allowed.
	stdout, _, err := runKoto(t, "transition", "done", "--state", statePath)
	if err == nil {
		t.Fatal("expected error for invalid transition")
	}

	code := errorCode(t, stdout)
	if code != "invalid_transition" {
		t.Errorf("error code = %q, want %q", code, "invalid_transition")
	}

	// Verify the error includes valid_transitions.
	m := parseJSON(t, stdout)
	errObj := m["error"].(map[string]interface{})
	validTrans, ok := errObj["valid_transitions"].([]interface{})
	if !ok {
		t.Fatalf("valid_transitions not in error response: %v", errObj)
	}
	if len(validTrans) == 0 {
		t.Error("valid_transitions should not be empty")
	}
}

// TestIntegration_TerminalStateError tests reaching a terminal state and
// attempting a transition, verifying error JSON with code "terminal_state".
func TestIntegration_TerminalStateError(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "terminal-test")

	// Advance to terminal state: assess -> plan -> implement -> done.
	mustRunKoto(t, "transition", "plan", "--state", statePath)
	mustRunKoto(t, "transition", "implement", "--state", statePath)
	mustRunKoto(t, "transition", "done", "--state", statePath)

	// Attempt transition from terminal state.
	stdout, _, err := runKoto(t, "transition", "assess", "--state", statePath)
	if err == nil {
		t.Fatal("expected error for transition from terminal state")
	}

	code := errorCode(t, stdout)
	if code != "terminal_state" {
		t.Errorf("error code = %q, want %q", code, "terminal_state")
	}
}

// TestIntegration_RewindUnvisited tests attempting to rewind to an unvisited
// state, verifying error JSON with code "rewind_failed".
func TestIntegration_RewindUnvisited(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "rewind-unvisited")

	// Advance: assess -> plan. Now "implement" has never been visited.
	mustRunKoto(t, "transition", "plan", "--state", statePath)

	// Attempt to rewind to "implement" (never visited).
	stdout, _, err := runKoto(t, "rewind", "--to", "implement", "--state", statePath)
	if err == nil {
		t.Fatal("expected error for rewind to unvisited state")
	}

	code := errorCode(t, stdout)
	if code != "rewind_failed" {
		t.Errorf("error code = %q, want %q", code, "rewind_failed")
	}
}

// TestIntegration_TemplateMismatch tests modifying the template file after
// init, then verifying that next and transition fail with template_mismatch.
func TestIntegration_TemplateMismatch(t *testing.T) {
	if testing.Short() {
		t.Skip("short mode: skipping integration test")
	}

	dir := t.TempDir()
	tmplPath := writeTemplate(t, dir, "lifecycle.md", integrationTemplate)
	stateDir := filepath.Join(dir, "state")

	statePath := initWorkflow(t, tmplPath, stateDir, "mismatch-test")

	// Modify the template directive content to change the compiled hash.
	// A trailing newline wouldn't change the compiled output, so we
	// rewrite the directive text which changes the compiled JSON.
	modifiedTemplate := `---
name: integration
version: "1.0"
description: Integration test workflow
initial_state: assess
variables:
  TASK:
    default: default-task
states:
  assess:
    transitions: [plan]
  plan:
    transitions: [implement]
  implement:
    transitions: [done]
  done:
    terminal: true
---

## assess

MODIFIED directive content.

## plan

Create a plan for {{TASK}}.

## implement

Build the solution.

## done

Work is complete.
`
	if err := os.WriteFile(tmplPath, []byte(modifiedTemplate), 0o600); err != nil {
		t.Fatalf("rewrite template: %v", err)
	}

	// next should fail with template_mismatch.
	stdout, _, err := runKoto(t, "next", "--state", statePath)
	if err == nil {
		t.Fatal("expected error for template mismatch on next")
	}
	code := errorCode(t, stdout)
	if code != "template_mismatch" {
		t.Errorf("next error code = %q, want %q", code, "template_mismatch")
	}

	// transition should also fail with template_mismatch.
	stdout, _, err = runKoto(t, "transition", "plan", "--state", statePath)
	if err == nil {
		t.Fatal("expected error for template mismatch on transition")
	}
	code = errorCode(t, stdout)
	if code != "template_mismatch" {
		t.Errorf("transition error code = %q, want %q", code, "template_mismatch")
	}
}
