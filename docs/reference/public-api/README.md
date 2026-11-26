# Public API snapshot

`koto-engine-types-and-error.txt` lists every public item of `koto::engine::types`
and `koto::error`, one per line, sorted: structs with their public fields,
enums with their variants and variant fields, functions and methods with
their signatures, constants with their values, the traits each type
implements, and the serde and error-message attributes on all of them.
Dependents import from these two modules, and `docs/STABILITY.md` sets the
rules for changing them; the snapshot is how a change that moves them can
show it changed nothing a dependent sees.

## Regenerate and compare

From the repository root:

```bash
scripts/public-api-snapshot.sh > docs/reference/public-api/koto-engine-types-and-error.txt
git diff --exit-code docs/reference/public-api/koto-engine-types-and-error.txt
```

The script builds rustdoc's JSON output for koto and prints the listing to
stdout; cargo's build output goes to stderr. Running it twice gives
byte-identical output. A non-empty diff means the public surface changed:
either the change is intended and the regenerated file is committed with
it, or it isn't and the code needs fixing.

## Toolchain

The committed snapshot was taken with:

- rustc 1.100.0 (0ef1bc386 2026-11-09), the stable toolchain, with
  `RUSTC_BOOTSTRAP=1` set by the script so stable rustdoc emits its
  unstable JSON format (`-Z unstable-options --output-format json`)
- rustdoc JSON `format_version` 61
- jq 1.7.1

No nightly toolchain and no extra cargo tool is needed. rustdoc's JSON
format is unstable, so the script checks `format_version` and warns on
stderr when it differs from 61; regenerate the snapshot on the old and new
toolchain before comparing across a toolchain change.

## What the listing does and doesn't cover

Each item is listed under the path a dependent writes, not the path it is
defined at: `koto::error::Error` is listed as that, though it is
`EngineError` in koto's engine. The script follows `pub use` re-exports,
including into a path dependency of koto, whose rustdoc JSON it builds as
well. So when these modules move into a workspace crate that koto
re-exports at the same paths, the same command, unchanged, still lists them
under `koto::` and the snapshot stays comparable. A trait impl written in
koto for a type defined in that crate is still listed with the type.

Types inside signatures are printed as the source writes them (`String`,
`Option<TemplateIdentity>`, `crate::engine::types::Event`), not resolved to
a canonical path. Auto-trait impls (`Send`, `Sync` and the like) and blanket
impls are left out, and so are trait impls' own items, which their trait
fixes. Lint and inline attributes are left out; doc comments aren't part of
the listing.
