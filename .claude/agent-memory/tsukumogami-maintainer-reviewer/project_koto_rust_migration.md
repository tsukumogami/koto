---
name: koto Rust migration status
description: koto is mid-migration from Go to Rust; Issue 1 scaffold completed with one blocker fixed
type: project
---

koto is being migrated from Go to Rust via a 5-issue plan (PLAN-migrate-koto-go-to-rust.md). Issue 1 (scaffold + CI) is the foundation; Issues 2-5 build on it.

Issue 1 had a blocker (unimplemented! panics) fixed in a second commit. R2 maintainer review found the fix correct with two advisory concerns about divergent exit patterns in cli/mod.rs.

**Why:** The migration replaces all Go source with a single Rust crate. All Go packages (cmd/, pkg/, internal/) are deleted.

**How to apply:** When reviewing Issues 2-5, the stub arms in cli/mod.rs (lines 75-106) use std::process::exit(1) directly — real implementations should use the Result path instead. Watch for this pattern being copied incorrectly.
