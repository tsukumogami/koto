#!/usr/bin/env bash
# Set-version hook called by the reusable release workflow.
# Receives the version without v prefix (e.g., 0.4.0 or 0.4.1-dev).
#
# Stamps the version in Cargo.toml, marketplace.json, and plugin.json.

set -euo pipefail

VERSION="${1:?Usage: set-version.sh <version>}"

MARKETPLACE_JSON=".claude-plugin/marketplace.json"
PLUGIN_JSON="plugins/koto-skills/.claude-plugin/plugin.json"

# Stamp Cargo.toml
sed -i "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml
echo "Stamped Cargo.toml to ${VERSION}"

# Stamp marketplace.json plugins[0].version
jq --arg v "$VERSION" '.plugins[0].version = $v' "$MARKETPLACE_JSON" > "$MARKETPLACE_JSON.tmp" \
  && mv "$MARKETPLACE_JSON.tmp" "$MARKETPLACE_JSON"
echo "Stamped $MARKETPLACE_JSON to ${VERSION}"

# Stamp plugin.json .version
jq --arg v "$VERSION" '.version = $v' "$PLUGIN_JSON" > "$PLUGIN_JSON.tmp" \
  && mv "$PLUGIN_JSON.tmp" "$PLUGIN_JSON"
echo "Stamped $PLUGIN_JSON to ${VERSION}"

# Stamp minimum koto version in SKILL.md files (strip -dev suffix for release versions)
RELEASE_VERSION="${VERSION%%-dev*}"
for versioned_file in plugins/koto-skills/skills/*/SKILL.md CLAUDE.md; do
  if [ -f "$versioned_file" ] && grep -q 'koto >= ' "$versioned_file"; then
    sed -i "s/koto >= [0-9][0-9.]*[0-9]/koto >= ${RELEASE_VERSION}/" "$versioned_file"
    echo "Stamped $versioned_file minimum version to ${RELEASE_VERSION}"
  fi
done
