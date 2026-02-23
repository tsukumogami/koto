package template

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsukumogami/koto/pkg/engine"
)

// validCompiledJSON is a fully populated compiled template used across tests.
const validCompiledJSON = `{
  "format_version": 1,
  "name": "review-workflow",
  "version": "2.0",
  "description": "A code review workflow",
  "initial_state": "assess",
  "variables": {
    "PR_URL": {
      "description": "Pull request URL",
      "required": true
    },
    "REVIEWER": {
      "description": "Assigned reviewer",
      "default": "auto"
    }
  },
  "states": {
    "assess": {
      "directive": "Review the pull request at {{PR_URL}}.",
      "transitions": ["review", "reject"],
      "gates": {
        "has_pr": {
          "type": "field_not_empty",
          "field": "PR_URL"
        }
      }
    },
    "review": {
      "directive": "Perform detailed code review.",
      "transitions": ["approve", "request-changes"],
      "gates": {
        "assigned": {
          "type": "field_equals",
          "field": "REVIEWER",
          "value": "auto"
        },
        "lint_pass": {
          "type": "command",
          "command": "make lint",
          "timeout": 60
        }
      }
    },
    "approve": {
      "directive": "Approve the PR.",
      "terminal": true
    },
    "request-changes": {
      "directive": "Request changes from the author.",
      "transitions": ["review"]
    },
    "reject": {
      "directive": "Reject the PR with explanation.",
      "terminal": true
    }
  }
}`

func TestParseJSON_ValidTemplate(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	if ct.FormatVersion != 1 {
		t.Errorf("FormatVersion = %d, want 1", ct.FormatVersion)
	}
	if ct.Name != "review-workflow" {
		t.Errorf("Name = %q, want %q", ct.Name, "review-workflow")
	}
	if ct.Version != "2.0" {
		t.Errorf("Version = %q, want %q", ct.Version, "2.0")
	}
	if ct.Description != "A code review workflow" {
		t.Errorf("Description = %q, want %q", ct.Description, "A code review workflow")
	}
	if ct.InitialState != "assess" {
		t.Errorf("InitialState = %q, want %q", ct.InitialState, "assess")
	}

	// Variables.
	if len(ct.Variables) != 2 {
		t.Fatalf("Variables count = %d, want 2", len(ct.Variables))
	}
	prURL := ct.Variables["PR_URL"]
	if !prURL.Required {
		t.Error("Variables[PR_URL].Required = false, want true")
	}
	if prURL.Description != "Pull request URL" {
		t.Errorf("Variables[PR_URL].Description = %q, want %q", prURL.Description, "Pull request URL")
	}
	reviewer := ct.Variables["REVIEWER"]
	if reviewer.Default != "auto" {
		t.Errorf("Variables[REVIEWER].Default = %q, want %q", reviewer.Default, "auto")
	}

	// States.
	if len(ct.States) != 5 {
		t.Fatalf("States count = %d, want 5", len(ct.States))
	}

	assessState := ct.States["assess"]
	if assessState.Directive != "Review the pull request at {{PR_URL}}." {
		t.Errorf("States[assess].Directive = %q", assessState.Directive)
	}
	if len(assessState.Transitions) != 2 || assessState.Transitions[0] != "review" || assessState.Transitions[1] != "reject" {
		t.Errorf("States[assess].Transitions = %v, want [review, reject]", assessState.Transitions)
	}
	if len(assessState.Gates) != 1 {
		t.Fatalf("States[assess].Gates count = %d, want 1", len(assessState.Gates))
	}
	hasPR := assessState.Gates["has_pr"]
	if hasPR.Type != "field_not_empty" {
		t.Errorf("Gates[has_pr].Type = %q, want %q", hasPR.Type, "field_not_empty")
	}
	if hasPR.Field != "PR_URL" {
		t.Errorf("Gates[has_pr].Field = %q, want %q", hasPR.Field, "PR_URL")
	}

	// Terminal states.
	if !ct.States["approve"].Terminal {
		t.Error("States[approve].Terminal = false, want true")
	}
	if !ct.States["reject"].Terminal {
		t.Error("States[reject].Terminal = false, want true")
	}

	// Command gate with timeout.
	lintGate := ct.States["review"].Gates["lint_pass"]
	if lintGate.Type != "command" {
		t.Errorf("Gates[lint_pass].Type = %q, want %q", lintGate.Type, "command")
	}
	if lintGate.Command != "make lint" {
		t.Errorf("Gates[lint_pass].Command = %q, want %q", lintGate.Command, "make lint")
	}
	if lintGate.Timeout != 60 {
		t.Errorf("Gates[lint_pass].Timeout = %d, want 60", lintGate.Timeout)
	}
}

func TestParseJSON_InvalidJSON(t *testing.T) {
	_, err := ParseJSON([]byte(`{not valid json`))
	if err == nil {
		t.Fatal("ParseJSON() expected error for invalid JSON")
	}
}

func TestParseJSON_UnsupportedFormatVersion(t *testing.T) {
	data := []byte(`{
		"format_version": 99,
		"name": "test",
		"version": "1.0",
		"initial_state": "start",
		"states": {"start": {"directive": "go"}}
	}`)

	_, err := ParseJSON(data)
	if err == nil {
		t.Fatal("ParseJSON() expected error for unsupported format version")
	}
	if !strings.Contains(err.Error(), "unsupported format version: 99") {
		t.Errorf("error = %q, want substring %q", err.Error(), "unsupported format version: 99")
	}
}

func TestParseJSON_MissingRequiredFields(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantErr string
	}{
		{
			name: "missing name",
			json: `{
				"format_version": 1,
				"version": "1.0",
				"initial_state": "start",
				"states": {"start": {"directive": "go"}}
			}`,
			wantErr: "missing required field: name",
		},
		{
			name: "missing version",
			json: `{
				"format_version": 1,
				"name": "test",
				"initial_state": "start",
				"states": {"start": {"directive": "go"}}
			}`,
			wantErr: "missing required field: version",
		},
		{
			name: "missing initial_state",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"states": {"start": {"directive": "go"}}
			}`,
			wantErr: "missing required field: initial_state",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := ParseJSON([]byte(tt.json))
			if err == nil {
				t.Fatalf("ParseJSON() expected error")
			}
			if err.Error() != tt.wantErr {
				t.Errorf("error = %q, want %q", err.Error(), tt.wantErr)
			}
		})
	}
}

func TestParseJSON_StateMachineIntegrity(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantErr string
	}{
		{
			name: "initial_state not in states",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "missing",
				"states": {"start": {"directive": "go"}}
			}`,
			wantErr: `initial_state "missing" is not a declared state`,
		},
		{
			name: "no states declared",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "start",
				"states": {}
			}`,
			wantErr: "template has no states",
		},
		{
			name: "transition target not in states",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "start",
				"states": {
					"start": {
						"directive": "begin",
						"transitions": ["nonexistent"]
					}
				}
			}`,
			wantErr: `state "start" references undefined transition target "nonexistent"`,
		},
		{
			name: "empty directive",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "start",
				"states": {
					"start": {
						"directive": ""
					}
				}
			}`,
			wantErr: `state "start" has empty directive`,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := ParseJSON([]byte(tt.json))
			if err == nil {
				t.Fatalf("ParseJSON() expected error")
			}
			if err.Error() != tt.wantErr {
				t.Errorf("error = %q, want %q", err.Error(), tt.wantErr)
			}
		})
	}
}

func TestParseJSON_GateValidation(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantErr string
	}{
		{
			name: "unknown gate type",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "s",
				"states": {
					"s": {
						"directive": "do",
						"gates": {
							"g1": {"type": "bogus"}
						}
					}
				}
			}`,
			wantErr: `state "s" gate "g1": unknown type "bogus"`,
		},
		{
			name: "field_not_empty missing field",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "s",
				"states": {
					"s": {
						"directive": "do",
						"gates": {
							"g1": {"type": "field_not_empty"}
						}
					}
				}
			}`,
			wantErr: `state "s" gate "g1": missing required field "field"`,
		},
		{
			name: "field_equals missing field",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "s",
				"states": {
					"s": {
						"directive": "do",
						"gates": {
							"g1": {"type": "field_equals", "value": "x"}
						}
					}
				}
			}`,
			wantErr: `state "s" gate "g1": missing required field "field"`,
		},
		{
			name: "field_equals missing value",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "s",
				"states": {
					"s": {
						"directive": "do",
						"gates": {
							"g1": {"type": "field_equals", "field": "X"}
						}
					}
				}
			}`,
			wantErr: `state "s" gate "g1": missing required field "value"`,
		},
		{
			name: "command gate empty command",
			json: `{
				"format_version": 1,
				"name": "test",
				"version": "1.0",
				"initial_state": "s",
				"states": {
					"s": {
						"directive": "do",
						"gates": {
							"g1": {"type": "command", "command": ""}
						}
					}
				}
			}`,
			wantErr: `state "s" gate "g1": command must not be empty`,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := ParseJSON([]byte(tt.json))
			if err == nil {
				t.Fatalf("ParseJSON() expected error")
			}
			if err.Error() != tt.wantErr {
				t.Errorf("error = %q, want %q", err.Error(), tt.wantErr)
			}
		})
	}
}

func TestBuildMachine_WithGates(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	m := ct.BuildMachine()

	if m.Name != "review-workflow" {
		t.Errorf("Machine.Name = %q, want %q", m.Name, "review-workflow")
	}
	if m.InitialState != "assess" {
		t.Errorf("Machine.InitialState = %q, want %q", m.InitialState, "assess")
	}
	if len(m.States) != 5 {
		t.Fatalf("Machine.States count = %d, want 5", len(m.States))
	}

	// Check gates on assess state.
	assessMS := m.States["assess"]
	if assessMS == nil {
		t.Fatal("Machine.States[assess] is nil")
	}
	if len(assessMS.Gates) != 1 {
		t.Fatalf("assess gates count = %d, want 1", len(assessMS.Gates))
	}
	hasPR := assessMS.Gates["has_pr"]
	if hasPR == nil {
		t.Fatal("assess gate has_pr is nil")
	}
	if hasPR.Type != "field_not_empty" {
		t.Errorf("has_pr.Type = %q, want %q", hasPR.Type, "field_not_empty")
	}
	if hasPR.Field != "PR_URL" {
		t.Errorf("has_pr.Field = %q, want %q", hasPR.Field, "PR_URL")
	}

	// Check gates on review state.
	reviewMS := m.States["review"]
	if reviewMS == nil {
		t.Fatal("Machine.States[review] is nil")
	}
	if len(reviewMS.Gates) != 2 {
		t.Fatalf("review gates count = %d, want 2", len(reviewMS.Gates))
	}
	lintGate := reviewMS.Gates["lint_pass"]
	if lintGate == nil {
		t.Fatal("review gate lint_pass is nil")
	}
	if lintGate.Type != "command" {
		t.Errorf("lint_pass.Type = %q, want %q", lintGate.Type, "command")
	}
	if lintGate.Command != "make lint" {
		t.Errorf("lint_pass.Command = %q, want %q", lintGate.Command, "make lint")
	}
	if lintGate.Timeout != 60 {
		t.Errorf("lint_pass.Timeout = %d, want 60", lintGate.Timeout)
	}

	// Terminal states should have no gates.
	approveMS := m.States["approve"]
	if !approveMS.Terminal {
		t.Error("approve.Terminal = false, want true")
	}
	if len(approveMS.Gates) != 0 {
		t.Errorf("approve gates count = %d, want 0", len(approveMS.Gates))
	}

	// DeclaredVars.
	if m.DeclaredVars == nil {
		t.Fatal("DeclaredVars is nil")
	}
	if !m.DeclaredVars["PR_URL"] {
		t.Error("DeclaredVars[PR_URL] = false, want true")
	}
	if !m.DeclaredVars["REVIEWER"] {
		t.Error("DeclaredVars[REVIEWER] = false, want true")
	}
	if len(m.DeclaredVars) != 2 {
		t.Errorf("DeclaredVars count = %d, want 2", len(m.DeclaredVars))
	}

	// Transitions.
	if len(assessMS.Transitions) != 2 || assessMS.Transitions[0] != "review" || assessMS.Transitions[1] != "reject" {
		t.Errorf("assess transitions = %v, want [review, reject]", assessMS.Transitions)
	}
}

func TestBuildMachine_NoVariables(t *testing.T) {
	data := []byte(`{
		"format_version": 1,
		"name": "simple",
		"version": "1.0",
		"initial_state": "start",
		"states": {
			"start": {
				"directive": "Begin.",
				"terminal": true
			}
		}
	}`)

	ct, err := ParseJSON(data)
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	m := ct.BuildMachine()
	if m.DeclaredVars != nil {
		t.Errorf("DeclaredVars = %v, want nil (no variables declared)", m.DeclaredVars)
	}
}

func TestEngineMachineDeepCopy_IncludesGates(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	machine := &engine.Machine{
		Name:         "gate-test",
		InitialState: "start",
		States: map[string]*engine.MachineState{
			"start": {
				Transitions: []string{"done"},
				Gates: map[string]*engine.GateDecl{
					"check": {
						Type:  "field_not_empty",
						Field: "TASK",
					},
				},
			},
			"done": {
				Terminal: true,
			},
		},
		DeclaredVars: map[string]bool{
			"TASK": true,
		},
	}

	eng, err := engine.Init(path, machine, engine.InitMeta{
		Name:         "gate-test-workflow",
		TemplateHash: "sha256:abc",
		TemplatePath: "/tmp/template.json",
	})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	// Get a copy of the machine and mutate it.
	copy1 := eng.Machine()

	// Mutate gates on the copy.
	copy1.States["start"].Gates["check"].Field = "TAMPERED"
	copy1.States["start"].Gates["injected"] = &engine.GateDecl{
		Type:    "command",
		Command: "echo evil",
	}

	// Mutate DeclaredVars on the copy.
	copy1.DeclaredVars["INJECTED"] = true

	// Get another copy and verify the original is untouched.
	copy2 := eng.Machine()

	// Gates field should be unchanged.
	startGate := copy2.States["start"].Gates["check"]
	if startGate == nil {
		t.Fatal("Gates[check] is nil in copy2")
	}
	if startGate.Field != "TASK" {
		t.Errorf("Gates[check].Field = %q, want %q (copy was not independent)", startGate.Field, "TASK")
	}

	// Injected gate should not exist.
	if _, exists := copy2.States["start"].Gates["injected"]; exists {
		t.Error("Gates[injected] exists in copy2, copy was not independent")
	}

	// DeclaredVars should be unchanged.
	if _, exists := copy2.DeclaredVars["INJECTED"]; exists {
		t.Error("DeclaredVars[INJECTED] exists in copy2, copy was not independent")
	}
	if !copy2.DeclaredVars["TASK"] {
		t.Error("DeclaredVars[TASK] = false in copy2, want true")
	}
}

func TestEngineMachineDeepCopy_NilGatesAndDeclaredVars(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "koto-test.state.json")

	// Machine with no gates and no declared vars (the common case
	// from v1 markdown templates).
	machine := &engine.Machine{
		Name:         "simple",
		InitialState: "start",
		States: map[string]*engine.MachineState{
			"start": {
				Transitions: []string{"done"},
			},
			"done": {
				Terminal: true,
			},
		},
	}

	eng, err := engine.Init(path, machine, engine.InitMeta{Name: "simple"})
	if err != nil {
		t.Fatalf("Init() error: %v", err)
	}

	m := eng.Machine()
	if m.DeclaredVars != nil {
		t.Errorf("DeclaredVars = %v, want nil", m.DeclaredVars)
	}
	if m.States["start"].Gates != nil {
		t.Errorf("States[start].Gates = %v, want nil", m.States["start"].Gates)
	}
}

func TestParseJSON_RoundTrip(t *testing.T) {
	// Parse, marshal, and parse again to verify JSON round-trip.
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() first pass error: %v", err)
	}

	data, err := json.MarshalIndent(ct, "", "  ")
	if err != nil {
		t.Fatalf("MarshalIndent() error: %v", err)
	}

	ct2, err := ParseJSON(data)
	if err != nil {
		t.Fatalf("ParseJSON() second pass error: %v", err)
	}

	if ct2.Name != ct.Name {
		t.Errorf("round-trip Name = %q, want %q", ct2.Name, ct.Name)
	}
	if ct2.InitialState != ct.InitialState {
		t.Errorf("round-trip InitialState = %q, want %q", ct2.InitialState, ct.InitialState)
	}
	if len(ct2.States) != len(ct.States) {
		t.Errorf("round-trip States count = %d, want %d", len(ct2.States), len(ct.States))
	}
}

func TestToTemplate_PopulatesSections(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	tmpl, err := ct.ToTemplate()
	if err != nil {
		t.Fatalf("ToTemplate() error: %v", err)
	}

	// Sections should map state names to directive text.
	if len(tmpl.Sections) != len(ct.States) {
		t.Fatalf("Sections count = %d, want %d", len(tmpl.Sections), len(ct.States))
	}

	for name, sd := range ct.States {
		got, ok := tmpl.Sections[name]
		if !ok {
			t.Errorf("Sections[%q] not found", name)
			continue
		}
		if got != sd.Directive {
			t.Errorf("Sections[%q] = %q, want %q", name, got, sd.Directive)
		}
	}
}

func TestToTemplate_PopulatesVariablesFromDefaults(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	tmpl, err := ct.ToTemplate()
	if err != nil {
		t.Fatalf("ToTemplate() error: %v", err)
	}

	// Variables should contain default values from VariableDecl.
	if len(tmpl.Variables) != len(ct.Variables) {
		t.Fatalf("Variables count = %d, want %d", len(tmpl.Variables), len(ct.Variables))
	}

	// PR_URL has no default (empty string).
	if tmpl.Variables["PR_URL"] != "" {
		t.Errorf("Variables[PR_URL] = %q, want empty", tmpl.Variables["PR_URL"])
	}

	// REVIEWER has default "auto".
	if tmpl.Variables["REVIEWER"] != "auto" {
		t.Errorf("Variables[REVIEWER] = %q, want %q", tmpl.Variables["REVIEWER"], "auto")
	}
}

func TestToTemplate_BuildsMachine(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	tmpl, err := ct.ToTemplate()
	if err != nil {
		t.Fatalf("ToTemplate() error: %v", err)
	}

	if tmpl.Machine == nil {
		t.Fatal("Machine is nil")
	}
	if tmpl.Machine.Name != ct.Name {
		t.Errorf("Machine.Name = %q, want %q", tmpl.Machine.Name, ct.Name)
	}
	if tmpl.Machine.InitialState != ct.InitialState {
		t.Errorf("Machine.InitialState = %q, want %q", tmpl.Machine.InitialState, ct.InitialState)
	}
	if len(tmpl.Machine.States) != len(ct.States) {
		t.Errorf("Machine.States count = %d, want %d", len(tmpl.Machine.States), len(ct.States))
	}
}

func TestToTemplate_Metadata(t *testing.T) {
	ct, err := ParseJSON([]byte(validCompiledJSON))
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	tmpl, err := ct.ToTemplate()
	if err != nil {
		t.Fatalf("ToTemplate() error: %v", err)
	}

	if tmpl.Name != "review-workflow" {
		t.Errorf("Name = %q, want %q", tmpl.Name, "review-workflow")
	}
	if tmpl.Version != "2.0" {
		t.Errorf("Version = %q, want %q", tmpl.Version, "2.0")
	}
	if tmpl.Description != "A code review workflow" {
		t.Errorf("Description = %q, want %q", tmpl.Description, "A code review workflow")
	}

	// Hash and Path are empty; caller sets them.
	if tmpl.Hash != "" {
		t.Errorf("Hash = %q, want empty (caller sets it)", tmpl.Hash)
	}
	if tmpl.Path != "" {
		t.Errorf("Path = %q, want empty (caller sets it)", tmpl.Path)
	}
}

func TestToTemplate_NoVariables(t *testing.T) {
	data := []byte(`{
		"format_version": 1,
		"name": "simple",
		"version": "1.0",
		"initial_state": "start",
		"states": {
			"start": {
				"directive": "Begin.",
				"terminal": true
			}
		}
	}`)

	ct, err := ParseJSON(data)
	if err != nil {
		t.Fatalf("ParseJSON() error: %v", err)
	}

	tmpl, err := ct.ToTemplate()
	if err != nil {
		t.Fatalf("ToTemplate() error: %v", err)
	}

	if tmpl.Variables == nil {
		t.Fatal("Variables is nil, want empty map")
	}
	if len(tmpl.Variables) != 0 {
		t.Errorf("Variables count = %d, want 0", len(tmpl.Variables))
	}
}

func TestParseJSON_FormatVersionZero(t *testing.T) {
	// format_version defaults to 0 when omitted from JSON (Go zero value).
	data := []byte(`{
		"name": "test",
		"version": "1.0",
		"initial_state": "start",
		"states": {"start": {"directive": "go"}}
	}`)

	_, err := ParseJSON(data)
	if err == nil {
		t.Fatal("ParseJSON() expected error for format_version 0")
	}
	if !strings.Contains(err.Error(), "unsupported format version: 0") {
		t.Errorf("error = %q, want substring %q", err.Error(), "unsupported format version: 0")
	}
}
