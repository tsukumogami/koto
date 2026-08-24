//! Every name koto states must be a name koto has (koto#216).
//!
//! v0.12.0 shipped an error message telling users to run `koto session rebind`,
//! a subcommand that does not exist. It went out behind `cargo fmt --check`,
//! `cargo clippy --all-targets`, the full test suite, and CI on every issue of
//! a fifteen-issue plan. All of it passed, and it was right to pass: none of
//! those gates asserts that a name in an error message is reachable from the
//! CLI. Worse, `tests/execution_anchor_test.rs` asserts the refusal contains
//! "rebind", so the suite did not miss the phantom verb -- it required one.
//!
//! This check resolves two kinds of name against the thing they refer to:
//!
//!   * a `koto <verb> [<subverb>]` token, against the live clap tree walked
//!     from `koto::cli::App::command()` -- never a list anyone maintains, so
//!     the check retires its own findings when a promised verb ships;
//!   * a repo-relative path, against the filesystem.
//!
//! What it deliberately does NOT flag, and why:
//!
//!   * Code identifiers. Seventy percent of backticked spans, and a measured
//!     17-23% of them are legitimately absent from source -- example state
//!     names, illustrative env vars, proposed types. Ten times the miss rate of
//!     command names. This is the rule that would get the check disabled.
//!   * Design docs, PRDs, briefs, the changelog, `tests/`, `scripts/`. They
//!     record what was true or proposed when written; preserving a rejected
//!     name is their purpose. They carry 120 of the corpus's 129 unresolved
//!     command names.
//!   * Prose outside code font. `koto writes`, `koto builds`, `koto renders`.
//!     Requiring code font takes src/'s raw unresolved count from 42 to 10, all
//!     ten genuine.
//!   * A `koto` that does not BEGIN its span or line. `Active koto workflow
//!     detected` and three like it are English sentences inside code font.
//!   * A path whose leading segment is not a real top-level entry. Without that
//!     anchor the corpus yields 93 path findings, 56 of them correct as
//!     written. The price is that a renamed top-level directory is not caught:
//!     `cmd/koto/` in CLAUDE.md is invisible here and was fixed by hand.
//!
//! Deliberate exceptions live in `tests/doc_names.allow`. A `promised` record
//! names the issue that will retire it; an `intentional` record carries a
//! witness digest so that editing the text it protects retires it instead. A
//! record matching nothing is itself a finding, in both directions, so the
//! allowlist cannot rot into permanent suppression.
//!
//! Run it: `cargo test --test doc_names`. Point it at another tree with
//! `KOTO_DOC_NAMES_ROOT=<path> cargo test --test doc_names`.

use clap::CommandFactory;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- verb set

/// The command surface, walked from clap: full paths, plus the first words
/// that own children.
#[derive(Debug, Clone, Default)]
pub struct Verbs {
    paths: BTreeSet<String>,
    parents: BTreeSet<String>,
}

impl Verbs {
    fn from_paths<I: IntoIterator<Item = String>>(iter: I) -> Self {
        let paths: BTreeSet<String> = iter.into_iter().collect();
        let parents = paths
            .iter()
            .filter_map(|p| p.split_once(' ').map(|(head, _)| head.to_string()))
            .collect();
        Verbs { paths, parents }
    }

    /// A token resolves when it is a full path, or when it is two words whose
    /// first word is a verb that owns no children -- in which case the second
    /// word is an argument.
    ///
    /// The parent check is what makes this check work at all. Without it
    /// `session rebind` falls back to `session`, which is a real verb, and the
    /// defect that motivated koto#216 reports nothing.
    fn resolves(&self, token: &str) -> bool {
        if self.paths.contains(token) {
            return true;
        }
        match token.split_once(' ') {
            Some((head, _)) => self.paths.contains(head) && !self.parents.contains(head),
            None => false,
        }
    }
}

fn walk(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
    for sub in cmd.get_subcommands() {
        let path = if prefix.is_empty() {
            sub.get_name().to_string()
        } else {
            format!("{prefix} {}", sub.get_name())
        };
        out.push(path.clone());
        for alias in sub.get_all_aliases() {
            let aliased = match prefix {
                "" => alias.to_string(),
                p => format!("{p} {alias}"),
            };
            out.push(aliased);
        }
        walk(sub, &path, out);
    }
}

/// koto's own command surface. The only place the clap tree is read.
pub fn koto_verbs() -> Verbs {
    let cmd = koto::cli::App::command();
    let mut out = Vec::new();
    walk(&cmd, "", &mut out);
    Verbs::from_paths(out)
}

// ---------------------------------------------------------------- surfaces

/// The checked surfaces. This list is the definition of what is in scope, not
/// a sample of it: anything not named here is out, including `docs/designs/`,
/// `docs/prds/`, `docs/briefs/`, `CHANGELOG.md`, `tests/`, `test/`, `benches/`,
/// `scripts/`, and `.github/`.
const DIRS: &[&str] = &[
    "src",
    "plugins/koto-skills",
    "docs/guides",
    "docs/reference",
    "docs/testing",
];

const FILES: &[&str] = &[
    "docs/STABILITY.md",
    "docs/workspace-layout.md",
    "README.md",
    "CLAUDE.md",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Flavor {
    Rust,
    Markdown,
    /// Code font throughout: a skill package's hooks and scripts are executed,
    /// not merely read, so a phantom verb in one fails a real workflow.
    CodeFile,
}

fn flavor(path: &Path, under_plugins: bool) -> Option<Flavor> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Some(Flavor::Rust),
        Some("md") | Some("mdc") => Some(Flavor::Markdown),
        Some("json") | Some("sh") if under_plugins => Some(Flavor::CodeFile),
        None if under_plugins => Some(Flavor::CodeFile),
        _ => None,
    }
}

fn collect(root: &Path) -> Vec<(PathBuf, Flavor)> {
    let mut out = Vec::new();
    for dir in DIRS {
        let base = root.join(dir);
        let under_plugins = dir.starts_with("plugins");
        walk_dir(&base, under_plugins, &mut out);
    }
    for f in FILES {
        let p = root.join(f);
        if p.is_file() {
            if let Some(fl) = flavor(&p, false) {
                out.push((p, fl));
            }
        }
    }
    out.sort();
    out
}

fn walk_dir(dir: &Path, under_plugins: bool, out: &mut Vec<(PathBuf, Flavor)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // An eval fixture naming a phantom verb or an illustrative path is
            // doing its job, the same reason `tests/` and `#[cfg(test)]` are
            // out of scope.
            if p.file_name().is_some_and(|n| n == "evals") {
                continue;
            }
            walk_dir(&p, under_plugins, out);
        } else if let Some(fl) = flavor(&p, under_plugins) {
            out.push((p, fl));
        }
    }
}

// ---------------------------------------------------------------- candidates

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Command,
    Path,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub kind: Kind,
    pub token: String,
    pub file: String,
    pub line: usize,
    /// The innermost enclosing code span, whitespace-collapsed. Hashed into an
    /// intentional record's witness, so it must not depend on where a
    /// formatter wrapped a literal.
    pub span: String,
}

fn normalize_span(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Inline code spans: split on backticks and take the odd indices. A regex
/// pairing `` `...` `` matches the closing tick of one span with the opening
/// tick of the next.
fn code_spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

/// The rest of `text` after a `koto` that begins it, or None. The word must be
/// delimited on both sides: `hello-koto` never fires, and `koto::cli` is a Rust
/// path rather than an invocation.
fn anchored(text: &str) -> Option<&str> {
    let t = text.trim_start();
    let rest = t.strip_prefix("koto")?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        Some(_) => None,
    }
}

fn command_token(rest: &str) -> Option<String> {
    let mut words = Vec::new();
    for w in rest.split_whitespace() {
        if words.len() == 2 {
            break;
        }
        let ok = !w.is_empty()
            && w.starts_with(|c: char| c.is_ascii_lowercase())
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok {
            break;
        }
        words.push(w);
    }
    if words.is_empty() {
        // A bare `koto` names the binary, not an invocation.
        None
    } else {
        Some(words.join(" "))
    }
}

/// Join Rust `\` string continuations, keeping the line each logical line
/// starts on. Without this the three v0.12.0 literals are invisible, which is
/// the exact case koto#216 exists for.
fn join_continuations(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let start = i;
        let mut buf = lines[i].trim_end().to_string();
        while buf.ends_with('\\') && i + 1 < lines.len() {
            buf.pop();
            let joined = format!("{} {}", buf.trim_end(), lines[i + 1].trim());
            buf = joined;
            i += 1;
        }
        // Report the line carrying the phrase rather than the one the literal
        // opens on, so a reader following the citation finds it.
        out.push((start + 1, buf));
        i += 1;
    }
    out
}

/// Rust string literals on a logical line, contents only.
fn string_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '"' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < bytes.len() {
                if bytes[j] == '\\' && j + 1 < bytes.len() {
                    buf.push(bytes[j + 1]);
                    j += 2;
                    continue;
                }
                if bytes[j] == '"' {
                    break;
                }
                buf.push(bytes[j]);
                j += 1;
            }
            out.push(buf);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn push_command(cands: &mut Vec<Candidate>, ctx: &str, file: &str, line: usize) {
    if let Some(rest) = anchored(ctx) {
        if let Some(token) = command_token(rest) {
            cands.push(Candidate {
                kind: Kind::Command,
                token,
                file: file.to_string(),
                line,
                span: normalize_span(ctx),
            });
        }
    }
}

fn extract_markdown(text: &str, file: &str, code_file: bool, cands: &mut Vec<Candidate>) {
    let mut in_fence = false;
    // A shell string wrapped across `\` continuations is one logical line, the
    // same as a Rust literal: without joining, a continuation that happens to
    // begin with `koto` looks line-initial and prose becomes an invocation.
    // Markdown prose is not joined, where a trailing `\` is a line break.
    let lines: Vec<(usize, String)> = if code_file {
        join_continuations(text)
    } else {
        text.lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .collect()
    };
    for (n, raw) in lines {
        let raw = raw.as_str();
        let trimmed = raw.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        // A fence line, or every line of a code file, is code font: an
        // occurrence anchors when it begins the line, after an optional prompt.
        if in_fence || code_file {
            // A `#` comment in a script is prose, the same as a `//` comment in
            // Rust: `# koto sessions run here will render into ...` is a
            // sentence about koto, not an invocation of it.
            if !(code_file && trimmed.starts_with('#')) {
                let body = trimmed.strip_prefix("$ ").unwrap_or(trimmed);
                push_command(cands, body, file, n);
                path_candidates(body, file, n, cands);
            }
        }
        for span in code_spans(raw) {
            push_command(cands, span, file, n);
            path_candidates(span, file, n, cands);
        }
        if !in_fence && !code_file {
            // Paths in prose are only candidates inside code font; nothing
            // else here reads bare prose.
        }
    }
}

fn extract_rust(text: &str, file: &str, cands: &mut Vec<Candidate>) {
    let mut depth_skip: Option<usize> = None;
    let mut brace_depth = 0usize;
    for (n, logical) in join_continuations(text) {
        let trimmed = logical.trim();

        // Skip `#[cfg(test)]` modules: a fixture naming a phantom verb is
        // doing its job, the same reason `tests/` is out of scope.
        if trimmed.starts_with("#[cfg(test)]") {
            depth_skip = Some(brace_depth);
        }
        let opens = logical.matches('{').count();
        let closes = logical.matches('}').count();
        if let Some(d) = depth_skip {
            brace_depth = brace_depth + opens - closes.min(brace_depth + opens);
            if brace_depth <= d && closes > 0 {
                depth_skip = None;
            }
            continue;
        }
        brace_depth = brace_depth + opens - closes.min(brace_depth + opens);

        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            // A doc comment is prose, so only a backticked span anchors.
            // Without this, sentences that wrap onto a line beginning with
            // `koto` -- "koto now has two log families", and three like it --
            // are findings.
            let body = &trimmed[3..];
            for span in code_spans(body) {
                push_command(cands, span, file, n);
                path_candidates(span, file, n, cands);
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        for lit in string_literals(&logical) {
            // A literal is code font in itself, so it self-anchors; a
            // backticked span inside it is the other anchor. Alternatives, not
            // a sequence: a future author may write the instruction with no
            // backticks at all.
            push_command(cands, &lit, file, n);
            path_candidates(&lit, file, n, cands);
            for span in code_spans(&lit) {
                push_command(cands, span, file, n);
                path_candidates(span, file, n, cands);
            }
        }
    }
}

// ---------------------------------------------------------------- paths

/// Leading segments excluded because koto's root shares its directory names
/// with every project, so a guide instructing the reader about THEIR repo
/// anchors against ours and fires. `.github` is deliberately absent: excluding
/// it silences one false positive and blinds three live internal citations.
const PATH_SEGMENT_EXCLUSIONS: &[&str] = &[".claude", "target"];

fn path_candidates(span: &str, file: &str, line: usize, cands: &mut Vec<Candidate>) {
    for tok in span.split_whitespace() {
        // Trim wrapping punctuation until it stops changing: a markdown link
        // that ends a sentence leaves both a paren and a period behind, and
        // one pass strips only the outermost.
        let mut tok = tok;
        loop {
            let next = tok
                .trim_matches(|c: char| {
                    matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\'' | '`')
                })
                .trim_end_matches('.');
            if next == tok {
                break;
            }
            tok = next;
        }
        if let Some(t) = path_token(tok) {
            cands.push(Candidate {
                kind: Kind::Path,
                token: t,
                file: file.to_string(),
                line,
                span: normalize_span(span),
            });
        }
    }
}

/// A repo-relative path candidate, or None. Rejects placeholders, absolute and
/// home-rooted paths, `..` traversal, and the excluded leading segments.
fn path_token(tok: &str) -> Option<String> {
    if !tok.contains('/') {
        return None;
    }
    if tok.contains('<') || tok.contains("{{") || tok.contains('*') || tok.contains('$') {
        return None;
    }
    if tok.starts_with('/') || tok.starts_with('~') || tok.starts_with("./") {
        return None;
    }
    if tok.contains("://") {
        return None;
    }
    // Strip a `:line`, `:start-end`, or `::symbol` citation suffix.
    let base = match tok.split_once("::") {
        Some((b, _)) => b,
        None => match tok.split_once(':') {
            Some((b, tail))
                if tail.chars().all(|c| c.is_ascii_digit() || c == '-') && !tail.is_empty() =>
            {
                b
            }
            Some(_) => return None,
            None => tok,
        },
    };
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let mut segs = base.split('/');
    let head = segs.next()?;
    if head.is_empty() || head == ".." || PATH_SEGMENT_EXCLUSIONS.contains(&head) {
        return None;
    }
    if base.split('/').any(|s| s == "..") {
        return None;
    }
    // A dotted namespace token (`gates.*`, `request_store.recursion`) is not a
    // path; a first segment with a dot and no slash-borne extension is prose.
    if head.contains('.') && !head.starts_with('.') {
        return None;
    }
    Some(base.to_string())
}

fn path_resolves(root: &Path, token: &str) -> bool {
    let mut segs = token.split('/');
    let Some(head) = segs.next() else {
        return false;
    };
    // Candidacy is anchored on a real top-level entry.
    let anchored = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy() == head)
        })
        .unwrap_or(false);
    if !anchored {
        // Not a candidate at all rather than a finding: this is the accepted
        // false negative, and it is why `cmd/koto/` was fixed by hand.
        return true;
    }
    std::fs::symlink_metadata(root.join(token)).is_ok()
}

// ---------------------------------------------------------------- allowlist

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Category {
    Promised,
    Intentional,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub kind: Kind,
    pub token: String,
    pub category: Category,
    pub issue: String,
    pub witness: String,
    pub reason: String,
}

pub fn parse_allow(text: &str) -> Result<Vec<Record>, Vec<String>> {
    let mut out: Vec<Record> = Vec::new();
    let mut errs = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let n = n + 1;
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(6, '\t').collect();
        if parts.len() < 6 {
            errs.push(format!(
                "line {n}: expected 6 tab-separated fields, found {}",
                parts.len()
            ));
            continue;
        }
        let kind = match parts[0] {
            "command" => Kind::Command,
            "path" => Kind::Path,
            other => {
                errs.push(format!("line {n}: unknown kind {other:?}"));
                continue;
            }
        };
        let category = match parts[2] {
            "promised" => Category::Promised,
            "intentional" => Category::Intentional,
            other => {
                errs.push(format!("line {n}: unknown category {other:?}"));
                continue;
            }
        };
        let issue = parts[3].to_string();
        let witness = parts[4].to_string();
        let reason = parts[5].trim().to_string();
        if reason.is_empty() {
            errs.push(format!("line {n}: reason may not be empty"));
            continue;
        }
        match category {
            Category::Promised => {
                if !is_issue_ref(&issue) {
                    errs.push(format!(
                        "line {n}: a promised record needs an owner/repo#N issue, found {issue:?}"
                    ));
                    continue;
                }
                if witness != "-" {
                    errs.push(format!(
                        "line {n}: a promised record carries no witness, found {witness:?}"
                    ));
                    continue;
                }
            }
            Category::Intentional => {
                if issue != "-" {
                    errs.push(format!(
                        "line {n}: an intentional record carries no issue reference, found \
                         {issue:?} -- a name an issue intends to create is promised"
                    ));
                    continue;
                }
                if witness != "?" && !is_witness(&witness) {
                    errs.push(format!(
                        "line {n}: witness must be '?' or 8 hex characters, found {witness:?}"
                    ));
                    continue;
                }
            }
        }
        if out.iter().any(|r| r.kind == kind && r.token == parts[1]) {
            errs.push(format!("line {n}: duplicate record for {:?}", parts[1]));
            continue;
        }
        out.push(Record {
            kind,
            token: parts[1].to_string(),
            category,
            issue,
            witness,
            reason,
        });
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

fn is_issue_ref(s: &str) -> bool {
    let Some((repo, num)) = s.split_once('#') else {
        return false;
    };
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

fn is_witness(s: &str) -> bool {
    s.len() == 8
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// A short digest over the normalized spans a record suppresses.
///
/// The member key is `(file, ordinal of this token's occurrences in that file)`
/// -- scoped to the token so an unrelated invocation added above a protected
/// sentence does not renumber it, and an ordinal rather than a line number so
/// an insertion anywhere above does not either.
fn witness_for(members: &[(String, usize, String)]) -> String {
    let mut sorted: Vec<String> = members
        .iter()
        .map(|(f, ord, span)| format!("{f}\t{ord}\t{span}"))
        .collect();
    sorted.sort();
    let joined = sorted.join("\n");
    short_hash(&joined)
}

/// FNV-1a, 64-bit, rendered as 8 hex characters. A digest here only has to
/// change when the text does; it is not a security primitive, and using it
/// avoids adding a hashing dependency for a check that has none.
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

// ---------------------------------------------------------------- scan

#[derive(Debug, Clone)]
pub struct Finding {
    pub headline: String,
    pub sites: Vec<String>,
    pub remedy: String,
}

/// The whole check, as a pure function of a corpus root, a verb set, and an
/// allowlist. The repository test is the only caller that derives `verbs` from
/// koto's own clap tree; fixtures supply their own.
pub fn scan(root: &Path, verbs: &Verbs, allow: &[Record]) -> Vec<Finding> {
    let mut cands = Vec::new();
    for (path, fl) in collect(root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match fl {
            Flavor::Rust => extract_rust(&text, &rel, &mut cands),
            Flavor::Markdown => extract_markdown(&text, &rel, false, &mut cands),
            Flavor::CodeFile => extract_markdown(&text, &rel, true, &mut cands),
        }
    }

    // Unresolved candidates, grouped by token, with a per-file ordinal.
    let mut unresolved: BTreeMap<(String, String), Vec<Candidate>> = BTreeMap::new();
    for c in cands {
        let ok = match c.kind {
            Kind::Command => verbs.resolves(&c.token),
            Kind::Path => path_resolves(root, &c.token),
        };
        if ok {
            continue;
        }
        let key = (
            match c.kind {
                Kind::Command => "command",
                Kind::Path => "path",
            }
            .to_string(),
            c.token.clone(),
        );
        let bucket = unresolved.entry(key).or_default();
        if !bucket
            .iter()
            .any(|e| e.file == c.file && e.line == c.line && e.span == c.span)
        {
            bucket.push(c);
        }
    }

    let mut findings = Vec::new();
    let mut matched: BTreeSet<(String, String)> = BTreeSet::new();

    for ((kind, token), sites) in &unresolved {
        let rec = allow
            .iter()
            .find(|r| kind_str(&r.kind) == kind && &r.token == token);
        let members = members_of(sites);
        match rec {
            None => findings.push(Finding {
                headline: match kind.as_str() {
                    "command" => {
                        format!("unresolved command `koto {token}` ({} sites)", sites.len())
                    }
                    _ => format!("unresolved path `{token}` ({} sites)", sites.len()),
                },
                sites: sites.iter().map(site_line).collect(),
                remedy: remedy_for(kind, token),
            }),
            Some(r) => {
                matched.insert((kind.clone(), token.clone()));
                if r.category == Category::Intentional {
                    let want = witness_for(&members);
                    if r.witness == "?" {
                        findings.push(Finding {
                            headline: format!(
                                "record `{token}` is unaffirmed -- its witness is still `?`"
                            ),
                            sites: sites.iter().map(site_line).collect(),
                            remedy: format!(
                                "  Adding an intentional record is a two-pass operation.\n  \
                                 Set the witness column to {want}."
                            ),
                        });
                    } else if r.witness != want {
                        findings.push(Finding {
                            headline: format!(
                                "record `{token}` needs re-affirming -- the text it protects changed"
                            ),
                            sites: sites.iter().map(site_line).collect(),
                            remedy: format!(
                                "  Re-affirming is the expected action; no prose needs correcting.\n  \
                                 Confirm every site above is still one this record should cover,\n  \
                                 then update the witness column to {want}.\n  The record says: {}",
                                r.reason
                            ),
                        });
                    }
                }
            }
        }
    }

    // A record matching nothing is a finding in its own right, in both
    // categories. This is what stops the allowlist rotting into permanent
    // suppression: the change that makes a record obsolete removes it.
    for r in allow {
        let key = (kind_str(&r.kind).to_string(), r.token.clone());
        if matched.contains(&key) {
            continue;
        }
        let headline = match r.category {
            Category::Promised => format!(
                "stale record `{}` -- the {} now resolves",
                r.token,
                kind_str(&r.kind)
            ),
            Category::Intentional => format!(
                "stale record `{}` -- it no longer matches anything",
                r.token
            ),
        };
        findings.push(Finding {
            headline,
            sites: vec![],
            remedy: format!(
                "  Remove the record from tests/doc_names.allow in the change that\n  \
                 made it obsolete, and correct anything its reason names.\n  The record said: {}",
                r.reason
            ),
        });
    }

    findings
}

fn kind_str(k: &Kind) -> &'static str {
    match k {
        Kind::Command => "command",
        Kind::Path => "path",
    }
}

fn site_line(c: &Candidate) -> String {
    format!("    {}:{}  {}", c.file, c.line, c.span)
}

fn members_of(sites: &[Candidate]) -> Vec<(String, usize, String)> {
    let mut per_file: BTreeMap<&str, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for c in sites {
        let ord = per_file.entry(c.file.as_str()).or_insert(0);
        *ord += 1;
        out.push((c.file.clone(), *ord, c.span.clone()));
    }
    out
}

fn remedy_for(kind: &str, token: &str) -> String {
    if kind == "command" {
        format!(
            "  koto has no `{token}`. Use a command that exists, or record it:\n    \
             command<TAB>{token}<TAB>promised<TAB>owner/repo#N<TAB>-<TAB><reason>\n  \
             or, if the name is correct as written and will not be built:\n    \
             command<TAB>{token}<TAB>intentional<TAB>-<TAB>?<TAB><reason>"
        )
    } else {
        format!(
            "  `{token}` does not exist. Correct the path, or record it:\n    \
             path<TAB>{token}<TAB>intentional<TAB>-<TAB>?<TAB><reason>"
        )
    }
}

pub fn report(findings: &[Finding]) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "\n{} name(s) koto states do not resolve.\n",
        findings.len()
    );
    for f in findings {
        let _ = writeln!(s, "FAIL: {}", f.headline);
        for site in &f.sites {
            let _ = writeln!(s, "{site}");
        }
        let _ = writeln!(s, "{}\n", f.remedy);
    }
    let _ = writeln!(
        s,
        "Why this check exists and what it deliberately ignores: see the header\n\
         of tests/doc_names.rs. Run it alone with `cargo test --test doc_names`."
    );
    s
}

// ---------------------------------------------------------------- entry

fn repo_root() -> PathBuf {
    match std::env::var("KOTO_DOC_NAMES_ROOT") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    }
}

fn load_allow(root: &Path) -> Vec<Record> {
    let p = root.join("tests/doc_names.allow");
    let Ok(text) = std::fs::read_to_string(p) else {
        // Absent is empty: a fixture tree with no allowlist reports everything.
        return Vec::new();
    };
    match parse_allow(&text) {
        Ok(v) => v,
        Err(errs) => panic!(
            "tests/doc_names.allow is malformed:\n  {}",
            errs.join("\n  ")
        ),
    }
}

#[test]
fn every_name_koto_states_resolves() {
    let root = repo_root();
    let verbs = koto_verbs();
    let allow = load_allow(&root);
    let findings = scan(&root, &verbs, &allow);
    if !findings.is_empty() {
        panic!("{}", report(&findings));
    }
}

// ---------------------------------------------------------------- fixtures
//
// Each case builds a tree in a tempdir and runs `scan` against it with a
// supplied verb set, so nothing here depends on koto's real command surface
// except the one test that says it does.

fn tmp() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::temp_dir().join(format!(
        "koto-doc-names-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn verbs_of(paths: &[&str]) -> Verbs {
    Verbs::from_paths(paths.iter().map(|s| s.to_string()))
}

fn tokens(findings: &[Finding]) -> Vec<String> {
    findings.iter().map(|f| f.headline.clone()).collect()
}

fn fires(findings: &[Finding], needle: &str) -> bool {
    findings.iter().any(|f| f.headline.contains(needle))
}

// --- the acceptance fixture: the v0.12.0 defect, verbatim -----------------

/// The three string literals koto v0.12.0 shipped, reproduced including the
/// `\` continuations. If this stops reporting `session rebind`, the check has
/// stopped detecting the defect it exists for.
const V0_12_0_LITERALS: &str = r#"
fn refuse(name: &str) {
    let err = NextError {
        message: format!(
            "workflow '{}' is bound to {}, which does not resolve on this machine{}; \
             run `koto session rebind {} --to <dir>` if the checkout moved",
            name, path, machine, name,
        ),
    };
    let other = NextError {
        message: format!(
            "workflow '{}' is bound to {}; `koto next` must run from that directory \
             or one beneath it, not {}. Run `koto session rebind {} --to <dir>` if \
             the checkout moved",
            name, anchor, cwd, name,
        ),
    };
}

pub fn execution_anchor_adopted_notice(name: &str, anchor: &Path) -> String {
    format!(
        "[koto] Session '{}' had no recorded directory; it is now bound to {}. \
         Later ticks must run there or below it -- `koto session rebind {}` moves it.\n\n",
        name, anchor, name,
    )
}
"#;

const REAL_VERBS: &[&str] = &[
    "cancel",
    "config",
    "config get",
    "config list",
    "config set",
    "config unset",
    "context",
    "context add",
    "context exists",
    "context get",
    "context list",
    "context remove",
    "dashboard",
    "decisions",
    "decisions list",
    "decisions record",
    "init",
    "next",
    "overrides",
    "overrides list",
    "overrides record",
    "request",
    "request abandon",
    "request bind",
    "request close",
    "request create",
    "request get",
    "request list",
    "request progress",
    "request resolve",
    "request wait",
    "rewind",
    "session",
    "session cleanup",
    "session dir",
    "session list",
    "session recover",
    "session resolve",
    "session start",
    "session update",
    "status",
    "template",
    "template compile",
    "template export",
    "template validate",
    "version",
    "workflows",
    "workflows publish",
    "workspace",
    "workspace prune",
];

#[test]
fn catches_the_v0_12_0_string_literals() {
    let root = tmp();
    write(&root, "src/cli/mod.rs", V0_12_0_LITERALS);
    let f = scan(&root, &verbs_of(REAL_VERBS), &[]);
    assert!(
        fires(&f, "session rebind"),
        "the v0.12.0 literals must be found; got {:?}",
        tokens(&f)
    );
    let sites = &f
        .iter()
        .find(|x| x.headline.contains("rebind"))
        .unwrap()
        .sites;
    assert!(
        sites.len() >= 3,
        "all three literals must be reported, got {}: {:?}",
        sites.len(),
        sites
    );
}

#[test]
fn joining_continuations_is_load_bearing() {
    let root = tmp();
    write(
        &root,
        "src/a.rs",
        "fn f() { let m = format!(\"see `koto \\\n    session rebind x` for that\"); }\n",
    );
    let f = scan(&root, &verbs_of(REAL_VERBS), &[]);
    assert!(fires(&f, "session rebind"), "got {:?}", tokens(&f));
}

// --- resolution ----------------------------------------------------------

#[test]
fn a_parent_verb_does_not_absorb_an_unknown_subverb() {
    let v = verbs_of(&[
        "session",
        "session start",
        "status",
        "workflows",
        "workflows publish",
    ]);
    assert!(!v.resolves("session rebind"), "session owns children");
    assert!(v.resolves("session start"));
    assert!(
        v.resolves("status my-flow"),
        "status owns none; arg follows"
    );
    assert!(v.resolves("workflows publish"));
    assert!(v.resolves("workflows"));
    assert!(!v.resolves("workflows garbage"));
    assert!(!v.resolves("migrate"));
}

#[test]
fn ground_truth_comes_from_the_supplied_verb_set() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Run `koto teleport now`.\n");
    let with = scan(&root, &verbs_of(&["teleport"]), &[]);
    assert!(
        with.is_empty(),
        "supplied set should resolve it: {:?}",
        tokens(&with)
    );
    let without = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(fires(&without, "teleport"), "got {:?}", tokens(&without));
}

// --- extraction ----------------------------------------------------------

#[test]
fn code_font_is_required_and_anchoring_kills_prose() {
    let root = tmp();
    write(
        &root,
        "docs/guides/g.md",
        "koto ghostverb in prose is not a candidate.\n\
         `Active koto ghostverb detected` is prose inside code font.\n\
         The hello-koto skill is not an invocation.\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(f.is_empty(), "expected no findings, got {:?}", tokens(&f));

    write(&root, "docs/guides/h.md", "Run `koto ghostverb`.\n");
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(fires(&f, "ghostverb"), "got {:?}", tokens(&f));
}

#[test]
fn a_fence_anchors_with_or_without_a_language_tag() {
    for fence in ["```bash", "```"] {
        let root = tmp();
        write(
            &root,
            "docs/guides/g.md",
            &format!("{fence}\nkoto ghostverb demo\n```\n"),
        );
        let f = scan(&root, &verbs_of(&["next"]), &[]);
        assert!(
            fires(&f, "ghostverb"),
            "fence {fence:?} gave {:?}",
            tokens(&f)
        );
    }
}

#[test]
fn a_backticked_span_nested_in_a_fence_anchors() {
    let root = tmp();
    write(
        &root,
        "docs/reference/e.md",
        "```json\n{\"message\":\"bound to X. Run `koto ghostverb y --to <dir>` if moved\"}\n```\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(fires(&f, "ghostverb"), "got {:?}", tokens(&f));
}

#[test]
fn rust_literals_self_anchor_but_doc_comments_need_backticks() {
    let root = tmp();
    write(
        &root,
        "src/a.rs",
        "//! koto nowhas two log families and this is prose.\n\
         /// koto delivers again, also prose.\n\
         /// via `koto ghostverb --events`.\n\
         // koto plaincomment is out of scope entirely.\n\
         fn f() { let s = \"koto otherghost x\"; }\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(
        fires(&f, "ghostverb"),
        "backticked doc comment: {:?}",
        tokens(&f)
    );
    assert!(fires(&f, "otherghost"), "bare literal: {:?}", tokens(&f));
    assert!(!fires(&f, "nowhas"), "doc-comment prose: {:?}", tokens(&f));
    assert!(
        !fires(&f, "delivers"),
        "doc-comment prose: {:?}",
        tokens(&f)
    );
    assert!(!fires(&f, "plaincomment"), "plain // : {:?}", tokens(&f));
}

#[test]
fn cfg_test_modules_are_skipped() {
    let root = tmp();
    write(
        &root,
        "src/a.rs",
        "fn real() {}\n\
         #[cfg(test)]\nmod tests {\n    fn t() { let s = \"koto ghostverb\"; }\n}\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(
        !fires(&f, "ghostverb"),
        "cfg(test) must be skipped: {:?}",
        tokens(&f)
    );
}

#[test]
fn a_bare_koto_is_not_a_candidate() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Install `koto` first.\n");
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(f.is_empty(), "bare koto named the binary: {:?}", tokens(&f));
}

#[test]
fn the_word_shape_rule_stops_at_a_metavariable() {
    let root = tmp();
    write(
        &root,
        "docs/guides/g.md",
        "Run `koto workflows <action>`.\n",
    );
    let f = scan(&root, &verbs_of(&["workflows", "workflows publish"]), &[]);
    assert!(
        f.is_empty(),
        "expected `workflows` alone to resolve: {:?}",
        tokens(&f)
    );
}

// --- surfaces ------------------------------------------------------------

#[test]
fn every_checked_surface_is_scanned_and_no_other() {
    let checked = [
        "src/a.rs",
        "plugins/koto-skills/s.md",
        "docs/guides/g.md",
        "docs/reference/r.md",
        "docs/testing/t.md",
        "docs/STABILITY.md",
        "docs/workspace-layout.md",
        "README.md",
        "CLAUDE.md",
    ];
    for rel in checked {
        let root = tmp();
        let body = if rel.ends_with(".rs") {
            "fn f() { let s = \"koto ghostverb\"; }\n".to_string()
        } else {
            "Run `koto ghostverb`.\n".to_string()
        };
        write(&root, rel, &body);
        let f = scan(&root, &verbs_of(&["next"]), &[]);
        assert!(
            fires(&f, "ghostverb"),
            "{rel} should be checked: {:?}",
            tokens(&f)
        );
    }

    let excluded = [
        "docs/designs/d.md",
        "docs/designs/current/d.md",
        "docs/prds/p.md",
        "docs/briefs/b.md",
        "CHANGELOG.md",
        "tests/t.rs",
        "test/t.md",
        "benches/b.rs",
        "scripts/s.sh",
        ".github/workflows/w.yml",
    ];
    for rel in excluded {
        let root = tmp();
        write(&root, rel, "Run `koto ghostverb`.\n");
        let f = scan(&root, &verbs_of(&["next"]), &[]);
        assert!(f.is_empty(), "{rel} must not be checked: {:?}", tokens(&f));
    }
}

// --- paths ---------------------------------------------------------------

#[test]
fn a_dead_path_under_a_real_root_entry_is_a_finding() {
    let root = tmp();
    std::fs::create_dir_all(root.join("docs/guides")).unwrap();
    write(
        &root,
        "docs/guides/g.md",
        "See `docs/template-format.md`.\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(fires(&f, "docs/template-format.md"), "got {:?}", tokens(&f));
}

#[test]
fn a_citation_suffix_is_stripped_before_resolution() {
    for suffix in ["", ":120", ":120-140", "::derive_expects"] {
        let root = tmp();
        std::fs::create_dir_all(root.join("docs/guides")).unwrap();
        write(
            &root,
            "docs/guides/g.md",
            &format!("See `docs/gone.md{suffix}`.\n"),
        );
        let f = scan(&root, &verbs_of(&["next"]), &[]);
        assert!(
            fires(&f, "docs/gone.md"),
            "suffix {suffix:?}: {:?}",
            tokens(&f)
        );
    }
}

#[test]
fn a_renamed_top_level_directory_is_the_accepted_false_negative() {
    let root = tmp();
    std::fs::create_dir_all(root.join("src")).unwrap();
    write(&root, "CLAUDE.md", "The entry point is `cmd/koto/`.\n");
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(
        !fires(&f, "cmd/koto"),
        "the anchor deliberately misses this: {:?}",
        tokens(&f)
    );
}

#[test]
fn non_paths_do_not_fire() {
    let root = tmp();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for tok in [
        "feature/anchor",
        "CI/CD",
        "path/to/your-template.md",
        "~/.koto/sessions",
        "gates.*",
        "https://example.com/x/y",
        "/usr/local/bin/koto",
        "../../etc/passwd",
        "src/<name>/x.rs",
        "target/release/koto",
        ".claude/settings.json",
    ] {
        write(&root, "docs/guides/g.md", &format!("See `{tok}`.\n"));
        let f = scan(&root, &verbs_of(&["next"]), &[]);
        assert!(f.is_empty(), "{tok} should not fire: {:?}", tokens(&f));
    }
}

// --- allowlist -----------------------------------------------------------

fn allow_one(line: &str) -> Vec<Record> {
    parse_allow(line).expect("fixture allowlist should parse")
}

#[test]
fn a_promised_record_suppresses_and_its_removal_restores() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Run `koto ghostverb`.\n");
    let rec = allow_one("command\tghostverb\tpromised\towner/repo#7\t-\tbeing built under #7\n");
    let f = scan(&root, &verbs_of(&["next"]), &rec);
    assert!(f.is_empty(), "record should suppress: {:?}", tokens(&f));
    let f = scan(&root, &verbs_of(&["next"]), &[]);
    assert!(
        fires(&f, "ghostverb"),
        "removal should restore: {:?}",
        tokens(&f)
    );
}

#[test]
fn an_intentional_record_needs_no_issue_but_needs_a_witness() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Run `koto ghostverb`.\n");
    let unaffirmed = allow_one("command\tghostverb\tintentional\t-\t?\tforward commitment\n");
    let f = scan(&root, &verbs_of(&["next"]), &unaffirmed);
    assert!(
        fires(&f, "unaffirmed"),
        "`?` must report the digest: {:?}",
        tokens(&f)
    );

    let digest = f[0]
        .remedy
        .rsplit(' ')
        .next()
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    let affirmed = allow_one(&format!(
        "command\tghostverb\tintentional\t-\t{digest}\tforward commitment\n"
    ));
    let f = scan(&root, &verbs_of(&["next"]), &affirmed);
    assert!(
        f.is_empty(),
        "affirmed record should suppress: {:?}",
        tokens(&f)
    );

    // Rewording the prose AROUND the span does not retire the record: the
    // witness binds to the code span, which is the stable unit. This is
    // deliberate -- binding to the line would retire every record whenever a
    // formatter rewrapped a paragraph.
    write(
        &root,
        "docs/guides/g.md",
        "Please run `koto ghostverb` now.\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &affirmed);
    assert!(
        f.is_empty(),
        "surrounding prose is not the protected text: {:?}",
        tokens(&f)
    );

    // Changing the span itself does retire it.
    write(&root, "docs/guides/g.md", "Run `koto ghostverb --now`.\n");
    let f = scan(&root, &verbs_of(&["next"]), &affirmed);
    assert!(fires(&f, "needs re-affirming"), "got {:?}", tokens(&f));
}

#[test]
fn a_record_matching_nothing_is_itself_a_finding() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Run `koto next`.\n");
    let promised = allow_one("command\tghostverb\tpromised\towner/repo#7\t-\tbeing built\n");
    let f = scan(&root, &verbs_of(&["next"]), &promised);
    assert!(fires(&f, "stale record"), "promised: {:?}", tokens(&f));

    let intentional = allow_one("command\tghostverb\tintentional\t-\tdeadbeef\tpolicy\n");
    let f = scan(&root, &verbs_of(&["next"]), &intentional);
    assert!(fires(&f, "stale record"), "intentional: {:?}", tokens(&f));
}

#[test]
fn a_stale_record_finding_carries_its_reason_verbatim() {
    let root = tmp();
    write(&root, "docs/guides/g.md", "Run `koto next`.\n");
    let rec = allow_one(
        "command\tghostverb\tpromised\towner/repo#7\t-\tcorrect the four passages in X and Y\n",
    );
    let f = scan(&root, &verbs_of(&["next"]), &rec);
    assert!(
        f[0].remedy.contains("correct the four passages in X and Y"),
        "reason must be surfaced: {}",
        f[0].remedy
    );
}

#[test]
fn the_allowlist_rejects_every_malformed_record() {
    let cases = [
        ("command\tx\tpromised\towner/repo#1\t-\n", "6 tab"),
        ("nope\tx\tpromised\towner/repo#1\t-\tr\n", "unknown kind"),
        (
            "command\tx\tmaybe\towner/repo#1\t-\tr\n",
            "unknown category",
        ),
        ("command\tx\tpromised\t-\t-\tr\n", "owner/repo#N"),
        ("command\tx\tpromised\tnotanissue\t-\tr\n", "owner/repo#N"),
        (
            "command\tx\tintentional\towner/repo#1\t?\tr\n",
            "no issue reference",
        ),
        ("command\tx\tintentional\t-\tnothex\tr\n", "8 hex"),
        ("command\tx\tpromised\towner/repo#1\t-\t\n", "reason"),
        (
            "command\tx\tpromised\towner/repo#1\t-\tr\ncommand\tx\tintentional\t-\t?\tr2\n",
            "duplicate",
        ),
    ];
    for (text, needle) in cases {
        let err = parse_allow(text).expect_err("should reject");
        assert!(
            err.iter().any(|e| e.contains(needle)),
            "expected {needle:?} in {err:?} for {text:?}"
        );
    }
}

#[test]
fn the_allowlist_tolerates_comments_blanks_and_absence() {
    assert!(parse_allow("").unwrap().is_empty());
    assert!(parse_allow("# just a comment\n\n   \n").unwrap().is_empty());
    let recs = parse_allow(
        "# header\n\ncommand\tx\tpromised\towner/repo#1\t-\ta reason\twith a tab in it\n",
    )
    .unwrap();
    assert_eq!(recs.len(), 1);
    assert!(
        recs[0].reason.contains("with a tab in it"),
        "splitn(6) keeps tabs"
    );
}

#[test]
fn a_path_record_matches_after_suffix_stripping() {
    let root = tmp();
    std::fs::create_dir_all(root.join("docs/guides")).unwrap();
    write(
        &root,
        "docs/guides/g.md",
        "See `docs/gone.md:12` and `docs/gone.md`.\n",
    );
    let rec = allow_one("path\tdocs/gone.md\tintentional\t-\t?\tdocumented removal\n");
    let f = scan(&root, &verbs_of(&["next"]), &rec);
    assert!(
        !fires(&f, "unresolved path"),
        "one record covers both forms: {:?}",
        tokens(&f)
    );
}
