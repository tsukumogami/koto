#!/usr/bin/env bash
# Set-version hook called by the reusable release workflow.
# Receives the version without v prefix (e.g., 0.4.0 or 0.4.1-dev).
#
# Stamps the version in Cargo.toml. The runtime version is derived from
# git tags by build.rs, but keeping Cargo.toml in sync makes the package
# metadata correct for `cargo publish` and crate registries.

set -euo pipefail

VERSION="${1:?Usage: set-version.sh <version>}"

sed -i "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml

echo "Stamped Cargo.toml to ${VERSION}"
