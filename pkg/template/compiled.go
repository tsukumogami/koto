package template

import (
	"encoding/json"
	"fmt"

	"github.com/tsukumogami/koto/pkg/engine"
)

// CompiledTemplate is the JSON-serializable compiled form of a workflow
// template. It contains the full state machine definition, variable
// declarations, and metadata needed to build an engine.Machine.
type CompiledTemplate struct {
	FormatVersion int                     `json:"format_version"`
	Name          string                  `json:"name"`
	Version       string                  `json:"version"`
	Description   string                  `json:"description,omitempty"`
	InitialState  string                  `json:"initial_state"`
	Variables     map[string]VariableDecl `json:"variables,omitempty"`
	States        map[string]StateDecl    `json:"states"`
}

// VariableDecl declares a template variable with optional description,
// required flag, and default value.
type VariableDecl struct {
	Description string `json:"description,omitempty"`
	Required    bool   `json:"required,omitempty"`
	Default     string `json:"default,omitempty"`
}

// StateDecl defines a single state in the compiled template, including
// the directive text, allowed transitions, terminal flag, and gates.
type StateDecl struct {
	Directive   string                     `json:"directive"`
	Transitions []string                   `json:"transitions,omitempty"`
	Terminal    bool                       `json:"terminal,omitempty"`
	Gates       map[string]engine.GateDecl `json:"gates,omitempty"`
}

// ParseJSON parses a compiled template from JSON bytes and validates
// all structural constraints. It returns an error if the JSON is
// malformed or any validation rule fails.
func ParseJSON(data []byte) (*CompiledTemplate, error) {
	var ct CompiledTemplate
	if err := json.Unmarshal(data, &ct); err != nil {
		return nil, err
	}

	if ct.FormatVersion != 1 {
		return nil, fmt.Errorf("unsupported format version: %d", ct.FormatVersion)
	}
	if ct.Name == "" {
		return nil, fmt.Errorf("missing required field: name")
	}
	if ct.Version == "" {
		return nil, fmt.Errorf("missing required field: version")
	}
	if ct.InitialState == "" {
		return nil, fmt.Errorf("missing required field: initial_state")
	}
	if len(ct.States) == 0 {
		return nil, fmt.Errorf("template has no states")
	}
	if _, ok := ct.States[ct.InitialState]; !ok {
		return nil, fmt.Errorf("initial_state %q is not a declared state", ct.InitialState)
	}

	for stateName, sd := range ct.States {
		if sd.Directive == "" {
			return nil, fmt.Errorf("state %q has empty directive", stateName)
		}

		for _, target := range sd.Transitions {
			if _, ok := ct.States[target]; !ok {
				return nil, fmt.Errorf("state %q references undefined transition target %q", stateName, target)
			}
		}

		for gateName, gd := range sd.Gates {
			switch gd.Type {
			case "field_not_empty":
				if gd.Field == "" {
					return nil, fmt.Errorf("state %q gate %q: missing required field %q", stateName, gateName, "field")
				}
			case "field_equals":
				if gd.Field == "" {
					return nil, fmt.Errorf("state %q gate %q: missing required field %q", stateName, gateName, "field")
				}
				if gd.Value == "" {
					return nil, fmt.Errorf("state %q gate %q: missing required field %q", stateName, gateName, "value")
				}
			case "command":
				if gd.Command == "" {
					return nil, fmt.Errorf("state %q gate %q: command must not be empty", stateName, gateName)
				}
			default:
				return nil, fmt.Errorf("state %q gate %q: unknown type %q", stateName, gateName, gd.Type)
			}
		}
	}

	return &ct, nil
}

// ToTemplate converts a CompiledTemplate to a Template struct for use
// with the controller. Sections are populated from StateDecl.Directive
// fields, Variables from VariableDecl.Default values, Machine from
// BuildMachine(), and Hash from the caller-supplied hash. Path must be
// set by the caller after this method returns.
func (ct *CompiledTemplate) ToTemplate() (*Template, error) {
	machine := ct.BuildMachine()

	sections := make(map[string]string, len(ct.States))
	for name, sd := range ct.States {
		sections[name] = sd.Directive
	}

	variables := make(map[string]string, len(ct.Variables))
	for name, vd := range ct.Variables {
		variables[name] = vd.Default
	}

	return &Template{
		Name:        ct.Name,
		Version:     ct.Version,
		Description: ct.Description,
		Machine:     machine,
		Sections:    sections,
		Variables:   variables,
	}, nil
}

// BuildMachine converts the compiled template into an engine.Machine
// suitable for use with engine.Init or engine.Load.
func (ct *CompiledTemplate) BuildMachine() *engine.Machine {
	states := make(map[string]*engine.MachineState, len(ct.States))
	for name, sd := range ct.States {
		var gates map[string]*engine.GateDecl
		if len(sd.Gates) > 0 {
			gates = make(map[string]*engine.GateDecl, len(sd.Gates))
			for gn, gd := range sd.Gates {
				g := gd // copy the value to avoid aliasing the loop variable
				gates[gn] = &g
			}
		}

		transitions := make([]string, len(sd.Transitions))
		copy(transitions, sd.Transitions)

		states[name] = &engine.MachineState{
			Transitions: transitions,
			Terminal:    sd.Terminal,
			Gates:       gates,
		}
	}

	var declaredVars map[string]bool
	if len(ct.Variables) > 0 {
		declaredVars = make(map[string]bool, len(ct.Variables))
		for k := range ct.Variables {
			declaredVars[k] = true
		}
	}

	return &engine.Machine{
		Name:         ct.Name,
		InitialState: ct.InitialState,
		States:       states,
		DeclaredVars: declaredVars,
	}
}
