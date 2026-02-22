// Package engine provides the core state machine for koto workflows.
//
// The engine manages workflow state persistence, transition validation,
// and atomic writes. It is the foundation that all other koto packages
// build on.
package engine

// State is the persisted workflow state.
type State struct {
	SchemaVersion int               `json:"schema_version"`
	Workflow      WorkflowMeta      `json:"workflow"`
	Version       int               `json:"version"`
	CurrentState  string            `json:"current_state"`
	Variables     map[string]string `json:"variables"`
	History       []HistoryEntry    `json:"history"`
}

// WorkflowMeta holds metadata about the workflow instance.
type WorkflowMeta struct {
	Name         string `json:"name"`
	TemplateHash string `json:"template_hash"`
	TemplatePath string `json:"template_path"`
	CreatedAt    string `json:"created_at"`
}

// HistoryEntry records a single state change.
type HistoryEntry struct {
	From      string `json:"from"`
	To        string `json:"to"`
	Timestamp string `json:"timestamp"`
	Type      string `json:"type"` // "transition" or "rewind"
}

// Machine is the in-memory representation of a state machine definition.
type Machine struct {
	Name         string
	InitialState string
	States       map[string]*MachineState
	DeclaredVars map[string]bool
}

// MachineState defines a single state in the machine, including its
// allowed transitions and whether it is terminal.
type MachineState struct {
	Transitions []string
	Terminal    bool
	Gates       map[string]*GateDecl
}

// GateDecl represents a gate declaration on a machine state. Gates are
// preconditions that must be satisfied before entering the state.
type GateDecl struct {
	Type    string
	Field   string
	Value   string
	Command string
	Timeout int // seconds, 0 = default (30s)
}
