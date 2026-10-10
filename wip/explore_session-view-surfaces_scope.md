# Explore Scope: session-view-surfaces

## Visibility

Public

## Mode

auto (max 3 rounds)

## Core Question

koto is about to gain a human-readable view of a session: the state and
directive, the execution anchor and remote, and every context key with its
content and size, with large or binary values shown legibly. The view must ride
koto's two existing visualization surfaces — the Claude Code workflow view's
session rendering and the local dashboard (TUI and its `--once` CLI mode) — and
no new surface may be added. What can each surface show today, what can each
carry, and which parts of the requirement belong on which surface?

## Context

Today `koto context list` prints key names, `koto context get` prints one key's
raw bytes, and `koto status` and the dashboard print state and directive but no
content, so no existing surface answers "what does this session hold?" for a
person. The maintainers have ruled (2026-10-10) that the view works through the
two surfaces koto already ships, as applicable — the workflow view is limited
in what it can show — and that no new top-level verb, subcommand tree, or third
rendering surface (a web page, a report file) may be added. New flags and
enrichment on the existing surfaces are permitted. The eventual design's
alternatives therefore compare within the two surfaces (which carries which
part), not among surface candidates.

## In Scope

- The native workflows render path end to end: how a koto session reaches the
  Claude Code workflow view, its data contract, and its limits.
- The dashboard end to end: data, render, state, the `--once` column contract,
  and where a per-session detail could live.
- What the context store can already answer (sizes, hashes, writers, content),
  and how cloud-backed and migrated sessions behave under reads.
- How large or binary values can be presented legibly on each surface.

## Out of Scope

- Session hygiene: listing by execution directory, prune verbs, the
  terminal-state signal, bucket-marker cleanup, run-journal retention.
- Any change outside tsukumogami/koto.
- Building or prototyping the feature itself; this exploration grounds the
  scoping that follows.

## Research Leads

1. **How does a koto session render in the Claude Code workflow view today, and what is that path's data contract?** (lead-workflows-render)
   `koto workflows publish` and the native render path exist, but what exactly
   is written, when, by whom, with what size/format limits, and what of a
   session's contents (context keys, directive, anchor, remote) could ride it?

2. **What does the dashboard show today, through which modules, and where would a per-session content view fit?** (lead-dashboard)
   The TUI and `--once` paths have an established column contract
   (docs/reference/session-feed.md) and a liveness layer; what are their data
   sources, refresh model, and extension points for showing one session's keys
   and content?

3. **What can the context store and session backend already answer about a session's contents?** (lead-context-store)
   KeyMeta carries size, hash, created_at and writer; the manifest lists keys.
   What APIs exist to enumerate keys with metadata and fetch content, how do
   cloud-backed reads and the `session_migrated` refusal behave, and is there
   any text/binary detection today?

4. **How do the two surfaces handle large or binary values, and what precedents exist in koto for bounded excerpts or truncation?** (lead-large-values)
   The view must never dump raw bytes at a terminal; what truncation, size
   display, or excerpt conventions already exist anywhere in koto (directive
   delivery, details rendering, dashboard labels, JSON output contracts)?
