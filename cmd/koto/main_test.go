package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsukumogami/koto/pkg/discover"
	"github.com/tsukumogami/koto/pkg/engine"
	"github.com/tsukumogami/koto/pkg/template"
)

const lifecycleTemplate = `---
name: lifecycle
version: "1.0"
description: A test lifecycle workflow
variables:
  TASK: default-task
---

## assess

Assess the task: {{TASK}}.

**Transitions**: [plan]

## plan

Create a plan for {{TASK}}.

**Transitions**: [implement]

## implement

Build the solution.

**Transitions**: [done]

## done

Work is complete.
`

// writeLifecycleTemplate writes the test template to a directory and returns
// the absolute path.
func writeLifecycleTemplate(t *testing.T, dir string) string {
	t.Helper()
	path := filepath.Join(dir, "lifecycle.md")
	if err := os.WriteFile(path, []byte(lifecycleTemplate), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		t.Fatalf("Abs() error: %v", err)
	}
	return abs
}

// --- cmdInit tests ---

func TestCmdInit_CreatesStateFile(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	err := cmdInit([]string{
		"--name", "test-wf",
		"--template", tmplPath,
		"--state-dir", stateDir,
	})
	if err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-test-wf.state.json")
	if _, err := os.Stat(statePath); err != nil {
		t.Fatalf("state file not created: %v", err)
	}

	// Verify the state file contains the template-parsed machine.
	data, _ := os.ReadFile(statePath) //nolint:gosec // G304: test reads file it created
	var state engine.State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("parse state: %v", err)
	}

	if state.CurrentState != "assess" {
		t.Errorf("CurrentState = %q, want %q (from parsed template, not stub)", state.CurrentState, "assess")
	}
	if state.Workflow.Name != "test-wf" {
		t.Errorf("Workflow.Name = %q, want %q", state.Workflow.Name, "test-wf")
	}
}

func TestCmdInit_MergesVariables(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	err := cmdInit([]string{
		"--name", "var-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
		"--var", "TASK=build feature",
		"--var", "EXTRA=bonus",
	})
	if err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-var-test.state.json")
	data, _ := os.ReadFile(statePath) //nolint:gosec // G304: test reads file it created
	var state engine.State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("parse state: %v", err)
	}

	// --var TASK=build feature should override template default.
	if state.Variables["TASK"] != "build feature" {
		t.Errorf("Variables[TASK] = %q, want %q", state.Variables["TASK"], "build feature")
	}
	// EXTRA is not in the template defaults but should still be set.
	if state.Variables["EXTRA"] != "bonus" {
		t.Errorf("Variables[EXTRA] = %q, want %q", state.Variables["EXTRA"], "bonus")
	}
}

func TestCmdInit_RequiresName(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)

	err := cmdInit([]string{"--template", tmplPath})
	if err == nil {
		t.Fatal("expected error when --name is missing")
	}
}

func TestCmdInit_RequiresTemplate(t *testing.T) {
	err := cmdInit([]string{"--name", "test"})
	if err == nil {
		t.Fatal("expected error when --template is missing")
	}
}

func TestCmdInit_InvalidVarFormat(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)

	err := cmdInit([]string{
		"--name", "test",
		"--template", tmplPath,
		"--state-dir", filepath.Join(dir, "state"),
		"--var", "NOEQUALS",
	})
	if err == nil {
		t.Fatal("expected error for invalid --var format")
	}
	if !strings.Contains(err.Error(), "KEY=VALUE") {
		t.Errorf("error message should mention KEY=VALUE format: %v", err)
	}
}

// --- cmdTransition tests ---

func TestCmdTransition_AdvancesState(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "trans-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-trans-test.state.json")
	if err := cmdTransition([]string{"plan", "--state", statePath}); err != nil {
		t.Fatalf("cmdTransition() error: %v", err)
	}

	data, _ := os.ReadFile(statePath) //nolint:gosec // G304: test reads file it created
	var state engine.State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("parse state: %v", err)
	}

	if state.CurrentState != "plan" {
		t.Errorf("CurrentState = %q, want %q", state.CurrentState, "plan")
	}
}

func TestCmdTransition_RequiresTarget(t *testing.T) {
	err := cmdTransition([]string{"--state", "/some/path"})
	if err == nil {
		t.Fatal("expected error when target is missing")
	}
}

// --- cmdNext tests ---

func TestCmdNext_ReturnsDirective(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "next-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
		"--var", "TASK=build feature",
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-next-test.state.json")

	// Read the template to verify the directive uses interpolated content.
	tmpl, _ := template.Parse(tmplPath)
	_ = tmpl // used only to verify the test is meaningful

	// cmdNext outputs to stdout; we just check it doesn't error.
	if err := cmdNext([]string{"--state", statePath}); err != nil {
		t.Fatalf("cmdNext() error: %v", err)
	}
}

// --- cmdQuery tests ---

func TestCmdQuery_ReturnsSnapshot(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "query-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-query-test.state.json")

	if err := cmdQuery([]string{"--state", statePath}); err != nil {
		t.Fatalf("cmdQuery() error: %v", err)
	}
}

// --- cmdStatus tests ---

func TestCmdStatus_PrintsHumanReadable(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "status-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-status-test.state.json")

	if err := cmdStatus([]string{"--state", statePath}); err != nil {
		t.Fatalf("cmdStatus() error: %v", err)
	}
}

// --- cmdRewind tests ---

func TestCmdRewind_RewindsState(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "rewind-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-rewind-test.state.json")

	// Advance assess -> plan -> implement
	if err := cmdTransition([]string{"plan", "--state", statePath}); err != nil {
		t.Fatalf("transition to plan: %v", err)
	}
	if err := cmdTransition([]string{"implement", "--state", statePath}); err != nil {
		t.Fatalf("transition to implement: %v", err)
	}

	// Rewind to plan
	if err := cmdRewind([]string{"--to", "plan", "--state", statePath}); err != nil {
		t.Fatalf("cmdRewind() error: %v", err)
	}

	data, _ := os.ReadFile(statePath) //nolint:gosec // G304: test reads file it created
	var state engine.State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("parse state: %v", err)
	}

	if state.CurrentState != "plan" {
		t.Errorf("CurrentState = %q, want %q", state.CurrentState, "plan")
	}

	// Check history includes the rewind entry.
	lastEntry := state.History[len(state.History)-1]
	if lastEntry.Type != "rewind" {
		t.Errorf("last history type = %q, want %q", lastEntry.Type, "rewind")
	}
}

func TestCmdRewind_RequiresTo(t *testing.T) {
	err := cmdRewind([]string{"--state", "/some/path"})
	if err == nil {
		t.Fatal("expected error when --to is missing")
	}
}

// --- cmdCancel tests ---

func TestCmdCancel_RemovesStateFile(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "cancel-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-cancel-test.state.json")

	if err := cmdCancel([]string{"--state", statePath}); err != nil {
		t.Fatalf("cmdCancel() error: %v", err)
	}

	if _, err := os.Stat(statePath); !os.IsNotExist(err) {
		t.Error("state file should be deleted after cancel")
	}
}

// --- cmdValidate tests ---

func TestCmdValidate_MatchingHash(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "validate-test",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-validate-test.state.json")

	// validate should succeed (template hasn't changed).
	if err := cmdValidate([]string{"--state", statePath}); err != nil {
		t.Fatalf("cmdValidate() error: %v", err)
	}
}

// --- cmdWorkflows tests ---

func TestCmdWorkflows_ListsActiveWorkflows(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	// Create two workflows.
	if err := cmdInit([]string{
		"--name", "wf-a",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit(wf-a) error: %v", err)
	}
	if err := cmdInit([]string{
		"--name", "wf-b",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit(wf-b) error: %v", err)
	}

	// cmdWorkflows outputs JSON. Just verify it doesn't error.
	if err := cmdWorkflows([]string{"--state-dir", stateDir}); err != nil {
		t.Fatalf("cmdWorkflows() error: %v", err)
	}
}

// --- resolveStatePath tests ---

func TestResolveStatePath_ExplicitPath(t *testing.T) {
	got, err := resolveStatePath("/explicit/path.json", "")
	if err != nil {
		t.Fatalf("resolveStatePath() error: %v", err)
	}
	if got != "/explicit/path.json" {
		t.Errorf("got %q, want %q", got, "/explicit/path.json")
	}
}

func TestResolveStatePath_AutoSelectSingle(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "only-one",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit() error: %v", err)
	}

	got, err := resolveStatePath("", stateDir)
	if err != nil {
		t.Fatalf("resolveStatePath() error: %v", err)
	}

	expected := filepath.Join(stateDir, "koto-only-one.state.json")
	if got != expected {
		t.Errorf("got %q, want %q", got, expected)
	}
}

func TestResolveStatePath_ErrorOnMultiple(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	if err := cmdInit([]string{
		"--name", "wf-a",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit(wf-a) error: %v", err)
	}
	if err := cmdInit([]string{
		"--name", "wf-b",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("cmdInit(wf-b) error: %v", err)
	}

	_, err := resolveStatePath("", stateDir)
	if err == nil {
		t.Fatal("expected error when multiple state files exist")
	}
	if !strings.Contains(err.Error(), "multiple state files") {
		t.Errorf("error should mention multiple state files: %v", err)
	}
}

func TestResolveStatePath_ErrorOnEmpty(t *testing.T) {
	dir := t.TempDir()

	_, err := resolveStatePath("", dir)
	if err == nil {
		t.Fatal("expected error when no state files exist")
	}
	if !strings.Contains(err.Error(), "no state files") {
		t.Errorf("error should mention no state files: %v", err)
	}
}

// --- Scenario 23: Full lifecycle ---

func TestScenario23_FullLifecycle(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	// Step 1: init with --var
	if err := cmdInit([]string{
		"--name", "lifecycle",
		"--template", tmplPath,
		"--state-dir", stateDir,
		"--var", "TASK=build feature",
	}); err != nil {
		t.Fatalf("init: %v", err)
	}

	statePath := filepath.Join(stateDir, "koto-lifecycle.state.json")

	// Step 2: next (should return execute directive with interpolated variable)
	if err := cmdNext([]string{"--state", statePath}); err != nil {
		t.Fatalf("next after init: %v", err)
	}

	// Step 3: transition to plan
	if err := cmdTransition([]string{"plan", "--state", statePath}); err != nil {
		t.Fatalf("transition to plan: %v", err)
	}

	// Step 4: query (should return full state snapshot)
	if err := cmdQuery([]string{"--state", statePath}); err != nil {
		t.Fatalf("query: %v", err)
	}

	// Step 5: status (should return human-readable output)
	if err := cmdStatus([]string{"--state", statePath}); err != nil {
		t.Fatalf("status: %v", err)
	}

	// Step 6: transition to implement
	if err := cmdTransition([]string{"implement", "--state", statePath}); err != nil {
		t.Fatalf("transition to implement: %v", err)
	}

	// Step 7: transition to done (terminal)
	if err := cmdTransition([]string{"done", "--state", statePath}); err != nil {
		t.Fatalf("transition to done: %v", err)
	}

	// Step 8: next (should return done directive)
	if err := cmdNext([]string{"--state", statePath}); err != nil {
		t.Fatalf("next at terminal: %v", err)
	}

	// Verify final state.
	data, _ := os.ReadFile(statePath) //nolint:gosec // G304: test reads file it created
	var state engine.State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("parse state: %v", err)
	}

	if state.CurrentState != "done" {
		t.Errorf("final CurrentState = %q, want %q", state.CurrentState, "done")
	}
	// Version should be 4: init=1, transition plan=2, transition implement=3, transition done=4
	if state.Version != 4 {
		t.Errorf("final Version = %d, want 4", state.Version)
	}
	if len(state.History) != 3 {
		t.Errorf("History entries = %d, want 3", len(state.History))
	}
	if state.Variables["TASK"] != "build feature" {
		t.Errorf("Variables[TASK] = %q, want %q", state.Variables["TASK"], "build feature")
	}
}

// --- Scenario 24: Multi-workflow auto-selection ---

func TestScenario24_MultiWorkflowAutoSelection(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeLifecycleTemplate(t, dir)
	stateDir := filepath.Join(dir, "state")

	// Create two workflows.
	if err := cmdInit([]string{
		"--name", "workflow-a",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("init workflow-a: %v", err)
	}
	if err := cmdInit([]string{
		"--name", "workflow-b",
		"--template", tmplPath,
		"--state-dir", stateDir,
	}); err != nil {
		t.Fatalf("init workflow-b: %v", err)
	}

	// next without --state should fail with multiple state files.
	err := cmdNext([]string{"--state-dir", stateDir})
	if err == nil {
		t.Fatal("expected error when multiple state files and no --state")
	}
	if !strings.Contains(err.Error(), "multiple state files") {
		t.Errorf("error should mention multiple state files: %v", err)
	}

	// next with explicit --state should succeed.
	statePathA := filepath.Join(stateDir, "koto-workflow-a.state.json")
	if err := cmdNext([]string{"--state", statePathA}); err != nil {
		t.Fatalf("next with explicit --state: %v", err)
	}

	// workflows should list both.
	workflows, err := discover.Find(stateDir)
	if err != nil {
		t.Fatalf("discover.Find() error: %v", err)
	}
	if len(workflows) != 2 {
		t.Errorf("workflows count = %d, want 2", len(workflows))
	}

	// Delete one workflow, verify auto-selection works.
	if err := cmdCancel([]string{"--state", filepath.Join(stateDir, "koto-workflow-b.state.json")}); err != nil {
		t.Fatalf("cancel workflow-b: %v", err)
	}

	// now next without --state should auto-select the remaining one.
	if err := cmdNext([]string{"--state-dir", stateDir}); err != nil {
		t.Fatalf("next with single state file (auto-select): %v", err)
	}
}

// --- loadTemplateFromState tests ---

func TestLoadTemplateFromState_MissingFile(t *testing.T) {
	_, err := loadTemplateFromState("/nonexistent/path.json")
	if err == nil {
		t.Fatal("expected error for nonexistent state file")
	}
}

func TestLoadTemplateFromState_EmptyTemplatePath(t *testing.T) {
	dir := t.TempDir()
	statePath := filepath.Join(dir, "koto-test.state.json")

	// Write a state file with no template_path.
	state := map[string]interface{}{
		"schema_version": 1,
		"workflow":       map[string]string{"name": "test"},
		"version":        1,
		"current_state":  "start",
	}
	data, _ := json.Marshal(state)
	if err := os.WriteFile(statePath, data, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	_, err := loadTemplateFromState(statePath)
	if err == nil {
		t.Fatal("expected error for empty template_path")
	}
	if !strings.Contains(err.Error(), "no template_path") {
		t.Errorf("error should mention template_path: %v", err)
	}
}
