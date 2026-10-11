# Dashboard

`koto dashboard` is a terminal view of the sessions in your workspace. The left
pane lists sessions, ordered so the ones that need you come first. The right
pane shows the focused session in four tabs: Summary, History, Remaining and
Session. It needs a terminal at least 80 columns wide to show both panes.

## Keys

| Key | Action |
|-----|--------|
| `j`, `k`, Down, Up | Move through the session list |
| Enter | Open the focused session's detail pane |
| Esc | Return to the list |
| Tab | Switch to the next tab |
| `a` | Show or hide finished and abandoned sessions |
| `r` | Refresh now |
| `q`, Ctrl+C | Quit |

## The Session tab

The Session tab shows what a session holds: its facts and the context keys it
has stored. It reads the local copy of the session and never changes it. It
writes no context and no events, fetches no remote content, and touches no
migration marker.

The top block lists the session's state, directive, anchor (the directory it
was created in), execution directory, store origin, intent, template, session
id and creation time. A field the session doesn't record prints its name
followed by a dash, for example `Intent: —`, so a missing value is never a
blank. The directive is shown behind a gutter, up to four lines.

Below the facts is the key table, with one row per context key: its name,
recorded size, writer, and where the content lives. The last column reads
`local`, `local, stale` (the local and remote manifests disagree on the key's
hash), `remote-only` (the content is in the cloud store and not on this
machine) or `invalid-name` (the manifest names a key that fails koto's key
rules, so koto never reads it). Names, writers and every other manifest string
are escaped before they're drawn, so control characters and invisible
formatting characters appear as `\u{XXXX}` text rather than reaching your
terminal.

Under the table, the preview shows the key under the cursor. Every line of
preview content starts with a `│ ` gutter, so a value can't pass for a fact or
a table row. What you see depends on the content:

- Text shows up to the first 4096 bytes. A cut ends with a `…[cut]` marker, and
  so does an excerpt too long for the pane.
- Binary content shows its type, recorded size and recorded hash, never the
  bytes.
- A remote-only key shows that it isn't local, with the recorded size and hash.
  koto doesn't download it.
- A key whose content can't be read shows `unreadable` and the reason. The
  other keys keep rendering.

Open a session with Enter, switch to the Session tab with Tab, then use `j`,
`k`, Down and Up to move the key cursor. PageDown and PageUp move ten keys. On
this tab the detail view's cursor keys walk the keys instead of the session
list; press Esc to go back to the list and pick another session. In the list
view, `j` and `k` still move between sessions and the tab shows whichever
session is focused.

### Migrated sessions

If the session was moved to another workspace, the tab shows the facts the
local copy still has and a banner:

```
migrated to session "<target>" in workspace "<workspace>"
```

Each quoted value comes from the migration marker, so it's escaped and
shortened: `"` and `\` get a backslash, and a value longer than 128 characters
is cut with `…[cut]`. A migrated session has no key table.

### How it loads

The tab does its reading on the dashboard's normal 50 ms tick, with no
background thread. The snapshot of facts and key metadata loads when you enter
the tab and again whenever the session's state file changes. A preview is one
bounded read of the focused key, cached until you move to another key or the
state file changes, so a session with thousands of keys doesn't slow the
refresh loop. The key table builds rows only for the part on screen.

Because the reads are synchronous, a session on a hung network filesystem
stalls the dashboard the same way the History tab would.
