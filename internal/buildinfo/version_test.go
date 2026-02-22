package buildinfo

import (
	"runtime/debug"
	"testing"
)

func TestDevVersion(t *testing.T) {
	tests := []struct {
		name     string
		info     *debug.BuildInfo
		expected string
	}{
		{
			name:     "no vcs info returns dev",
			info:     &debug.BuildInfo{},
			expected: "dev",
		},
		{
			name: "with revision only",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: "abc123def456789"},
				},
			},
			expected: "dev-abc123def456",
		},
		{
			name: "with short revision",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: "abc123"},
				},
			},
			expected: "dev-abc123",
		},
		{
			name: "with revision and dirty flag",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: "abc123def456789"},
					{Key: "vcs.modified", Value: "true"},
				},
			},
			expected: "dev-abc123def456-dirty",
		},
		{
			name: "with revision and clean flag",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: "abc123def456789"},
					{Key: "vcs.modified", Value: "false"},
				},
			},
			expected: "dev-abc123def456",
		},
		{
			name: "empty revision returns dev",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: ""},
				},
			},
			expected: "dev",
		},
		{
			name: "other settings ignored",
			info: &debug.BuildInfo{
				Settings: []debug.BuildSetting{
					{Key: "vcs", Value: "git"},
					{Key: "vcs.time", Value: "2025-01-15T12:00:00Z"},
					{Key: "vcs.revision", Value: "abc123def456"},
				},
			},
			expected: "dev-abc123def456",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := devVersion(tt.info)
			if got != tt.expected {
				t.Errorf("devVersion() = %q, want %q", got, tt.expected)
			}
		})
	}
}

func TestVersion_Integration(t *testing.T) {
	v := Version()

	if v == "" {
		t.Error("Version() returned empty string")
	}

	validPrefixes := []string{"v", "dev", "unknown"}
	valid := false
	for _, prefix := range validPrefixes {
		if len(v) >= len(prefix) && v[:len(prefix)] == prefix {
			valid = true
			break
		}
	}

	if !valid {
		t.Errorf("Version() = %q, expected to start with one of %v", v, validPrefixes)
	}
}
