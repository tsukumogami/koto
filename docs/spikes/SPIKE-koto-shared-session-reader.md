---
status: Complete
question: |
  Can the session reader that koto dashboard uses (log reading, change
  detection, liveness classification and row derivation) be separated so a
  second program reads koto sessions and their recorded context without
  taking on koto's cloud storage, TLS or HTTP dependencies, and without
  writing anything?
timebox: "1 day"
---

# SPIKE: A session reader shared outside koto

## Status

Complete

## Question

Can the code `koto dashboard` uses to read sessions be separated so another
program, such as a fleet view in shirabe, reads koto sessions and one
recorded context value per session through the same code, without linking
koto's S3 client, TLS stack or HTTP clients, and without writing to the
store? If not, what does reading session logs directly under koto's
published contract mean for a seat's holders read and a fleet snapshot
built on it?

## Context

A view of shirabe's fleet wants to read koto, and only koto, to show where
each piece of work stands. Rebuilding koto's session reading in another
program would give the logic two owners and let them drift; reading
through koto's own reader keeps it in one place and leaves koto in charge
of its storage layout. The constraint over all of it is that koto is the
only source, agents write nothing new, and their koto calls cost what they
did, so whatever reads the store must not write to it.

koto today is one package. Its lock file carries rust-s3 with rustls, the
attohttpc client, the http and url crates, and the ratatui and crossterm
terminal stack. A dependent can't link koto without them, and shirabe's own
lock is 72 packages. An earlier read of the dashboard's imports suggested a
small reader could be cut out, but it read imports and signatures only.

## Approach

1. Read the dashboard's data layer, `src/cli/dashboard_data.rs`, and what
   it imports and what imports it.
2. Build a throwaway crate on a branch that compiles koto's own reader
   source files, unchanged except for `cfg` attributes that are inert in
   koto's build, with no storage or network dependency declared. Let the
   compiler name each item that crosses the boundary.
3. Build a minimal second program on that crate. Measure both with
   `cargo tree`.
4. Run `koto dashboard --once` built from main and from the trial branch,
   and the trial program, over one copy of a real session store, and diff
   the rows.
5. Run the trial program a hundred times on a host using koto's cloud
   storage, inside a sandbox that denies every file write and every network
   call, and a hundred times unsandboxed on a store copy, comparing a full
   manifest of the copy before and after.
6. Take the crate as a git dependency, first from a scratch program, then
   from a scratch copy of shirabe, and measure what each lock gains.

The trial code is on the branch `spike/koto-shared-session-reader-trial`
(head `6a16ac7`) and is not meant to merge; excerpts are below.

## Findings

### Verdict

Yes for the logic, measured; a crate koto itself links is shown possible
but not built. The reader's code compiles in a crate with no storage or
network dependency, reports the same rows `koto dashboard` shows, and reads
a real cloud-host store without writing or connecting. The trial does this
by compiling koto's own source files a second time with about a dozen
validation and machine-identity functions fenced off. It does not have
koto link the crate. Doing that is the real split: those functions have to
move off the shared types first (they are inherent methods on a type that
would then live in another crate), and the reader's read paths have to be
separated from the write paths they share a file with.

### What the reader is

`src/cli/dashboard_data.rs` (2,731 lines) is the dashboard's data layer. It
has no terminal types. Its users are the dashboard's other three files,
`dashboard.rs`, `dashboard_state.rs` and `dashboard_render.rs`, and nothing
else. It imports:

- `engine::persistence`: `read_header`, `read_events`,
  `derive_state_from_log`, `derive_machine_state`,
  `derive_last_gate_evaluated`, `is_terminal_state`, `is_failed_state`
- `engine::types`: `StateFileHeader`, `Event`, `EventPayload`, timestamp
  parsing
- `engine::leg_pointer::read_pointer`, the request-leg sidecar
- `session::row`: `derive_row`, `latest_import`, `salient_var`
- `session::version::validate_machine_name`
- `template::types::CompiledTemplate`, for terminal states and the detail
  view
- `session::SessionBackend` and `state_file_name`, only to list sessions
  and find their files
- `session::remote_feed`, the rows other machines pushed to the bucket,
  which is where the S3 client comes in

Liveness (`classify_liveness`, `attention_key`), the modification-time
diff (`stat_and_diff`, `refresh`) and the tree (`SessionTree`, roots,
cycle detection) all live in this file.

### Module level says no, item level says yes

Followed module by module, the reader's imports reach 120 of koto's 184
modules, including `session::cloud`, `session::remote_feed` and
`decider::http`. Read that way, the split looks impossible. The items the
reader actually uses are a much smaller set, and the compiler confirms it:
22 of koto's source files compile in the trial crate, unchanged, once the
following are fenced with `#[cfg(not(koto_reader))]`:

| File | Fenced off | Why it reaches outside the reader |
|------|-----------|-----------------------------------|
| `src/template/types.rs` | two `validate` methods, `validate_request_leg_gate`, `validate_directive_rules`, `validate_clear_on_entry`, `validate_vars_condition`, `validate_evidence_routing`, `collect_pass_env_warnings` | template validation calls the request store, command environment, run journal, link keys and variables |
| `src/template/assignments.rs` | `validate_transition_assignments` | link-key checks |
| `src/session/version.rs` | `machine_name`, `user_config_machine_name`, `generate_machine_id`, `get_or_create_machine_id` | user config resolution, which carries the decider's URL handling |
| `src/engine/terminal_result.rs` | `failure_reason_for_current_run` | the context store trait |
| `src/cli/dashboard_data.rs` | the `remote_feed` import, `remote_session`, `local_view`, `local_view_of`, `drop_shadowed_remote`, `apply_snapshot`, two remote fields | the bucket feed |

In place of `SessionBackend`, the crate takes a three-method listing trait
that answers from a sessions directory the way `LocalBackend` does:

```rust
pub trait SessionSource {
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn count_unreadable(&self) -> usize;
}
```

and the dashboard file switches between the two with a pair of imports:

```rust
#[cfg(not(koto_reader))]
use crate::session::{state_file_name, SessionBackend};
#[cfg(koto_reader)]
use crate::source::{state_file_name, SessionSource as SessionBackend};
```

koto built from the trial branch compiles with no warnings, so the fences
change nothing in koto's own build.

### Measured dependency tree

`cargo tree -p koto-reader-trial -e normal` on the trial branch:

```
koto-reader-trial v0.0.0
├── anyhow v1.0.102
└── koto-session-reader v0.0.0
    ├── aho-corasick v1.1.4
    ├── anyhow v1.0.102
    ├── hex v0.4.3
    ├── libc v0.2.183
    ├── regex v1.12.3
    ├── serde v1.0.228
    ├── serde_json v1.0.149
    ├── sha2 v0.10.9
    ├── thiserror v1.0.69
    └── toml v0.8.23
```

| Graph | Distinct packages | rust-s3, rustls, attohttpc, http, url, ratatui, crossterm |
|-------|------------------:|-----------------------------------------------------------|
| koto (normal edges) | 169 | all present |
| reader crate and trial program (normal and build edges) | 40 | none |
| scratch program taking the reader as a git dependency (lock) | 41 | none |
| shirabe before (lock) | 72 | none |
| shirabe with the reader as a git dependency (lock) | 86 | none |

The trial binary is 1.5 MB, links only libSystem and libiconv, and imports
no `connect`, `socket` or `getaddrinfo` symbol. The remaining dependencies
come from code the reader doesn't need: `sha2` from the idempotency hash
on the append path, `libc` from file locks, `toml` from the leg pointer's
format, `aho-corasick` from redaction in the findings types.

### koto dashboard unchanged, same rows

The comparison used one copy of a real store (5,458 session directories)
under the local backend, network denied. Columns 3 and 7 of `--once`
(elapsed and idle) tick with the clock and were left out of the diff;
every other column was compared.

| Run | Default rows | `--all` rows |
|-----|------------:|------------:|
| `koto dashboard --once`, built from main (260c068) | 251 | 2,170 |
| `koto dashboard --once`, built from the trial branch | 251 | 2,170 |
| trial program, `once` mode | 251 | 2,170 |

All three outputs are identical, in the same order, including the
liveness column. The trial program's `once` mode reimplements only
formatting: the status bucket and token tables, which sit in the
dashboard's entry file rather than its data layer.

### One root, its states, one context value each

`list --root <name> --key <key>` walks a root and its descendants and
prints each session's parent, current state, liveness, time since its last
event, and one context value read from `ctx/<key>`:

```
execute-skill-regression-detection         -                                   plan_completion        needs-you-stalled  15h26m  https://github.com/tsukumogami/shirabe/pull/1495
execute-skill-regression-detection.step-1  execute-skill-regression-detection  done_already_complete  done               254h1m  <absent>
execute-skill-regression-detection.step-2  execute-skill-regression-detection  done                   done               252h15m <absent>
...
```

The value is read in place: koto's key grammar check, no symlink followed,
no backend call. koto's own `koto context get` can't be used for this under
the cloud backend, because it can pull and rewrite the local copy, push a
locally newer key, and always records the read (in an anchor's log or a
host-local spool). Reading the file matches what `koto context get` returns
except in one window: when a transition assigned a value and the store
write was lost, koto answers from the log. A reader that wants exact parity
applies the same log override, which needs only the event types.

### The store unchanged after a hundred reads

On this host koto uses its cloud backend, and the dashboard already reads
the backend's local mirror (`cloud.local()`), never the bucket, for its
session rows. The trial program read that mirror a hundred times, each run
a separate process under a macOS sandbox profile that denies `file-write*`
and `network*`. All hundred runs succeeded with 2,170 rows and nothing on
stderr, in 284 seconds. A write or a connection would have failed the run.

The live store changes under other sessions all the time, so the
before-and-after comparison ran on a copy: a manifest of all 557,157 files
and directories (path, size, modification and change times, content hash)
before and after a hundred unsandboxed runs was byte-identical. Nothing was
pushed: the program contains no network client to push with.

A full read of 5,458 sessions takes about 2.6 to 2.8 seconds, because every
refresh lists the directory and reads every header.

### Installing shirabe without a koto build step

Not tested end to end, because that needs a shirabe release carrying the
reader. What was tested points to yes. shirabe's documented install has
two parts and neither compiles anything: the plugin is copied by
`claude plugin install`, and the binary is a prebuilt release asset fetched
by `tsuku` or shirabe's install script. A reader dependency would be
compiled once, in shirabe's release build. A scratch copy of shirabe with
the reader as a git dependency builds in release mode and its lock goes
from 72 to 86 packages, none of them network crates. Testing the full
claim would take a shirabe release built that way and an install on a
clean machine with no koto source or Rust toolchain present.

One limit showed up here. The trial crate reaches koto's source through
`#[path]` attributes that walk out of its own directory. That works as a
git dependency, since cargo checks out the whole repository, once the
intermediate directories exist in git (an empty directory doesn't survive
a checkout; the trial needed placeholder files). It could never be
packaged for a registry, because a package can't include files outside its
directory. A real split moves the files.

### Findings for a later design

These are what the trial showed about the questions it was told to leave
open. None of them is settled here.

**The boundary of what would be shared.** The state-file types and log
parsing, row derivation, the leg pointer read, liveness, the
modification-time tree and refresh over a read-only listing trait, and a
read-only context read. Outside it: the remote bucket feed, rendering, the
backends, and template validation. Two cuts are needed that the trial only
fenced: template validation moves off `CompiledTemplate`'s inherent impls
(an extension trait or free functions in koto), and `engine::persistence`
splits its read paths from its append and lock paths, so the shared crate
can't write rather than merely not writing. Several items the reader needs
are crate-private today (`compute_elapsed_since`, for one), and the
dashboard's status buckets live in its entry file.

**How a dependent takes it, and how releases tie.** koto publishes nothing
to a registry, so a dependent would pin a git tag of a workspace member in
koto's repository. That adds a compile-time tie on top of the runtime koto
version floor shirabe already checks: the reader shirabe links has to read
the logs the user's koto writes. The log format helps: it is at schema
version 1, changes additively, rejects a higher version at the header line,
and maps an event type it doesn't know to `EventPayload::Unknown`. So an
older reader keeps reading a newer koto's logs, and the pin can trail
koto's releases. What isn't covered is everything outside the log, below.

**Room for koto#556.** That issue wants signals from outside koto recorded
beside a session's events and shown in the dashboard, tolerant of a
missing or malformed source. The boundary leaves room for it if the shared
crate keeps `EventPayload::Unknown` as the tolerant fallback, keeps row
derivation pure with facts from beside the log passed in by the caller (as
the request-leg flag already is), and reports a bad source per session
rather than failing the scan, as the unreadable tally already does.

**What the published contract covers.** `docs/STABILITY.md` freezes eight
items in `koto::engine::types`, four `SessionBackend` methods and the
versioned log format. The session directory layout, the state file name,
the `ctx/` store and its manifest, the leg pointer and the dashboard's data
layer are not promised anywhere. A shared crate that koto owns makes that
gap koto's problem rather than the dependent's, which is the point of
sharing it.

### If the split had failed

The fallback was to read session logs directly under koto's published
contract, plus one koto read for a seat's holders. It isn't needed, but the
trial shows what it would take. A direct reader would have to rely on the
directory layout, the state file name, the `ctx/` layout and the leg
pointer, none of which is published, and to repeat terminal-state detection
from the compiled template, which the log reference names as a known gap.
The contract would have to grow to cover those, or the reader would break
without warning when koto changed them.

### What this means for the holders read and the fleet snapshot

Neither changes because of a failed split. One measured fact bears on
both. The reader answers by reading every header in the store, about 2.6
to 2.8 seconds for 5,458 sessions on this host, so it can't list a seat's
holders without a scan. A holders read whose time depends on the seat's
history needs something koto records at succession, which the shared
reader would then read; the reader can't supply that alone. A fleet
snapshot on the shared reader starts from a full read today. That is fine
for a one-shot print, and a view that refreshes would rely on the
modification-time diff, which re-reads only the sessions that changed.

## Recommendation

Go. The split works for the reader's logic without koto's storage and
network dependencies and without writing, measured against koto dashboard
on a real store. Before a dependent relies on it, the real split has to
land under its own design. That means a crate koto itself links. Template
validation and machine identity have to move off the shared types, and the
read paths in `engine::persistence` have to be separated from its write
paths. It also needs a decision on what of the store layout the crate
promises. The clean-machine shirabe install stays to be shown with a real
release.
