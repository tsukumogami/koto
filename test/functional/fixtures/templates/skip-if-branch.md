---
name: skip-if-branch
version: "1.0"
description: skip_if selects correct conditional branch based on vars.ROUTE
initial_state: triage

variables:
  ROUTE:
    description: "Routing variable; when set, triage auto-advances to main_track"
    required: false

states:
  triage:
    skip_if:
      vars.ROUTE:
        is_set: true
    transitions:
      - target: main_track
        when:
          vars.ROUTE:
            is_set: true
      - target: hotfix_track
  main_track:
    terminal: true
  hotfix_track:
    terminal: true
---

## triage

When ROUTE is set, skip_if fires and auto-advances to main_track via the
conditional transition. When ROUTE is unset, falls through to hotfix_track.

## main_track

Terminal state reached when ROUTE was set.

## hotfix_track

Terminal state reached when ROUTE was not set.
