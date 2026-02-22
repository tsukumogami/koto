package compile

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/tsukumogami/koto/pkg/template"
)

// scenario9Source is a full source format template used for the basic
// compilation test. It declares variables with metadata, states with
// transitions/gates, and provides markdown body sections for each state.
const scenario9Source = `---
name: quick-task
version: "1.0"
description: A focused task workflow
initial_state: assess

variables:
  TASK:
    description: What to build
    required: true
  REPO:
    description: Repository path
    default: "."

states:
  assess:
    transitions: [plan, escalate]
    gates:
      task_defined:
        type: field_not_empty
        field: TASK
  plan:
    transitions: [implement]
  implement:
    transitions: [done]
    gates:
      tests_pass:
        type: command
        command: go test ./...
        timeout: 120
  escalate:
    terminal: true
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

Review the codebase in {{REPO}} and determine:
- What files need to change
- How complex the change is
- Whether tests exist for the affected code

## plan

Create an implementation plan for {{TASK}}.

Break the work into steps. Identify tests to write.

## implement

Execute the plan. Write code and tests.

## done

Work is complete.

## escalate

Task could not be completed in this workflow.
`

// TestCompile_ValidSource (scenario 9) verifies that a well-formed source
// template compiles to a valid CompiledTemplate that passes ParseJSON
// round-trip validation.
func TestCompile_ValidSource(t *testing.T) {
	ct, _, err := Compile([]byte(scenario9Source))
	if err != nil {
		t.Fatalf("Compile() error: %v", err)
	}

	// format_version must be 1.
	if ct.FormatVersion != 1 {
		t.Errorf("FormatVersion = %d, want 1", ct.FormatVersion)
	}

	// Metadata.
	if ct.Name != "quick-task" {
		t.Errorf("Name = %q, want %q", ct.Name, "quick-task")
	}
	if ct.Version != "1.0" {
		t.Errorf("Version = %q, want %q", ct.Version, "1.0")
	}
	if ct.Description != "A focused task workflow" {
		t.Errorf("Description = %q, want %q", ct.Description, "A focused task workflow")
	}
	if ct.InitialState != "assess" {
		t.Errorf("InitialState = %q, want %q", ct.InitialState, "assess")
	}

	// Variables.
	if len(ct.Variables) != 2 {
		t.Fatalf("Variables count = %d, want 2", len(ct.Variables))
	}
	taskVar := ct.Variables["TASK"]
	if taskVar.Description != "What to build" {
		t.Errorf("Variables[TASK].Description = %q, want %q", taskVar.Description, "What to build")
	}
	if !taskVar.Required {
		t.Error("Variables[TASK].Required = false, want true")
	}
	repoVar := ct.Variables["REPO"]
	if repoVar.Default != "." {
		t.Errorf("Variables[REPO].Default = %q, want %q", repoVar.Default, ".")
	}

	// States.
	if len(ct.States) != 5 {
		t.Fatalf("States count = %d, want 5", len(ct.States))
	}

	// Check assess state.
	assess := ct.States["assess"]
	if !strings.Contains(assess.Directive, "Analyze the task: {{TASK}}") {
		t.Errorf("States[assess].Directive does not contain expected text: %q", assess.Directive)
	}
	if len(assess.Transitions) != 2 || assess.Transitions[0] != "plan" || assess.Transitions[1] != "escalate" {
		t.Errorf("States[assess].Transitions = %v, want [plan, escalate]", assess.Transitions)
	}
	if len(assess.Gates) != 1 {
		t.Fatalf("States[assess].Gates count = %d, want 1", len(assess.Gates))
	}
	taskGate := assess.Gates["task_defined"]
	if taskGate.Type != "field_not_empty" {
		t.Errorf("Gates[task_defined].Type = %q, want %q", taskGate.Type, "field_not_empty")
	}
	if taskGate.Field != "TASK" {
		t.Errorf("Gates[task_defined].Field = %q, want %q", taskGate.Field, "TASK")
	}

	// Check terminal states.
	if !ct.States["done"].Terminal {
		t.Error("States[done].Terminal = false, want true")
	}
	if !ct.States["escalate"].Terminal {
		t.Error("States[escalate].Terminal = false, want true")
	}

	// Check command gate with timeout.
	implGate := ct.States["implement"].Gates["tests_pass"]
	if implGate.Type != "command" {
		t.Errorf("Gates[tests_pass].Type = %q, want %q", implGate.Type, "command")
	}
	if implGate.Command != "go test ./..." {
		t.Errorf("Gates[tests_pass].Command = %q, want %q", implGate.Command, "go test ./...")
	}
	if implGate.Timeout != 120 {
		t.Errorf("Gates[tests_pass].Timeout = %d, want 120", implGate.Timeout)
	}

	// Directives should have leading/trailing whitespace trimmed.
	for name, sd := range ct.States {
		if sd.Directive != strings.TrimSpace(sd.Directive) {
			t.Errorf("States[%s].Directive has untrimmed whitespace", name)
		}
	}

	// Round-trip: serialize to JSON and parse back through template.ParseJSON.
	data, err := json.MarshalIndent(ct, "", "  ")
	if err != nil {
		t.Fatalf("MarshalIndent() error: %v", err)
	}

	ct2, err := template.ParseJSON(data)
	if err != nil {
		t.Fatalf("ParseJSON() round-trip error: %v", err)
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

// TestCompile_SubheadingNotBoundary (scenario 10) verifies that subheadings
// like ### Decision Criteria inside a state are treated as directive content,
// not state boundaries.
func TestCompile_SubheadingNotBoundary(t *testing.T) {
	source := `---
name: subheading-test
version: "1.0"
initial_state: assess

states:
  assess:
    transitions: [done]
  done:
    terminal: true
---

## assess

Analyze the situation.

### Decision Criteria

Use these criteria to evaluate:
- Quality
- Performance

## done

Work is complete.
`

	ct, _, err := Compile([]byte(source))
	if err != nil {
		t.Fatalf("Compile() error: %v", err)
	}

	assess := ct.States["assess"]

	// The ### heading must be part of the assess directive content.
	if !strings.Contains(assess.Directive, "### Decision Criteria") {
		t.Errorf("States[assess].Directive should contain '### Decision Criteria', got: %q", assess.Directive)
	}
	if !strings.Contains(assess.Directive, "Use these criteria to evaluate:") {
		t.Errorf("States[assess].Directive should contain criteria text, got: %q", assess.Directive)
	}

	// Should only have 2 states, not 3.
	if len(ct.States) != 2 {
		t.Errorf("States count = %d, want 2", len(ct.States))
	}
}

// TestCompile_HeadingCollisionWarning (scenario 11) verifies that when a
// state's directive area contains a ## heading matching another declared
// state, the compiler emits a warning but still succeeds.
func TestCompile_HeadingCollisionWarning(t *testing.T) {
	// In this template, the assess state's body contains a ## plan line.
	// Since "plan" is a declared state, the ## plan heading acts as a
	// state boundary, ending the assess section earlier than the author
	// may have intended. The compiler warns about this.
	source := `---
name: collision-test
version: "1.0"
initial_state: assess

states:
  assess:
    transitions: [plan]
  plan:
    transitions: [done]
  done:
    terminal: true
---

## assess

First part of assess.

## plan

Create an implementation plan.

## done

Work is complete.
`

	ct, warnings, err := Compile([]byte(source))
	if err != nil {
		t.Fatalf("Compile() error: %v", err)
	}

	// Compilation should succeed.
	if ct == nil {
		t.Fatal("Compile() returned nil template")
	}

	// Should have at least one warning about the heading collision.
	found := false
	for _, w := range warnings {
		if strings.Contains(w.Message, "state \"assess\" directive contains ## heading matching state \"plan\"") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("expected warning about assess/plan heading collision, got warnings: %v", warnings)
	}

	// The assess directive should only contain content before ## plan.
	assess := ct.States["assess"]
	if !strings.Contains(assess.Directive, "First part of assess.") {
		t.Errorf("States[assess].Directive should contain 'First part of assess.', got: %q", assess.Directive)
	}

	// The plan directive should contain its content.
	plan := ct.States["plan"]
	if !strings.Contains(plan.Directive, "Create an implementation plan.") {
		t.Errorf("States[plan].Directive should contain 'Create an implementation plan.', got: %q", plan.Directive)
	}
}

// TestCompile_MissingHeading (scenario 12) verifies that compilation fails
// when a declared state has no matching ## heading in the body.
func TestCompile_MissingHeading(t *testing.T) {
	source := `---
name: missing-test
version: "1.0"
initial_state: assess

states:
  assess:
    transitions: [verify]
  verify:
    terminal: true
---

## assess

Analyze the situation.
`

	_, _, err := Compile([]byte(source))
	if err == nil {
		t.Fatal("Compile() expected error for missing ## verify heading")
	}
	if !strings.Contains(err.Error(), "verify") {
		t.Errorf("error should mention 'verify', got: %q", err.Error())
	}
	if !strings.Contains(err.Error(), "no matching") {
		t.Errorf("error should mention 'no matching', got: %q", err.Error())
	}
}

// TestCompile_DeterministicOutput (scenario 13) verifies that compiling
// the same source twice produces byte-identical JSON and SHA-256 hashes.
func TestCompile_DeterministicOutput(t *testing.T) {
	ct1, _, err := Compile([]byte(scenario9Source))
	if err != nil {
		t.Fatalf("Compile() #1 error: %v", err)
	}

	ct2, _, err := Compile([]byte(scenario9Source))
	if err != nil {
		t.Fatalf("Compile() #2 error: %v", err)
	}

	hash1, data1, err := Hash(ct1)
	if err != nil {
		t.Fatalf("Hash() #1 error: %v", err)
	}

	hash2, data2, err := Hash(ct2)
	if err != nil {
		t.Fatalf("Hash() #2 error: %v", err)
	}

	// JSON bytes must be identical.
	if string(data1) != string(data2) {
		t.Error("compiled JSON is not byte-identical across compilations")
	}

	// Hashes must match.
	if hash1 != hash2 {
		t.Errorf("hash mismatch: %q vs %q", hash1, hash2)
	}

	// Hash must have correct format.
	if !strings.HasPrefix(hash1, "sha256:") {
		t.Errorf("hash should start with 'sha256:', got: %q", hash1)
	}
	if len(hash1) != len("sha256:")+64 {
		t.Errorf("hash length = %d, want %d (sha256: prefix + 64 hex chars)", len(hash1), len("sha256:")+64)
	}
}

// TestCompile_NonStateHeadingIsContent verifies that a ## heading that
// does NOT match any declared state is treated as directive content.
func TestCompile_NonStateHeadingIsContent(t *testing.T) {
	source := `---
name: heading-test
version: "1.0"
initial_state: work

states:
  work:
    transitions: [done]
  done:
    terminal: true
---

## work

Do the work.

## Background

This is background info that is NOT a declared state.

More work content.

## done

Finished.
`

	ct, _, err := Compile([]byte(source))
	if err != nil {
		t.Fatalf("Compile() error: %v", err)
	}

	work := ct.States["work"]
	if !strings.Contains(work.Directive, "## Background") {
		t.Errorf("States[work].Directive should contain '## Background', got: %q", work.Directive)
	}
	if !strings.Contains(work.Directive, "This is background info") {
		t.Errorf("States[work].Directive should contain background text, got: %q", work.Directive)
	}
	if !strings.Contains(work.Directive, "More work content.") {
		t.Errorf("States[work].Directive should contain 'More work content.', got: %q", work.Directive)
	}
}

// TestCompile_MissingFrontmatter verifies error on missing frontmatter.
func TestCompile_MissingFrontmatter(t *testing.T) {
	source := `## state1

Content.
`
	_, _, err := Compile([]byte(source))
	if err == nil {
		t.Fatal("Compile() expected error for missing frontmatter")
	}
}

// TestCompile_MissingRequiredFields verifies errors for missing fields.
func TestCompile_MissingRequiredFields(t *testing.T) {
	tests := []struct {
		name    string
		source  string
		wantErr string
	}{
		{
			name: "missing name",
			source: `---
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
---

## start

Content.
`,
			wantErr: "missing required field: name",
		},
		{
			name: "missing version",
			source: `---
name: test
initial_state: start
states:
  start:
    terminal: true
---

## start

Content.
`,
			wantErr: "missing required field: version",
		},
		{
			name: "missing initial_state",
			source: `---
name: test
version: "1.0"
states:
  start:
    terminal: true
---

## start

Content.
`,
			wantErr: "missing required field: initial_state",
		},
		{
			name: "no states declared",
			source: `---
name: test
version: "1.0"
initial_state: start
---

## start

Content.
`,
			wantErr: "template has no states",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, _, err := Compile([]byte(tt.source))
			if err == nil {
				t.Fatal("Compile() expected error")
			}
			if err.Error() != tt.wantErr {
				t.Errorf("error = %q, want %q", err.Error(), tt.wantErr)
			}
		})
	}
}

// TestCompile_InitialStateNotDeclared verifies error when initial_state
// references a state not in the states map.
func TestCompile_InitialStateNotDeclared(t *testing.T) {
	source := `---
name: test
version: "1.0"
initial_state: missing
states:
  start:
    terminal: true
---

## start

Content.
`
	_, _, err := Compile([]byte(source))
	if err == nil {
		t.Fatal("Compile() expected error for undeclared initial_state")
	}
	if !strings.Contains(err.Error(), "initial_state") {
		t.Errorf("error should mention initial_state, got: %q", err.Error())
	}
}

// TestHash_Determinism verifies that Hash produces the same output
// for the same input across multiple calls.
func TestHash_Determinism(t *testing.T) {
	ct := &template.CompiledTemplate{
		FormatVersion: 1,
		Name:          "test",
		Version:       "1.0",
		InitialState:  "start",
		States: map[string]template.StateDecl{
			"start": {
				Directive: "Begin.",
				Terminal:  true,
			},
		},
	}

	hash1, data1, err := Hash(ct)
	if err != nil {
		t.Fatalf("Hash() #1 error: %v", err)
	}

	hash2, data2, err := Hash(ct)
	if err != nil {
		t.Fatalf("Hash() #2 error: %v", err)
	}

	if hash1 != hash2 {
		t.Errorf("hash not deterministic: %q vs %q", hash1, hash2)
	}
	if string(data1) != string(data2) {
		t.Error("JSON bytes not deterministic")
	}
}

// TestCompile_WarningStringMethod verifies Warning.String().
func TestCompile_WarningStringMethod(t *testing.T) {
	w := Warning{Message: "test warning"}
	if w.String() != "test warning" {
		t.Errorf("Warning.String() = %q, want %q", w.String(), "test warning")
	}
}
