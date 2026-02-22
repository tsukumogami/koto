package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsukumogami/koto/internal/buildinfo"
	"github.com/tsukumogami/koto/pkg/controller"
	"github.com/tsukumogami/koto/pkg/engine"
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
	case "rewind", "cancel", "query", "status", "validate", "workflows":
		err = cmdStub(os.Args[1])
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

	// Resolve template path to absolute
	absTemplatePath, err := filepath.Abs(templatePath)
	if err != nil {
		return fmt.Errorf("resolve template path: %w", err)
	}

	// Compute template hash
	templateData, err := os.ReadFile(absTemplatePath) //nolint:gosec // G304: CLI reads user-specified template path
	if err != nil {
		return fmt.Errorf("read template file: %w", err)
	}
	hash := sha256.Sum256(templateData)
	templateHash := "sha256:" + hex.EncodeToString(hash[:])

	// Ensure state directory exists
	stateDir = filepath.Clean(stateDir)
	if err := os.MkdirAll(stateDir, 0o750); err != nil { //nolint:gosec // G703: stateDir is cleaned; CLI accepts user-specified paths
		return fmt.Errorf("create state directory: %w", err)
	}

	statePath := filepath.Join(stateDir, fmt.Sprintf("koto-%s.state.json", name))

	// Use a hardcoded stub machine for this skeleton.
	// Real template parsing will replace this in issue #7.
	machine := stubMachine()

	eng, err := engine.Init(statePath, machine, engine.InitMeta{
		Name:         name,
		TemplateHash: templateHash,
		TemplatePath: absTemplatePath,
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
	var target, statePath string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--state":
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		default:
			if target == "" && !isFlag(args[i]) {
				target = args[i]
			}
		}
	}

	if target == "" {
		return fmt.Errorf("target state is required")
	}
	if statePath == "" {
		return fmt.Errorf("--state is required")
	}

	machine := stubMachine()

	eng, err := engine.Load(statePath, machine)
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
	var statePath string

	for i := 0; i < len(args); i++ {
		if args[i] == "--state" {
			if i+1 >= len(args) || isFlag(args[i+1]) {
				return fmt.Errorf("--state requires a value")
			}
			i++
			statePath = args[i]
		}
	}

	if statePath == "" {
		return fmt.Errorf("--state is required")
	}

	machine := stubMachine()

	eng, err := engine.Load(statePath, machine)
	if err != nil {
		return err
	}

	ctrl, err := controller.New(eng, "")
	if err != nil {
		return err
	}
	d, err := ctrl.Next()
	if err != nil {
		return err
	}

	return printJSON(d)
}

func cmdStub(name string) error {
	return &engine.TransitionError{
		Code:    "not_implemented",
		Message: fmt.Sprintf("%s is not yet implemented", name),
	}
}

// stubMachine returns a hardcoded three-state machine for the walking
// skeleton. Real template parsing will replace this in issue #7.
func stubMachine() *engine.Machine {
	return &engine.Machine{
		Name:         "stub",
		InitialState: "ready",
		States: map[string]*engine.MachineState{
			"ready": {
				Transitions: []string{"running"},
			},
			"running": {
				Transitions: []string{"done"},
			},
			"done": {
				Terminal: true,
			},
		},
	}
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
