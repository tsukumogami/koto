# Template freshness CI

koto ships a reusable GitHub Actions workflow that verifies committed
diagrams stay in sync with their source templates. Add it to any repo
that uses koto templates.

## Quick start

Add this to `.github/workflows/check-templates.yml` in your repo:

```yaml
name: Check templates

on:
  pull_request:
    branches: [main]

jobs:
  freshness:
    uses: tsukumogami/koto/.github/workflows/check-template-freshness.yml@v1
    with:
      template-paths: 'templates/**/*.md'
```

That's it. The workflow installs koto via the official install script
(with checksum verification), expands the glob, and runs
`koto template export --check` for each template. If any committed
`.mermaid.md` file is stale or missing, the check fails with an error
annotation showing the exact command to fix it.

## How it works

For each template matching the glob pattern, the workflow runs:

```bash
koto template export <template>.md --format mermaid --output <template>.mermaid.md --check
```

This compares what `export` would generate against the committed file.
If they match, the check passes (exit 0). If they differ or the file
doesn't exist, it fails (exit 1) and prints:

```
error: templates/my-workflow.mermaid.md is out of date
run: koto template export templates/my-workflow.md --format mermaid --output templates/my-workflow.mermaid.md
```

The fix command is copy-pasteable. Run it locally, commit the result,
and push.

## Inputs

| Input | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `template-paths` | string | yes | -- | Glob pattern matching template `.md` files |
| `check-html` | boolean | no | `false` | Also verify HTML diagram freshness |
| `html-output-dir` | string | no | `docs` | Directory for HTML output files (relative to repo root) |

The workflow always installs the latest koto release via the official
install script. Version pinning isn't supported yet -- pin the workflow
reference itself (`@v1`, `@main`, or a commit SHA) for reproducibility.

## HTML freshness checks

If your project deploys interactive HTML diagrams to a website (GitHub
Pages or similar), enable HTML checking:

```yaml
with:
  template-paths: 'templates/**/*.md'
  check-html: true
  html-output-dir: 'docs/diagrams'
```

This runs an additional check for each template:

```bash
koto template export <template>.md --format html --output docs/diagrams/<stem>.html --check
```

## Workflow for template authors

The recommended workflow for maintaining committed diagrams:

1. Edit the template source (`.md` file)
2. Run `koto template export my-workflow.md --output my-workflow.mermaid.md`
3. Commit both the template and the updated diagram
4. Push -- CI verifies the diagram is fresh

If you forget step 2, CI fails and tells you the exact command to run.

## Line ending considerations

koto export always produces LF line endings regardless of platform.
To prevent false drift from Windows contributors with CRLF git settings,
add this to your repo's `.gitattributes`:

```
*.mermaid.md text eol=lf
```

## Troubleshooting

**"No templates found matching pattern"** -- The glob pattern didn't match
any files. Check the path relative to the repo root. Common mistake:
using `./templates/` instead of `templates/`.

**"koto: command not found"** -- The install script failed. Check the
workflow logs for the install step. The script requires `curl` and network
access to GitHub releases.

**Stale after a clean checkout** -- If CI fails on a fresh branch, the
diagram was never generated or was committed from a different koto version.
Run the fix command locally with the same koto version CI uses.
