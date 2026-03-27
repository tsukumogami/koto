# Decision 7: GHA reusable workflow architecture

## Alternatives

### a. Reusable workflow (`on: workflow_call`)

A reusable workflow defines its own `runs-on`, checkout, binary download, and check loop. Callers reference it with `uses: tsukumogami/koto/.github/workflows/check-templates.yml@v1` and pass inputs. The workflow is fully self-contained: the caller's YAML is 5-10 lines.

Strengths:

- Self-contained: caller doesn't manage runner, checkout, or binary installation
- Clean versioning story: callers pin `@v1` (a branch or tag), breaking changes bump major
- Matches the distribution model described in R10 ("callable via `uses:` with a tag reference")
- Can define `permissions` to minimize token scope
- Parallel job strategies (matrix) are straightforward

Weaknesses:

- Can't share a job with other caller steps (it runs in its own job with its own runner)
- Maximum 10 nesting levels, though this case only uses 1
- Secrets must be explicitly passed if needed (not relevant here -- no secrets required)

### b. Composite action

A composite action runs as steps within the caller's job. It lives under a directory (e.g., `.github/actions/check-templates/action.yml`) and uses `runs: composite`. The caller adds it as a step in their own job.

Strengths:

- Runs within the caller's existing job, so it can share workspace and steps
- More granular: caller controls the runner, can add pre/post steps in the same job
- No runner cost for a separate job

Weaknesses:

- Can't define its own `runs-on` -- caller must provide an appropriate runner
- Caller must handle checkout themselves
- More boilerplate in the caller's workflow
- Composite actions don't support `if` conditions on the action level
- Versioning requires the caller to reference the full path, which is less clean

## Recommendation

**Reusable workflow.** The PRD explicitly says "callable via `uses:` with a tag reference" (R10), which maps directly to `on: workflow_call`. The workflow needs no secrets, no shared workspace with caller steps, and no custom runner. Self-containment is the primary design goal -- callers should add 5 lines of YAML and get template freshness checking. The composite action's flexibility is unnecessary overhead for this use case.

## Workflow structure (YAML skeleton)

```yaml
name: check-template-freshness

on:
  workflow_call:
    inputs:
      template-paths:
        description: >
          Glob pattern matching template .md files to check.
          Example: 'templates/**/*.md'
        required: true
        type: string
      koto-version:
        description: >
          Koto release version to download. Use 'latest' for the most
          recent release, or pin a specific tag like 'v0.5.0'.
        required: false
        type: string
        default: 'latest'
      check-html:
        description: >
          Also check HTML freshness. When true, the workflow verifies
          both .mermaid.md and .html artifacts for each template.
        required: false
        type: boolean
        default: false
      html-output-dir:
        description: >
          Directory for HTML output files (relative to repo root).
          Only used when check-html is true. HTML files are named
          <template-stem>.html within this directory.
        required: false
        type: string
        default: 'docs'

jobs:
  check-freshness:
    name: Template Freshness
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Download koto binary
        env:
          GH_TOKEN: ${{ github.token }}
          KOTO_VERSION: ${{ inputs.koto-version }}
        run: |
          if [ "$KOTO_VERSION" = "latest" ]; then
            TAG=$(gh release view --repo tsukumogami/koto --json tagName -q '.tagName')
          else
            TAG="$KOTO_VERSION"
          fi

          ARCH=$(uname -m)
          case "$ARCH" in
            x86_64)  ASSET="koto-linux-amd64" ;;
            aarch64) ASSET="koto-linux-arm64" ;;
            *)       echo "::error::Unsupported architecture: $ARCH"; exit 1 ;;
          esac

          gh release download "$TAG" \
            --repo tsukumogami/koto \
            --pattern "$ASSET" \
            --output /usr/local/bin/koto \
            --clobber

          chmod +x /usr/local/bin/koto
          echo "Downloaded koto $TAG ($ASSET)"
          koto version

      - name: Check Mermaid freshness
        run: |
          failed=0
          while IFS= read -r template; do
            [ -z "$template" ] && continue
            stem="${template%.md}"
            output="${stem}.mermaid.md"

            echo "--- Checking: $template -> $output"
            if koto template export "$template" \
                --format mermaid \
                --output "$output" \
                --check; then
              echo "PASS: $output is fresh"
            else
              echo "::error file=${output}::Mermaid diagram is stale. Run: koto template export \"$template\" --format mermaid --output \"$output\""
              failed=1
            fi
          done < <(compgen -G '${{ inputs.template-paths }}' || true)

          if [ "$failed" -eq 1 ]; then
            echo ""
            echo "::error::One or more Mermaid diagrams are stale. See above for fix commands."
            exit 1
          fi
          echo "All Mermaid diagrams are fresh."

      - name: Check HTML freshness
        if: inputs.check-html
        run: |
          failed=0
          html_dir="${{ inputs.html-output-dir }}"

          while IFS= read -r template; do
            [ -z "$template" ] && continue
            stem=$(basename "${template%.md}")
            output="${html_dir}/${stem}.html"

            echo "--- Checking: $template -> $output"
            if koto template export "$template" \
                --format html \
                --output "$output" \
                --check; then
              echo "PASS: $output is fresh"
            else
              echo "::error file=${output}::HTML diagram is stale. Run: koto template export \"$template\" --format html --output \"$output\""
              failed=1
            fi
          done < <(compgen -G '${{ inputs.template-paths }}' || true)

          if [ "$failed" -eq 1 ]; then
            echo ""
            echo "::error::One or more HTML diagrams are stale. See above for fix commands."
            exit 1
          fi
          echo "All HTML diagrams are fresh."
```

### Caller example

```yaml
name: validate

on:
  pull_request:
    branches: [main]

jobs:
  template-freshness:
    uses: tsukumogami/koto/.github/workflows/check-template-freshness.yml@v1
    with:
      template-paths: 'plugins/koto-skills/skills/**/*.md'
      koto-version: 'latest'
```

## Inputs specification

| Input | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `template-paths` | string | yes | -- | Glob pattern for template `.md` files |
| `koto-version` | string | no | `latest` | Release tag or `latest` |
| `check-html` | boolean | no | `false` | Also verify HTML artifact freshness |
| `html-output-dir` | string | no | `docs` | Directory for HTML outputs (relative to repo root) |

The `template-paths` input is a glob, not a directory. This lets callers target specific subdirectories or exclude patterns at the shell level. The glob is expanded with `compgen -G` in bash, which handles zero matches gracefully (no error, just no iterations).

An `exclude-patterns` input was considered but dropped. Shell glob negation is fragile across bash versions, and callers can use a more specific positive glob instead. If exclusion becomes a common need, a future version can add it without breaking existing callers (additive input = non-breaking change).

## Binary download mechanism

The workflow uses `gh release download` with the caller's `github.token`. This is the same pattern used in the existing `release.yml` for verification. Key details:

- **`gh` is pre-installed** on all GitHub-hosted runners, so no extra setup step
- **`github.token`** has read access to public repos by default; no secrets needed
- **Architecture detection** via `uname -m`, mapping to the release asset names already established in `release.yml` (`koto-linux-amd64`, `koto-linux-arm64`)
- **Version resolution**: `latest` calls `gh release view` to get the current tag; pinned versions pass directly to `gh release download`
- **No checksum verification in v1**: The download happens over authenticated GitHub API (HTTPS + token). Adding checksum verification would require downloading `checksums.txt` and parsing it. This can be added in a minor version bump if users request it.

The alternative -- `curl` with a constructed URL -- works but requires manually building the URL and handling redirects. `gh release download` is cleaner and handles authentication automatically.

## Versioning strategy

Callers reference the workflow via `@v1` (a git tag or branch). The contract:

- **Non-breaking changes** (new optional inputs, bug fixes, better error messages): update `v1` tag
- **Breaking changes** (removed inputs, changed semantics of existing inputs): create `v2`

This follows the same convention as `actions/checkout@v4` and other first-party actions. The koto repo can maintain a `v1` branch that tracks the latest compatible version, or use a floating tag that gets moved on each compatible release.
