package engine

// Error code constants for all engine failure modes.
const (
	// ErrTerminalState indicates the current state has no transitions.
	ErrTerminalState = "terminal_state"

	// ErrInvalidTransition indicates the target is not in the current
	// state's allowed transitions list.
	ErrInvalidTransition = "invalid_transition"

	// ErrUnknownState indicates a state name was not found in the
	// machine definition.
	ErrUnknownState = "unknown_state"

	// ErrTemplateMismatch indicates the template hash in the state file
	// does not match the template on disk. Defined here but triggered
	// by the Controller layer.
	ErrTemplateMismatch = "template_mismatch"

	// ErrVersionConflict indicates the state file version changed
	// between read and write, suggesting concurrent modification.
	ErrVersionConflict = "version_conflict"

	// ErrRewindFailed indicates a rewind could not be completed. The
	// target was not found in history (and is not the initial state),
	// or the target is a terminal state.
	ErrRewindFailed = "rewind_failed"
)

// TransitionError represents a structured engine error that serializes
// to a consistent JSON shape for machine-parseable output.
type TransitionError struct {
	Code             string   `json:"code"`
	Message          string   `json:"message"`
	CurrentState     string   `json:"current_state,omitempty"`
	TargetState      string   `json:"target_state,omitempty"`
	ValidTransitions []string `json:"valid_transitions,omitempty"`
}

// Error implements the error interface.
func (e *TransitionError) Error() string {
	return e.Message
}
