// Package template parses workflow template files into engine.Machine
// instances. A template file uses YAML front-matter for metadata and
// markdown sections for state definitions.
//
// Template format:
//
//	---
//	name: workflow-name
//	version: "1.0"
//	description: A workflow description
//	variables:
//	  KEY: default-value
//	---
//
//	## state-name
//
//	Directive text for the agent.
//
//	**Transitions**: [next-state-a, next-state-b]
package template

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"strings"

	"github.com/tsukumogami/koto/pkg/engine"
)

// Template holds a parsed workflow template, including the state machine
// definition, section content, variables, and a SHA-256 hash of the
// source file.
type Template struct {
	Name        string
	Version     string
	Description string
	Machine     *engine.Machine
	Sections    map[string]string // state name -> raw markdown content (between heading and transitions)
	Variables   map[string]string // default variable values from template header
	Hash        string            // SHA-256 of the template file content, formatted as "sha256:<hex>"
	Path        string            // filesystem path to the template file
}

// Parse reads a template file at the given path and returns a Template.
// It extracts the YAML front-matter header, parses state sections from
// markdown headings, constructs an engine.Machine, and computes a
// SHA-256 hash of the full file content.
//
// The first ## heading in the body becomes the machine's initial state.
//
// Parse returns an error if:
//   - the file cannot be read
//   - the front-matter delimiters are missing or malformed
//   - a state references a transition target that is not defined as a state
//   - no states are defined in the template
func Parse(path string) (*Template, error) {
	data, err := os.ReadFile(path) //nolint:gosec // G304: template reads caller-specified template path
	if err != nil {
		return nil, fmt.Errorf("read template file: %w", err)
	}

	content := string(data)

	// Compute hash of the full file content.
	sum := sha256.Sum256(data)
	hash := "sha256:" + hex.EncodeToString(sum[:])

	// Split front-matter from body.
	header, body, err := SplitFrontMatter(content)
	if err != nil {
		return nil, err
	}

	// Parse the front-matter header.
	name, version, description, variables, err := parseHeader(header)
	if err != nil {
		return nil, err
	}

	// Parse state sections from the body.
	stateNames, sections, transitions, err := parseSections(body)
	if err != nil {
		return nil, err
	}

	if len(stateNames) == 0 {
		return nil, fmt.Errorf("template has no states defined")
	}

	// Validate that all transition targets reference defined states.
	stateSet := make(map[string]bool, len(stateNames))
	for _, s := range stateNames {
		stateSet[s] = true
	}
	for stateName, targets := range transitions {
		for _, target := range targets {
			if !stateSet[target] {
				return nil, fmt.Errorf("state %q references undefined transition target %q", stateName, target)
			}
		}
	}

	// Build the engine.Machine.
	states := make(map[string]*engine.MachineState, len(stateNames))
	for _, s := range stateNames {
		ms := &engine.MachineState{}
		if t, ok := transitions[s]; ok {
			ms.Transitions = t
		} else {
			// States without transitions are terminal.
			ms.Terminal = true
		}
		states[s] = ms
	}

	machineName := name
	if machineName == "" {
		machineName = "unnamed"
	}

	machine := &engine.Machine{
		Name:         machineName,
		InitialState: stateNames[0], // first state is the initial state
		States:       states,
	}

	return &Template{
		Name:        name,
		Version:     version,
		Description: description,
		Machine:     machine,
		Sections:    sections,
		Variables:   variables,
		Hash:        hash,
		Path:        path,
	}, nil
}

// Interpolate replaces {{KEY}} placeholders in text with values from ctx.
// Replacement is single-pass: a replaced value that itself contains
// placeholder syntax is not re-expanded. Unresolved placeholders are
// left unchanged.
func Interpolate(text string, ctx map[string]string) string {
	var b strings.Builder
	b.Grow(len(text))

	i := 0
	for i < len(text) {
		// Look for opening braces.
		idx := strings.Index(text[i:], "{{")
		if idx < 0 {
			b.WriteString(text[i:])
			break
		}

		// Write everything before the opening braces.
		b.WriteString(text[i : i+idx])

		// Find closing braces.
		rest := text[i+idx+2:]
		end := strings.Index(rest, "}}")
		if end < 0 {
			// No closing braces; write the rest and stop.
			b.WriteString(text[i+idx:])
			break
		}

		key := rest[:end]
		if val, ok := ctx[key]; ok {
			b.WriteString(val)
		} else {
			// Unresolved placeholder: leave as-is.
			b.WriteString("{{")
			b.WriteString(key)
			b.WriteString("}}")
		}

		i = i + idx + 2 + end + 2
	}

	return b.String()
}

// SplitFrontMatter separates the YAML front-matter from the markdown body.
// The front-matter is delimited by "---" lines. Returns the header content
// (between the delimiters) and the body (after the closing delimiter).
func SplitFrontMatter(content string) (header, body string, err error) {
	// Trim leading whitespace/newlines.
	trimmed := strings.TrimLeft(content, " \t\r\n")

	if !strings.HasPrefix(trimmed, "---") {
		return "", "", fmt.Errorf("template missing opening front-matter delimiter (---)")
	}

	// Find the end of the opening delimiter line.
	afterOpen := strings.Index(trimmed, "\n")
	if afterOpen < 0 {
		return "", "", fmt.Errorf("template has only an opening front-matter delimiter")
	}

	rest := trimmed[afterOpen+1:]

	// Find the closing delimiter.
	closeIdx := strings.Index(rest, "\n---")
	if closeIdx < 0 {
		// Check if the rest starts with --- (edge case: empty header).
		if strings.HasPrefix(rest, "---") {
			return "", rest[3:], nil
		}
		return "", "", fmt.Errorf("template missing closing front-matter delimiter (---)")
	}

	header = rest[:closeIdx]
	// Body starts after the closing "---" line.
	afterClose := rest[closeIdx+4:] // skip "\n---"
	// Skip the rest of the closing delimiter line.
	nlIdx := strings.Index(afterClose, "\n")
	if nlIdx >= 0 {
		body = afterClose[nlIdx+1:]
	} else {
		body = ""
	}

	return header, body, nil
}

// parseHeader extracts name, version, description, and variables from
// the YAML front-matter. This is a simple manual parser for the flat
// key-value structure used in koto templates; it does not handle
// arbitrary YAML.
func parseHeader(header string) (name, version, description string, variables map[string]string, err error) {
	variables = make(map[string]string)

	lines := strings.Split(header, "\n")
	inVariables := false

	for _, line := range lines {
		// Skip empty lines.
		if strings.TrimSpace(line) == "" {
			continue
		}

		// Check for variable entries (indented lines under "variables:").
		if inVariables {
			trimmed := strings.TrimSpace(line)
			// If the line is not indented, we've left the variables block.
			if !strings.HasPrefix(line, " ") && !strings.HasPrefix(line, "\t") {
				inVariables = false
				// Fall through to process as a top-level key.
			} else {
				// Parse variable: "  KEY: value" or "  KEY: \"value\""
				parts := strings.SplitN(trimmed, ":", 2)
				if len(parts) == 2 {
					k := strings.TrimSpace(parts[0])
					v := strings.TrimSpace(parts[1])
					v = unquote(v)
					variables[k] = v
				}
				continue
			}
		}

		// Top-level key: value
		parts := strings.SplitN(line, ":", 2)
		if len(parts) != 2 {
			continue
		}
		key := strings.TrimSpace(parts[0])
		val := strings.TrimSpace(parts[1])

		switch key {
		case "name":
			name = unquote(val)
		case "version":
			version = unquote(val)
		case "description":
			description = unquote(val)
		case "variables":
			inVariables = true
			// The value after "variables:" is typically empty.
		}
	}

	return name, version, description, variables, nil
}

// parseSections extracts state names, section content, and transitions
// from the markdown body. Each "## <state-name>" heading starts a new
// state. Within a state section, a "**Transitions**: [...]" line
// defines allowed transitions.
func parseSections(body string) (stateNames []string, sections map[string]string, transitions map[string][]string, err error) {
	sections = make(map[string]string)
	transitions = make(map[string][]string)

	lines := strings.Split(body, "\n")
	var currentState string
	var contentLines []string

	flushState := func() {
		if currentState == "" {
			return
		}
		content := strings.Join(contentLines, "\n")
		sections[currentState] = strings.TrimSpace(content)
		contentLines = nil
	}

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		// Check for state heading: "## state-name"
		if strings.HasPrefix(trimmed, "## ") {
			flushState()
			stateName := strings.TrimSpace(trimmed[3:])
			if stateName == "" {
				continue
			}
			currentState = stateName
			stateNames = append(stateNames, stateName)
			continue
		}

		if currentState == "" {
			continue
		}

		// Check for transitions line: "**Transitions**: [state-a, state-b]"
		if strings.HasPrefix(trimmed, "**Transitions**:") {
			targets, parseErr := parseTransitionsLine(trimmed)
			if parseErr != nil {
				return nil, nil, nil, fmt.Errorf("state %q: %w", currentState, parseErr)
			}
			transitions[currentState] = targets
			// Don't include the transitions line in the section content.
			continue
		}

		contentLines = append(contentLines, line)
	}

	flushState()

	return stateNames, sections, transitions, nil
}

// parseTransitionsLine extracts state names from a transitions line like
// "**Transitions**: [state-a, state-b]".
func parseTransitionsLine(line string) ([]string, error) {
	// Extract the part after "**Transitions**:"
	idx := strings.Index(line, ":")
	if idx < 0 {
		return nil, fmt.Errorf("malformed transitions line: %q", line)
	}
	rest := strings.TrimSpace(line[idx+1:])

	// Expect [...] format.
	if !strings.HasPrefix(rest, "[") || !strings.HasSuffix(rest, "]") {
		return nil, fmt.Errorf("transitions must be in [state1, state2] format, got: %q", rest)
	}

	inner := rest[1 : len(rest)-1]
	if strings.TrimSpace(inner) == "" {
		return nil, fmt.Errorf("transitions list is empty")
	}

	parts := strings.Split(inner, ",")
	targets := make([]string, 0, len(parts))
	for _, p := range parts {
		target := strings.TrimSpace(p)
		if target == "" {
			continue
		}
		targets = append(targets, target)
	}

	if len(targets) == 0 {
		return nil, fmt.Errorf("transitions list is empty")
	}

	return targets, nil
}

// unquote removes surrounding double quotes from a string value.
func unquote(s string) string {
	if len(s) >= 2 && s[0] == '"' && s[len(s)-1] == '"' {
		return s[1 : len(s)-1]
	}
	return s
}
