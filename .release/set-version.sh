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
