// Package cache provides a filesystem-backed compilation cache for koto templates.
//
// Compiled templates are stored in the cache directory as JSON files keyed by
// the SHA-256 hex hash of the source file contents. The cache directory defaults
// to ~/.koto/cache/ but can be overridden by setting the KOTO_HOME environment
// variable, in which case the cache directory is $KOTO_HOME/cache/.
package cache

import (
	"fmt"
	"os"
	"path/filepath"
)

// cacheDir returns the cache directory path. It checks KOTO_HOME first,
// falling back to ~/.koto/cache/.
func cacheDir() (string, error) {
	if kotoHome := os.Getenv("KOTO_HOME"); kotoHome != "" {
		return filepath.Join(kotoHome, "cache"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("resolve home directory: %w", err)
	}
	return filepath.Join(home, ".koto", "cache"), nil
}

// Get returns the cached compiled JSON for the given source hash, or nil on miss.
// The source hash should be a hex-encoded SHA-256 digest of the source file contents.
func Get(sourceHash string) ([]byte, error) {
	dir, err := cacheDir()
	if err != nil {
		return nil, err
	}

	path := filepath.Join(dir, sourceHash+".json")
	data, err := os.ReadFile(path) //nolint:gosec // G304: cache path is derived from hex hash, not user input
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read cache file: %w", err)
	}

	return data, nil
}

// Put stores compiled JSON keyed by source hash. It creates the cache directory
// if it doesn't exist.
func Put(sourceHash string, compiledJSON []byte) error {
	dir, err := cacheDir()
	if err != nil {
		return err
	}

	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("create cache directory: %w", err)
	}

	path := filepath.Join(dir, sourceHash+".json")
	if err := os.WriteFile(path, compiledJSON, 0o600); err != nil {
		return fmt.Errorf("write cache file: %w", err)
	}

	return nil
}
