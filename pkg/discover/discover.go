// Package discover locates active koto workflow state files in a directory.
//
// It scans for files matching the koto-*.state.json naming pattern and
// reads minimal metadata from each, returning a list of Workflow structs
// without fully unmarshaling the state file contents.
package discover

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// Workflow represents a discovered active workflow with minimal metadata
// extracted from its state file.
type Workflow struct {
	Path         string `json:"path"`
	Name         string `json:"name"`
	CurrentState string `json:"current_state"`
	TemplatePath string `json:"template_path"`
	CreatedAt    string `json:"created_at"`
}

// stateHeader is a minimal struct for unmarshaling only the fields
// needed from a koto state file. It avoids reading history, variables,
// or other heavyweight fields.
//
// The JSON field names here must match the tags on engine.State and
// engine.WorkflowMeta in pkg/engine/types.go. A cross-package round-trip
// test in discover_test.go guards against schema drift.
type stateHeader struct {
	Workflow     workflowHeader `json:"workflow"`
	CurrentState string         `json:"current_state"`
}

type workflowHeader struct {
	Name         string `json:"name"`
	TemplatePath string `json:"template_path"`
	CreatedAt    string `json:"created_at"`
}

// Find scans the directory for koto state files (koto-*.state.json)
// and returns metadata for each active workflow. It returns an empty
// slice (not nil) when no matching files are found. If a matching file
// cannot be parsed, Find continues scanning and returns partial results
// along with a non-nil error describing the failures.
func Find(dir string) ([]Workflow, error) {
	pattern := filepath.Join(dir, "koto-*.state.json")
	matches, err := filepath.Glob(pattern)
	if err != nil {
		return []Workflow{}, fmt.Errorf("glob state files: %w", err)
	}

	workflows := make([]Workflow, 0, len(matches))
	var errs []error

	for _, path := range matches {
		data, err := os.ReadFile(path) //nolint:gosec // G304: discover reads caller-specified directory contents
		if err != nil {
			errs = append(errs, fmt.Errorf("read %s: %w", filepath.Base(path), err))
			continue
		}

		var header stateHeader
		if err := json.Unmarshal(data, &header); err != nil {
			errs = append(errs, fmt.Errorf("parse %s: %w", filepath.Base(path), err))
			continue
		}

		workflows = append(workflows, Workflow{
			Path:         path,
			Name:         header.Workflow.Name,
			CurrentState: header.CurrentState,
			TemplatePath: header.Workflow.TemplatePath,
			CreatedAt:    header.Workflow.CreatedAt,
		})
	}

	return workflows, errors.Join(errs...)
}
