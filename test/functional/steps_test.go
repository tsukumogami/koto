package functional

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/cucumber/godog"
)

// scenarioContext holds per-scenario state.
type scenarioContext struct {
	tempDir  string
	exitCode int
	stdout   string
	stderr   string
}

var sc *scenarioContext

// InitializeScenario registers step definitions and hooks.
func InitializeScenario(ctx *godog.ScenarioContext) {
	ctx.Before(func(ctx context.Context, sc2 *godog.Scenario) (context.Context, error) {
		dir, err := os.MkdirTemp("", "koto-func-*")
		if err != nil {
			return ctx, fmt.Errorf("failed to create temp dir: %w", err)
		}
		sc = &scenarioContext{tempDir: dir}
		return ctx, nil
	})

	ctx.After(func(ctx context.Context, sc2 *godog.Scenario, err error) (context.Context, error) {
		if sc != nil && sc.tempDir != "" {
			os.RemoveAll(sc.tempDir)
		}
		return ctx, nil
	})

	// Given steps
	ctx.Step(`^a clean koto environment$`, aCleanKotoEnvironment)
	ctx.Step(`^the template "([^"]*)" exists$`, theTemplateExists)
	ctx.Step(`^I am on branch "([^"]*)"$`, iAmOnBranch)
	ctx.Step(`^the file "([^"]*)" contains "([^"]*)"$`, theFileContains)
	ctx.Step(`^the file "([^"]*)" contains:$`, theFileContainsDocString)

	// When / And steps (also used as Given for chaining)
	ctx.Step(`^I run "([^"]*)"$`, iRun)
	ctx.Step(`^I run:$`, iRunDocString)

	// Then steps
	ctx.Step(`^the exit code is (\d+)$`, theExitCodeIs)
	ctx.Step(`^the exit code is not (\d+)$`, theExitCodeIsNot)
	ctx.Step(`^the output contains "([^"]*)"$`, theOutputContains)
	ctx.Step(`^the output does not contain "([^"]*)"$`, theOutputDoesNotContain)
	ctx.Step(`^the error output contains "([^"]*)"$`, theErrorOutputContains)
	ctx.Step(`^the file "([^"]*)" exists$`, theFileExists)
	ctx.Step(`^the file "([^"]*)" does not exist$`, theFileDoesNotExist)
	ctx.Step(`^the JSON output has field "([^"]*)"$`, theJSONOutputHasField)
	ctx.Step(`^the JSON output field "([^"]*)" equals "([^"]*)"$`, theJSONOutputFieldEquals)
	ctx.Step(`^the JSON output field "([^"]*)" equals (\d+)$`, theJSONOutputFieldEqualsInt)
	ctx.Step(`^the JSON output field "([^"]*)" is (true|false)$`, theJSONOutputFieldEqualsBool)
	ctx.Step(`^the state file for "([^"]*)" exists$`, theStateFileExists)
}

// aCleanKotoEnvironment initializes a git repo in the temp dir.
func aCleanKotoEnvironment() error {
	// git init
	cmd := exec.Command("git", "init")
	cmd.Dir = sc.tempDir
	cmd.Env = gitEnv(sc.tempDir)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git init failed: %s: %w", out, err)
	}

	// git config for commits
	for _, kv := range [][2]string{
		{"user.email", "test@example.com"},
		{"user.name", "Test"},
	} {
		cmd = exec.Command("git", "config", kv[0], kv[1])
		cmd.Dir = sc.tempDir
		if out, err := cmd.CombinedOutput(); err != nil {
			return fmt.Errorf("git config %s failed: %s: %w", kv[0], out, err)
		}
	}

	// Create initial commit so HEAD exists.
	readme := filepath.Join(sc.tempDir, "README.md")
	if err := os.WriteFile(readme, []byte("test\n"), 0644); err != nil {
		return err
	}
	cmd = exec.Command("git", "add", ".")
	cmd.Dir = sc.tempDir
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git add failed: %s: %w", out, err)
	}
	cmd = exec.Command("git", "commit", "-m", "initial")
	cmd.Dir = sc.tempDir
	cmd.Env = gitEnv(sc.tempDir)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git commit failed: %s: %w", out, err)
	}

	// Create .koto/templates directory.
	templatesDir := filepath.Join(sc.tempDir, ".koto", "templates")
	return os.MkdirAll(templatesDir, 0755)
}

// theTemplateExists copies a fixture template into the scenario's .koto/templates/ dir.
func theTemplateExists(name string) error {
	fixtureDir := filepath.Join(repoRoot, "test", "functional", "fixtures", "templates")

	// Try <name>.md first, then <name>/<name>.md (like the hello-koto plugin).
	src := filepath.Join(fixtureDir, name+".md")
	if _, err := os.Stat(src); os.IsNotExist(err) {
		// Check if this is the hello-koto template from the plugins directory.
		pluginSrc := filepath.Join(repoRoot, "plugins", "koto-skills", "skills", name, name+".md")
		if _, err := os.Stat(pluginSrc); err == nil {
			src = pluginSrc
		} else {
			return fmt.Errorf("fixture template %q not found at %s or %s", name, src, pluginSrc)
		}
	}

	dst := filepath.Join(sc.tempDir, ".koto", "templates", name+".md")
	content, err := os.ReadFile(src)
	if err != nil {
		return fmt.Errorf("reading fixture %s: %w", src, err)
	}
	return os.WriteFile(dst, content, 0644)
}

// iAmOnBranch creates and checks out a branch.
func iAmOnBranch(branch string) error {
	cmd := exec.Command("git", "checkout", "-b", branch)
	cmd.Dir = sc.tempDir
	cmd.Env = gitEnv(sc.tempDir)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git checkout -b %s failed: %s: %w", branch, out, err)
	}
	return nil
}

// theFileContains writes content to a file relative to the temp dir.
func theFileContains(path, content string) error {
	full := filepath.Join(sc.tempDir, path)
	if err := os.MkdirAll(filepath.Dir(full), 0755); err != nil {
		return err
	}
	return os.WriteFile(full, []byte(content), 0644)
}

// theFileContainsDocString writes multiline content to a file relative to the temp dir.
func theFileContainsDocString(path string, content *godog.DocString) error {
	full := filepath.Join(sc.tempDir, path)
	if err := os.MkdirAll(filepath.Dir(full), 0755); err != nil {
		return err
	}
	return os.WriteFile(full, []byte(content.Content), 0644)
}

// iRun executes a command, replacing "koto" with the actual binary path.
func iRun(command string) error {
	args := splitCommand(command)
	if len(args) == 0 {
		return fmt.Errorf("empty command")
	}

	// Replace "koto" with the actual binary.
	if args[0] == "koto" {
		args[0] = kotoBinary
	}

	cmd := exec.Command(args[0], args[1:]...)
	cmd.Dir = sc.tempDir
	// Set HOME to temp dir so .koto cache is local.
	cmd.Env = append(os.Environ(),
		"HOME="+sc.tempDir,
		"XDG_CACHE_HOME="+filepath.Join(sc.tempDir, ".cache"),
	)

	var stdout, stderr strings.Builder
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()
	sc.stdout = stdout.String()
	sc.stderr = stderr.String()
	sc.exitCode = 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			sc.exitCode = exitErr.ExitCode()
		} else {
			return fmt.Errorf("command execution error: %w", err)
		}
	}
	return nil
}

// iRunDocString executes a multiline command.
func iRunDocString(command *godog.DocString) error {
	return iRun(command.Content)
}

// theExitCodeIs asserts the exit code matches.
func theExitCodeIs(expected int) error {
	if sc.exitCode != expected {
		return fmt.Errorf("expected exit code %d, got %d\nstdout: %s\nstderr: %s",
			expected, sc.exitCode, sc.stdout, sc.stderr)
	}
	return nil
}

// theExitCodeIsNot asserts the exit code does not match.
func theExitCodeIsNot(unexpected int) error {
	if sc.exitCode == unexpected {
		return fmt.Errorf("expected exit code NOT to be %d\nstdout: %s\nstderr: %s",
			unexpected, sc.stdout, sc.stderr)
	}
	return nil
}

// theOutputContains asserts stdout contains the text.
func theOutputContains(text string) error {
	if !strings.Contains(sc.stdout, text) {
		return fmt.Errorf("expected stdout to contain %q, got:\n%s", text, sc.stdout)
	}
	return nil
}

// theOutputDoesNotContain asserts stdout does not contain the text.
func theOutputDoesNotContain(text string) error {
	if strings.Contains(sc.stdout, text) {
		return fmt.Errorf("expected stdout NOT to contain %q, got:\n%s", text, sc.stdout)
	}
	return nil
}

// theErrorOutputContains asserts stderr contains the text.
func theErrorOutputContains(text string) error {
	if !strings.Contains(sc.stderr, text) {
		return fmt.Errorf("expected stderr to contain %q, got:\n%s", text, sc.stderr)
	}
	return nil
}

// theFileExists asserts a file exists relative to the temp dir.
func theFileExists(path string) error {
	full := filepath.Join(sc.tempDir, path)
	if _, err := os.Stat(full); os.IsNotExist(err) {
		return fmt.Errorf("expected file %s to exist", path)
	}
	return nil
}

// theFileDoesNotExist asserts a file does not exist relative to the temp dir.
func theFileDoesNotExist(path string) error {
	full := filepath.Join(sc.tempDir, path)
	if _, err := os.Stat(full); err == nil {
		return fmt.Errorf("expected file %s to NOT exist", path)
	}
	return nil
}

// theJSONOutputHasField parses stdout as JSON and checks field existence.
// Supports dotted paths like "decisions.count".
func theJSONOutputHasField(field string) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return fmt.Errorf("JSON field %q not found in output:\n%s", field, sc.stdout)
	}
	return nil
}

// theJSONOutputFieldEquals parses stdout as JSON and checks field string value.
func theJSONOutputFieldEquals(field, expected string) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return fmt.Errorf("JSON field %q not found in output:\n%s", field, sc.stdout)
	}
	actual := fmt.Sprintf("%v", val)
	// For string values, strip quotes.
	if s, ok := val.(string); ok {
		actual = s
	}
	if actual != expected {
		return fmt.Errorf("JSON field %q: expected %q, got %q\nfull output:\n%s",
			field, expected, actual, sc.stdout)
	}
	return nil
}

// theJSONOutputFieldEqualsBool parses stdout as JSON and checks field boolean value.
func theJSONOutputFieldEqualsBool(field, expected string) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return fmt.Errorf("JSON field %q not found in output:\n%s", field, sc.stdout)
	}
	b, ok := val.(bool)
	if !ok {
		return fmt.Errorf("JSON field %q is not a boolean: %v", field, val)
	}
	if fmt.Sprintf("%v", b) != expected {
		return fmt.Errorf("JSON field %q: expected %s, got %v\nfull output:\n%s",
			field, expected, b, sc.stdout)
	}
	return nil
}

// theJSONOutputFieldEqualsInt parses stdout as JSON and checks field numeric value.
func theJSONOutputFieldEqualsInt(field string, expected int) error {
	val, err := getJSONField(sc.stdout, field)
	if err != nil {
		return err
	}
	if val == nil {
		return fmt.Errorf("JSON field %q not found in output:\n%s", field, sc.stdout)
	}
	// JSON numbers are float64.
	num, ok := val.(float64)
	if !ok {
		return fmt.Errorf("JSON field %q is not a number: %v", field, val)
	}
	if int(num) != expected {
		return fmt.Errorf("JSON field %q: expected %d, got %v\nfull output:\n%s",
			field, expected, num, sc.stdout)
	}
	return nil
}

// theStateFileExists checks that koto-<name>.state.jsonl exists.
func theStateFileExists(name string) error {
	stateFile := filepath.Join(sc.tempDir, fmt.Sprintf("koto-%s.state.jsonl", name))
	if _, err := os.Stat(stateFile); os.IsNotExist(err) {
		return fmt.Errorf("expected state file koto-%s.state.jsonl to exist", name)
	}
	return nil
}

// getJSONField navigates a dotted path in a JSON object.
func getJSONField(jsonStr, field string) (interface{}, error) {
	// Find the last JSON line in stdout (in case there are multiple lines).
	lines := strings.Split(strings.TrimSpace(jsonStr), "\n")
	jsonLine := ""
	for i := len(lines) - 1; i >= 0; i-- {
		trimmed := strings.TrimSpace(lines[i])
		if strings.HasPrefix(trimmed, "{") {
			jsonLine = trimmed
			break
		}
	}
	if jsonLine == "" {
		return nil, fmt.Errorf("no JSON object found in output:\n%s", jsonStr)
	}

	var data interface{}
	if err := json.Unmarshal([]byte(jsonLine), &data); err != nil {
		return nil, fmt.Errorf("failed to parse JSON: %w\nraw: %s", err, jsonLine)
	}

	parts := strings.Split(field, ".")
	current := data
	for _, part := range parts {
		switch v := current.(type) {
		case map[string]interface{}:
			val, ok := v[part]
			if !ok {
				return nil, nil
			}
			current = val
		case []interface{}:
			idx := 0
			if _, err := fmt.Sscanf(part, "%d", &idx); err != nil {
				return nil, nil
			}
			if idx < 0 || idx >= len(v) {
				return nil, nil
			}
			current = v[idx]
		default:
			return nil, nil
		}
	}
	return current, nil
}

// splitCommand splits a command string into arguments, handling single-quoted
// strings (for --with-data JSON payloads).
func splitCommand(s string) []string {
	var args []string
	var current strings.Builder
	inSingleQuote := false

	for i := 0; i < len(s); i++ {
		ch := s[i]
		switch {
		case ch == '\'' && !inSingleQuote:
			inSingleQuote = true
		case ch == '\'' && inSingleQuote:
			inSingleQuote = false
		case ch == ' ' && !inSingleQuote:
			if current.Len() > 0 {
				args = append(args, current.String())
				current.Reset()
			}
		default:
			current.WriteByte(ch)
		}
	}
	if current.Len() > 0 {
		args = append(args, current.String())
	}
	return args
}

// gitEnv returns environment variables for git commands.
func gitEnv(dir string) []string {
	return append(os.Environ(),
		"GIT_AUTHOR_NAME=Test",
		"GIT_AUTHOR_EMAIL=test@example.com",
		"GIT_COMMITTER_NAME=Test",
		"GIT_COMMITTER_EMAIL=test@example.com",
		"HOME="+dir,
	)
}
