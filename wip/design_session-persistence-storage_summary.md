# Design summary: session-persistence-storage

## Input context (Phase 0)
**Source PRD:** docs/prds/PRD-session-persistence-storage.md
**Problem (implementation framing):** koto has no storage abstraction — engine state
and skill artifacts write to hardcoded paths in the git working tree. Need a session
management layer with pluggable backends (local, S3, git), implicit cloud sync in
existing commands, config system, and template path integration.

## Current status
**Phase:** 0 - Setup (PRD)
**Last Updated:** 2026-03-24
