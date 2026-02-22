// Package compile implements a source format compiler that reads .md template
// files (YAML frontmatter + markdown body) and produces template.CompiledTemplate
// values. The YAML frontmatter declares all structure; the markdown body provides
// directive content for each state.
package compile

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/tsukumogami/koto/pkg/engine"
	"github.com/tsukumogami/koto/pkg/template"

	"gopkg.in/yaml.v3"
)

// Warning represents a non-fatal issue discovered during compilation.
// Warnings don't prevent successful compilation but indicate something
// the template author should review.
type Warning struct {
	Message string
}

func (w Warning) String() string {
	return w.Message
}

// sourceFrontmatter is the YAML structure expected in the frontmatter.
type sourceFrontmatter struct {
	Name         string                       `yaml:"name"`
	Version      string                       `yaml:"version"`
	Description  string                       `yaml:"description"`
	InitialState string                       `yaml:"initial_state"`
	Variables    map[string]sourceVariableDecl `yaml:"variables"`
	States       map[string]sourceStateDecl    `yaml:"states"`
}

type sourceVariableDecl struct {
	Description string `yaml:"description"`
	Required    bool   `yaml:"required"`
	Default     string `yaml:"default"`
}

type sourceStateDecl struct {
	Transitions []string                  `yaml:"transitions"`
	Terminal    bool                      `yaml:"terminal"`
	Gates       map[string]sourceGateDecl `yaml:"gates"`
}

type sourceGateDecl struct {
	Type    string `yaml:"type"`
	Field   string `yaml:"field"`
	Value   string `yaml:"value"`
	Command string `yaml:"command"`
	Timeout int    `yaml:"timeout"`
}

// Compile reads a source format template (YAML frontmatter + markdown body)
// and produces a CompiledTemplate. It returns warnings for non-fatal issues
// and an error if compilation fails.
func Compile(source []byte) (*template.CompiledTemplate, []Warning, error) {
	header, body, err := splitFrontMatter(string(source))
	if err != nil {
		return nil, nil, err
	}

	var fm sourceFrontmatter
	if err := yaml.Unmarshal([]byte(header), &fm); err != nil {
		return nil, nil, fmt.Errorf("parse frontmatter: %w", err)
	}

	if fm.Name == "" {
		return nil, nil, fmt.Errorf("missing required field: name")
	}
	if fm.Version == "" {
		return nil, nil, fmt.Errorf("missing required field: version")
	}
	if fm.InitialState == "" {
		return nil, nil, fmt.Errorf("missing required field: initial_state")
	}
	if len(fm.States) == 0 {
		return nil, nil, fmt.Errorf("template has no states")
	}

	// Build set of declared state names for heading resolution.
	declaredStates := make(map[string]bool, len(fm.States))
	for name := range fm.States {
		declaredStates[name] = true
	}

	if !declaredStates[fm.InitialState] {
		return nil, nil, fmt.Errorf("initial_state %q is not a declared state", fm.InitialState)
	}

	// Parse body into directives keyed by state name.
	directives, warnings, err := parseBody(body, declaredStates)
	if err != nil {
		return nil, nil, err
	}

	// Every declared state must have a matching heading in the body.
	for name := range fm.States {
		if _, ok := directives[name]; !ok {
			return nil, nil, fmt.Errorf("declared state %q has no matching ## heading in body", name)
		}
	}

	// Build the compiled template.
	states := make(map[string]template.StateDecl, len(fm.States))
	for name, sd := range fm.States {
		stateDecl := template.StateDecl{
			Directive:   directives[name],
			Transitions: sd.Transitions,
			Terminal:    sd.Terminal,
		}

		if len(sd.Gates) > 0 {
			gates := make(map[string]engine.GateDecl, len(sd.Gates))
			for gn, gd := range sd.Gates {
				gates[gn] = engine.GateDecl{
					Type:    gd.Type,
					Field:   gd.Field,
					Value:   gd.Value,
					Command: gd.Command,
					Timeout: gd.Timeout,
				}
			}
			stateDecl.Gates = gates
		}

		states[name] = stateDecl
	}

	var variables map[string]template.VariableDecl
	if len(fm.Variables) > 0 {
		variables = make(map[string]template.VariableDecl, len(fm.Variables))
		for name, vd := range fm.Variables {
			variables[name] = template.VariableDecl{
				Description: vd.Description,
				Required:    vd.Required,
				Default:     vd.Default,
			}
		}
	}

	ct := &template.CompiledTemplate{
		FormatVersion: 1,
		Name:          fm.Name,
		Version:       fm.Version,
		Description:   fm.Description,
		InitialState:  fm.InitialState,
		Variables:     variables,
		States:        states,
	}

	return ct, warnings, nil
}

// Hash serializes a CompiledTemplate to deterministic JSON and returns
// the SHA-256 hash formatted as "sha256:<hex>", along with the JSON bytes.
func Hash(ct *template.CompiledTemplate) (string, []byte, error) {
	data, err := json.MarshalIndent(ct, "", "  ")
	if err != nil {
		return "", nil, fmt.Errorf("marshal compiled template: %w", err)
	}

	// Append trailing newline for consistency.
	if len(data) > 0 && data[len(data)-1] != '\n' {
		data = append(data, '\n')
	}

	sum := sha256.Sum256(data)
	hash := "sha256:" + hex.EncodeToString(sum[:])
	return hash, data, nil
}

// splitFrontMatter separates YAML frontmatter from the markdown body.
// The frontmatter is delimited by "---" lines.
func splitFrontMatter(content string) (header, body string, err error) {
	trimmed := strings.TrimLeft(content, " \t\r\n")

	if !strings.HasPrefix(trimmed, "---") {
		return "", "", fmt.Errorf("template missing opening front-matter delimiter (---)")
	}

	afterOpen := strings.Index(trimmed, "\n")
	if afterOpen < 0 {
		return "", "", fmt.Errorf("template has only an opening front-matter delimiter")
	}

	rest := trimmed[afterOpen+1:]

	closeIdx := strings.Index(rest, "\n---")
	if closeIdx < 0 {
		if strings.HasPrefix(rest, "---") {
			return "", rest[3:], nil
		}
		return "", "", fmt.Errorf("template missing closing front-matter delimiter (---)")
	}

	header = rest[:closeIdx]

	afterClose := rest[closeIdx+4:] // skip "\n---"
	nlIdx := strings.Index(afterClose, "\n")
	if nlIdx >= 0 {
		body = afterClose[nlIdx+1:]
	} else {
		body = ""
	}

	return header, body, nil
}

// parseBody splits the markdown body into per-state directive text using
// declared state names to identify state boundaries.
//
// Only ## headings whose text matches a declared state name are treated as
// state boundaries. Other headings (### Decision Criteria, ## Something Else)
// become part of the directive content for the current state.
//
// When a ## heading inside a state's content matches another declared state,
// the compiler emits a warning because the heading acts as a boundary and the
// previous state's directive may be shorter than intended.
func parseBody(body string, declaredStates map[string]bool) (map[string]string, []Warning, error) {
	directives := make(map[string]string)
	var warnings []Warning
	var lines []string

	// Split preserving line structure.
	if body != "" {
		lines = strings.Split(body, "\n")
	}

	var currentState string
	var contentLines []string

	flushState := func() {
		if currentState == "" {
			return
		}
		directives[currentState] = strings.TrimSpace(strings.Join(contentLines, "\n"))
		contentLines = nil
	}

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		// Check for ## heading.
		if strings.HasPrefix(trimmed, "## ") && !strings.HasPrefix(trimmed, "### ") {
			headingName := strings.TrimSpace(trimmed[3:])
			if headingName != "" && declaredStates[headingName] {
				// This heading matches a declared state: it's a state boundary.
				if currentState != "" {
					// Emit warning: we're inside a state and found a ## heading
					// matching another declared state.
					warnings = append(warnings, Warning{
						Message: fmt.Sprintf(
							"state %q directive contains ## heading matching state %q; is this intentional?",
							currentState, headingName,
						),
					})
				}
				flushState()
				currentState = headingName
				continue
			}
		}

		// Any line that isn't a state boundary heading is content.
		if currentState != "" {
			contentLines = append(contentLines, line)
		}
	}

	flushState()

	return directives, warnings, nil
}
