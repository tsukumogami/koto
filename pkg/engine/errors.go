package engine

import "encoding/json"

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

// MarshalJSON produces the JSON representation of the error.
func (e *TransitionError) MarshalJSON() ([]byte, error) {
	type alias TransitionError
	return json.Marshal((*alias)(e))
}
