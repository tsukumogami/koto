package controller

import (
	"path/filepath"
	"testing"

	"github.com/tsukumogami/koto/pkg/engine"
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

func TestNext_NonTerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	ctrl, err := New(eng, "")
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

	ctrl, err := New(eng, "")
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
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{
		Name:         "test",
		TemplateHash: "sha256:abc123",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Same hash should succeed.
	ctrl, err := New(eng, "sha256:abc123")
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	if ctrl == nil {
		t.Fatal("New() returned nil controller")
	}
}

func TestNew_TemplateHashMismatch(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{
		Name:         "test",
		TemplateHash: "sha256:abc123",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Different hash should fail with template_mismatch.
	_, err = New(eng, "sha256:different")
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

func TestNew_EmptyHashSkipsVerification(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := engine.Init(path, testMachine(), engine.InitMeta{
		Name:         "test",
		TemplateHash: "sha256:abc123",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Empty hash should skip verification.
	ctrl, err := New(eng, "")
	if err != nil {
		t.Fatalf("New() error: %v", err)
	}
	if ctrl == nil {
		t.Fatal("New() returned nil controller")
	}
}
