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
			Code:    ErrUnknownState,
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
			Code:         ErrUnknownState,
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
// is allowed, updates state, and persists atomically. If persistence
// fails, the in-memory state is restored to its pre-transition value.
func (e *Engine) Transition(target string) error {
	current := e.state.CurrentState
	ms, ok := e.machine.States[current]
	if !ok {
		return &TransitionError{
			Code:         ErrUnknownState,
			Message:      fmt.Sprintf("current state %q not found in machine definition", current),
			CurrentState: current,
		}
	}

	if ms.Terminal {
		return &TransitionError{
			Code:         ErrTerminalState,
			Message:      fmt.Sprintf("cannot transition from terminal state %q", current),
			CurrentState: current,
			TargetState:  target,
		}
	}

	if !contains(ms.Transitions, target) {
		return &TransitionError{
			Code:             ErrInvalidTransition,
			Message:          fmt.Sprintf("cannot transition from %q to %q: not in allowed transitions %v", current, target, ms.Transitions),
			CurrentState:     current,
			TargetState:      target,
			ValidTransitions: ms.Transitions,
		}
	}

	prev := deepCopyState(e.state)

	e.state.CurrentState = target
	e.state.Version++
	e.state.History = append(e.state.History, HistoryEntry{
		From:      current,
		To:        target,
		Timestamp: time.Now().UTC().Format(time.RFC3339),
		Type:      "transition",
	})

	if err := e.persist(); err != nil {
		e.state = prev
		return err
	}
	return nil
}

// Rewind resets to a prior state. The target must have been visited
// (appear in history as a "to" field) or be the machine's initial state.
// Rewinding to a terminal state is not allowed. Rewinding from a terminal
// state is allowed (this is the recovery path).
func (e *Engine) Rewind(target string) error {
	ms, ok := e.machine.States[target]
	if !ok {
		return &TransitionError{
			Code:         ErrRewindFailed,
			Message:      fmt.Sprintf("cannot rewind to %q: state not found in machine definition", target),
			CurrentState: e.state.CurrentState,
			TargetState:  target,
		}
	}

	if ms.Terminal {
		return &TransitionError{
			Code:         ErrRewindFailed,
			Message:      fmt.Sprintf("cannot rewind to %q: target is a terminal state", target),
			CurrentState: e.state.CurrentState,
			TargetState:  target,
		}
	}

	// The initial state is always a valid rewind target.
	if target != e.machine.InitialState {
		// Check that the target appears in history as a "to" field.
		found := false
		for _, entry := range e.state.History {
			if entry.To == target {
				found = true
				break
			}
		}
		if !found {
			return &TransitionError{
				Code:         ErrRewindFailed,
				Message:      fmt.Sprintf("cannot rewind to %q: state has never been visited", target),
				CurrentState: e.state.CurrentState,
				TargetState:  target,
			}
		}
	}

	prev := deepCopyState(e.state)

	from := e.state.CurrentState
	e.state.CurrentState = target
	e.state.Version++
	e.state.History = append(e.state.History, HistoryEntry{
		From:      from,
		To:        target,
		Timestamp: time.Now().UTC().Format(time.RFC3339),
		Type:      "rewind",
	})

	if err := e.persist(); err != nil {
		e.state = prev
		return err
	}
	return nil
}

// Cancel deletes the state file, abandoning the workflow.
// Returns an error if the state file cannot be removed.
func (e *Engine) Cancel() error {
	return os.Remove(e.path)
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

		var gates map[string]*GateDecl
		if ms.Gates != nil {
			gates = make(map[string]*GateDecl, len(ms.Gates))
			for gn, gd := range ms.Gates {
				gates[gn] = &GateDecl{
					Type:    gd.Type,
					Field:   gd.Field,
					Value:   gd.Value,
					Command: gd.Command,
					Timeout: gd.Timeout,
				}
			}
		}

		states[name] = &MachineState{
			Transitions: transitions,
			Terminal:    ms.Terminal,
			Gates:       gates,
		}
	}

	var declaredVars map[string]bool
	if e.machine.DeclaredVars != nil {
		declaredVars = make(map[string]bool, len(e.machine.DeclaredVars))
		for k, v := range e.machine.DeclaredVars {
			declaredVars[k] = v
		}
	}

	return &Machine{
		Name:         e.machine.Name,
		InitialState: e.machine.InitialState,
		States:       states,
		DeclaredVars: declaredVars,
	}
}

// persist writes the current state to disk atomically. Before writing,
// it re-reads the on-disk version to detect concurrent modifications.
// If the on-disk version differs from the expected version, persist
// returns a version_conflict error.
func (e *Engine) persist() error {
	data, err := json.MarshalIndent(e.state, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal state: %w", err)
	}
	data = append(data, '\n')

	// Check for version conflict before writing. The expected version
	// is the version we had before the current mutation incremented it.
	// For Init (version=1, no prior file), we skip the check.
	expectedVersion := e.state.Version - 1
	if expectedVersion > 0 {
		if err := e.checkVersionConflict(expectedVersion); err != nil {
			return err
		}
	}

	return atomicWrite(e.path, data)
}

// checkVersionConflict re-reads the state file's version field and
// returns a version_conflict error if it differs from expected.
func (e *Engine) checkVersionConflict(expectedVersion int) error {
	diskData, err := os.ReadFile(e.path) //nolint:gosec // G304: engine re-reads its own state file path
	if err != nil {
		// If the file doesn't exist (e.g., first write after Init), no conflict.
		if os.IsNotExist(err) {
			return nil
		}
		return fmt.Errorf("re-read state file for version check: %w", err)
	}

	var diskState struct {
		Version int `json:"version"`
	}
	if err := json.Unmarshal(diskData, &diskState); err != nil {
		return fmt.Errorf("parse state file for version check: %w", err)
	}

	if diskState.Version != expectedVersion {
		return &TransitionError{
			Code:    ErrVersionConflict,
			Message: fmt.Sprintf("version conflict: expected version %d but found %d on disk", expectedVersion, diskState.Version),
		}
	}

	return nil
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

// deepCopyState returns a deep copy of a State value, duplicating the
// History slice and Variables map so the copy shares no references with
// the original.
func deepCopyState(s State) State {
	cp := s

	if s.Variables != nil {
		cp.Variables = make(map[string]string, len(s.Variables))
		for k, v := range s.Variables {
			cp.Variables[k] = v
		}
	}

	if s.History != nil {
		cp.History = make([]HistoryEntry, len(s.History))
		copy(cp.History, s.History)
	}

	return cp
}

func contains(ss []string, s string) bool {
	for _, v := range ss {
		if v == s {
			return true
		}
	}
	return false
}
