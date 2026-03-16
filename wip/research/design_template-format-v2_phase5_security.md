# Security Review: template-format-v2

## Dimension Analysis

### Download Verification

**Applies:** No

Template format v2 does not download external artifacts. Templates are local markdown files with YAML front-matter, read from disk by the compiler. The compiled output is a JSON file written to the local filesystem. No network requests are made during compilation or template loading. The design introduces no new download paths -- it changes in-memory data structures and validation rules for locally-authored files.

### Execution Isolation

**Applies:** Yes, but no new risk introduced.

Command gates execute shell commands with the same permissions as the koto process. This is an existing v1 behavior, and v2 does not change it. The three new constructs -- `accepts`, `when`, and `integration` -- are all declarative:

- **`accepts`** defines a field schema (types, required flags, allowed values). It's pure data that the compiler stores and `koto next` reads. No code execution.
- **`when`** maps field names to expected values for routing. Evaluated as equality checks against submitted evidence. No shell execution, no expression evaluation, no injection surface.
- **`integration`** is a string tag stored verbatim in compiled JSON. The compiler does not resolve or execute it. Execution responsibility belongs to the integration runner (#49), which has its own design and isolation model.

**Risk assessment:** Low. The only execution vector (command gates) is unchanged from v1. No new privilege escalation paths are introduced. The design correctly defers integration execution to a separate component.

**One note on `when` evaluation:** The design uses `serde_json::Value` for condition values, matched by equality. This is safe as long as the matching logic stays as simple equality. If future extensions add expression evaluation (regex matching, shell expansion), that would need its own security review. The current design does not do this.

### Supply Chain Risks

**Applies:** No

Templates are authored locally by the user or their team. They are not fetched from a remote registry, CDN, or package index. The compiler reads files from the local filesystem and produces local output. No new external dependencies (crates, libraries) are introduced by this design -- it modifies existing Rust structs and adds validation logic using the standard library and serde.

The `integration` tag references a processing tool by name, but the tag itself is an inert string. Resolution to an actual executable happens at runtime through project configuration (not template content), so a compromised template cannot point the integration runner at an arbitrary binary through the template format alone. The project configuration that maps tags to executables is a separate trust boundary owned by the user.

### User Data Exposure

**Applies:** Yes, but minimal and unchanged from baseline.

The `accepts` block declares what evidence fields agents should submit. This schema is stored in compiled JSON on the local filesystem. It describes data shapes (field names, types, allowed values) but does not contain or transmit actual user data.

Evidence values flow through `koto next --with-data`, which is an existing CLI path from v1. The v2 design does not add new data transmission channels, network calls, or telemetry hooks. Evidence is written to the local state file as event log entries (per #46).

**One consideration:** The `accepts` block makes evidence schemas explicit and machine-readable, which means tooling can programmatically discover what data a workflow collects. This is a feature (self-describing templates), but template authors should be aware that field names and enum values in `accepts` are visible to anyone with access to the compiled template. This is not a new exposure -- v1 gate names were equally visible -- but the structured format makes it easier to enumerate. No mitigation needed; this is working as intended.

## Recommended Outcome

**OPTION 1: No security concerns that would block this design.**

The design is narrowly scoped to declarative schema changes (type definitions, compiler validation, data structures). It introduces no new execution vectors, no network access, no external artifact fetching, and no new data transmission paths. The existing execution vector (command gates) is unchanged. The `integration` field is correctly deferred to a separate component for execution.

## Summary

Template format v2 is a declarative schema change with no security implications beyond the existing v1 baseline. It adds data structures (`accepts`, `when`, `Transition`, `FieldSchema`) and compiler validation rules, all operating on local files with no network access or new execution paths. The `integration` tag is an inert string stored in compiled JSON; execution is deferred to the integration runner (#49), which has its own isolation model. Command gates remain the only execution vector, unchanged from v1.
