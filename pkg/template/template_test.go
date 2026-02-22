package template

import (
	"crypto/sha256"
	"encoding/hex"
	"os"
	"path/filepath"
	"testing"
)

const validTemplate = `---
name: example-workflow
version: "1.0"
description: An example workflow
variables:
  TASK: ""
  REVIEWER: ""
---

## assess

Assess the task at hand. Review {{TASK}} and determine the approach.

**Transitions**: [plan]

## plan

Create an implementation plan for {{TASK}}.

**Transitions**: [implement]

## implement

Execute the plan. Build the solution.

**Transitions**: [done]

## done

Work is complete.
`

func writeTemplate(t *testing.T, dir, content string) string {
	t.Helper()
	path := filepath.Join(dir, "template.md")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}
	return path
}

func TestParse_ValidTemplate(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, validTemplate)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	// Check metadata.
	if tmpl.Name != "example-workflow" {
		t.Errorf("Name = %q, want %q", tmpl.Name, "example-workflow")
	}
	if tmpl.Version != "1.0" {
		t.Errorf("Version = %q, want %q", tmpl.Version, "1.0")
	}
	if tmpl.Description != "An example workflow" {
		t.Errorf("Description = %q, want %q", tmpl.Description, "An example workflow")
	}
	if tmpl.Path != path {
		t.Errorf("Path = %q, want %q", tmpl.Path, path)
	}

	// Check variables.
	if len(tmpl.Variables) != 2 {
		t.Fatalf("Variables count = %d, want 2", len(tmpl.Variables))
	}
	if tmpl.Variables["TASK"] != "" {
		t.Errorf("Variables[TASK] = %q, want empty", tmpl.Variables["TASK"])
	}
	if tmpl.Variables["REVIEWER"] != "" {
		t.Errorf("Variables[REVIEWER] = %q, want empty", tmpl.Variables["REVIEWER"])
	}

	// Check Machine.
	if tmpl.Machine == nil {
		t.Fatal("Machine is nil")
	}
	if tmpl.Machine.Name != "example-workflow" {
		t.Errorf("Machine.Name = %q, want %q", tmpl.Machine.Name, "example-workflow")
	}
	if tmpl.Machine.InitialState != "assess" {
		t.Errorf("Machine.InitialState = %q, want %q", tmpl.Machine.InitialState, "assess")
	}
	if len(tmpl.Machine.States) != 4 {
		t.Fatalf("Machine.States count = %d, want 4", len(tmpl.Machine.States))
	}

	// Check state transitions.
	assessState := tmpl.Machine.States["assess"]
	if assessState == nil {
		t.Fatal("Machine.States[assess] is nil")
	}
	if len(assessState.Transitions) != 1 || assessState.Transitions[0] != "plan" {
		t.Errorf("assess transitions = %v, want [plan]", assessState.Transitions)
	}
	if assessState.Terminal {
		t.Error("assess should not be terminal")
	}

	planState := tmpl.Machine.States["plan"]
	if planState == nil {
		t.Fatal("Machine.States[plan] is nil")
	}
	if len(planState.Transitions) != 1 || planState.Transitions[0] != "implement" {
		t.Errorf("plan transitions = %v, want [implement]", planState.Transitions)
	}

	implementState := tmpl.Machine.States["implement"]
	if implementState == nil {
		t.Fatal("Machine.States[implement] is nil")
	}
	if len(implementState.Transitions) != 1 || implementState.Transitions[0] != "done" {
		t.Errorf("implement transitions = %v, want [done]", implementState.Transitions)
	}

	// Check terminal state.
	doneState := tmpl.Machine.States["done"]
	if doneState == nil {
		t.Fatal("Machine.States[done] is nil")
	}
	if !doneState.Terminal {
		t.Error("done should be terminal")
	}
	if len(doneState.Transitions) != 0 {
		t.Errorf("done transitions = %v, want empty", doneState.Transitions)
	}

	// Check sections.
	if len(tmpl.Sections) != 4 {
		t.Fatalf("Sections count = %d, want 4", len(tmpl.Sections))
	}
	if got := tmpl.Sections["assess"]; got != "Assess the task at hand. Review {{TASK}} and determine the approach." {
		t.Errorf("Sections[assess] = %q", got)
	}
	if got := tmpl.Sections["done"]; got != "Work is complete." {
		t.Errorf("Sections[done] = %q, want %q", got, "Work is complete.")
	}
}

func TestParse_InvalidTemplate_MissingFrontMatter(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `## state1

Content.
`)

	_, err := Parse(path)
	if err == nil {
		t.Fatal("Parse() expected error for missing front-matter")
	}
}

func TestParse_InvalidTemplate_NoClosingDelimiter(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: broken
`)

	_, err := Parse(path)
	if err == nil {
		t.Fatal("Parse() expected error for missing closing delimiter")
	}
}

func TestParse_InvalidTemplate_NoStates(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: empty
---

Just some text, no state headings.
`)

	_, err := Parse(path)
	if err == nil {
		t.Fatal("Parse() expected error for template with no states")
	}
}

func TestParse_InvalidTemplate_UndefinedTransitionTarget(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: bad-ref
---

## start

Starting state.

**Transitions**: [nonexistent]
`)

	_, err := Parse(path)
	if err == nil {
		t.Fatal("Parse() expected error for undefined transition target")
	}
}

func TestParse_InvalidTemplate_FileNotFound(t *testing.T) {
	_, err := Parse("/nonexistent/path/template.md")
	if err == nil {
		t.Fatal("Parse() expected error for nonexistent file")
	}
}

func TestParse_MultipleTransitionTargets(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: branching
---

## start

Choose a path.

**Transitions**: [path-a, path-b]

## path-a

Path A content.

**Transitions**: [done]

## path-b

Path B content.

**Transitions**: [done]

## done

Finished.
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	startState := tmpl.Machine.States["start"]
	if len(startState.Transitions) != 2 {
		t.Fatalf("start transitions count = %d, want 2", len(startState.Transitions))
	}
	if startState.Transitions[0] != "path-a" || startState.Transitions[1] != "path-b" {
		t.Errorf("start transitions = %v, want [path-a, path-b]", startState.Transitions)
	}
}

func TestParse_HashDeterminism(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, validTemplate)

	tmpl1, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() #1 error: %v", err)
	}

	tmpl2, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() #2 error: %v", err)
	}

	if tmpl1.Hash != tmpl2.Hash {
		t.Errorf("hash mismatch: %q vs %q", tmpl1.Hash, tmpl2.Hash)
	}

	// Verify the hash format.
	if len(tmpl1.Hash) < 7 || tmpl1.Hash[:7] != "sha256:" {
		t.Errorf("Hash = %q, want sha256:<hex> prefix", tmpl1.Hash)
	}

	// Verify against manual computation.
	data, _ := os.ReadFile(path) //nolint:gosec // G304: test reads file it created
	sum := sha256.Sum256(data)
	expected := "sha256:" + hex.EncodeToString(sum[:])
	if tmpl1.Hash != expected {
		t.Errorf("Hash = %q, want %q", tmpl1.Hash, expected)
	}
}

func TestParse_HashChangesWithContent(t *testing.T) {
	dir := t.TempDir()

	path1 := filepath.Join(dir, "template1.md")
	if err := os.WriteFile(path1, []byte(validTemplate), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	// Write a slightly different template.
	path2 := filepath.Join(dir, "template2.md")
	modified := validTemplate + "\n## extra\n\nExtra state.\n"
	// Need the extra state to be reachable or terminal, and not break parse.
	modified = `---
name: modified
---

## start

A state.

**Transitions**: [done]

## done

Done.
`
	if err := os.WriteFile(path2, []byte(modified), 0o600); err != nil {
		t.Fatalf("WriteFile() error: %v", err)
	}

	tmpl1, err := Parse(path1)
	if err != nil {
		t.Fatalf("Parse(template1) error: %v", err)
	}

	tmpl2, err := Parse(path2)
	if err != nil {
		t.Fatalf("Parse(template2) error: %v", err)
	}

	if tmpl1.Hash == tmpl2.Hash {
		t.Error("different templates produced the same hash")
	}
}

func TestParse_VariablesWithValues(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: vars-test
variables:
  NAME: default-name
  COUNT: "42"
---

## start

Hello {{NAME}}.

**Transitions**: [done]

## done

Done.
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	if tmpl.Variables["NAME"] != "default-name" {
		t.Errorf("Variables[NAME] = %q, want %q", tmpl.Variables["NAME"], "default-name")
	}
	if tmpl.Variables["COUNT"] != "42" {
		t.Errorf("Variables[COUNT] = %q, want %q", tmpl.Variables["COUNT"], "42")
	}
}

func TestParse_EmptyTransitionsList(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: empty-transitions
---

## start

A state.

**Transitions**: []
`)

	_, err := Parse(path)
	if err == nil {
		t.Fatal("Parse() expected error for empty transitions list")
	}
}

func TestParse_InitialStateIsFirst(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: order-test
---

## alpha

First state.

**Transitions**: [beta]

## beta

Second state.

**Transitions**: [gamma]

## gamma

Third state (terminal).
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	if tmpl.Machine.InitialState != "alpha" {
		t.Errorf("InitialState = %q, want %q", tmpl.Machine.InitialState, "alpha")
	}
}

func TestInterpolate_AllKeysPresent(t *testing.T) {
	text := "Hello {{NAME}}, your task is {{TASK}}."
	ctx := map[string]string{
		"NAME": "agent",
		"TASK": "refactoring",
	}

	got := Interpolate(text, ctx)
	want := "Hello agent, your task is refactoring."
	if got != want {
		t.Errorf("Interpolate() = %q, want %q", got, want)
	}
}

func TestInterpolate_MissingKeys(t *testing.T) {
	text := "Hello {{NAME}}, task: {{TASK}}"
	ctx := map[string]string{
		"NAME": "agent",
	}

	got := Interpolate(text, ctx)
	want := "Hello agent, task: {{TASK}}"
	if got != want {
		t.Errorf("Interpolate() = %q, want %q", got, want)
	}
}

func TestInterpolate_NoPlaceholders(t *testing.T) {
	text := "No placeholders here."
	ctx := map[string]string{"KEY": "value"}

	got := Interpolate(text, ctx)
	if got != text {
		t.Errorf("Interpolate() = %q, want %q", got, text)
	}
}

func TestInterpolate_EmptyContext(t *testing.T) {
	text := "Hello {{NAME}}."
	ctx := map[string]string{}

	got := Interpolate(text, ctx)
	if got != text {
		t.Errorf("Interpolate() = %q, want %q (unresolved should be preserved)", got, text)
	}
}

func TestInterpolate_NilContext(t *testing.T) {
	text := "Hello {{NAME}}."

	got := Interpolate(text, nil)
	if got != text {
		t.Errorf("Interpolate() = %q, want %q", got, text)
	}
}

func TestInterpolate_EmptyString(t *testing.T) {
	got := Interpolate("", map[string]string{"KEY": "value"})
	if got != "" {
		t.Errorf("Interpolate() = %q, want empty", got)
	}
}

func TestInterpolate_AdjacentPlaceholders(t *testing.T) {
	text := "{{A}}{{B}}{{C}}"
	ctx := map[string]string{"A": "1", "B": "2", "C": "3"}

	got := Interpolate(text, ctx)
	want := "123"
	if got != want {
		t.Errorf("Interpolate() = %q, want %q", got, want)
	}
}

func TestInterpolate_SinglePass(t *testing.T) {
	// If KEY's value contains another placeholder, it should not be expanded.
	text := "Value: {{KEY}}"
	ctx := map[string]string{
		"KEY":    "{{NESTED}}",
		"NESTED": "should-not-appear",
	}

	got := Interpolate(text, ctx)
	want := "Value: {{NESTED}}"
	if got != want {
		t.Errorf("Interpolate() = %q, want %q (should be single-pass)", got, want)
	}
}

func TestInterpolate_UnclosedBraces(t *testing.T) {
	text := "Hello {{NAME, goodbye."
	ctx := map[string]string{"NAME": "agent"}

	got := Interpolate(text, ctx)
	// Unclosed braces are left as-is.
	if got != text {
		t.Errorf("Interpolate() = %q, want %q", got, text)
	}
}

func TestParse_MachineNameFromHeader(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: my-workflow
---

## start

Starting.

**Transitions**: [end]

## end

Done.
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	if tmpl.Machine.Name != "my-workflow" {
		t.Errorf("Machine.Name = %q, want %q", tmpl.Machine.Name, "my-workflow")
	}
}

func TestParse_NoVariablesInHeader(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: no-vars
---

## start

Starting.

**Transitions**: [done]

## done

Done.
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	if tmpl.Variables == nil {
		t.Fatal("Variables is nil, want empty map")
	}
	if len(tmpl.Variables) != 0 {
		t.Errorf("Variables count = %d, want 0", len(tmpl.Variables))
	}
}

func TestParse_SectionContentExcludesTransitionsLine(t *testing.T) {
	dir := t.TempDir()
	path := writeTemplate(t, dir, `---
name: section-test
---

## work

Do some work here.
More details about the work.

**Transitions**: [done]

## done

Finished.
`)

	tmpl, err := Parse(path)
	if err != nil {
		t.Fatalf("Parse() error: %v", err)
	}

	section := tmpl.Sections["work"]
	if section == "" {
		t.Fatal("Sections[work] is empty")
	}

	// Section should not contain the transitions line.
	if contains(section, "**Transitions**") {
		t.Errorf("Sections[work] contains transitions line: %q", section)
	}

	// Section should contain the directive content.
	if !contains(section, "Do some work here.") {
		t.Errorf("Sections[work] missing directive content: %q", section)
	}
	if !contains(section, "More details about the work.") {
		t.Errorf("Sections[work] missing second line: %q", section)
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsSubstring(s, substr))
}

func containsSubstring(s, sub string) bool {
	for i := 0; i <= len(s)-len(sub); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}
