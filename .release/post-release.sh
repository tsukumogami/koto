#!/usr/bin/env bash
# Post-release hook called by the finalize-release reusable workflow.
# Receives the version without v prefix (e.g., 0.4.0).
#
# Pins the koto-version default in check-template-freshness.yml so callers
# get the latest release by default.

set -euo pipefail

VERSION="${1:?Usage: post-release.sh <version>}"
TAG="v${VERSION}"
WORKFLOW=".github/workflows/check-template-freshness.yml"

if [ ! -f "$WORKFLOW" ]; then
  echo "Reusable workflow not found, skipping version pin"
  exit 0
fi

sed -i "/koto-version/,/default:/ s/default: '.*'/default: '${TAG}'/" "$WORKFLOW"

if git diff --quiet "$WORKFLOW"; then
  echo "No change needed (already pinned to ${TAG})"
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add "$WORKFLOW"
git commit -m "chore(release): pin koto-version default to ${TAG}"
git push origin HEAD
