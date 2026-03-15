---
name: koto_rust_migration
description: koto is being migrated from Go to Rust in a 5-issue single-PR plan; Issue 4 is the CLI layer (src/cli/mod.rs)
type: project
---

koto is being rewritten from Go to Rust. The migration is structured as a 5-issue plan delivered in a single PR. Issue 4 implements the CLI layer in `src/cli/mod.rs`.

**Why:** Rust migration for the orchestration engine, presumably for performance/safety/distribution reasons.

**How to apply:** When reviewing issues in this migration, understand the layering: engine persistence (`src/engine/persistence.rs`), engine types (`src/engine/types.rs`), template types (`src/template/types.rs`), cache (`src/cache.rs`), discovery (`src/discover.rs`), and CLI (`src/cli/mod.rs`). The CLI is the topmost layer and should only orchestrate calls into those lower modules.
