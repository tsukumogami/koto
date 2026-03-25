# Exploration Decisions: session-persistence-storage

## Round 1
- Files remain the agent-koto medium: agent tools (Read/Edit/Write) are optimized for files; CLI/socket alternatives lose offset/limit and targeted edit efficiency
- Koto owns location, not content: session dir API provides paths, agents manage file content directly
- No backward compatibility constraint: no existing users, so wip/ model can be replaced cleanly rather than requiring coexistence
- Terraform-style backend config: config-driven backend selection with local as default
- Bundle-level cloud sync: sync session directory at state transitions, not per-file
