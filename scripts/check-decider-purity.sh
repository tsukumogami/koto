#!/usr/bin/env bash
# Check the structural rules of src/decider/ that the compiler can't.
#
#  1. The provider-neutral files (types.rs, request.rs, evaluate.rs,
#     record.rs) do no I/O: they import nothing from std::net, std::fs, std::thread,
#     std::time::Instant or SystemTime, or attohttpc. They also never
#     mention Jev's wire vocabulary (the strings noul, criteria, "choice").
#  2. build_decider gates on DeciderSettings::opted_in() and nothing else:
#     its body has no mode comparison, key-presence check, or origin check.
#  3. build_decider is the only production code that constructs a
#     JevDecider.
#  4. No function named effective_mode exists under src/decider/, and no
#     file there orders configured modes against template modes: evaluate.rs
#     never names GlobalMode, DeciderSettings, or an ordering, and no file
#     names both GlobalMode and DeciderMode.
#
# Test modules (a `#[cfg(test)]` line followed by `mod tests`) are ignored
# for rules 3 and 4, since tests may build fixtures freely.
#
# Usage: scripts/check-decider-purity.sh [repo-root]
# Prints PASS:/FAIL: lines and exits 1 on any failure.

set -u

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
DIR="$ROOT/src/decider"
failed=0

fail() {
    echo "FAIL: $*"
    failed=1
}

pass() {
    echo "PASS: $*"
}

if [ ! -d "$DIR" ]; then
    fail "src/decider/ not found under $ROOT"
    exit 1
fi

# Print a file without its top-level test modules (from a `#[cfg(test)]`
# line followed by `mod tests` through the module's closing `}` in column 0).
strip_tests() {
    awk '
        intest { if ($0 ~ /^}/) intest = 0; next }
        /^#\[cfg\(test\)\]/ { pending = 1; held = $0; next }
        pending && /^mod tests/ { pending = 0; intest = 1; next }
        pending { print held; pending = 0 }
        { print }
    ' "$1"
}

# --- Rule 1: purity of the provider-neutral files --------------------------

for name in types.rs request.rs evaluate.rs record.rs; do
    f="$DIR/$name"
    if [ ! -f "$f" ]; then
        fail "src/decider/$name is missing"
        continue
    fi
    bad=0
    if grep -nE 'std::(net|fs|thread)\b|\battohttpc\b|\bInstant\b|\bSystemTime\b' "$f"; then
        fail "src/decider/$name imports I/O (std::net, std::fs, std::thread, Instant, SystemTime, or attohttpc)"
        bad=1
    fi
    if grep -nE 'use std::\{[^}]*\b(net|fs|thread)\b' "$f"; then
        fail "src/decider/$name imports std::net, std::fs, or std::thread in a grouped use"
        bad=1
    fi
    if grep -nE 'noul|criteria|"choice"' "$f"; then
        fail "src/decider/$name names provider wire vocabulary (noul, criteria, \"choice\")"
        bad=1
    fi
    [ "$bad" -eq 0 ] && pass "src/decider/$name is pure and provider-neutral"
done

# --- Rule 2: build_decider gates only on opted_in() -----------------------

MOD="$DIR/mod.rs"
body="$(awk '
    /^pub fn build_decider\(/ { on = 1 }
    on { print }
    on && /^}/ { exit }
' "$MOD")"
if [ -z "$body" ]; then
    fail "build_decider not found in src/decider/mod.rs"
else
    bad=0
    if ! printf '%s\n' "$body" | grep -q 'opted_in()'; then
        fail "build_decider does not call opted_in()"
        bad=1
    fi
    if printf '%s\n' "$body" | grep -nE '\bmode\b|mode\(|GlobalMode|origin|is_some|is_none|==|!=|\bmatch\b'; then
        fail "build_decider re-derives opt-in (mode, key presence, or endpoint origin check)"
        bad=1
    fi
    [ "$bad" -eq 0 ] && pass "build_decider gates only on DeciderSettings::opted_in()"
fi

# --- Rule 3: the only production JevDecider constructor -------------------

ctor_calls=0
ctor_outside=0
for f in "$DIR"/*.rs; do
    n=$(strip_tests "$f" | grep -cE 'JevDecider::new\(' || true)
    ctor_calls=$((ctor_calls + n))
    if [ "$n" -gt 0 ] && [ "$f" != "$MOD" ]; then
        ctor_outside=1
        fail "$(basename "$f") calls JevDecider::new outside tests"
    fi
done
if [ "$ctor_calls" -ne 1 ]; then
    fail "expected exactly one production JevDecider::new call (in build_decider), found $ctor_calls"
elif [ "$ctor_outside" -eq 0 ]; then
    in_body=$(printf '%s\n' "$body" | grep -cE 'JevDecider::new\(' || true)
    if [ "$in_body" -eq 1 ]; then
        pass "only build_decider constructs a JevDecider"
    else
        fail "the JevDecider::new call in mod.rs is not inside build_decider"
    fi
fi
literals=$(strip_tests "$DIR/jev.rs" | grep -cE '^[[:space:]]+JevDecider \{' || true)
if [ "$literals" -ne 1 ]; then
    fail "jev.rs should build JevDecider in exactly one place (JevDecider::new), found $literals"
fi

# --- Rule 4: no effective_mode, no ordering of modes ----------------------

bad=0
if grep -rnE '\bfn[[:space:]]+effective_mode\b' "$DIR"; then
    fail "a function named effective_mode exists under src/decider/"
    bad=1
fi
if strip_tests "$DIR/evaluate.rs" | grep -nE 'GlobalMode|DeciderSettings|config::|\.min\(|\bcmp\(|PartialOrd|\bOrd\b'; then
    fail "evaluate.rs reads settings or orders modes"
    bad=1
fi
for f in "$DIR"/*.rs; do
    src="$(strip_tests "$f")"
    if printf '%s\n' "$src" | grep -q 'GlobalMode' && printf '%s\n' "$src" | grep -q 'DeciderMode'; then
        fail "$(basename "$f") names both GlobalMode and DeciderMode (configured vs template mode ordering belongs to the engine)"
        bad=1
    fi
done
[ "$bad" -eq 0 ] && pass "no effective_mode and no mode ordering under src/decider/"

exit $failed
