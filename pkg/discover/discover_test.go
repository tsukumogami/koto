package discover

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// writeStateFile creates a minimal koto state file in dir with the given
// workflow name and current state.
func writeStateFile(t *testing.T, dir, name, currentState string) string {
	t.Helper()

	state := map[string]interface{}{
		"schema_version": 1,
		"workflow": map[string]string{
			"name":          name,
			"template_hash": "sha256:abc123",
			"template_path": "/tmp/template.md",
			"created_at":    "2026-02-22T12:00:00Z",
		},
		"version":       1,
		"current_state": currentState,
		"variables":     map[string]string{},
		"history":       []interface{}{},
	}

	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		t.Fatalf("marshal state file: %v", err)
	}

	path := filepath.Join(dir, "koto-"+name+".state.json")
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("write state file: %v", err)
	}
	return path
}

func TestFind_EmptyDirectory(t *testing.T) {
	dir := t.TempDir()

	workflows, err := Find(dir)
	if err != nil {
		t.Fatalf("Find() error: %v", err)
	}

	if workflows == nil {
		t.Fatal("Find() returned nil, want empty slice")
	}
	if len(workflows) != 0 {
		t.Errorf("Find() returned %d workflows, want 0", len(workflows))
	}
}

func TestFind_SingleFile(t *testing.T) {
	dir := t.TempDir()
	expectedPath := writeStateFile(t, dir, "my-workflow", "implementing")

	workflows, err := Find(dir)
	if err != nil {
		t.Fatalf("Find() error: %v", err)
	}

	if len(workflows) != 1 {
		t.Fatalf("Find() returned %d workflows, want 1", len(workflows))
	}

	w := workflows[0]
	if w.Path != expectedPath {
		t.Errorf("Path = %q, want %q", w.Path, expectedPath)
	}
	if w.Name != "my-workflow" {
		t.Errorf("Name = %q, want %q", w.Name, "my-workflow")
	}
	if w.CurrentState != "implementing" {
		t.Errorf("CurrentState = %q, want %q", w.CurrentState, "implementing")
	}
	if w.TemplatePath != "/tmp/template.md" {
		t.Errorf("TemplatePath = %q, want %q", w.TemplatePath, "/tmp/template.md")
	}
	if w.CreatedAt != "2026-02-22T12:00:00Z" {
		t.Errorf("CreatedAt = %q, want %q", w.CreatedAt, "2026-02-22T12:00:00Z")
	}
}

func TestFind_MultipleFiles(t *testing.T) {
	dir := t.TempDir()
	writeStateFile(t, dir, "alpha", "ready")
	writeStateFile(t, dir, "beta", "running")
	writeStateFile(t, dir, "gamma", "done")

	workflows, err := Find(dir)
	if err != nil {
		t.Fatalf("Find() error: %v", err)
	}

	if len(workflows) != 3 {
		t.Fatalf("Find() returned %d workflows, want 3", len(workflows))
	}

	// Build a map by name for order-independent assertions.
	byName := make(map[string]Workflow)
	for _, w := range workflows {
		byName[w.Name] = w
	}

	for _, tc := range []struct {
		name  string
		state string
	}{
		{"alpha", "ready"},
		{"beta", "running"},
		{"gamma", "done"},
	} {
		w, ok := byName[tc.name]
		if !ok {
			t.Errorf("workflow %q not found in results", tc.name)
			continue
		}
		if w.CurrentState != tc.state {
			t.Errorf("workflow %q: CurrentState = %q, want %q", tc.name, w.CurrentState, tc.state)
		}
	}
}

func TestFind_NonMatchingFilesIgnored(t *testing.T) {
	dir := t.TempDir()

	// Create files that don't match the koto-*.state.json pattern.
	nonMatching := []string{
		"readme.md",
		"state.json",
		"koto.state.json",
		"koto-workflow.json",
		"other-koto-test.state.json",
	}
	for _, name := range nonMatching {
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, []byte("{}"), 0o600); err != nil {
			t.Fatalf("write %s: %v", name, err)
		}
	}

	// Also create one valid matching file to confirm it's found.
	writeStateFile(t, dir, "valid", "ready")

	workflows, err := Find(dir)
	if err != nil {
		t.Fatalf("Find() error: %v", err)
	}

	if len(workflows) != 1 {
		t.Fatalf("Find() returned %d workflows, want 1", len(workflows))
	}
	if workflows[0].Name != "valid" {
		t.Errorf("Name = %q, want %q", workflows[0].Name, "valid")
	}
}

func TestFind_CorruptedFile(t *testing.T) {
	dir := t.TempDir()

	// Create one valid and one corrupted state file.
	writeStateFile(t, dir, "good", "ready")

	corruptedPath := filepath.Join(dir, "koto-bad.state.json")
	if err := os.WriteFile(corruptedPath, []byte("not valid json{{{"), 0o600); err != nil {
		t.Fatalf("write corrupted file: %v", err)
	}

	workflows, err := Find(dir)

	// Should return partial results AND a non-nil error.
	if err == nil {
		t.Fatal("Find() expected non-nil error for corrupted file")
	}

	if len(workflows) != 1 {
		t.Fatalf("Find() returned %d workflows, want 1 (partial results)", len(workflows))
	}
	if workflows[0].Name != "good" {
		t.Errorf("Name = %q, want %q", workflows[0].Name, "good")
	}
}

func TestFind_AllCorruptedFiles(t *testing.T) {
	dir := t.TempDir()

	// Create two corrupted state files.
	for _, name := range []string{"koto-bad1.state.json", "koto-bad2.state.json"} {
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, []byte("{invalid"), 0o600); err != nil {
			t.Fatalf("write %s: %v", name, err)
		}
	}

	workflows, err := Find(dir)

	if err == nil {
		t.Fatal("Find() expected non-nil error for corrupted files")
	}
	if workflows == nil {
		t.Fatal("Find() returned nil, want empty slice")
	}
	if len(workflows) != 0 {
		t.Errorf("Find() returned %d workflows, want 0", len(workflows))
	}
}

func TestFind_ReadsOnlyMinimalFields(t *testing.T) {
	dir := t.TempDir()

	// Create a state file with a large history and variables section.
	// Find should still work because it only reads the header fields.
	state := map[string]interface{}{
		"schema_version": 1,
		"workflow": map[string]string{
			"name":          "big-workflow",
			"template_hash": "sha256:xyz",
			"template_path": "/tmp/big.md",
			"created_at":    "2026-02-22T12:00:00Z",
		},
		"version":       10,
		"current_state": "review",
		"variables": map[string]string{
			"TASK":   "build something large",
			"BRANCH": "feature/big",
			"ISSUE":  "42",
		},
		"history": []map[string]string{
			{"from": "start", "to": "research", "timestamp": "2026-02-22T12:01:00Z", "type": "transition"},
			{"from": "research", "to": "review", "timestamp": "2026-02-22T12:02:00Z", "type": "transition"},
		},
	}

	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	path := filepath.Join(dir, "koto-big-workflow.state.json")
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("write: %v", err)
	}

	workflows, err := Find(dir)
	if err != nil {
		t.Fatalf("Find() error: %v", err)
	}

	if len(workflows) != 1 {
		t.Fatalf("Find() returned %d workflows, want 1", len(workflows))
	}

	w := workflows[0]
	if w.Name != "big-workflow" {
		t.Errorf("Name = %q, want %q", w.Name, "big-workflow")
	}
	if w.CurrentState != "review" {
		t.Errorf("CurrentState = %q, want %q", w.CurrentState, "review")
	}
	if w.TemplatePath != "/tmp/big.md" {
		t.Errorf("TemplatePath = %q, want %q", w.TemplatePath, "/tmp/big.md")
	}
}

func TestWorkflow_JSONTags(t *testing.T) {
	w := Workflow{
		Path:         "/tmp/koto-test.state.json",
		Name:         "test",
		CurrentState: "ready",
		TemplatePath: "/tmp/template.md",
		CreatedAt:    "2026-02-22T12:00:00Z",
	}

	data, err := json.Marshal(w)
	if err != nil {
		t.Fatalf("Marshal() error: %v", err)
	}

	var m map[string]interface{}
	if err := json.Unmarshal(data, &m); err != nil {
		t.Fatalf("Unmarshal() error: %v", err)
	}

	expectedKeys := []string{"path", "name", "current_state", "template_path", "created_at"}
	for _, key := range expectedKeys {
		if _, ok := m[key]; !ok {
			t.Errorf("JSON missing key %q", key)
		}
	}
	if len(m) != len(expectedKeys) {
		t.Errorf("JSON has %d keys, want %d", len(m), len(expectedKeys))
	}
}
