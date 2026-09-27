#!/usr/bin/env bash
# Run koto's decider against the live provider with synthetic fixtures and
# check the transport and the answer schema, not which value won.
#
# Each test/decider-live/<state>.<field>.jsonl runs through
# `koto decider report --fixtures` against test/decider-live/smoke.md, on the
# default endpoint, in shadow mode, under a throwaway HOME so no ledger or user
# config outside the run is read or written. A fixture file fails the check
# when:
#   - koto exits non-zero (a refused connection or a rejected key does this);
#   - the report's case count differs from the file's line count;
#   - any case ended in no_answer or carries an error class;
#   - the endpoint wasn't the default.
# What each case answered is printed, not asserted: the provider's judgement
# isn't what this checks.
#
# Usage: scripts/decider-live-smoke.sh [koto-binary]
# Needs KOTO_DECIDER_API_KEY. Without it the script fails and sends nothing.
set -euo pipefail

KOTO="${1:-koto}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/test/decider-live"
TEMPLATE="$DIR/smoke.md"

if [ -z "${KOTO_DECIDER_API_KEY:-}" ]; then
  echo "FAIL: KOTO_DECIDER_API_KEY is not set; nothing was sent" >&2
  exit 1
fi
if [ -n "${KOTO_DECIDER_ENDPOINT:-}" ]; then
  echo "FAIL: KOTO_DECIDER_ENDPOINT is set; this check runs against the default endpoint only" >&2
  exit 1
fi

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export HOME="$scratch"
export KOTO_DECIDER=shadow

shopt -s nullglob
files=("$DIR"/*.jsonl)
if [ "${#files[@]}" -eq 0 ]; then
  echo "FAIL: no fixture files in $DIR"
  exit 1
fi

failed=0
total=0
for f in "${files[@]}"; do
  base="$(basename "$f" .jsonl)"
  state="${base%%.*}"
  field="${base#*.}"
  want="$(grep -c . "$f" || true)"
  if [ "$want" -eq 0 ]; then
    echo "FAIL: $base: the fixture file has no cases"
    failed=1
    continue
  fi

  out="$scratch/$base.json"
  rc=0
  "$KOTO" decider report --ledger "$scratch/ledger.jsonl" --fixtures "$f" \
    --template "$TEMPLATE" --state "$state" --field "$field" --json \
    >"$out" 2>"$scratch/$base.err" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: $base: koto exited $rc"
    sed 's/^/  /' "$scratch/$base.err"
    failed=1
    continue
  fi

  check() {
    if jq -e --argjson want "$want" "$2" "$out" >/dev/null; then
      echo "PASS: $base: $1"
    else
      echo "FAIL: $base: $1"
      failed=1
    fi
  }
  check "the report counts all $want cases" \
    '.fixtures.cases == $want and (.fixtures.results | length) == $want'
  check "every case got an answer" \
    '.fixtures.no_answer == 0 and all(.fixtures.results[]; .answer != "no_answer")'
  check "no case has an error class" \
    'all(.fixtures.results[]; .error_class == null)'
  check "the run used the default endpoint" \
    '.fixtures.endpoint_origin == "default"'
  jq -r '.fixtures.results[]? | "  \(.id): expected \(.expected), answered \(.answer)"' "$out"
  total=$((total + want))
done

if [ "$failed" -ne 0 ]; then
  echo "decider live smoke: FAILED"
  exit 1
fi
echo "decider live smoke: $total cases across ${#files[@]} fixture files, all answered"
