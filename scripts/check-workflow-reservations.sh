#!/usr/bin/env bash
# check-workflow-reservations.sh - find workflow changes a person must approve
#
# Used by the /work-on verification map (.claude/shirabe-extensions/work-on.md)
# for changes to validate.yml and run-evals.yml. Compares them with the merge
# base against origin/main and prints one line per change that stays with a
# person. Adding an ordinary job, and adding it to the validate job's needs and
# result check, prints nothing.
#
# Usage: scripts/check-workflow-reservations.sh [base]  (from the repository root;
# base defaults to the merge base with origin/main)
# Exit code: 0 when nothing printed; 1 when anything printed, which the map
# reads as cannot-verify, not as a failed change. Needs git, yq and jq.
set -u

V=.github/workflows/validate.yml
R=.github/workflows/run-evals.yml
found=0
say() { echo "$1"; found=1; }

if ! command -v yq >/dev/null || ! command -v jq >/dev/null; then
  echo "yq and jq are required"
  exit 1
fi
if ! B=$(git rev-parse --verify --quiet "${1:-$(git merge-base origin/main HEAD)}^{commit}"); then
  echo "cannot resolve the base (default: the merge base with origin/main)"
  exit 1
fi

at_base() { git show "$B:$1" | yq -o=json -I=0 "$2"; }
at_head() { yq -o=json -I=0 "$2" "$1"; }

# The entry's qualifying rule rests on each workflow's trigger.
for w in "$V" "$R"; do
  [ "$(at_base "$w" .on)" = "$(at_head "$w" .on)" ] || say "$w: trigger changed"
done

# A job that touches secrets, at the base or at the head, is compared whole,
# so an edited step body next to unchanged secret env is caught too.
for w in "$V" "$R"; do
  jobs=$( { at_base "$w" .jobs; at_head "$w" .jobs; } |
    jq -r 'to_entries[] | select(.value | tojson | test("secrets")) | .key' | sort -u)
  for j in $jobs; do
    [ "$(at_base "$w" ".jobs[\"$j\"]")" = "$(at_head "$w" ".jobs[\"$j\"]")" ] ||
      say "$w: job $j uses secrets and changed"
  done
done

# Added or removed lines that widen access or let a step pass without working.
pattern='secrets([^A-Za-z0-9_]|$)|permissions:|pull_request_target|continue-on-error|if:.*false|\|\| *(echo|true|:|exit|\{)|uses: *[^ ]*\.github/workflows/'
while IFS= read -r line; do
  [ -n "$line" ] && say "changed line: $line"
done <<< "$(git diff "$B" -- "$V" "$R" | grep -vE '^(\+\+\+|---) ' | grep -E '^[-+]' | grep -E "$pattern")"

# The validate job gates the PR: nothing it needed may go, and everything it
# needs must have its result checked.
base_needs=$(git show "$B:$V" | yq '.jobs.validate.needs[]')
if [ -z "$base_needs" ]; then
  say "cannot read the validate job's needs at the base"
fi
for j in $base_needs; do
  yq -e ".jobs.validate.needs[] | select(. == \"$j\")" "$V" >/dev/null 2>&1 ||
    say "dropped from the validate job's needs: $j"
done
check=$(yq '.jobs.validate.steps[].run' "$V")
for j in $(yq '.jobs.validate.needs[]' "$V"); do
  grep -qF "needs.$j.result" <<< "$check" || say "validate needs $j but does not check its result"
done

exit "$found"
