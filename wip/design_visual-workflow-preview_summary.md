# Design Summary: visual-workflow-preview

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** Compiled koto workflows are JSON state graphs that need interactive visualization for debugging and documentation. Exploration evaluated rendering approaches and chose Cytoscape.js + dagre via CDN.
**Constraints:** CDN-loaded (no inlined bundles), dual-purpose output (local preview + GH Pages), vanilla JS tooltips, Mermaid as separate MVP, opener crate for browser launch.

## Current Status
**Phase:** 0 - Setup (Explore Handoff)
**Last Updated:** 2026-03-25
