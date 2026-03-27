package functional

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/cucumber/godog"
	"github.com/cucumber/godog/colors"
)

// kotoBinary holds the absolute path to the koto binary used by tests.
var kotoBinary string

// repoRoot holds the absolute path to the koto repository root.
var repoRoot string

func TestMain(m *testing.M) {
	var err error
	repoRoot, err = findRepoRoot()
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to find repo root: %v\n", err)
		os.Exit(1)
	}

	kotoBinary = os.Getenv("KOTO_TEST_BINARY")
	if kotoBinary == "" {
		kotoBinary, err = buildKoto(repoRoot)
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to build koto: %v\n", err)
			os.Exit(1)
		}
	}

	// Verify the binary exists and is executable.
	if _, err := os.Stat(kotoBinary); err != nil {
		fmt.Fprintf(os.Stderr, "koto binary not found at %s: %v\n", kotoBinary, err)
		os.Exit(1)
	}

	opts := godog.Options{
		Output: colors.Colored(os.Stdout),
		Format: "pretty",
		Paths:  []string{filepath.Join(repoRoot, "test", "functional", "features")},
	}

	status := godog.TestSuite{
		Name:                "koto-functional",
		ScenarioInitializer: InitializeScenario,
		Options:             &opts,
	}.Run()

	os.Exit(status)
}

// findRepoRoot walks up from the current directory to find the koto repo root
// (identified by Cargo.toml).
func findRepoRoot() (string, error) {
	dir, err := os.Getwd()
	if err != nil {
		return "", err
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "Cargo.toml")); err == nil {
			return dir, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", fmt.Errorf("could not find Cargo.toml in any parent directory")
		}
		dir = parent
	}
}

// buildKoto runs cargo build --release and returns the path to the binary.
func buildKoto(root string) (string, error) {
	cmd := exec.Command("cargo", "build", "--release")
	cmd.Dir = root
	cmd.Stdout = os.Stderr
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("cargo build --release failed: %w", err)
	}
	return filepath.Join(root, "target", "release", "koto"), nil
}
