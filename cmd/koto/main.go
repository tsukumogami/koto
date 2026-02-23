package main

import (
	"crypto/sha256"
	"encoding/hex"
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
	"github.com/tsukumogami/koto/pkg/template/compile"
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
	case "template":
		err = cmdTemplate(os.Args[2:])
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

// parsedArgs holds the result of argument parsing.
type parsedArgs struct {
	flags      map[string]string   // single-value flags (--name, --state, etc.)
	multi      map[string][]string // multi-value flags (--var)
	positional []string            // non-flag arguments
}

// parseFlags parses command arguments into flags and positional args.
// Flags are expected in --key value format. Multi-value flag names
// (like "--var") can appear multiple times and their values accumulate.
func parseFlags(args []string, multiFlags map[string]bool) (*parsedArgs, error) {
	result := &parsedArgs{
		flags: make(map[string]string),
		multi: make(map[string][]string),
	}

	for i := 0; i < len(args); i++ {
		arg := args[i]
		if !isFlag(arg) {
			result.positional = append(result.positional, arg)
			continue
		}

		// Consume the next argument as the flag value.
		if i+1 >= len(args) {
			return nil, fmt.Errorf("%s requires a value", arg)
		}
		next := args[i+1]
		if isFlag(next) {
			return nil, fmt.Errorf("%s requires a value", arg)
		}
		i++ // advance past the value

		if multiFlags[arg] {
			result.multi[arg] = append(result.multi[arg], next)
		} else {
			result.flags[arg] = next
		}
	}

	return result, nil
}

func cmdInit(args []string) error {
	p, err := parseFlags(args, map[string]bool{"--var": true})
	if err != nil {
		return err
	}

	name := p.flags["--name"]
	templatePath := p.flags["--template"]
	stateDir := p.flags["--state-dir"]

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

	// Read the source file.
	sourceBytes, err := os.ReadFile(absTemplatePath) //nolint:gosec // G304: CLI reads user-specified template path
	if err != nil {
		return fmt.Errorf("read template file: %w", err)
	}

	// Compile the source template.
	ct, _, err := compile.Compile(sourceBytes)
	if err != nil {
		return fmt.Errorf("compile template: %w", err)
	}

	// Validate the compiled output through ParseJSON round-trip.
	compiledJSON, err := json.Marshal(ct)
	if err != nil {
		return fmt.Errorf("marshal compiled template: %w", err)
	}
	ct, err = template.ParseJSON(compiledJSON)
	if err != nil {
		return fmt.Errorf("validate compiled template: %w", err)
	}

	// Build the engine machine.
	machine := ct.BuildMachine()

	// Compute the compiler hash (SHA-256 of compiled JSON, not raw source).
	templateHash, _, err := compile.Hash(ct)
	if err != nil {
		return fmt.Errorf("hash compiled template: %w", err)
	}

	// Convert to Template for variable defaults.
	tmpl, err := ct.ToTemplate()
	if err != nil {
		return fmt.Errorf("convert compiled template: %w", err)
	}

	// Merge variables: start with template defaults, then overlay --var flags.
	variables := make(map[string]string, len(tmpl.Variables)+len(p.multi["--var"]))
	for k, v := range tmpl.Variables {
		variables[k] = v
	}
	for _, kv := range p.multi["--var"] {
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

	eng, err := engine.Init(statePath, machine, engine.InitMeta{
		Name:         name,
		TemplateHash: templateHash,
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	if len(p.positional) == 0 {
		return fmt.Errorf("target state is required")
	}
	target := p.positional[0]

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, storedHash, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	if storedHash != tmpl.Hash {
		return &engine.TransitionError{
			Code: engine.ErrTemplateMismatch,
			Message: fmt.Sprintf(
				"template hash mismatch: state file has %q but template on disk is %q",
				storedHash, tmpl.Hash),
		}
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, _, err := loadTemplateFromState(resolved)
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, _, err := loadTemplateFromState(resolved)
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, _, err := loadTemplateFromState(resolved)
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	target := p.flags["--to"]
	if target == "" {
		return fmt.Errorf("--to is required")
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, storedHash, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	if storedHash != tmpl.Hash {
		return &engine.TransitionError{
			Code: engine.ErrTemplateMismatch,
			Message: fmt.Sprintf(
				"template hash mismatch: state file has %q but template on disk is %q",
				storedHash, tmpl.Hash),
		}
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
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, _, err := loadTemplateFromState(resolved)
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

	fmt.Println("workflow canceled")
	return nil
}

func cmdValidate(args []string) error {
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	resolved, err := resolveStatePath(p.flags["--state"], p.flags["--state-dir"])
	if err != nil {
		return err
	}

	tmpl, storedHash, err := loadTemplateFromState(resolved)
	if err != nil {
		return err
	}

	if storedHash != tmpl.Hash {
		return &engine.TransitionError{
			Code: engine.ErrTemplateMismatch,
			Message: fmt.Sprintf(
				"template hash mismatch: state file has %q but template on disk is %q",
				storedHash, tmpl.Hash),
		}
	}

	fmt.Println("OK: template hash matches")
	return nil
}

func cmdWorkflows(args []string) error {
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	stateDir := p.flags["--state-dir"]
	if stateDir == "" {
		stateDir = "wip"
	}

	workflows, err := discover.Find(stateDir)
	if err != nil {
		return err
	}

	return printJSON(workflows)
}

func cmdTemplate(args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("usage: koto template <subcommand>\navailable subcommands: compile")
	}

	switch args[0] {
	case "compile":
		return cmdTemplateCompile(args[1:])
	default:
		return fmt.Errorf("unknown template subcommand: %s", args[0])
	}
}

func cmdTemplateCompile(args []string) error {
	p, err := parseFlags(args, nil)
	if err != nil {
		return err
	}

	if len(p.positional) == 0 {
		return fmt.Errorf("usage: koto template compile <path> [--output <file>]")
	}

	sourcePath := p.positional[0]
	outputPath := p.flags["--output"]

	// Read the source file.
	sourceBytes, err := os.ReadFile(sourcePath) //nolint:gosec // G304: CLI reads user-specified template path
	if err != nil {
		return fmt.Errorf("read source file: %w", err)
	}

	// Compile the source template.
	ct, warnings, err := compile.Compile(sourceBytes)
	if err != nil {
		return fmt.Errorf("compile: %w", err)
	}

	// Print warnings to stderr.
	for _, w := range warnings {
		fmt.Fprintf(os.Stderr, "warning: %s\n", w.Message) //nolint:gosec // G705: warning message is from compiler, not user input
	}

	// Marshal to JSON.
	compiledJSON, err := json.MarshalIndent(ct, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal compiled template: %w", err)
	}
	compiledJSON = append(compiledJSON, '\n')

	// Write to output file or stdout.
	if outputPath != "" {
		if err := os.WriteFile(outputPath, compiledJSON, 0o644); err != nil { //nolint:gosec // G306: compiled template output is not sensitive
			return fmt.Errorf("write output file: %w", err)
		}
		return nil
	}

	fmt.Print(string(compiledJSON))
	return nil
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

// loadTemplateFromState reads a state file, extracts the template path
// and stored template hash, compiles the template via the compiler path,
// and performs dual-hash comparison. If the compiler hash matches the
// stored hash, the template is returned directly. If not, the legacy hash
// (SHA-256 of raw source) is tried. On legacy match, a deprecation warning
// is printed to stderr and the returned template's Hash is set to the
// stored hash so that callers' existing storedHash != tmpl.Hash checks
// remain valid. If neither hash matches, ErrTemplateMismatch is returned.
func loadTemplateFromState(statePath string) (tmpl *template.Template, storedHash string, err error) {
	data, err := os.ReadFile(statePath) //nolint:gosec // G304: CLI reads user-specified state path
	if err != nil {
		return nil, "", fmt.Errorf("read state file: %w", err)
	}

	// Minimal parse to extract template_path and template_hash.
	var header struct {
		Workflow struct {
			TemplatePath string `json:"template_path"`
			TemplateHash string `json:"template_hash"`
		} `json:"workflow"`
	}
	if err := json.Unmarshal(data, &header); err != nil {
		return nil, "", fmt.Errorf("parse state file: %w", err)
	}

	if header.Workflow.TemplatePath == "" {
		return nil, "", fmt.Errorf("state file has no template_path")
	}

	// Read the source file.
	sourceBytes, err := os.ReadFile(header.Workflow.TemplatePath) //nolint:gosec // G304: CLI reads template path from state file
	if err != nil {
		return nil, "", fmt.Errorf("read template file: %w", err)
	}

	// Compile the source template.
	ct, _, err := compile.Compile(sourceBytes)
	if err != nil {
		return nil, "", fmt.Errorf("compile template: %w", err)
	}

	// Validate through ParseJSON round-trip.
	compiledJSON, err := json.Marshal(ct)
	if err != nil {
		return nil, "", fmt.Errorf("marshal compiled template: %w", err)
	}
	ct, err = template.ParseJSON(compiledJSON)
	if err != nil {
		return nil, "", fmt.Errorf("validate compiled template: %w", err)
	}

	// Compute the compiler hash.
	compilerHash, _, err := compile.Hash(ct)
	if err != nil {
		return nil, "", fmt.Errorf("hash compiled template: %w", err)
	}

	// Convert to Template via adapter.
	tmpl, err = ct.ToTemplate()
	if err != nil {
		return nil, "", fmt.Errorf("convert compiled template: %w", err)
	}
	tmpl.Path = header.Workflow.TemplatePath
	tmpl.Hash = compilerHash

	// Dual-hash comparison: try compiler hash first.
	if compilerHash == header.Workflow.TemplateHash {
		return tmpl, header.Workflow.TemplateHash, nil
	}

	// Fall back to legacy hash (SHA-256 of raw source bytes).
	sum := sha256.Sum256(sourceBytes)
	legacyHash := "sha256:" + hex.EncodeToString(sum[:])

	if legacyHash == header.Workflow.TemplateHash {
		fmt.Fprintln(os.Stderr, "note: workflow uses legacy template hash; re-init to upgrade")
		// Set tmpl.Hash to the stored hash so callers' storedHash != tmpl.Hash
		// checks remain valid.
		tmpl.Hash = header.Workflow.TemplateHash
		return tmpl, header.Workflow.TemplateHash, nil
	}

	// Neither hash matches.
	return nil, "", &engine.TransitionError{
		Code: engine.ErrTemplateMismatch,
		Message: fmt.Sprintf(
			"template hash mismatch: state file has %q but template on disk produces %q (compiler) or %q (legacy)",
			header.Workflow.TemplateHash, compilerHash, legacyHash),
	}
}

// isFlag reports whether s looks like a flag argument. It checks for
// a "--" prefix, matching the double-dash convention used by all koto
// flags (--name, --state, --var, etc.). Single-dash values like "-1"
// are not treated as flags, so they can appear as flag values.
func isFlag(s string) bool {
	return strings.HasPrefix(s, "--")
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
