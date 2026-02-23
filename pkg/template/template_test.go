package template

import (
	"strings"
	"testing"
)

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

func TestSplitFrontMatter_Valid(t *testing.T) {
	content := `---
name: test
version: "1.0"
---

## state1

Content.
`
	header, body, err := SplitFrontMatter(content)
	if err != nil {
		t.Fatalf("SplitFrontMatter() error: %v", err)
	}

	if !strings.Contains(header, "name: test") {
		t.Errorf("header = %q, want it to contain 'name: test'", header)
	}
	if !strings.Contains(body, "## state1") {
		t.Errorf("body = %q, want it to contain '## state1'", body)
	}
}

func TestSplitFrontMatter_MissingOpeningDelimiter(t *testing.T) {
	_, _, err := SplitFrontMatter("no front matter here")
	if err == nil {
		t.Fatal("SplitFrontMatter() expected error for missing opening delimiter")
	}
}

func TestSplitFrontMatter_MissingClosingDelimiter(t *testing.T) {
	_, _, err := SplitFrontMatter("---\nname: broken\n")
	if err == nil {
		t.Fatal("SplitFrontMatter() expected error for missing closing delimiter")
	}
}

func TestSplitFrontMatter_EmptyHeader(t *testing.T) {
	content := "---\n---\n\nbody content"
	header, body, err := SplitFrontMatter(content)
	if err != nil {
		t.Fatalf("SplitFrontMatter() error: %v", err)
	}

	if strings.TrimSpace(header) != "" {
		t.Errorf("header = %q, want empty", header)
	}
	if !strings.Contains(body, "body content") {
		t.Errorf("body = %q, want it to contain 'body content'", body)
	}
}

func TestSplitFrontMatter_LeadingWhitespace(t *testing.T) {
	content := "\n\n  ---\nname: test\n---\n\nbody"
	header, body, err := SplitFrontMatter(content)
	if err != nil {
		t.Fatalf("SplitFrontMatter() error: %v", err)
	}

	if !strings.Contains(header, "name: test") {
		t.Errorf("header = %q, want it to contain 'name: test'", header)
	}
	if !strings.Contains(body, "body") {
		t.Errorf("body = %q, want it to contain 'body'", body)
	}
}
