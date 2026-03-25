# Lead: How should koto own session state?

## Findings

### What "ownership" means — four separable dimensions

1. **Lifecycle management**: koto creates, tracks, and destroys sessions. Init starts
   a session, transitions advance it, cancel/complete end it.
2. **Location abstraction**: agents don't hardcode paths. Koto resolves where state
   lives based on backend config.
3. **Cleanup responsibility**: koto knows what belongs to a session and can remove it
   atomically. No more manual `rm -rf wip/`.
4. **Coordination interface**: koto tracks which artifacts exist via a manifest,
   enabling resume logic without scanning the filesystem.

### Koto already owns engine state

The workflow-tool manages .state.jsonl with atomic appends, advisory flock, and
integrity hashes. This is real ownership. The gap is everything else: research
files, plans, decisions, test plans.

### API shape evaluation

**Path-based (`koto session path <key>`)**: returns a filesystem path. Agent uses
file tools. Koto manages the directory. Good for large artifacts. But koto doesn't
know what's in the files — can't validate or coordinate.

**Namespace-based (`koto session dir <namespace>`)**: returns a directory. Agent
manages files within it. Koto owns the directory lifecycle. More flexible than
per-key paths but less structured.

**Hybrid**: koto provides `session dir` for agent artifacts and manages structured
state (manifests, engine state) through its own API. Agent writes research files
to the session dir; koto writes coordination files through CLI commands.

### Koto doesn't need to understand content

Koto should manage the container, not the content. It doesn't need to parse research
markdown or understand decision reports. It just needs to know: this session has
these files, they live here, clean them up when done.

## Implications

The hybrid model is strong: koto provides a session directory, agents write freely
within it using file tools, koto handles lifecycle (create dir at init, clean at
complete, sync to cloud at boundaries). A manifest.json in the session dir tracks
what exists for resume logic.

## Open Questions

- Should the manifest be koto-managed (koto updates it on every artifact write) or
  agent-managed (agents register artifacts)?
- How does resume work if the manifest is out of sync with actual files?
