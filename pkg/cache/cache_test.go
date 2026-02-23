package cache

import (
	"os"
	"path/filepath"
	"testing"
)

// setKotoHome sets KOTO_HOME for a test and restores it on cleanup.
func setKotoHome(t *testing.T, dir string) {
	t.Helper()
	old := os.Getenv("KOTO_HOME")
	t.Setenv("KOTO_HOME", dir)
	t.Cleanup(func() {
		os.Setenv("KOTO_HOME", old) //nolint:errcheck // best-effort restore
	})
}

func TestGet_Miss(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	data, err := Get("0000000000000000000000000000000000000000000000000000000000000000")
	if err != nil {
		t.Fatalf("Get() error: %v", err)
	}
	if data != nil {
		t.Errorf("Get() = %v, want nil on cache miss", data)
	}
}

func TestPut_CreatesDirectory(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	hash := "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
	content := []byte(`{"name":"test"}`)

	if err := Put(hash, content); err != nil {
		t.Fatalf("Put() error: %v", err)
	}

	// Verify the cache directory was created.
	cacheDir := filepath.Join(dir, "cache")
	info, err := os.Stat(cacheDir)
	if err != nil {
		t.Fatalf("cache directory not created: %v", err)
	}
	if !info.IsDir() {
		t.Errorf("cache path is not a directory")
	}

	// Verify the file exists.
	filePath := filepath.Join(cacheDir, hash+".json")
	if _, err := os.Stat(filePath); err != nil {
		t.Fatalf("cache file not created: %v", err)
	}
}

func TestPut_ThenGet_RoundTrip(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	hash := "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
	content := []byte(`{"name":"lifecycle","version":"1.0","states":{"start":{}}}`)

	if err := Put(hash, content); err != nil {
		t.Fatalf("Put() error: %v", err)
	}

	got, err := Get(hash)
	if err != nil {
		t.Fatalf("Get() error: %v", err)
	}
	if got == nil {
		t.Fatal("Get() returned nil after Put()")
	}
	if string(got) != string(content) {
		t.Errorf("Get() = %q, want %q", string(got), string(content))
	}
}

func TestClear_RemovesAllFiles(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	// Put two entries.
	hash1 := "1111111111111111111111111111111111111111111111111111111111111111"
	hash2 := "2222222222222222222222222222222222222222222222222222222222222222"
	if err := Put(hash1, []byte(`{"a":1}`)); err != nil {
		t.Fatalf("Put(hash1) error: %v", err)
	}
	if err := Put(hash2, []byte(`{"b":2}`)); err != nil {
		t.Fatalf("Put(hash2) error: %v", err)
	}

	// Clear the cache.
	if err := Clear(); err != nil {
		t.Fatalf("Clear() error: %v", err)
	}

	// Both entries should be misses.
	data1, err := Get(hash1)
	if err != nil {
		t.Fatalf("Get(hash1) error: %v", err)
	}
	if data1 != nil {
		t.Errorf("Get(hash1) = %v after Clear(), want nil", data1)
	}

	data2, err := Get(hash2)
	if err != nil {
		t.Fatalf("Get(hash2) error: %v", err)
	}
	if data2 != nil {
		t.Errorf("Get(hash2) = %v after Clear(), want nil", data2)
	}
}

func TestClear_NoDirectory(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	// Clear when no cache directory exists should not error.
	if err := Clear(); err != nil {
		t.Fatalf("Clear() error when no directory: %v", err)
	}
}

func TestGet_ReadError(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	// Create the cache directory and a non-readable file to trigger a read error.
	cacheDir := filepath.Join(dir, "cache")
	if err := os.MkdirAll(cacheDir, 0o700); err != nil {
		t.Fatalf("MkdirAll() error: %v", err)
	}

	hash := "badhashbadhashbadhashbadhashbadhashbadhashbadhashbadhashbadhash"
	filePath := filepath.Join(cacheDir, hash+".json")

	// Create a directory where the file should be -- reading it will fail.
	if err := os.MkdirAll(filePath, 0o700); err != nil {
		t.Fatalf("MkdirAll() error: %v", err)
	}

	_, err := Get(hash)
	if err == nil {
		t.Fatal("Get() should return error when cache file is a directory")
	}
}

func TestPut_OverwritesExisting(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	hash := "abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01abcdef01"
	original := []byte(`{"version":"1.0"}`)
	updated := []byte(`{"version":"2.0"}`)

	if err := Put(hash, original); err != nil {
		t.Fatalf("Put(original) error: %v", err)
	}
	if err := Put(hash, updated); err != nil {
		t.Fatalf("Put(updated) error: %v", err)
	}

	got, err := Get(hash)
	if err != nil {
		t.Fatalf("Get() error: %v", err)
	}
	if string(got) != string(updated) {
		t.Errorf("Get() = %q, want %q", string(got), string(updated))
	}
}

func TestCacheDir_UsesKotoHome(t *testing.T) {
	dir := t.TempDir()
	setKotoHome(t, dir)

	got, err := cacheDir()
	if err != nil {
		t.Fatalf("cacheDir() error: %v", err)
	}

	want := filepath.Join(dir, "cache")
	if got != want {
		t.Errorf("cacheDir() = %q, want %q", got, want)
	}
}

func TestCacheDir_FallsBackToHome(t *testing.T) {
	setKotoHome(t, "")

	got, err := cacheDir()
	if err != nil {
		t.Fatalf("cacheDir() error: %v", err)
	}

	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("UserHomeDir() error: %v", err)
	}

	want := filepath.Join(home, ".koto", "cache")
	if got != want {
		t.Errorf("cacheDir() = %q, want %q", got, want)
	}
}
