# Library Usage

koto is distributed as a compiled binary. There is no importable library interface in this release.

The Go packages (`pkg/engine`, `pkg/template`, `pkg/controller`, `pkg/discover`) were removed as part of the migration to Rust. If you need to integrate koto into a larger system, call the CLI as a subprocess and parse its JSON output.
