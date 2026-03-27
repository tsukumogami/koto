# Design Summary: config-and-cloud-sync

## Input Context (Phase 0)
**Source:** Freeform topic (Features 2+4 from ROADMAP-session-persistence.md)
**Problem:** Sessions are machine-local. Cloud sync needs config for backend selection and credentials.
**Constraints:** Invisible sync, credentials never in project config, S3 behind feature flag, local remains default.

## Current Status
**Phase:** 0 - Setup (Freeform)
**Last Updated:** 2026-03-27
