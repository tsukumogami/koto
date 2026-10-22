#!/usr/bin/env bash
# Stop reminder: print a resume hint when `koto workflows` lists active sessions.
#
# Not registered in hooks/hooks.json, so Claude Code never runs it: as a Stop
# hook it would start a command at every turn end for every koto-skills user.
# It is kept so the reminder can be checked by hand and in CI
# (.github/workflows/validate-plugins.yml runs it). The command below is the
# text the plugin once listed as its Stop hook, unchanged.

WORKFLOWS=$(koto workflows 2>/dev/null); if [ "$WORKFLOWS" != "[]" ] && [ -n "$WORKFLOWS" ] && echo "$WORKFLOWS" | grep -qE '^\[.*\]$'; then echo 'Active koto workflow detected. Run `koto next <name>` to resume.'; fi
