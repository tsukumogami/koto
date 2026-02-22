// Package controller generates directives for agents based on the
// current engine state. It wraps the engine to provide a read-only
// view of what the agent should do next.
package controller

import (
	"fmt"

	"github.com/tsukumogami/koto/pkg/engine"
	"github.com/tsukumogami/koto/pkg/template"
)

// Controller generates directives for the current workflow state.
type Controller struct {
	eng  *engine.Engine
	tmpl *template.Template
}

// Directive represents an instruction for the agent.
type Directive struct {
	Action    string `json:"action"`              // "execute" or "done"
	State     string `json:"state"`               // current state name
	Directive string `json:"directive,omitempty"` // instruction text (execute only)
	Message   string `json:"message,omitempty"`   // completion message (done only)
}

// New creates a controller wrapping the given engine. If tmpl is non-nil,
// its hash is compared to the hash stored in the engine's state file.
// A mismatch returns a template_mismatch error. When tmpl is nil, hash
// verification is skipped and Next returns a generic directive stub.
func New(eng *engine.Engine, tmpl *template.Template) (*Controller, error) {
	if tmpl != nil {
		storedHash := eng.Snapshot().Workflow.TemplateHash
		if storedHash != tmpl.Hash {
			return nil, &engine.TransitionError{
				Code: engine.ErrTemplateMismatch,
				Message: fmt.Sprintf(
					"template hash mismatch: state file has %q but template on disk is %q",
					storedHash, tmpl.Hash),
			}
		}
	}
	return &Controller{eng: eng, tmpl: tmpl}, nil
}

// Next returns the directive for the current state.
//
// For non-terminal states, returns action="execute" with the interpolated
// template section content. For terminal states, returns action="done".
// If the controller has no template, a generic stub directive is returned.
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

	directive := "Execute the " + current + " phase of the workflow."
	if c.tmpl != nil {
		if section, ok := c.tmpl.Sections[current]; ok {
			directive = template.Interpolate(section, c.eng.Variables())
		}
	}

	return &Directive{
		Action:    "execute",
		State:     current,
		Directive: directive,
	}, nil
}
