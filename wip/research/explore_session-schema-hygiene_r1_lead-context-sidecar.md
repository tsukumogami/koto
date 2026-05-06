# Lead: Context sidecar and context_added event

## Findings

The context sidecar is defined in `src/session/context.rs`:

```rust
pub struct KeyMeta {
    pub created_at: String,  // RFC 3339
    pub size: u64,
    pub hash: String,        // SHA-256 hex
}

pub struct Manifest {
    pub keys: BTreeMap<String, KeyMeta>,  // key = hierarchical path string
}
```

The manifest is stored at `~/.koto/sessions/<repo-id>/<session>/ctx/manifest.json`. Actual content is stored as files under `ctx/`. For cloud sessions the manifest syncs to S3 at `<prefix>/<session>/ctx/manifest.json` (see `src/session/sync.rs`).

`ContextStore::add()` writes content and updates the manifest. It does not emit any JSONL event. No existing event type represents a context addition.

The `EventPayload` enum has no `ContextAdded` variant.

## Implications

A `context_added` event must capture: `key` (the manifest key), `hash` (SHA-256 from `KeyMeta`), `size` (byte count from `KeyMeta`). The `Event.timestamp` covers timing — no need to duplicate `KeyMeta.created_at`.

Synchronous emission: the `handle_context_add` CLI handler must call `backend.append_event()` after `store.add()` returns. This requires `SessionBackend` access in the context add path. It's the only approach that preserves causal ordering relative to subsequent `koto next` calls.

## Surprises

The manifest already tracks exactly the right metadata (hash, size) for the event payload. The event fields map 1:1 to existing manifest fields.

## Open Questions

The ordering guarantee wording for the PRD: must be precise enough to be testable. Resolved: "appended during the same CLI invocation as `koto context add`, before any subsequent `koto next` call."

## Summary

Context is stored in `ctx/manifest.json` with per-key `created_at`, `size`, and `hash` fields; no JSONL event is emitted on add. A `context_added` event with `key`, `hash`, and `size` fields covers the gap, emitted synchronously from `handle_context_add`. The manifest metadata maps directly to event fields with no information loss.
