package engine

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// Engine manages a workflow's lifecycle. It holds the in-memory state
// and machine definition, and handles persistence to disk.
type Engine struct {
	state   State
	machine *Machine
	path    string
}

// InitMeta provides metadata for creating a new workflow.
type InitMeta struct {
	Name         string
	TemplateHash string
	TemplatePath string
	Variables    map[string]string
}

// Init creates a new workflow state file and returns an engine for it.
// The state file is written atomically to the given path.
func Init(statePath string, machine *Machine, meta InitMeta) (*Engine, error) {
	if _, ok := machine.States[machine.InitialState]; !ok {
		return nil, &TransitionError{
			Code:    "unknown_state",
			Message: fmt.Sprintf("initial state %q not found in machine definition", machine.InitialState),
		}
	}

	vars := make(map[string]string)
	for k, v := range meta.Variables {
		vars[k] = v
	}

	state := State{
		SchemaVersion: 1,
		Workflow: WorkflowMeta{
			Name:         meta.Name,
			TemplateHash: meta.TemplateHash,
			TemplatePath: meta.TemplatePath,
			CreatedAt:    time.Now().UTC().Format(time.RFC3339),
		},
		Version:      1,
		CurrentState: machine.InitialState,
		Variables:    vars,
		History:      []HistoryEntry{},
	}

	e := &Engine{
		state:   state,
		machine: machine,
		path:    statePath,
	}

	if err := e.persist(); err != nil {
		return nil, err
	}

	return e, nil
}

// Load reads an existing state file and validates it against the machine.
func Load(statePath string, machine *Machine) (*Engine, error) {
	data, err := os.ReadFile(statePath) //nolint:gosec // G304: engine reads caller-specified state file path
	if err != nil {
		return nil, fmt.Errorf("read state file: %w", err)
	}

	var state State
	if err := json.Unmarshal(data, &state); err != nil {
		return nil, fmt.Errorf("parse state file: %w", err)
	}

	if _, ok := machine.States[state.CurrentState]; !ok {
		return nil, &TransitionError{
			Code:         "unknown_state",
			Message:      fmt.Sprintf("current state %q not found in machine definition", state.CurrentState),
			CurrentState: state.CurrentState,
		}
	}

	return &Engine{
		state:   state,
		machine: machine,
		path:    statePath,
	}, nil
}

// Transition advances to the target state. It validates the transition
// is allowed, updates state, and persists atomically.
func (e *Engine) Transition(target string) error {
	current := e.state.CurrentState
	ms, ok := e.machine.States[current]
	if !ok {
		return &TransitionError{
			Code:         "unknown_state",
			Message:      fmt.Sprintf("current state %q not found in machine definition", current),
			CurrentState: current,
		}
	}

	if ms.Terminal {
		return &TransitionError{
			Code:         "terminal_state",
			Message:      fmt.Sprintf("cannot transition from terminal state %q", current),
			CurrentState: current,
			TargetState:  target,
		}
	}

	if !contains(ms.Transitions, target) {
		return &TransitionError{
			Code:             "invalid_transition",
			Message:          fmt.Sprintf("cannot transition from %q to %q: not in allowed transitions %v", current, target, ms.Transitions),
			CurrentState:     current,
			TargetState:      target,
			ValidTransitions: ms.Transitions,
		}
	}

	e.state.CurrentState = target
	e.state.Version++
	e.state.History = append(e.state.History, HistoryEntry{
		From:      current,
		To:        target,
		Timestamp: time.Now().UTC().Format(time.RFC3339),
		Type:      "transition",
	})

	return e.persist()
}

// CurrentState returns the name of the current state.
func (e *Engine) CurrentState() string {
	return e.state.CurrentState
}

// Variables returns a copy of the workflow variables.
func (e *Engine) Variables() map[string]string {
	out := make(map[string]string, len(e.state.Variables))
	for k, v := range e.state.Variables {
		out[k] = v
	}
	return out
}

// History returns the transition history.
func (e *Engine) History() []HistoryEntry {
	out := make([]HistoryEntry, len(e.state.History))
	copy(out, e.state.History)
	return out
}

// Snapshot returns a copy of the full state for serialization to JSON.
func (e *Engine) Snapshot() State {
	s := e.state
	s.Variables = e.Variables()
	s.History = e.History()
	return s
}

// Path returns the state file path.
func (e *Engine) Path() string {
	return e.path
}

// Machine returns a deep copy of the machine definition associated with
// this engine. The copy prevents callers from mutating internal state.
func (e *Engine) Machine() *Machine {
	states := make(map[string]*MachineState, len(e.machine.States))
	for name, ms := range e.machine.States {
		transitions := make([]string, len(ms.Transitions))
		copy(transitions, ms.Transitions)
		states[name] = &MachineState{
			Transitions: transitions,
			Terminal:    ms.Terminal,
		}
	}
	return &Machine{
		Name:         e.machine.Name,
		InitialState: e.machine.InitialState,
		States:       states,
	}
}

// persist writes the current state to disk atomically.
func (e *Engine) persist() error {
	data, err := json.MarshalIndent(e.state, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal state: %w", err)
	}
	data = append(data, '\n')
	return atomicWrite(e.path, data)
}

// atomicWrite writes data to path using write-to-temp-then-rename.
func atomicWrite(path string, data []byte) error {
	dir := filepath.Dir(path)

	tmp, err := os.CreateTemp(dir, ".koto-*.tmp")
	if err != nil {
		return fmt.Errorf("create temp file: %w", err)
	}
	tmpPath := tmp.Name()

	success := false
	defer func() {
		if !success {
			os.Remove(tmpPath)
		}
	}()

	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return fmt.Errorf("write temp file: %w", err)
	}

	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return fmt.Errorf("sync temp file: %w", err)
	}

	if err := tmp.Close(); err != nil {
		return fmt.Errorf("close temp file: %w", err)
	}

	// Check for symlinks at the target path
	if info, err := os.Lstat(path); err == nil && info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("state file path is a symlink: %s", path)
	}

	if err := os.Rename(tmpPath, path); err != nil { //nolint:gosec // G703: tmpPath is from CreateTemp in same dir
		return fmt.Errorf("rename temp to state file: %w", err)
	}

	success = true
	return nil
}

func contains(ss []string, s string) bool {
	for _, v := range ss {
		if v == s {
			return true
		}
	}
	return false
}
