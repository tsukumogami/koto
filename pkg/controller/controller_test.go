package controller

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsukumogami/koto/pkg/engine"
	"github.com/tsukumogami/koto/pkg/template"
)

func testMachine() *engine.Machine {
	return &engine.Machine{
		Name:         "test-machine",
		InitialState: "start",
		States: map[string]*engine.MachineState{
			"start": {
				Transitions: []string{"done"},
			},
			"done": {
				Terminal: true,
			},
		},
	}
}

// writeTestTemplate writes a template file and returns its path.
func writeTestTemplate(t *testing.T, dir, content string) string {
	t.Helper()
	path := filepath.Join(dir, "template.md")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}
	return path
}

const testTemplateContent = `---
name: test-machine
---

## start

Do the {{TASK}} work now.

**Transitions**: [done]

## done

Work is complete.
`

func TestNext_NonTerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	ctrl, err := New(eng, nil)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	d, err := ctrl.Next()
	if err != nil {
		t.Fatalf("Next() error: %v", err)
	}

	if d.Action != "execute" {
		t.Errorf("Action = %q, want %q", d.Action, "execute")
	}
	if d.State != "start" {
		t.Errorf("State = %q, want %q", d.State, "start")
	}
	if d.Directive == "" {
		t.Error("Directive is empty, want non-empty stub directive")
	}
	if d.Message != "" {
		t.Errorf("Message = %q, want empty for execute action", d.Message)
	}
}

func TestNext_TerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance to terminal state
	if err := eng.Transition("done"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	ctrl, err := New(eng, nil)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	d, err := ctrl.Next()
	if err != nil {
		t.Fatalf("Next() error: %v", err)
	}

	if d.Action != "done" {
		t.Errorf("Action = %q, want %q", d.Action, "done")
	}
	if d.State != "done" {
		t.Errorf("State = %q, want %q", d.State, "done")
	}
	if d.Message == "" {
		t.Error("Message is empty, want non-empty completion message")
	}
	if d.Directive != "" {
		t.Errorf("Directive = %q, want empty for done action", d.Directive)
	}
}

func TestNew_TemplateHashMatch(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeTestTemplate(t, dir, testTemplateContent)

	tmpl, err := template.Parse(tmplPath)
	if err != nil {
		t.Fatalf("template.Parse() error: %v", err)
	}

	statePath := filepath.Join(dir, "koto-test.state.json")
	eng, err := engine.Init(statePath, tmpl.Machine, engine.InitMeta{
		Name:         "test",
		TemplateHash: tmpl.Hash,
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Same hash should succeed.
	ctrl, err := New(eng, tmpl)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	if ctrl == nil {
		t.Fatal("New() returned nil controller")
	}
}

func TestNew_TemplateHashMismatch(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeTestTemplate(t, dir, testTemplateContent)

	tmpl, err := template.Parse(tmplPath)
	if err != nil {
		t.Fatalf("template.Parse() error: %v", err)
	}

	statePath := filepath.Join(dir, "koto-test.state.json")
	eng, err := engine.Init(statePath, tmpl.Machine, engine.InitMeta{
		Name:         "test",
		TemplateHash: "sha256:different",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Hash mismatch should fail.
	_, err = New(eng, tmpl)
	if err == nil {
		t.Fatal("New() expected error for template hash mismatch")
	}

	te, ok := err.(*engine.TransitionError)
	if !ok {
		t.Fatalf("expected *engine.TransitionError, got %T", err)
	}
	if te.Code != engine.ErrTemplateMismatch {
		t.Errorf("error code = %q, want %q", te.Code, engine.ErrTemplateMismatch)
	}
}

func TestNew_NilTemplateSkipsVerification(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{
		Name:         "test",
		TemplateHash: "sha256:abc123",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Nil template should skip verification.
	ctrl, err := New(eng, nil)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	if ctrl == nil {
		t.Fatal("New() returned nil controller")
	}
}

func TestNext_WithTemplate_InterpolatesVariables(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeTestTemplate(t, dir, testTemplateContent)

	tmpl, err := template.Parse(tmplPath)
	if err != nil {
		t.Fatalf("template.Parse() error: %v", err)
	}

	statePath := filepath.Join(dir, "koto-test.state.json")
	eng, err := engine.Init(statePath, tmpl.Machine, engine.InitMeta{
		Name:         "test",
		TemplateHash: tmpl.Hash,
		Variables:    map[string]string{"TASK": "important"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	ctrl, err := New(eng, tmpl)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}

	d, err := ctrl.Next()
	if err != nil {
		t.Fatalf("Next() error: %v", err)
	}

	if d.Action != "execute" {
		t.Errorf("Action = %q, want %q", d.Action, "execute")
	}

	// The directive should contain the interpolated variable.
	if !strings.Contains(d.Directive, "important") {
		t.Errorf("Directive = %q, want it to contain %q (interpolated variable)", d.Directive, "important")
	}

	// The directive should not contain the raw placeholder.
	if strings.Contains(d.Directive, "{{TASK}}") {
		t.Errorf("Directive = %q, should not contain unresolved placeholder {{TASK}}", d.Directive)
	}
}

const testTemplateWithEvidence = `---
name: test-machine
---

## start

Do the {{TASK}} work. Result: {{RESULT}}

**Transitions**: [done]

## done

Work is complete.
`

func TestNext_WithTemplate_MergesEvidenceIntoContext(t *testing.T) {
	dir := t.TempDir()
	tmplPath := writeTestTemplate(t, dir, testTemplateWithEvidence)

	tmpl, err := template.Parse(tmplPath)
	if err != nil {
		t.Fatalf("template.Parse() error: %v", err)
	}

	statePath := filepath.Join(dir, "koto-test.state.json")
	eng, err := engine.Init(statePath, tmpl.Machine, engine.InitMeta{
		Name:         "test",
		TemplateHash: tmpl.Hash,
		Variables:    map[string]string{"TASK": "important"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// The template has {{RESULT}} placeholder but there's no variable
	// for it. Without evidence, it remains unresolved.
	ctrl, err := New(eng, tmpl)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}

	d, err := ctrl.Next()
	if err != nil {
		t.Fatalf("Next() error: %v", err)
	}

	if !strings.Contains(d.Directive, "{{RESULT}}") {
		t.Errorf("Directive = %q, want it to contain unresolved {{RESULT}}", d.Directive)
	}

	// Now rewind and add evidence via a self-transition trick:
	// We need to use a machine that allows staying in start. Instead,
	// let's create a fresh engine that has evidence pre-loaded.
	// Actually, we can write a v2 state file with evidence directly.
	stateData := `{
		"schema_version": 2,
		"workflow": {"name": "test", "template_hash": "` + tmpl.Hash + `", "template_path": "", "created_at": "2024-01-01T00:00:00Z"},
		"version": 1,
		"current_state": "start",
		"variables": {"TASK": "important"},
		"evidence": {"RESULT": "pass"},
		"history": []
	}`
	statePath2 := filepath.Join(dir, "koto-test2.state.json")
	if err := os.WriteFile(statePath2, []byte(stateData), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	eng2, err := engine.Load(statePath2, tmpl.Machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	ctrl2, err := New(eng2, tmpl)
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}

	d2, err := ctrl2.Next()
	if err != nil {
		t.Fatalf("Next() error: %v", err)
	}

	// The evidence value should be interpolated.
	if !strings.Contains(d2.Directive, "pass") {
		t.Errorf("Directive = %q, want it to contain %q (evidence value)", d2.Directive, "pass")
	}
	if strings.Contains(d2.Directive, "{{RESULT}}") {
		t.Errorf("Directive = %q, should not contain unresolved {{RESULT}}", d2.Directive)
	}

	// Variables should also still be interpolated.
	if !strings.Contains(d2.Directive, "important") {
		t.Errorf("Directive = %q, want it to contain %q (variable value)", d2.Directive, "important")
	}
}
