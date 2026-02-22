package engine

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// testMachine returns a simple three-state machine for testing:
// start -> middle -> done (terminal)
func testMachine() *Machine {
	return &Machine{
		Name:         "test-machine",
		InitialState: "start",
		States: map[string]*MachineState{
			"start": {
				Transitions: []string{"middle"},
			},
			"middle": {
				Transitions: []string{"done"},
			},
			"done": {
				Terminal: true,
			},
		},
	}
}

func TestInit_CreatesValidStateFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{
		Name:         "test-workflow",
		TemplateHash: "sha256:abc123",
		TemplatePath: "/tmp/template.md",
		Variables:    map[string]string{"TASK": "unit test"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Verify the engine state
	if got := eng.CurrentState(); got != "start" {
		t.Errorf("CurrentState() = %q, want %q", got, "start")
	}
	if got := eng.Path(); got != path {
		t.Errorf("Path() = %q, want %q", got, path)
	}

	// Verify the file on disk
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}

	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	if state.SchemaVersion != 1 {
		t.Errorf("schema_version = %d, want 1", state.SchemaVersion)
	}
	if state.Version != 1 {
		t.Errorf("version = %d, want 1", state.Version)
	}
	if state.CurrentState != "start" {
		t.Errorf("current_state = %q, want %q", state.CurrentState, "start")
	}
	if len(state.History) != 0 {
		t.Errorf("history length = %d, want 0", len(state.History))
	}
	if state.Workflow.Name != "test-workflow" {
		t.Errorf("workflow.name = %q, want %q", state.Workflow.Name, "test-workflow")
	}
	if state.Workflow.TemplateHash != "sha256:abc123" {
		t.Errorf("workflow.template_hash = %q, want %q", state.Workflow.TemplateHash, "sha256:abc123")
	}
	if state.Variables["TASK"] != "unit test" {
		t.Errorf("variables[TASK] = %q, want %q", state.Variables["TASK"], "unit test")
	}
}

func TestInit_InvalidInitialState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := &Machine{
		Name:         "bad",
		InitialState: "nonexistent",
		States:       map[string]*MachineState{},
	}

	_, err := Init(path, machine, InitMeta{Name: "test"})
	if err == nil {
		t.Fatal("Init() expected error for invalid initial state")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != "unknown_state" {
		t.Errorf("error code = %q, want %q", te.Code, "unknown_state")
	}
}

func TestLoad_RoundTrips(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	orig, err := Init(path, machine, InitMeta{
		Name:         "roundtrip",
		TemplateHash: "sha256:def456",
		TemplatePath: "/tmp/template.md",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	loaded, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	if loaded.CurrentState() != orig.CurrentState() {
		t.Errorf("loaded state = %q, want %q", loaded.CurrentState(), orig.CurrentState())
	}
	if loaded.Path() != orig.Path() {
		t.Errorf("loaded path = %q, want %q", loaded.Path(), orig.Path())
	}

	origSnap := orig.Snapshot()
	loadedSnap := loaded.Snapshot()
	if origSnap.Version != loadedSnap.Version {
		t.Errorf("loaded version = %d, want %d", loadedSnap.Version, origSnap.Version)
	}
}

func TestLoad_InvalidCurrentState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	// Write a state file with a state not in the machine
	state := State{
		SchemaVersion: 1,
		Version:       1,
		CurrentState:  "nonexistent",
		Variables:     map[string]string{},
		History:       []HistoryEntry{},
	}
	data, _ := json.Marshal(state)
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	_, err := Load(path, testMachine())
	if err == nil {
		t.Fatal("Load() expected error for unknown current state")
	}
	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != "unknown_state" {
		t.Errorf("error code = %q, want %q", te.Code, "unknown_state")
	}
}

func TestTransition_FromInitialState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	if got := eng.CurrentState(); got != "middle" {
		t.Errorf("CurrentState() = %q, want %q", got, "middle")
	}

	snap := eng.Snapshot()
	if snap.Version != 2 {
		t.Errorf("version = %d, want 2", snap.Version)
	}
	if len(snap.History) != 1 {
		t.Fatalf("history length = %d, want 1", len(snap.History))
	}
	if snap.History[0].From != "start" {
		t.Errorf("history[0].from = %q, want %q", snap.History[0].From, "start")
	}
	if snap.History[0].To != "middle" {
		t.Errorf("history[0].to = %q, want %q", snap.History[0].To, "middle")
	}
	if snap.History[0].Type != "transition" {
		t.Errorf("history[0].type = %q, want %q", snap.History[0].Type, "transition")
	}
}

func TestTransition_FromTerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance to terminal state
	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition(middle) error: %v", err)
	}
	if err := eng.Transition("done"); err != nil {
		t.Fatalf("Transition(done) error: %v", err)
	}

	// Try to transition from terminal state
	err = eng.Transition("start")
	if err == nil {
		t.Fatal("Transition() expected error from terminal state")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != "terminal_state" {
		t.Errorf("error code = %q, want %q", te.Code, "terminal_state")
	}
	if te.CurrentState != "done" {
		t.Errorf("current_state = %q, want %q", te.CurrentState, "done")
	}
}

func TestTransition_InvalidTarget(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	err = eng.Transition("done")
	if err == nil {
		t.Fatal("Transition() expected error for invalid target")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != "invalid_transition" {
		t.Errorf("error code = %q, want %q", te.Code, "invalid_transition")
	}
	if te.CurrentState != "start" {
		t.Errorf("current_state = %q, want %q", te.CurrentState, "start")
	}
	if te.TargetState != "done" {
		t.Errorf("target_state = %q, want %q", te.TargetState, "done")
	}
	if len(te.ValidTransitions) != 1 || te.ValidTransitions[0] != "middle" {
		t.Errorf("valid_transitions = %v, want [middle]", te.ValidTransitions)
	}
}

func TestTransitionError_JSON(t *testing.T) {
	te := &TransitionError{
		Code:             "invalid_transition",
		Message:          "cannot transition",
		CurrentState:     "start",
		TargetState:      "done",
		ValidTransitions: []string{"middle"},
	}

	data, err := json.Marshal(te)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}

	var m map[string]interface{}
	if err := json.Unmarshal(data, &m); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	if m["code"] != "invalid_transition" {
		t.Errorf("code = %v, want %q", m["code"], "invalid_transition")
	}
	if m["message"] != "cannot transition" {
		t.Errorf("message = %v, want %q", m["message"], "cannot transition")
	}
}

func TestAtomicWrite_NoTempFileOnFailure(t *testing.T) {
	dir := t.TempDir()

	// Write to a directory that doesn't exist within the temp dir
	badPath := filepath.Join(dir, "nonexistent", "state.json")

	err := atomicWrite(badPath, []byte("data"))
	if err == nil {
		t.Fatal("atomicWrite() expected error for nonexistent directory")
	}

	// Verify no temp files remain in the parent (dir)
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("ReadDir() error: %v", err)
	}
	for _, entry := range entries {
		if filepath.Ext(entry.Name()) == ".tmp" {
			t.Errorf("temp file left behind: %s", entry.Name())
		}
	}
}

func TestAtomicWrite_SymlinkCheck(t *testing.T) {
	dir := t.TempDir()

	// Create a real file and a symlink to it
	realPath := filepath.Join(dir, "real.json")
	if err := os.WriteFile(realPath, []byte("original"), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}
	linkPath := filepath.Join(dir, "link.json")
	if err := os.Symlink(realPath, linkPath); err != nil {
		t.Fatalf("Symlink() error: %v", err)
	}

	// Attempt to write to the symlink path
	err := atomicWrite(linkPath, []byte("replacement"))
	if err == nil {
		t.Fatal("atomicWrite() expected error for symlink target")
	}

	// Verify original file is untouched
	data, err := os.ReadFile(realPath) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}
	if string(data) != "original" {
		t.Errorf("original file modified: got %q", string(data))
	}
}

func TestVariables_ReturnsCopy(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{
		Name:      "test",
		Variables: map[string]string{"KEY": "value"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	vars := eng.Variables()
	vars["KEY"] = "modified"

	// Engine's internal state should not be affected
	if got := eng.Variables()["KEY"]; got != "value" {
		t.Errorf("Variables()[KEY] = %q, want %q (copy was not independent)", got, "value")
	}
}

func TestHistory_ReturnsCopy(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	hist := eng.History()
	hist[0].From = "tampered"

	// Engine's internal state should not be affected
	if got := eng.History()[0].From; got != "start" {
		t.Errorf("History()[0].From = %q, want %q (copy was not independent)", got, "start")
	}
}

func TestSnapshot_ReturnsFullState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{
		Name:         "snapshot-test",
		TemplateHash: "sha256:xyz",
		TemplatePath: "/path/to/template",
		Variables:    map[string]string{"A": "1"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	snap := eng.Snapshot()
	if snap.SchemaVersion != 1 {
		t.Errorf("SchemaVersion = %d, want 1", snap.SchemaVersion)
	}
	if snap.Workflow.Name != "snapshot-test" {
		t.Errorf("Workflow.Name = %q, want %q", snap.Workflow.Name, "snapshot-test")
	}
	if snap.CurrentState != "start" {
		t.Errorf("CurrentState = %q, want %q", snap.CurrentState, "start")
	}
}

func TestInit_EmptyVariables(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	vars := eng.Variables()
	if vars == nil {
		t.Error("Variables() returned nil, want empty map")
	}
	if len(vars) != 0 {
		t.Errorf("Variables() length = %d, want 0", len(vars))
	}
}

func TestMachine_ReturnsCopy(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	m := eng.Machine()

	// Mutate the returned machine's state map
	m.States["done"].Terminal = false

	// Engine's internal machine should not be affected
	internal := eng.Machine()
	if !internal.States["done"].Terminal {
		t.Error("Machine().States[done].Terminal = false, want true (copy was not independent)")
	}

	// Mutate the returned machine's transitions slice
	m.States["start"].Transitions[0] = "tampered"
	internal2 := eng.Machine()
	if internal2.States["start"].Transitions[0] != "middle" {
		t.Errorf("Machine().States[start].Transitions[0] = %q, want %q (copy was not independent)",
			internal2.States["start"].Transitions[0], "middle")
	}

	// Add a new state to the returned machine
	m.States["injected"] = &MachineState{Terminal: true}
	internal3 := eng.Machine()
	if _, exists := internal3.States["injected"]; exists {
		t.Error("Machine().States contains injected state, copy was not independent")
	}
}

func TestTransition_PersistsToFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Reload and verify persistence
	loaded, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	if got := loaded.CurrentState(); got != "middle" {
		t.Errorf("loaded CurrentState() = %q, want %q", got, "middle")
	}
	if snap := loaded.Snapshot(); snap.Version != 2 {
		t.Errorf("loaded Version = %d, want 2", snap.Version)
	}
}
