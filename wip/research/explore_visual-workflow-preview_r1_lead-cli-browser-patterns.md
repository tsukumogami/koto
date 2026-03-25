# Lead: How do other CLI tools handle "compile and preview in browser"?

## Findings

### Pattern 1: Write File, Then Open (cargo doc --open)

Cargo generates static HTML to `target/doc/`, then opens `index.html` in the default browser using the `opener` crate. This is the simplest pattern -- no server required.

**How it works:**
- Build step produces HTML files on disk
- `opener::open(path)` calls the platform's default handler
- On Linux: tries `xdg-open`, `gio open`, `gnome-open`, `kde-open`, or `wslview` (WSL)
- On macOS: uses the `open` command
- On Windows: uses `start`
- Respects `$BROWSER` environment variable as an override

**Pitfalls documented in cargo issues:**
- Terminal blocks if browser isn't already running (Issue #5701) -- the browser process is forked but on some systems the opener blocks until the browser window closes
- WSL: `xdg-open` doesn't work; needs `wslview` from `wslu` package (Issues #7557, #12372). The `opener` crate fixed this in v0.6.1 by detecting WSL and preferring `wslview`
- On some Linux desktops, `xdg-open` opens VSCode instead of a browser for `.html` files (Issue #7447) because MIME associations are wrong
- File protocol limitations: `file://` URLs can't load ES modules, can't make fetch() requests, and have inconsistent CORS behavior across browsers (Issue #4966 suggested cargo should start a server instead)

**Verdict:** Good enough for simple static HTML. Breaks down when the HTML needs to load external resources or use modern JS features.

### Pattern 2: Local Server with Auto-Open (mdBook serve --open)

mdBook starts a local HTTP server on `localhost:3000`, then opens the URL in a browser.

**How it works:**
- Builds HTML output to a `book/` directory
- Starts an HTTP server bound to localhost
- Watches source files for changes via filesystem watcher
- Uses WebSocket connection for live-reload in the browser
- The `--open` / `-o` flag triggers `opener::open("http://localhost:3000")`
- Server runs until Ctrl-C

**Advantages over file-open:**
- No `file://` protocol restrictions -- all JS features work
- Live-reload during authoring
- Consistent behavior across platforms (HTTP is HTTP)

**Disadvantages:**
- Requires a running process (not fire-and-forget)
- Port conflicts if multiple instances run
- Heavier dependency footprint (HTTP server, WebSocket, file watcher)

**Verdict:** Best for iterative authoring workflows. Overkill for one-shot preview.

### Pattern 3: Generate Intermediate Format, Pipe to External Tool (terraform graph)

Terraform outputs DOT format to stdout: `terraform graph | dot -Tsvg > graph.svg`. The user chooses how to view it.

**How it works:**
- CLI produces a machine-readable format (DOT/Graphviz)
- User pipes to an external renderer (`dot`, `xdot`, etc.)
- No browser involvement from the CLI itself

**Ecosystem tools that add browser preview on top:**
- **Rover**: Parses Terraform state/config, serves interactive visualization on `0.0.0.0:9000`
- **Blast Radius**: Local web server with d3.js rendering of the graph
- **Pluralith**: CI integration that posts diagram images to PR comments

**Verdict:** Maximum flexibility but poor out-of-box experience. Users need to install additional tools. Works well when the intermediate format is a standard (DOT, SVG, JSON).

### Pattern 4: Static Export for Hosting (Storybook build)

Storybook's `build-storybook` produces a self-contained static site that can be served from any HTTP server or deployed to GitHub Pages.

**How it works:**
- `storybook dev` runs a local dev server with hot reload (like mdBook serve)
- `storybook build` produces a `storybook-static/` directory
- Output is a complete SPA with all assets inlined or bundled
- Can be deployed to GitHub Pages, Netlify, Vercel, etc.

**Verdict:** The "export" pattern is ideal for committable documentation. Separate from the "preview" pattern which is for live authoring.

### Crate Comparison: opener vs. open vs. webbrowser

| Crate | Downloads | Strategy | WSL Support | Notes |
|-------|-----------|----------|-------------|-------|
| `opener` | High (used by Cargo) | Platform-native commands | Yes (wslview) | Based on Cargo's implementation, improved error handling |
| `open` | High | xdg-open chain + wslview | Yes | More general-purpose (opens any file type, not just URLs) |
| `webbrowser` | Moderate | Guarantees browser opens | Yes | Only crate that guarantees a *browser* specifically opens |

**Recommendation:** `opener` is the safest choice since it's battle-tested by cargo and mdBook. `webbrowser` is better if you must guarantee a browser (not a text editor) opens the file.

### Embedding HTML in Rust Binaries

For generating self-contained HTML from a CLI, the standard patterns are:

- **`include_str!("template.html")`** -- embeds an HTML template at compile time as `&'static str`. Simple, no runtime file reads.
- **`rust-embed`** crate -- embeds entire directories (HTML, CSS, JS) into the binary. In debug mode reads from filesystem for fast iteration; in release mode embeds into binary.
- **String interpolation** -- generate the HTML by replacing placeholders in the embedded template with runtime data (the compiled workflow JSON).

The self-contained single-file HTML approach avoids all `file://` protocol issues because everything (CSS, JS, data) is inlined into one `.html` file. No external resources means no CORS, no module loading restrictions, no broken relative paths.

### Headless / SSH / CI Environment Detection

No standard Rust crate exists for detecting "can I open a browser?" but common heuristics are:

- Check `$DISPLAY` on Linux (empty = no GUI)
- Check `$SSH_CONNECTION` or `$SSH_TTY` (set = remote session)
- Check if running in CI (`$CI`, `$GITHUB_ACTIONS`, etc.)
- The `opener` crate just tries and returns an error; callers handle gracefully

The practical pattern is: attempt to open, catch the error, print the file path as fallback:
```
Wrote preview to /path/to/output.html
Could not open browser automatically. Open the file above in your browser.
```

## Implications

### For koto's Design

1. **Two distinct commands, not one.** Preview (interactive authoring aid) and export (committable documentation) have different requirements. A `--preview` flag on `template compile` could write+open a single HTML file, while `template export` could produce a directory suitable for GitHub Pages.

2. **Self-contained single-file HTML is the sweet spot for preview.** Embed a visualization template (with D3.js or similar) into the koto binary via `include_str!`. At runtime, inject the compiled workflow JSON into the template and write a single `.html` file. This avoids all `file://` protocol issues and requires no server.

3. **Use the `opener` crate for browser launching.** It handles WSL, macOS, Windows, and Linux. Fall back gracefully to printing the file path when opening fails (SSH sessions, headless CI, containers).

4. **No local server needed for v1.** A server adds complexity (port management, process lifecycle, signal handling) that isn't justified for viewing a static visualization. If live-reload during template authoring becomes important later, that's a natural v2 addition.

5. **Export for GitHub Pages is a separate concern.** The export command should produce a directory with `index.html` plus any assets, structured for static hosting. This could reuse the same HTML template but output multiple pages for multi-template projects.

6. **Avoid depending on external tools.** Unlike Terraform's "pipe to graphviz" approach, koto should produce viewable output directly. Users shouldn't need to install additional software.

## Surprises

1. **cargo doc --open still uses `file://` after 8+ years of complaints.** Issue #4966 (opened 2017) requested a local server to avoid `file://` restrictions. It's still open. This validates koto's choice to generate self-contained HTML rather than relying on `file://` behaving well.

2. **WSL browser launching was broken in cargo for years.** The `opener` crate only fixed WSL detection properly in v0.6.1. This is a real-world pitfall that koto gets for free by using `opener`, but it underscores the importance of graceful fallback messaging.

3. **The `open` crate is fragile on Linux.** Its own documentation warns it's "fragile in UNIX environments, as MIME tables can incorrectly specify applications." The `webbrowser` crate is the only one that guarantees a browser (not a random app) opens.

4. **ES modules don't work over `file://` at all.** The WHATWG HTML spec (Issue #8121) explicitly blocks `<script type="module">` from `file://` origins. Any visualization using modern JS must either be fully inlined or served over HTTP.

## Open Questions

1. **Which visualization library to embed?** D3.js is powerful but heavy. Mermaid.js renders state diagrams natively. Cytoscape.js handles large graphs well. ELK.js does automatic layout. This needs its own investigation.

2. **How large can a single-file HTML get before browsers struggle?** For 30+ state workflows with rich metadata, the inlined JSON + JS library could be several hundred KB. Need to verify this is fine.

3. **Should the export format be the same as the preview format?** Preview could be a single file; export could be a multi-page site with navigation. Or they could share the same template with different data injection.

4. **How do GitHub Pages constraints affect the export format?** GitHub Pages serves static files, but has limits on file sizes and doesn't support server-side logic. Need to verify the visualization works as a static site.

5. **Should koto support `--no-open` as the default in CI?** Or should it detect CI environments and skip the open attempt automatically?

## Summary

The dominant Rust CLI pattern is "write HTML to disk, open with `opener` crate, fall back to printing the path" -- used by cargo doc and mdBook. Self-contained single-file HTML (with JS/CSS/data inlined via `include_str!`) avoids all `file://` protocol pitfalls that have plagued cargo doc for years. For koto, the recommended approach is: embed an HTML visualization template in the binary, inject compiled workflow JSON at runtime, write one `.html` file, and attempt to open it with `opener` -- no local server needed for v1.
