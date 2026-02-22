// Package controller generates directives for agents based on the
// current engine state. It wraps the engine to provide a read-only
// view of what the agent should do next.
package controller

import (
	"fmt"

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

// New creates a controller wrapping the given engine. The templateHash
// parameter is the SHA-256 hash of the current template file on disk.
// If it does not match the hash stored in the engine's state file,
// New returns a template_mismatch error.
//
// Pass an empty string to skip hash verification (useful when the
// template package is not yet available).
func New(eng *engine.Engine, templateHash string) (*Controller, error) {
	if templateHash != "" {
		storedHash := eng.Snapshot().Workflow.TemplateHash
		if storedHash != templateHash {
			return nil, &engine.TransitionError{
				Code: engine.ErrTemplateMismatch,
				Message: fmt.Sprintf(
					"template hash mismatch: state file has %q but template on disk is %q",
					storedHash, templateHash),
			}
		}
	}
	return &Controller{eng: eng}, nil
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
			Code:         engine.ErrUnknownState,
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
