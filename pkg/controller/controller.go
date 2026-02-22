// Package controller generates directives for agents based on the
// current engine state. It wraps the engine to provide a read-only
// view of what the agent should do next.
package controller

import (
	"github.com/tsukumogami/koto/pkg/engine"
)

// Controller generates directives for the current workflow state.
type Controller struct {
	eng *engine.Engine
}

// Directive represents an instruction for the agent.
type Directive struct {
	Action    string `json:"action"`              // "execute" or "done"
	State     string `json:"state"`               // current state name
	Directive string `json:"directive,omitempty"` // instruction text (execute only)
	Message   string `json:"message,omitempty"`   // completion message (done only)
}

// New creates a controller wrapping the given engine.
//
// In this skeleton, template hash verification is skipped. Full hash
// verification will be added in issue #6.
func New(eng *engine.Engine) *Controller {
	return &Controller{eng: eng}
}

// Next returns the directive for the current state.
//
// For non-terminal states, returns action="execute" with a stub directive
// string. For terminal states, returns action="done".
func (c *Controller) Next() (*Directive, error) {
	current := c.eng.CurrentState()
	machine := c.eng.Machine()

	ms, ok := machine.States[current]
	if !ok {
		return nil, &engine.TransitionError{
			Code:         "unknown_state",
			Message:      "current state not found in machine definition: " + current,
			CurrentState: current,
		}
	}

	if ms.Terminal {
		return &Directive{
			Action:  "done",
			State:   current,
			Message: "workflow complete",
		}, nil
	}

	return &Directive{
		Action:    "execute",
		State:     current,
		Directive: "Execute the " + current + " phase of the workflow.",
	}, nil
}
