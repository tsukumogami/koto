---
name: koto Rust migration plan
description: koto is migrating from Go to Rust in a 5-issue single-PR plan; issues 1-5 cover scaffold, template, engine, CLI, and integration tests
type: project
---

koto is being fully rewritten from Go to Rust as a single-crate binary. The 5-issue plan
is: I1 scaffold, I2 template layer, I3 engine+persistence, I4 CLI commands, I5 integration tests.
I2 and I3 are independent of each other; both depend on I1. I4 depends on both I2 and I3.

**Why:** The planned event-sourced refactor (#46-#49) would require implementing changes
twice in Go then Rust. Migrating first avoids the double work. Pre-release means no backward
compatibility constraints.

**How to apply:** When reviewing issues in this migration, the skeleton scope is intentional.
Missing commands (init, next, rewind) in early issues are expected stubs, not gaps.
The simple JSONL state format is forward-compatible with the full event schema from #46.
