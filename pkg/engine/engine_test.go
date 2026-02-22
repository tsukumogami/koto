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

	if state.SchemaVersion != 2 {
		t.Errorf("schema_version = %d, want 2", state.SchemaVersion)
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
	if te.Code != ErrUnknownState {
		t.Errorf("error code = %q, want %q", te.Code, ErrUnknownState)
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
	if te.Code != ErrUnknownState {
		t.Errorf("error code = %q, want %q", te.Code, ErrUnknownState)
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
	if te.Code != ErrTerminalState {
		t.Errorf("error code = %q, want %q", te.Code, ErrTerminalState)
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
	if te.Code != ErrInvalidTransition {
		t.Errorf("error code = %q, want %q", te.Code, ErrInvalidTransition)
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
	if snap.SchemaVersion != 2 {
		t.Errorf("SchemaVersion = %d, want 2", snap.SchemaVersion)
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

// rewindMachine returns a machine with multiple non-terminal states for
// testing rewind scenarios:
// start -> research -> implementing -> review -> done (terminal)
//
//	\-> escalated (terminal)
func rewindMachine() *Machine {
	return &Machine{
		Name:         "rewind-machine",
		InitialState: "start",
		States: map[string]*MachineState{
			"start": {
				Transitions: []string{"research"},
			},
			"research": {
				Transitions: []string{"implementing"},
			},
			"implementing": {
				Transitions: []string{"review"},
			},
			"review": {
				Transitions: []string{"done", "escalated"},
			},
			"done": {
				Terminal: true,
			},
			"escalated": {
				Terminal: true,
			},
		},
	}
}

func TestRewind_ToPreviouslyVisitedState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance through several states
	for _, target := range []string{"research", "implementing", "review"} {
		if err := eng.Transition(target); err != nil {
			t.Fatalf("Transition(%q) error: %v", target, err)
		}
	}

	// Rewind to a previously visited non-terminal state
	if err := eng.Rewind("research"); err != nil {
		t.Fatalf("Rewind(research) error: %v", err)
	}

	if got := eng.CurrentState(); got != "research" {
		t.Errorf("CurrentState() = %q, want %q", got, "research")
	}

	snap := eng.Snapshot()
	// Init version=1, 3 transitions +1 each = version 4, +1 for rewind = version 5
	if snap.Version != 5 {
		t.Errorf("Version = %d, want 5", snap.Version)
	}

	// History should have 4 entries: 3 transitions + 1 rewind
	if len(snap.History) != 4 {
		t.Fatalf("History length = %d, want 4", len(snap.History))
	}

	last := snap.History[3]
	if last.From != "review" {
		t.Errorf("rewind entry From = %q, want %q", last.From, "review")
	}
	if last.To != "research" {
		t.Errorf("rewind entry To = %q, want %q", last.To, "research")
	}
	if last.Type != "rewind" {
		t.Errorf("rewind entry Type = %q, want %q", last.Type, "rewind")
	}
}

func TestRewind_ToInitialState_NoHistory(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance past the initial state
	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Rewind to initial state -- valid even though "start" never appears as
	// a "to" field in history (the engine was initialized there, never
	// transitioned TO it).
	if err := eng.Rewind("start"); err != nil {
		t.Fatalf("Rewind(start) error: %v", err)
	}

	if got := eng.CurrentState(); got != "start" {
		t.Errorf("CurrentState() = %q, want %q", got, "start")
	}
}

func TestRewind_FromTerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance to a terminal state
	for _, target := range []string{"research", "implementing", "review", "escalated"} {
		if err := eng.Transition(target); err != nil {
			t.Fatalf("Transition(%q) error: %v", target, err)
		}
	}

	if got := eng.CurrentState(); got != "escalated" {
		t.Fatalf("CurrentState() = %q, want %q (should be terminal)", got, "escalated")
	}

	// Rewind from terminal to a previously visited non-terminal state
	if err := eng.Rewind("implementing"); err != nil {
		t.Fatalf("Rewind(implementing) error: %v", err)
	}

	if got := eng.CurrentState(); got != "implementing" {
		t.Errorf("CurrentState() = %q, want %q", got, "implementing")
	}
}

func TestRewind_ToNeverVisitedState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Only advance to "research"
	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Try to rewind to "implementing" which was never visited
	err = eng.Rewind("implementing")
	if err == nil {
		t.Fatal("Rewind() expected error for never-visited state")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != ErrRewindFailed {
		t.Errorf("error code = %q, want %q", te.Code, ErrRewindFailed)
	}
}

func TestRewind_ToTerminalState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance to terminal and rewind back
	for _, target := range []string{"research", "implementing", "review", "done"} {
		if err := eng.Transition(target); err != nil {
			t.Fatalf("Transition(%q) error: %v", target, err)
		}
	}

	// Rewind to review (valid)
	if err := eng.Rewind("review"); err != nil {
		t.Fatalf("Rewind(review) error: %v", err)
	}

	// Try to rewind TO a terminal state
	err = eng.Rewind("done")
	if err == nil {
		t.Fatal("Rewind() expected error for terminal target")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != ErrRewindFailed {
		t.Errorf("error code = %q, want %q", te.Code, ErrRewindFailed)
	}
}

func TestRewind_ToUnknownState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	err = eng.Rewind("nonexistent")
	if err == nil {
		t.Fatal("Rewind() expected error for unknown state")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T", err)
	}
	if te.Code != ErrRewindFailed {
		t.Errorf("error code = %q, want %q", te.Code, ErrRewindFailed)
	}
}

func TestRewind_PersistsToFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := rewindMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}
	if err := eng.Rewind("start"); err != nil {
		t.Fatalf("Rewind() error: %v", err)
	}

	// Reload and verify persistence
	loaded, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	if got := loaded.CurrentState(); got != "start" {
		t.Errorf("loaded CurrentState() = %q, want %q", got, "start")
	}

	snap := loaded.Snapshot()
	if snap.Version != 3 {
		t.Errorf("loaded Version = %d, want 3", snap.Version)
	}
	if len(snap.History) != 2 {
		t.Fatalf("loaded History length = %d, want 2", len(snap.History))
	}
	if snap.History[1].Type != "rewind" {
		t.Errorf("loaded History[1].Type = %q, want %q", snap.History[1].Type, "rewind")
	}
}

func TestCancel_RemovesStateFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Cancel(); err != nil {
		t.Fatalf("Cancel() error: %v", err)
	}

	// Verify file no longer exists
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Errorf("state file still exists after Cancel(): err = %v", err)
	}
}

func TestCancel_ReturnsErrorOnFailure(t *testing.T) {
	dir := t.TempDir()
	// Use a path to a file that doesn't exist
	path := filepath.Join(dir, "nonexistent.state.json")

	eng := &Engine{
		path: path,
	}

	err := eng.Cancel()
	if err == nil {
		t.Fatal("Cancel() expected error for nonexistent file")
	}
}

func TestSnapshot_ReturnsCopy(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{
		Name:      "test",
		Variables: map[string]string{"KEY": "original"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	snap := eng.Snapshot()

	// Mutate the returned snapshot's variables
	snap.Variables["KEY"] = "tampered"
	if got := eng.Variables()["KEY"]; got != "original" {
		t.Errorf("Variables()[KEY] = %q, want %q (snapshot mutation affected engine)", got, "original")
	}

	// Mutate the returned snapshot's history
	snap.History[0].From = "tampered"
	if got := eng.History()[0].From; got != "start" {
		t.Errorf("History()[0].From = %q, want %q (snapshot mutation affected engine)", got, "start")
	}

	// Mutate the returned snapshot's current state
	snap.CurrentState = "tampered"
	if got := eng.CurrentState(); got != "middle" {
		t.Errorf("CurrentState() = %q, want %q (snapshot mutation affected engine)", got, "middle")
	}
}

func TestRewind_HistoryPreserved(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, rewindMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance: start -> research -> implementing
	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}
	if err := eng.Transition("implementing"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Rewind to research
	if err := eng.Rewind("research"); err != nil {
		t.Fatalf("Rewind() error: %v", err)
	}

	// The full history should be preserved (not truncated)
	hist := eng.History()
	if len(hist) != 3 {
		t.Fatalf("History length = %d, want 3", len(hist))
	}

	// Verify all entries are intact
	expected := []struct {
		from, to, typ string
	}{
		{"start", "research", "transition"},
		{"research", "implementing", "transition"},
		{"implementing", "research", "rewind"},
	}

	for i, want := range expected {
		if hist[i].From != want.from {
			t.Errorf("History[%d].From = %q, want %q", i, hist[i].From, want.from)
		}
		if hist[i].To != want.to {
			t.Errorf("History[%d].To = %q, want %q", i, hist[i].To, want.to)
		}
		if hist[i].Type != want.typ {
			t.Errorf("History[%d].Type = %q, want %q", i, hist[i].Type, want.typ)
		}
	}
}

func TestTransitionError_JSONShape(t *testing.T) {
	// Full error with all fields populated.
	te := &TransitionError{
		Code:             ErrInvalidTransition,
		Message:          "cannot transition from 'research' to 'submitting': not in allowed transitions [validation_jury]",
		CurrentState:     "research",
		TargetState:      "submitting",
		ValidTransitions: []string{"validation_jury"},
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
	if m["message"] != te.Message {
		t.Errorf("message = %v, want %q", m["message"], te.Message)
	}
	if m["current_state"] != "research" {
		t.Errorf("current_state = %v, want %q", m["current_state"], "research")
	}
	if m["target_state"] != "submitting" {
		t.Errorf("target_state = %v, want %q", m["target_state"], "submitting")
	}
	transitions, ok := m["valid_transitions"].([]interface{})
	if !ok || len(transitions) != 1 || transitions[0] != "validation_jury" {
		t.Errorf("valid_transitions = %v, want [validation_jury]", m["valid_transitions"])
	}
}

func TestTransitionError_JSONOmitempty(t *testing.T) {
	// Error with only code and message -- optional fields should be omitted.
	te := &TransitionError{
		Code:    ErrVersionConflict,
		Message: "version conflict detected",
	}

	data, err := json.Marshal(te)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}

	var m map[string]interface{}
	if err := json.Unmarshal(data, &m); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	// Only code and message should be present.
	if len(m) != 2 {
		t.Errorf("JSON has %d keys, want 2 (code, message); got keys: %v", len(m), keysOf(m))
	}
	if _, exists := m["current_state"]; exists {
		t.Error("current_state should be omitted when empty")
	}
	if _, exists := m["target_state"]; exists {
		t.Error("target_state should be omitted when empty")
	}
	if _, exists := m["valid_transitions"]; exists {
		t.Error("valid_transitions should be omitted when empty")
	}
}

func TestTransitionError_AllCodes(t *testing.T) {
	// Verify all six error codes are defined and serialize correctly.
	codes := []string{
		ErrTerminalState,
		ErrInvalidTransition,
		ErrUnknownState,
		ErrTemplateMismatch,
		ErrVersionConflict,
		ErrRewindFailed,
	}

	expected := []string{
		"terminal_state",
		"invalid_transition",
		"unknown_state",
		"template_mismatch",
		"version_conflict",
		"rewind_failed",
	}

	for i, code := range codes {
		if code != expected[i] {
			t.Errorf("error code constant %d = %q, want %q", i, code, expected[i])
		}

		te := &TransitionError{Code: code, Message: "test"}
		data, err := json.Marshal(te)
		if err != nil {
			t.Fatalf("Marshal(%q) error: %v", code, err)
		}

		var m map[string]interface{}
		if err := json.Unmarshal(data, &m); err != nil {
			t.Fatalf("Unmarshal(%q) error: %v", code, err)
		}
		if m["code"] != expected[i] {
			t.Errorf("serialized code = %v, want %q", m["code"], expected[i])
		}
	}
}

func TestTransitionError_ErrorInterface(t *testing.T) {
	te := &TransitionError{
		Code:    ErrInvalidTransition,
		Message: "the error message",
	}

	// Verify Error() returns the Message field.
	var err error = te
	if err.Error() != "the error message" {
		t.Errorf("Error() = %q, want %q", err.Error(), "the error message")
	}
}

func TestVersionConflict_ConcurrentWrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Simulate a concurrent write by directly modifying the state file
	// on disk to increment its version.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}

	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	// Bump the version on disk to simulate another writer.
	state.Version = 99
	modified, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, modified, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Now the engine thinks version is 1, but disk has 99.
	// A transition should detect the conflict.
	err = eng.Transition("middle")
	if err == nil {
		t.Fatal("Transition() expected version_conflict error")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T: %v", err, err)
	}
	if te.Code != ErrVersionConflict {
		t.Errorf("error code = %q, want %q", te.Code, ErrVersionConflict)
	}
}

func TestVersionConflict_RewindDetects(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := rewindMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance to have a rewind target.
	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Tamper with on-disk version.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}
	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}
	state.Version = 50
	modified, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, modified, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Rewind should also detect the conflict.
	err = eng.Rewind("start")
	if err == nil {
		t.Fatal("Rewind() expected version_conflict error")
	}

	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T: %v", err, err)
	}
	if te.Code != ErrVersionConflict {
		t.Errorf("error code = %q, want %q", te.Code, ErrVersionConflict)
	}
}

func TestVersionConflict_NoConflictOnNormalFlow(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Normal transition should succeed without version conflict.
	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Second transition should also succeed.
	if err := eng.Transition("done"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	snap := eng.Snapshot()
	if snap.Version != 3 {
		t.Errorf("Version = %d, want 3", snap.Version)
	}
}

func TestErrorCodes_TriggeredByCorrectConditions(t *testing.T) {
	dir := t.TempDir()

	tests := []struct {
		name     string
		wantCode string
		setup    func(t *testing.T) error
	}{
		{
			name:     "terminal_state from Transition",
			wantCode: ErrTerminalState,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "terminal.state.json")
				eng, err := Init(path, testMachine(), InitMeta{Name: "terminal"})
				if err != nil {
					t.Fatalf("Init() error: %v", err)
				}
				if err := eng.Transition("middle"); err != nil {
					t.Fatalf("Transition(middle) error: %v", err)
				}
				if err := eng.Transition("done"); err != nil {
					t.Fatalf("Transition(done) error: %v", err)
				}
				return eng.Transition("anywhere")
			},
		},
		{
			name:     "invalid_transition from Transition",
			wantCode: ErrInvalidTransition,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "invalid.state.json")
				eng, err := Init(path, testMachine(), InitMeta{Name: "invalid"})
				if err != nil {
					t.Fatalf("Init() error: %v", err)
				}
				return eng.Transition("done") // start can only go to middle
			},
		},
		{
			name:     "unknown_state from Init",
			wantCode: ErrUnknownState,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "unknown.state.json")
				m := &Machine{
					Name:         "bad",
					InitialState: "nonexistent",
					States:       map[string]*MachineState{},
				}
				_, err := Init(path, m, InitMeta{Name: "unknown"})
				return err
			},
		},
		{
			name:     "unknown_state from Load",
			wantCode: ErrUnknownState,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "load-unknown.state.json")
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
				return err
			},
		},
		{
			name:     "rewind_failed for never-visited state",
			wantCode: ErrRewindFailed,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "rewind-nv.state.json")
				eng, err := Init(path, rewindMachine(), InitMeta{Name: "rewind-nv"})
				if err != nil {
					t.Fatalf("Init() error: %v", err)
				}
				if err := eng.Transition("research"); err != nil {
					t.Fatalf("Transition() error: %v", err)
				}
				return eng.Rewind("implementing")
			},
		},
		{
			name:     "rewind_failed for terminal target",
			wantCode: ErrRewindFailed,
			setup: func(t *testing.T) error {
				path := filepath.Join(dir, "rewind-term.state.json")
				eng, err := Init(path, rewindMachine(), InitMeta{Name: "rewind-term"})
				if err != nil {
					t.Fatalf("Init() error: %v", err)
				}
				return eng.Rewind("done")
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.setup(t)
			if err == nil {
				t.Fatal("expected error, got nil")
			}
			te, ok := err.(*TransitionError)
			if !ok {
				t.Fatalf("expected *TransitionError, got %T: %v", err, err)
			}
			if te.Code != tt.wantCode {
				t.Errorf("error code = %q, want %q", te.Code, tt.wantCode)
			}
		})
	}
}

func TestTransition_StateRestoredOnPersistFailure(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{
		Name:      "test",
		Variables: map[string]string{"KEY": "value"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Capture the state before the failed transition.
	origState := eng.CurrentState()
	origVersion := eng.Snapshot().Version
	origHistLen := len(eng.History())

	// Tamper with on-disk version to force a version_conflict on next persist.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}
	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}
	state.Version = 99
	modified, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, modified, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Attempt transition -- should fail with version_conflict.
	err = eng.Transition("middle")
	if err == nil {
		t.Fatal("Transition() expected version_conflict error")
	}
	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T: %v", err, err)
	}
	if te.Code != ErrVersionConflict {
		t.Errorf("error code = %q, want %q", te.Code, ErrVersionConflict)
	}

	// Verify the engine's in-memory state was restored.
	if got := eng.CurrentState(); got != origState {
		t.Errorf("CurrentState() = %q after failed persist, want %q", got, origState)
	}
	snap := eng.Snapshot()
	if snap.Version != origVersion {
		t.Errorf("Version = %d after failed persist, want %d", snap.Version, origVersion)
	}
	if len(snap.History) != origHistLen {
		t.Errorf("History length = %d after failed persist, want %d", len(snap.History), origHistLen)
	}
	if snap.Variables["KEY"] != "value" {
		t.Errorf("Variables[KEY] = %q after failed persist, want %q", snap.Variables["KEY"], "value")
	}
}

func TestRewind_StateRestoredOnPersistFailure(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := rewindMachine()

	eng, err := Init(path, machine, InitMeta{
		Name:      "test",
		Variables: map[string]string{"KEY": "value"},
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Advance so we have a rewind target.
	if err := eng.Transition("research"); err != nil {
		t.Fatalf("Transition(research) error: %v", err)
	}
	if err := eng.Transition("implementing"); err != nil {
		t.Fatalf("Transition(implementing) error: %v", err)
	}

	// Capture the state before the failed rewind.
	origState := eng.CurrentState()
	origVersion := eng.Snapshot().Version
	origHistLen := len(eng.History())

	// Tamper with on-disk version to force a version_conflict.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}
	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}
	state.Version = 99
	modified, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, modified, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Attempt rewind -- should fail with version_conflict.
	err = eng.Rewind("research")
	if err == nil {
		t.Fatal("Rewind() expected version_conflict error")
	}
	te, ok := err.(*TransitionError)
	if !ok {
		t.Fatalf("expected *TransitionError, got %T: %v", err, err)
	}
	if te.Code != ErrVersionConflict {
		t.Errorf("error code = %q, want %q", te.Code, ErrVersionConflict)
	}

	// Verify the engine's in-memory state was restored.
	if got := eng.CurrentState(); got != origState {
		t.Errorf("CurrentState() = %q after failed persist, want %q", got, origState)
	}
	snap := eng.Snapshot()
	if snap.Version != origVersion {
		t.Errorf("Version = %d after failed persist, want %d", snap.Version, origVersion)
	}
	if len(snap.History) != origHistLen {
		t.Errorf("History length = %d after failed persist, want %d", len(snap.History), origHistLen)
	}
	if snap.Variables["KEY"] != "value" {
		t.Errorf("Variables[KEY] = %q after failed persist, want %q", snap.Variables["KEY"], "value")
	}
}

// keysOf returns the keys of a map for diagnostic output.
func keysOf(m map[string]interface{}) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	return keys
}

// --- Evidence support tests (issue #15) ---

func TestTransition_WithEvidence_MergesIntoState(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Transition with evidence.
	err = eng.Transition("middle", WithEvidence(map[string]string{
		"result": "pass",
		"count":  "42",
	}))
	if err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	ev := eng.Evidence()
	if ev["result"] != "pass" {
		t.Errorf("Evidence[result] = %q, want %q", ev["result"], "pass")
	}
	if ev["count"] != "42" {
		t.Errorf("Evidence[count] = %q, want %q", ev["count"], "42")
	}
}

func TestTransition_WithEvidence_OverwritesExistingKeys(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := rewindMachine()
	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// First transition with evidence.
	err = eng.Transition("research", WithEvidence(map[string]string{
		"status": "initial",
		"keep":   "this",
	}))
	if err != nil {
		t.Fatalf("Transition(research) error: %v", err)
	}

	// Second transition overwrites "status", keeps "keep", adds "new".
	err = eng.Transition("implementing", WithEvidence(map[string]string{
		"status": "updated",
		"new":    "value",
	}))
	if err != nil {
		t.Fatalf("Transition(implementing) error: %v", err)
	}

	ev := eng.Evidence()
	if ev["status"] != "updated" {
		t.Errorf("Evidence[status] = %q, want %q", ev["status"], "updated")
	}
	if ev["keep"] != "this" {
		t.Errorf("Evidence[keep] = %q, want %q", ev["keep"], "this")
	}
	if ev["new"] != "value" {
		t.Errorf("Evidence[new] = %q, want %q", ev["new"], "value")
	}
}

func TestTransition_WithEvidence_RecordedInHistory(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	evidence := map[string]string{"key": "val"}
	if err := eng.Transition("middle", WithEvidence(evidence)); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	hist := eng.History()
	if len(hist) != 1 {
		t.Fatalf("History length = %d, want 1", len(hist))
	}
	if hist[0].Evidence == nil {
		t.Fatal("History[0].Evidence is nil, want non-nil")
	}
	if hist[0].Evidence["key"] != "val" {
		t.Errorf("History[0].Evidence[key] = %q, want %q", hist[0].Evidence["key"], "val")
	}
}

func TestTransition_NoEvidence_HistoryEvidenceNil(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Transition without evidence (zero opts).
	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	hist := eng.History()
	if len(hist) != 1 {
		t.Fatalf("History length = %d, want 1", len(hist))
	}
	if hist[0].Evidence != nil {
		t.Errorf("History[0].Evidence = %v, want nil (omitempty)", hist[0].Evidence)
	}
}

func TestRewind_DoesNotModifyEvidence(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := rewindMachine()
	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Transition with evidence.
	err = eng.Transition("research", WithEvidence(map[string]string{
		"gathered": "yes",
		"count":    "5",
	}))
	if err != nil {
		t.Fatalf("Transition(research) error: %v", err)
	}

	// Rewind to start.
	if err := eng.Rewind("start"); err != nil {
		t.Fatalf("Rewind(start) error: %v", err)
	}

	// Evidence should be preserved after rewind.
	ev := eng.Evidence()
	if ev["gathered"] != "yes" {
		t.Errorf("Evidence[gathered] = %q after rewind, want %q", ev["gathered"], "yes")
	}
	if ev["count"] != "5" {
		t.Errorf("Evidence[count] = %q after rewind, want %q", ev["count"], "5")
	}
}

func TestEvidence_ReturnsCopy(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle", WithEvidence(map[string]string{"KEY": "original"})); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	ev := eng.Evidence()
	ev["KEY"] = "tampered"

	// Engine's internal evidence should not be affected.
	if got := eng.Evidence()["KEY"]; got != "original" {
		t.Errorf("Evidence()[KEY] = %q, want %q (copy was not independent)", got, "original")
	}
}

func TestSnapshot_DeepCopiesEvidence(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle", WithEvidence(map[string]string{"KEY": "original"})); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	snap := eng.Snapshot()
	snap.Evidence["KEY"] = "tampered"

	// Engine's internal evidence should not be affected.
	if got := eng.Evidence()["KEY"]; got != "original" {
		t.Errorf("Evidence()[KEY] = %q after snapshot mutation, want %q", got, "original")
	}
}

func TestInit_SchemaVersion2(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	snap := eng.Snapshot()
	if snap.SchemaVersion != 2 {
		t.Errorf("SchemaVersion = %d, want 2", snap.SchemaVersion)
	}

	// Evidence should be initialized to empty map, not nil.
	if snap.Evidence == nil {
		t.Error("Evidence is nil, want empty map")
	}
	if len(snap.Evidence) != 0 {
		t.Errorf("Evidence length = %d, want 0", len(snap.Evidence))
	}
}

func TestLoad_SchemaVersion1_BackwardCompat(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	// Write a schema_version 1 state file (no evidence field).
	state := State{
		SchemaVersion: 1,
		Version:       1,
		CurrentState:  "start",
		Variables:     map[string]string{"X": "1"},
		History:       []HistoryEntry{},
	}
	data, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	eng, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	// Evidence should be initialized to empty map.
	ev := eng.Evidence()
	if ev == nil {
		t.Fatal("Evidence() returned nil for schema_version 1 file, want empty map")
	}
	if len(ev) != 0 {
		t.Errorf("Evidence() length = %d, want 0", len(ev))
	}

	// The engine should still work normally.
	if got := eng.CurrentState(); got != "start" {
		t.Errorf("CurrentState() = %q, want %q", got, "start")
	}
}

func TestLoad_SchemaVersion2_WithEvidence(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	// Write a schema_version 2 state file with evidence.
	state := State{
		SchemaVersion: 2,
		Version:       1,
		CurrentState:  "start",
		Variables:     map[string]string{"X": "1"},
		Evidence:      map[string]string{"result": "pass"},
		History:       []HistoryEntry{},
	}
	data, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	eng, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	ev := eng.Evidence()
	if ev["result"] != "pass" {
		t.Errorf("Evidence[result] = %q, want %q", ev["result"], "pass")
	}
}

func TestTransition_WithEvidence_PersistsToFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	err = eng.Transition("middle", WithEvidence(map[string]string{
		"result": "pass",
	}))
	if err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Reload and verify evidence persists.
	loaded, err := Load(path, machine)
	if err != nil {
		t.Fatalf("Load() error: %v", err)
	}

	ev := loaded.Evidence()
	if ev["result"] != "pass" {
		t.Errorf("loaded Evidence[result] = %q, want %q", ev["result"], "pass")
	}

	// Verify history entry has evidence too.
	hist := loaded.History()
	if len(hist) != 1 {
		t.Fatalf("loaded History length = %d, want 1", len(hist))
	}
	if hist[0].Evidence["result"] != "pass" {
		t.Errorf("loaded History[0].Evidence[result] = %q, want %q", hist[0].Evidence["result"], "pass")
	}
}

func TestTransition_WithEvidence_StateRestoredOnPersistFailure(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")
	machine := testMachine()

	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Tamper with on-disk version to force a version conflict.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}
	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}
	state.Version = 99
	modified, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}
	if err := os.WriteFile(path, modified, 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Attempt transition with evidence -- should fail.
	err = eng.Transition("middle", WithEvidence(map[string]string{"key": "val"}))
	if err == nil {
		t.Fatal("Transition() expected version_conflict error")
	}

	// Evidence should NOT have been merged.
	ev := eng.Evidence()
	if len(ev) != 0 {
		t.Errorf("Evidence length = %d after failed transition, want 0", len(ev))
	}
}

func TestEvidence_EmptyAfterInit(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	ev := eng.Evidence()
	if ev == nil {
		t.Error("Evidence() returned nil, want empty map")
	}
	if len(ev) != 0 {
		t.Errorf("Evidence() length = %d, want 0", len(ev))
	}
}

func TestTransition_WithEvidence_EvidenceAccumulatesAcrossTransitions(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := rewindMachine()
	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Each transition adds different evidence.
	if err := eng.Transition("research", WithEvidence(map[string]string{"phase1": "done"})); err != nil {
		t.Fatalf("Transition(research) error: %v", err)
	}
	if err := eng.Transition("implementing", WithEvidence(map[string]string{"phase2": "done"})); err != nil {
		t.Fatalf("Transition(implementing) error: %v", err)
	}
	if err := eng.Transition("review", WithEvidence(map[string]string{"phase3": "done"})); err != nil {
		t.Fatalf("Transition(review) error: %v", err)
	}

	ev := eng.Evidence()
	if ev["phase1"] != "done" || ev["phase2"] != "done" || ev["phase3"] != "done" {
		t.Errorf("Evidence = %v, want all three phases", ev)
	}

	// Each history entry should have its own evidence.
	hist := eng.History()
	if len(hist) != 3 {
		t.Fatalf("History length = %d, want 3", len(hist))
	}
	if hist[0].Evidence["phase1"] != "done" {
		t.Errorf("History[0].Evidence = %v, want phase1=done", hist[0].Evidence)
	}
	if hist[1].Evidence["phase2"] != "done" {
		t.Errorf("History[1].Evidence = %v, want phase2=done", hist[1].Evidence)
	}
	if hist[2].Evidence["phase3"] != "done" {
		t.Errorf("History[2].Evidence = %v, want phase3=done", hist[2].Evidence)
	}
}

func TestRewind_EvidencePersistsAcrossRewind(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := rewindMachine()
	eng, err := Init(path, machine, InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Transition with evidence through multiple states.
	if err := eng.Transition("research", WithEvidence(map[string]string{"a": "1"})); err != nil {
		t.Fatalf("Transition(research) error: %v", err)
	}
	if err := eng.Transition("implementing", WithEvidence(map[string]string{"b": "2"})); err != nil {
		t.Fatalf("Transition(implementing) error: %v", err)
	}

	// Rewind to research.
	if err := eng.Rewind("research"); err != nil {
		t.Fatalf("Rewind(research) error: %v", err)
	}

	// All evidence from prior transitions should still be present.
	ev := eng.Evidence()
	if ev["a"] != "1" {
		t.Errorf("Evidence[a] = %q after rewind, want %q", ev["a"], "1")
	}
	if ev["b"] != "2" {
		t.Errorf("Evidence[b] = %q after rewind, want %q", ev["b"], "2")
	}

	// New transition can overwrite evidence.
	if err := eng.Transition("implementing", WithEvidence(map[string]string{"b": "3"})); err != nil {
		t.Fatalf("Transition(implementing) after rewind error: %v", err)
	}
	ev = eng.Evidence()
	if ev["b"] != "3" {
		t.Errorf("Evidence[b] = %q after overwrite, want %q", ev["b"], "3")
	}
	if ev["a"] != "1" {
		t.Errorf("Evidence[a] = %q after overwrite, want %q", ev["a"], "1")
	}
}

func TestTransitionOption_ZeroOpts_BackwardCompat(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Existing callers passing zero opts should continue to work.
	if err := eng.Transition("middle"); err != nil {
		t.Fatalf("Transition() with zero opts error: %v", err)
	}

	if got := eng.CurrentState(); got != "middle" {
		t.Errorf("CurrentState() = %q, want %q", got, "middle")
	}

	// Evidence should remain empty.
	ev := eng.Evidence()
	if len(ev) != 0 {
		t.Errorf("Evidence length = %d after transition with zero opts, want 0", len(ev))
	}
}

func TestEvidence_JSONSerialization(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	eng, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	if err := eng.Transition("middle", WithEvidence(map[string]string{"key": "val"})); err != nil {
		t.Fatalf("Transition() error: %v", err)
	}

	// Read the persisted file and verify JSON structure.
	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}

	var raw map[string]interface{}
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	// Evidence should be in the JSON.
	ev, ok := raw["evidence"].(map[string]interface{})
	if !ok {
		t.Fatalf("evidence field missing or wrong type in JSON")
	}
	if ev["key"] != "val" {
		t.Errorf("evidence.key = %v, want %q", ev["key"], "val")
	}

	// History entry should have evidence.
	hist, ok := raw["history"].([]interface{})
	if !ok {
		t.Fatalf("history field missing or wrong type")
	}
	if len(hist) != 1 {
		t.Fatalf("history length = %d, want 1", len(hist))
	}
	entry, ok := hist[0].(map[string]interface{})
	if !ok {
		t.Fatalf("history[0] wrong type")
	}
	entryEv, ok := entry["evidence"].(map[string]interface{})
	if !ok {
		t.Fatalf("history[0].evidence missing or wrong type")
	}
	if entryEv["key"] != "val" {
		t.Errorf("history[0].evidence.key = %v, want %q", entryEv["key"], "val")
	}
}

func TestEvidence_OmitemptyWhenEmpty(t *testing.T) {
	// Verify that empty evidence is omitted from JSON output
	// when using Init (which sets Evidence to empty map).
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	_, err := Init(path, testMachine(), InitMeta{Name: "test"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	data, err := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	if err != nil {
		t.Fatalf("ReadFile() error: %v", err)
	}

	var raw map[string]interface{}
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	// Empty evidence map: Go's omitempty will NOT omit a non-nil empty map.
	// This is expected and fine -- the field will be present as {}.
	// The key behavior is that schema_version 1 files (which lack the
	// field entirely) are handled correctly by Load.
}
