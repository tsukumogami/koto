# Design summary: template-variable-substitution

## Input context (Phase 0)
**Source:** /explore handoff
**Problem:** koto templates declare variables and the WorkflowInitialized event carries
a variables field, but nothing is wired up. Gate commands and directive text can't
reference instance-specific values like issue numbers or artifact prefixes.
**Constraints:** must be reusable by #71 (default action execution), must prevent
command injection in gate commands, must match existing `{{KEY}}` syntax convention

## Current status
**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-03-22
