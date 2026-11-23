#!/usr/bin/env bash
# Runs koto's tests the way CI's `tests` and `stability-tests` jobs do: under
# cargo-nextest's `ci` profile, one process per test, so process-wide state
# (environment variables, the current directory, HOME) set by one test can't
# leak into another.
#
#   run-tests.sh [<cargo package arguments>...]
#       e.g. `run-tests.sh` for the whole suite, or
#       `run-tests.sh -p koto-stability-tests` for one package.
#
# With a cargo-nextest at or above the `nextest-version` minimum in
# .config/nextest.toml it runs `cargo nextest run --locked --profile ci`, then,
# only when called with no arguments, the doctests nextest doesn't run. With
# no cargo-nextest, or an older one, it says so on stderr and runs
# `cargo test --locked ... -- --test-threads=1`, the serial run that keeps
# tests from sharing process-wide state.
#
# The verification map (.claude/shirabe-extensions/verification-map.json)
# runs its cargo-test and stability-tests commands through this script. The
# supervisor starts them with a fixed environment, so anything this script
# needs it sets or reads itself.
set -euo pipefail

config="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.config/nextest.toml"

if [ ! -f "$config" ]; then
  echo "run-tests: no nextest configuration at $config" >&2
  exit 66
fi

minimum=$(sed -n 's/^nextest-version *= *"\([0-9][0-9.]*\)".*/\1/p' "$config" | head -n 1)
if [ -z "$minimum" ]; then
  echo "run-tests: $config declares no nextest-version" >&2
  exit 66
fi

# Succeeds when version $1 is at or above version $2.
at_least() {
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]
}

nextest_ok=false
if reported=$(cargo nextest --version 2>/dev/null); then
  version=$(printf '%s\n' "$reported" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1 || true)
  if [ -n "$version" ] && at_least "$version" "$minimum"; then
    nextest_ok=true
  fi
fi

if [ "$nextest_ok" = true ]; then
  cargo nextest run --locked --profile ci "$@"
  if [ "$#" -eq 0 ]; then
    cargo test --locked --doc
  fi
else
  echo "run-tests: cargo-nextest not found; running cargo test serially (--test-threads=1)" >&2
  cargo test --locked "$@" -- --test-threads=1
fi
