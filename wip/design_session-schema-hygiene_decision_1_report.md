# Decision 1: UUID generation approach

## Recommendation
Option B: Inline UUID v4

## Confidence
High

## Rationale
The inline approach matches the existing pattern established by `now_iso8601()`, requires no new crate, adds no transitive dependencies, and the implementation is comparable in scope — roughly 15 lines of bit manipulation and string formatting. `uuid` is absent from Cargo.lock, so adding it would be a net-new dependency. `getrandom` is already present in the lock file (three versions: 0.2.17, 0.3.4, 0.4.2), but none are direct dependencies of koto itself; they arrive transitively through `sha2`/`ring` and `tempfile`. Option C would add `getrandom` as a direct dependency without eliminating the inline bit-manipulation work. Option B does the same job in the same character count as the date arithmetic already in tree, at zero dependency cost.

## Option Analysis

### Option A: uuid crate

**What it involves:** Add `uuid = { version = "1", features = ["v4"] }` to `[dependencies]`. The crate generates UUID v4 internally using `getrandom` and formats the result as a lowercase hyphenated string. Usage at the call site is a single expression: `uuid::Uuid::new_v4().to_string()`.

**Pros:**
- No manual bit manipulation; RFC 4122 compliance is tested upstream.
- Call site is idiomatic and readable.
- `getrandom` is already in the lock file transitively, so adding `uuid` pulls in `getrandom` as a new direct dep but may not add a new resolved version.

**Cons:**
- `uuid` itself is not in Cargo.lock. Adding it is a new crate with its own transitive closure. Even with `lto = true` and `strip = true`, every new crate is additional compile-time surface.
- Inconsistent with the project philosophy. `now_iso8601()` was deliberately implemented inline rather than pulling `chrono`. Pulling `uuid` for a 15-line operation contradicts that precedent without a proportionate benefit.
- The only functionality used from the crate is one constructor and one formatter — a high crate-to-value ratio.

### Option B: Inline UUID v4

**What it involves:** Open `/dev/urandom` via `std::fs::File::open`, read 16 bytes, set version nibble (byte 6, high 4 bits = `0x40`) and variant bits (byte 8, high 2 bits = `0b10`), then format with `format!` as `{:08x}-{:04x}-{:04x}-{:04x}-{:012x}` using `u32`/`u16` subslices. The entire function is ~15 lines and mirrors `now_iso8601()` in complexity.

**Pros:**
- Zero new dependencies. No change to Cargo.toml or Cargo.lock.
- Matches the established inline pattern. A reviewer familiar with `now_iso8601()` will understand the motivation immediately.
- `/dev/urandom` is available on all Unix targets koto already supports; the `cfg(unix)` block in `Cargo.toml` confirms the project accepts Unix-specific code.
- Cryptographic quality: `/dev/urandom` is a CSPRNG on Linux, macOS, and all BSDs.
- Binary size: no additional compiled code beyond the function itself.

**Cons:**
- `/dev/urandom` is Unix-only. koto targets Unix (the `[target.'cfg(unix)'.dependencies]` block makes this explicit), so this is not a practical concern, but it should be noted in a code comment for any future Windows port.
- Manual bit-masking for RFC 4122 compliance. The implementation must set two fields correctly; a bug here would produce structurally invalid UUIDs. This risk is mitigated by a unit test that verifies the version nibble, variant bits, and hyphenated format.
- `read_exact` must be used rather than `read`, and error handling must propagate correctly. Straightforward, but a point of care.

### Option C: getrandom crate directly

**What it involves:** Add `getrandom = "0.2"` to `[dependencies]` (matching the version already in the lock file transitively). Call `getrandom::getrandom(&mut buf)` to fill 16 bytes, then apply the same bit manipulation as Option B.

**Pros:**
- `getrandom` is already resolved in the lock file, so adding it as a direct dependency would not increase the number of resolved crates on the current versions.
- Slightly more portable than `/dev/urandom` directly (getrandom abstracts over WASM, WASI, etc.).

**Cons:**
- Still requires the same RFC 4122 bit manipulation as Option B — so Option C does not eliminate the "manual" work that motivates considering Option A.
- Promotes a transitive dependency to a direct one. This is a policy cost: direct deps appear in `Cargo.toml` and are audited; transitive deps at specific versions can drift when their parents update. Making `getrandom` direct pins a concern that currently belongs to `sha2` and `tempfile`.
- The portability benefit is irrelevant — koto does not target WASM or WASI.
- No meaningful advantage over Option B given koto's Unix target constraint.

## Key Assumptions

- koto's supported targets remain Unix-only. The existing `[target.'cfg(unix)'.dependencies]` block, libc usage, and signal-hook dependency confirm this. If a Windows port were ever required, the inline `/dev/urandom` path would need a `#[cfg(unix)]` guard and a `windows` counterpart using `BCryptGenRandom`; at that point reconsidering the `uuid` crate would be reasonable.
- The inline implementation is accompanied by a unit test that verifies version nibble (`byte & 0xF0 == 0x40`), variant bits (`byte & 0xC0 == 0x80`), and hyphenated format (`8-4-4-4-12` with all-lowercase hex). Without this test the manual bit manipulation carries meaningful regression risk.
- The project philosophy of avoiding crate dependencies for trivial inline implementations holds. This is stated explicitly in the background and evidenced by `now_iso8601()`.

## Rejected Options

**Option A (uuid crate):** Ruled out because `uuid` is not currently in the dependency tree. Adding a crate whose sole used functionality is a 15-line function contradicts the pattern established by `now_iso8601()` and the project's stated preference for keeping the binary self-contained. The call-site simplicity does not justify the policy cost when the inline implementation is equally simple.

**Option C (getrandom crate):** Ruled out because it provides the worst of both worlds — a new direct dependency, the same manual bit manipulation as Option B, and no meaningful advantage given koto's Unix target. It is the weakest option under all three decision drivers simultaneously.
