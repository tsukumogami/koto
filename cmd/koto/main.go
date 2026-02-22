package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsukumogami/koto/internal/buildinfo"
	"github.com/tsukumogami/koto/pkg/controller"
	"github.com/tsukumogami/koto/pkg/discover"
	"github.com/tsukumogami/koto/pkg/engine"
	"github.com/tsukumogami/koto/pkg/template"
)

func main() {
	if len(os.Args) < 2 {
		printError("no_command", "usage: koto <command> [flags]")
		os.Exit(1)
	}

	var err error
	switch os.Args[1] {
	case "version":
		fmt.Println("koto", buildinfo.Version())
		return
	case "init":
		err = cmdInit(os.Args[2:])
	case "transition":
		err = cmdTransition(os.Args[2:])
	case "next":
		err = cmdNext(os.Args[2:])
	case "query":
		err = cmdQuery(os.Args[2:])
	case "status":
		err = cmdStatus(os.Args[2:])
	case "rewind":
		err = cmdRewind(os.Args[2:])
	case "cancel":
		err = cmdCancel(os.Args[2:])
	case "validate":
		err = cmdValidate(os.Args[2:])
	case "workflows":
		err = cmdWorkflows(os.Args[2:])
	default:
		printError("unknown_command", fmt.Sprintf("unknown command: %s", os.Args[1]))
		os.Exit(1)
	}

	if err != nil {
		if te, ok := err.(*engine.TransitionError); ok {
			printTransitionError(te)
		} else {
			printError("internal_error", err.Error())
		}
		os.Exit(1)
	}
}

func cmdInit(args []string) error {
	var name, templatePath, stateDir string
	var varFlags []string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--name":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--name requires a value")
			}
			i++
			name = args[i]
		case "--template":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--template requires a value")
			}
			i++
			templatePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		case "--var":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--var requires a KEY=VALUE argument")
			}
			i++
			varFlags = append(varFlags, args[i])
		}
	}

	if name == "" {
		return fmt.Errorf("--name is required")
	}
	if templatePath == "" {
		return fmt.Errorf("--template is required")
	}
	if stateDir == "" {
		stateDir = "wip"
	}

	// Resolve template path to absolute.
	absTemplatePath, err := filepath.Abs(templatePath)
	if err != nil {
		return fmt.Errorf("resolve template path: %w", err)
	}

	// Parse the template file.
	tmpl, err := template.Parse(absTemplatePath)
	if err != nil {
		return err
	}

	// Merge variables: start with template defaults, then overlay --var flags.
	variables := make(map[string]string, len(tmpl.Variables)+len(varFlags))
	for k, v := range tmpl.Variables {
		variables[k] = v
	}
	for _, kv := range varFlags {
		parts := strings.SplitN(kv, "=", 2)
		if len(parts) != 2 {
			return fmt.Errorf("invalid --var format %q: expected KEY=VALUE", kv)
		}
		variables[parts[0]] = parts[1]
	}

	// Ensure state directory exists.
	stateDir = filepath.Clean(stateDir)
	if err := os.MkdirAll(stateDir, 0o750); err != nil { //nolint:gosec // G301: stateDir is cleaned; CLI accepts user-specified paths
		return fmt.Errorf("create state directory: %w", err)
	}

	statePath := filepath.Join(stateDir, fmt.Sprintf("koto-%s.state.json", name))

	eng, err := engine.Init(statePath, tmpl.Machine, engine.InitMeta{
		Name:         name,
		TemplateHash: tmpl.Hash,
		TemplatePath: absTemplatePath,
		Variables:    variables,
	})
	if err != nil {
		return err
	}

	return printJSON(map[string]string{
		"state": eng.CurrentState(),
		"path":  eng.Path(),
	})
}

func cmdTransition(args []string) error {
	var target, statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		default:
			if target == "" && !isFlag(args[i]) {
				target = args[i]
			}
		}
	}

	if target == "" {
		return fmt.Errorf("target state is required")
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	if err := eng.Transition(target); err != nil {
		return err
	}

	snap := eng.Snapshot()
	return printJSON(map[string]interface{}{
		"state":   snap.CurrentState,
		"version": snap.Version,
	})
}

func cmdNext(args []string) error {
	var statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	ctrl, err := controller.New(eng, tmpl)
	if err != nil {
		return err
	}
	d, err := ctrl.Next()
	if err != nil {
		return err
	}

	return printJSON(d)
}

func cmdQuery(args []string) error {
	var statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	return printJSON(eng.Snapshot())
}

func cmdStatus(args []string) error {
	var statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	snap := eng.Snapshot()
	fmt.Printf("Workflow: %s\n", snap.Workflow.Name)
	fmt.Printf("State:    %s\n", snap.CurrentState)
	fmt.Printf("History:  %d entries\n", len(snap.History))
	return nil
}

func cmdRewind(args []string) error {
	var target, statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--to":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--to requires a value")
			}
			i++
			target = args[i]
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	if target == "" {
		return fmt.Errorf("--to is required")
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	if err := eng.Rewind(target); err != nil {
		return err
	}

	snap := eng.Snapshot()
	return printJSON(map[string]interface{}{
		"state":   snap.CurrentState,
		"version": snap.Version,
	})
}

func cmdCancel(args []string) error {
	var statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	tmpl, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	eng, err := engine.Load(resolved, tmpl.Machine)
	if err != nil {
		return err
	}

	if err := eng.Cancel(); err != nil {
		return err
	}

	fmt.Println("workflow cancelled")
	return nil
}

func cmdValidate(args []string) error {
	var statePath, stateDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		case "--state-dir":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	resolved, err := resolveStatePath(statePath, stateDir)
	if err != nil {
		return err
	}

	// Read the state file to get the stored template path and hash.
	stateData, err := os.ReadFile(resolved) //nolint:gosec // G304: CLI reads user-specified state path
	if err != nil {
		return fmt.Errorf("read state file: %w", err)
	}
	var state engine.State
	if err := json.Unmarshal(stateData, &state); err != nil {
		return fmt.Errorf("parse state file: %w", err)
	}

	// Parse the template to get its current hash.
	tmpl, err := template.Parse(state.Workflow.TemplatePath)
	if err != nil {
		return fmt.Errorf("read template: %w", err)
	}

	if state.Workflow.TemplateHash != tmpl.Hash {
		fmt.Printf("MISMATCH: state file hash %s does not match template on disk %s\n",
			state.Workflow.TemplateHash, tmpl.Hash)
		os.Exit(1)
	}

	fmt.Println("OK: template hash matches")
	return nil
}

func cmdWorkflows(args []string) error {
	stateDir := "wip"

	for i := 0; i < len(args); i++ {
		if args[i] == "--state-dir" {
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state-dir requires a value")
			}
			i++
			stateDir = args[i]
		}
	}

	workflows, err := discover.Find(stateDir)
	if err != nil {
		return err
	}

	return printJSON(workflows)
}

// resolveStatePath determines the state file path from explicit --state,
// or by auto-selecting when exactly one state file exists in the state
// directory. When multiple state files exist and no --state is given,
// it returns an error listing the available files.
func resolveStatePath(statePath, stateDir string) (string, error) {
	if statePath != "" {
		return statePath, nil
	}

	if stateDir == "" {
		stateDir = "wip"
	}

	workflows, err := discover.Find(stateDir)
	if err != nil {
		return "", fmt.Errorf("scan state directory: %w", err)
	}

	switch len(workflows) {
	case 0:
		return "", fmt.Errorf("no state files found in %s", stateDir)
	case 1:
		return workflows[0].Path, nil
	default:
		var paths []string
		for _, w := range workflows {
			paths = append(paths, w.Path)
		}
		return "", fmt.Errorf(
			"multiple state files found in %s, use --state to select one: %s",
			stateDir, strings.Join(paths, ", "))
	}
}

// loadTemplateFromState reads a state file, extracts the template path,
// and parses the template. This is the standard way to recover the Machine
// and Template for commands that operate on an existing workflow.
func loadTemplateFromState(statePath string) (*template.Template, error) {
	data, err := os.ReadFile(statePath) //nolint:gosec // G304: CLI reads user-specified state path
	if err != nil {
		return nil, fmt.Errorf("read state file: %w", err)
	}

	// Minimal parse to extract template_path.
	var header struct {
		Workflow struct {
			TemplatePath string `json:"template_path"`
		} `json:"workflow"`
	}
	if err := json.Unmarshal(data, &header); err != nil {
		return nil, fmt.Errorf("parse state file: %w", err)
	}

	if header.Workflow.TemplatePath == "" {
		return nil, fmt.Errorf("state file has no template_path")
	}

	tmpl, err := template.Parse(header.Workflow.TemplatePath)
	if err != nil {
		return nil, fmt.Errorf("parse template: %w", err)
	}

	return tmpl, nil
}

func isFlag(s string) bool {
	return len(s) > 0 && s[0] == '-'
}

func printJSON(v interface{}) error {
	data, err := json.Marshal(v)
	if err != nil {
		return fmt.Errorf("marshal output: %w", err)
	}
	fmt.Println(string(data))
	return nil
}

func printError(code, message string) {
	data, _ := json.Marshal(map[string]interface{}{
		"error": map[string]string{
			"code":    code,
			"message": message,
		},
	})
	fmt.Println(string(data))
}

func printTransitionError(te *engine.TransitionError) {
	data, _ := json.Marshal(map[string]interface{}{
		"error": te,
	})
	fmt.Println(string(data))
}
