//! What a release changes in a repository's files: `rev` pins in pixi
//! manifests, version fields, CI workflows that check a sibling out by
//! commit, and the changelog's `Unreleased` section.
//!
//! Every edit keeps the file's formatting and comments; only the value that
//! moves is rewritten.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A dependency pinned by git revision, found in a pixi manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Dotted table path: `dependencies`, `package.host-dependencies`,
    /// `feature.codecs.dependencies`, `target.linux-64.dependencies`, …
    pub table: String,
    /// The dependency's key, which is its package name.
    pub dep: String,
    pub git: String,
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

/// A git URL reduced to what identifies the repository (`host/owner/repo`),
/// the same normalization the registry's publish-time pin check uses.
pub fn repo_key(url: &str) -> String {
    shelf_core::normalize_url(url)
}

/// Every `rev` pin in a manifest, in any table whose name ends in
/// `dependencies` — `[dependencies]`, `[package.host-dependencies]`,
/// `[package.run-dependencies]`, `[package.build-dependencies]`,
/// `[feature.*.dependencies]`, `[target.*.dependencies]` and the nested
/// target variants — the same rule the registry's path check uses.
pub fn pins(text: &str) -> Result<Vec<Pin>> {
    let doc: toml_edit::DocumentMut = text.parse().context("could not parse the manifest")?;
    let mut found = Vec::new();
    walk(doc.as_table(), "", &mut found);
    Ok(found)
}

fn walk(table: &dyn toml_edit::TableLike, prefix: &str, found: &mut Vec<Pin>) {
    for (key, item) in table.iter() {
        let Some(child) = item.as_table_like() else {
            continue;
        };
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        if key.ends_with("dependencies") {
            for (dep, spec) in child.iter() {
                let Some(spec) = spec.as_table_like() else {
                    continue;
                };
                let get = |k: &str| spec.get(k).and_then(|v| v.as_str()).map(str::to_string);
                if let (Some(git), Some(rev)) = (get("git"), get("rev")) {
                    found.push(Pin {
                        table: path.clone(),
                        dep: dep.to_string(),
                        git,
                        rev,
                        subdirectory: get("subdirectory"),
                    });
                }
            }
            continue;
        }
        walk(child, &path, found);
    }
}

/// Replace a string value in place, keeping the whitespace and comment
/// around it.
fn set_str(item: &mut toml_edit::Item, new: &str) -> bool {
    let Some(value) = item.as_value_mut() else {
        return false;
    };
    if value.as_str() == Some(new) {
        return false;
    }
    let decor = value.decor().clone();
    *value = new.into();
    *value.decor_mut() = decor;
    true
}

fn item_at<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    path: &[&str],
) -> Option<&'a mut toml_edit::Item> {
    let mut item = doc.as_item_mut();
    for key in path {
        item = item.as_table_like_mut()?.get_mut(key)?;
    }
    Some(item)
}

/// Move every pin whose repository `new_rev` returns a revision for. Returns
/// the new text and the pins that moved (with their old revs).
pub fn repin(
    text: &str,
    new_rev: impl Fn(&Pin) -> Option<String>,
) -> Result<(String, Vec<(Pin, String)>)> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("could not parse the manifest")?;
    let mut moved = Vec::new();
    for pin in pins(text)? {
        let Some(rev) = new_rev(&pin) else { continue };
        if rev == pin.rev {
            continue;
        }
        let mut path: Vec<&str> = pin.table.split('.').collect();
        path.push(&pin.dep);
        path.push("rev");
        if let Some(item) = item_at(&mut doc, &path) {
            if set_str(item, &rev) {
                moved.push((pin, rev));
            }
        }
    }
    Ok((doc.to_string(), moved))
}

/// Set `version` in the given tables (`""` is the top level, as in
/// shelf.toml) where it is present as a plain string. A table without one, or
/// one inheriting it (`version = { workspace = true }`), is left alone.
pub fn set_version(text: &str, tables: &[&str], version: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("could not parse the manifest")?;
    for table in tables {
        let mut path: Vec<&str> = table.split('.').filter(|p| !p.is_empty()).collect();
        path.push("version");
        if let Some(item) = item_at(&mut doc, &path) {
            if item.as_str().is_some() {
                set_str(item, version);
            }
        }
    }
    Ok(doc.to_string())
}

/// The `version` string at the top of a shelf.toml.
pub fn version_of(text: &str) -> Option<String> {
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("version")?.as_str().map(str::to_string)
}

/// One occurrence of a pinned commit in a CI workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPin {
    pub file: String,
    pub line: usize,
    pub rev: String,
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Byte offsets of `sha` in `text` as a whole 40-character commit, not as part
/// of a longer hex run.
fn sha_offsets(text: &str, sha: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    text.match_indices(sha)
        .map(|(i, _)| i)
        .filter(|&i| {
            let before = i == 0 || !is_hex(bytes[i - 1]);
            let end = i + sha.len();
            let after = end >= bytes.len() || !is_hex(bytes[end]);
            before && after
        })
        .collect()
}

/// Lines of a workflow holding one of `revs` exactly.
pub fn workflow_pins(file: &str, text: &str, revs: &[String]) -> Vec<WorkflowPin> {
    let mut found = Vec::new();
    for rev in revs.iter().filter(|r| shelf_core::is_full_sha(r)) {
        for i in sha_offsets(text, rev) {
            found.push(WorkflowPin {
                file: file.to_string(),
                line: text[..i].matches('\n').count() + 1,
                rev: rev.clone(),
            });
        }
    }
    found.sort_by_key(|p| p.line);
    found
}

/// Replace each of `old` with `new` wherever it stands as a whole commit.
pub fn replace_sha(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for i in sha_offsets(text, old) {
        out.push_str(&text[last..i]);
        out.push_str(new);
        last = i + old.len();
    }
    out.push_str(&text[last..]);
    out
}

/// How a workflow names the commit of another repository it builds from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefKind {
    /// An `actions/checkout` step: `repository: owner/name` with
    /// `ref: <sha>`, in flow style or as a block.
    Checkout,
    /// A `git clone <url>` followed by a git command naming a commit
    /// (`git checkout <sha>`, `git -C dir checkout <sha>`, `git fetch origin
    /// <sha>`), on the same line or the lines after it.
    Clone,
}

impl RefKind {
    pub fn describe(self) -> &'static str {
        match self {
            RefKind::Checkout => "checkout",
            RefKind::Clone => "clone",
        }
    }
}

/// A commit of a named repository that a CI workflow checks out.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRef {
    /// The line the commit is on, from 1.
    pub line: usize,
    /// The repository, as [`repo_key`] has it.
    pub repo: String,
    pub rev: String,
    pub kind: RefKind,
    /// Where the commit starts in the text.
    pub offset: usize,
}

/// Every commit a workflow checks another repository out at, by the
/// repository it names. Only full 40-character commits count; a checkout of
/// a branch or tag isn't a pin.
pub fn workflow_refs(text: &str) -> Vec<WorkflowRef> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut at = 0;
    for l in text.split_inclusive('\n') {
        lines.push((at, l.trim_end_matches(['\n', '\r'])));
        at += l.len();
    }
    let mut found = Vec::new();
    for (i, &(start, line)) in lines.iter().enumerate() {
        for (_, slug) in yaml_values(line, "repository") {
            if slug.contains('$') || !slug.contains('/') {
                continue;
            }
            let repo = repo_key(&format!("https://github.com/{slug}"));
            // The key's column, when it's a block mapping's key.
            let t = line.trim_start();
            let key_col = if t.starts_with("repository:") {
                Some(indent(line))
            } else if t.starts_with("- repository:") {
                Some(indent(line) + 2)
            } else {
                None
            };
            let same_line = yaml_values(line, "ref").into_iter().next();
            let found_ref = match (same_line, key_col) {
                (Some((c, v)), _) => Some((start + c, v)),
                (None, Some(col)) => block_sibling(&lines, i, col, "ref"),
                (None, None) => None,
            };
            if let Some((offset, rev)) = found_ref {
                if shelf_core::is_full_sha(&rev) {
                    found.push(WorkflowRef {
                        line: text[..offset].matches('\n').count() + 1,
                        repo: repo.clone(),
                        rev,
                        kind: RefKind::Checkout,
                        offset,
                    });
                }
            }
        }
        for (p, _) in line.match_indices("git clone") {
            clone_refs(&lines, i, p, &mut found);
        }
    }
    found.sort_by_key(|r| r.offset);
    found.dedup_by_key(|r| r.offset);
    for r in &mut found {
        r.line = text[..r.offset].matches('\n').count() + 1;
    }
    found
}

/// Move every commit [`workflow_refs`] finds whose repository `new_rev`
/// returns a commit for. Returns the new text and what moved (with the old
/// commits).
pub fn repoint_refs(
    text: &str,
    new_rev: impl Fn(&str) -> Option<String>,
) -> (String, Vec<(WorkflowRef, String)>) {
    let mut out = String::with_capacity(text.len());
    let mut moved = Vec::new();
    let mut last = 0;
    for r in workflow_refs(text) {
        let Some(new) = new_rev(&r.repo) else {
            continue;
        };
        if new == r.rev {
            continue;
        }
        out.push_str(&text[last..r.offset]);
        out.push_str(&new);
        last = r.offset + r.rev.len();
        moved.push((r, new));
    }
    out.push_str(&text[last..]);
    (out, moved)
}

/// The values of a YAML key on one line, flow style (`{a: 1, key: v}`) or
/// block (`key: v`, `- key: v`), each with the byte column it starts at.
/// Quotes around a value are dropped.
fn yaml_values(line: &str, key: &str) -> Vec<(usize, String)> {
    let pat = format!("{key}:");
    let bytes = line.as_bytes();
    let code = match line.find(" #") {
        Some(c) => &line[..c],
        None if line.trim_start().starts_with('#') => "",
        None => line,
    };
    let mut out = Vec::new();
    for (i, _) in code.match_indices(&pat) {
        // `foo-key:` and `key:x` aren't this key.
        if i > 0 && !matches!(bytes[i - 1], b' ' | b'\t' | b'{' | b',') {
            continue;
        }
        let mut j = i + pat.len();
        if j < bytes.len() && !matches!(bytes[j], b' ' | b'\t') {
            continue;
        }
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
            j += 1;
        }
        let quote = bytes.get(j).copied().filter(|b| matches!(b, b'"' | b'\''));
        if quote.is_some() {
            j += 1;
        }
        let end = (j..bytes.len())
            .find(|&k| match quote {
                Some(q) => bytes[k] == q,
                None => matches!(bytes[k], b',' | b'}' | b' ' | b'\t' | b'#'),
            })
            .unwrap_or(bytes.len());
        if end > j {
            out.push((j, line[j..end].to_string()));
        }
    }
    out
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `key`'s value in the block mapping that the key at column `col` of line
/// `at` is in: the lines around it at that indentation, as far as a less
/// indented line (or the start of another list item). With the value's
/// offset in the whole text.
fn block_sibling(
    lines: &[(usize, &str)],
    at: usize,
    col: usize,
    key: &str,
) -> Option<(usize, String)> {
    let pat = format!("{key}:");
    let value = |&(start, line): &(usize, &str), ind: usize| {
        yaml_values(line, key)
            .into_iter()
            .find(|(c, _)| *c > ind)
            .map(|(c, v)| (start + c, v))
    };
    for l in &lines[at + 1..] {
        let t = l.1.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let ind = indent(l.1);
        if ind < col || (ind == col && t.starts_with("- ")) {
            break;
        }
        if ind == col && t.starts_with(&pat) {
            return value(l, ind);
        }
    }
    // Above, unless the key opens its list item (`- repository: …`).
    let here = lines[at].1;
    if indent(here) + 2 == col && here.trim_start().starts_with("- ") {
        return None;
    }
    for l in lines[..at].iter().rev() {
        let t = l.1.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let ind = indent(l.1);
        if ind + 2 == col && t.starts_with("- ") {
            // The item's first key, `- ref: …`, is a sibling too.
            return t[2..].starts_with(&pat).then(|| value(l, ind)).flatten();
        }
        if ind < col {
            break;
        }
        if ind == col && t.starts_with(&pat) {
            return value(l, ind);
        }
    }
    None
}

/// After a `git clone` at byte `col` of line `at`: the commits named by git
/// commands that follow it, to the end of that run block or the next clone.
fn clone_refs(lines: &[(usize, &str)], at: usize, col: usize, found: &mut Vec<WorkflowRef>) {
    let (start, line) = lines[at];
    let rest = &line[col..];
    let seg_end = rest[1..]
        .find("git clone")
        .map(|e| e + 1)
        .unwrap_or(rest.len());
    let seg = &rest[..seg_end];
    let cmd_end = command_ends(seg).first().copied().unwrap_or(seg.len());
    let Some(url) = seg[..cmd_end]
        .split_whitespace()
        .map(|t| t.trim_matches(['"', '\'']))
        .find(|t| t.contains("://") || t.starts_with("git@"))
    else {
        return;
    };
    if url.contains('$') {
        return;
    }
    let repo = repo_key(url);
    let mut push = |offset: usize, text: &str| {
        for (i, sha) in git_shas(text) {
            found.push(WorkflowRef {
                line: 0,
                repo: repo.clone(),
                rev: sha,
                kind: RefKind::Clone,
                offset: offset + i,
            });
        }
    };
    push(start + col + cmd_end, &seg[cmd_end..]);
    if seg_end < rest.len() {
        return;
    }
    let base = indent(line);
    for &(s, l) in lines.iter().skip(at + 1).take(12) {
        let t = l.trim_start();
        if t.is_empty() {
            continue;
        }
        if indent(l) < base || t.starts_with("- ") || l.contains("git clone") {
            break;
        }
        push(s, l);
    }
}

/// Where the shell commands of a line end: `&&`, `||`, `;`, `|`.
fn command_ends(text: &str) -> Vec<usize> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b';' => out.push(i),
            b'&' | b'|' => {
                out.push(i);
                if i + 1 < b.len() && b[i + 1] == b[i] {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Full commits named by the git commands in a piece of shell, with their
/// offsets.
fn git_shas(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut ends = command_ends(text);
    ends.push(text.len());
    for end in ends {
        let cmd = &text[from..end];
        let lead = cmd.len()
            - cmd
                .trim_start_matches([' ', '\t', '(', '&', '|', ';'])
                .len();
        if cmd[lead..].starts_with("git ") {
            for i in sha_runs(cmd) {
                out.push((from + i, cmd[i..i + 40].to_string()));
            }
        }
        from = end;
    }
    out
}

/// Offsets of the 40-character hex runs in `text`.
fn sha_runs(text: &str) -> Vec<usize> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_hex(b[i]) {
            let end = (i..b.len()).find(|&j| !is_hex(b[j])).unwrap_or(b.len());
            if end - i == 40 {
                out.push(i);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

// ── CHANGELOG.md ─────────────────────────────────────────────────────────────

/// The heading line of the `Unreleased` section and the section's end.
fn unreleased_section(text: &str) -> Option<(usize, usize, usize)> {
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let is_h2 = trimmed.starts_with("## ");
        if let Some((heading, body)) = start {
            if is_h2 {
                return Some((heading, body, offset));
            }
        } else if is_h2
            && trimmed[3..]
                .trim()
                .trim_matches(['[', ']'])
                .eq_ignore_ascii_case("unreleased")
        {
            start = Some((offset, offset + line.len()));
        }
        offset += line.len();
    }
    start.map(|(h, b)| (h, b, text.len()))
}

/// What the `Unreleased` section says, if the changelog has one.
pub fn unreleased(text: &str) -> Option<&str> {
    unreleased_section(text).map(|(_, body, end)| &text[body..end])
}

/// Whether the `Unreleased` section declares a breaking change: a
/// `### Breaking` or `### Removed` heading.
pub fn unreleased_breaks(text: &str) -> bool {
    unreleased(text).is_some_and(|section| {
        section.lines().any(|l| {
            let l = l.trim().to_lowercase();
            l.starts_with("### breaking") || l.starts_with("### removed")
        })
    })
}

/// Move `Unreleased` under `## [version] - date`, leaving an empty
/// `Unreleased` above it. When there was nothing unreleased, `fallback` (the
/// pins a pin-only release moves) becomes the version's entry. `None` when the
/// changelog has no `Unreleased` section.
pub fn release_changelog(text: &str, version: &str, date: &str, fallback: &str) -> Option<String> {
    let (heading, body, end) = unreleased_section(text)?;
    let section = &text[body..end];
    let bracketed = text[heading..body].contains('[');
    let title = if bracketed {
        format!("## [{version}] - {date}\n")
    } else {
        format!("## {version} - {date}\n")
    };
    let mut entry = section.trim().to_string();
    if entry.is_empty() {
        entry = fallback.trim().to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str(&text[..body]);
    out.push('\n');
    out.push_str(&title);
    if !entry.is_empty() {
        out.push('\n');
        out.push_str(&entry);
        out.push('\n');
    }
    if end < text.len() {
        out.push('\n');
    }
    out.push_str(&text[end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[workspace]
name = "avro.mojo"
version = "0.4.5" # keep in step with shelf.toml

[dependencies]
lint-mojo = { git = "https://github.com/magmalake/lint.mojo.git", rev = "5799cd56eca15ad142a9c8ba300ad45c5eca96f4" }

[package]
name = "avro-mojo"
version = "0.4.5"

[package.host-dependencies]
mojo-compiler = "==1.2.0"
# Raw DEFLATE for the `deflate` block codec.
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af" } # published 0.2.0

[package.run-dependencies]
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af" }

[package.build-dependencies.deflate-mojo]
git = "https://github.com/magmalake/deflate.mojo"
rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af"  # a regular table

[feature.bench.dependencies]
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af" }
bench-mojo = { git = "https://github.com/magmalake/bench.mojo", rev = "7b6c423bbd9f31d3f126ca00e74994e08d64fe65" }

[target.linux-64.dependencies]
deflate-mojo = { git = "git@github.com:magmalake/deflate.mojo.git", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af" }

[feature.nightly.target.osx-arm64.dependencies]
deflate-shim = { git = "https://github.com/magmalake/deflate.mojo", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af", subdirectory = "shim" }

[package.target.linux-64.host-dependencies]
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "c3dfb87f5b09fb2559449dcb8b748629da4908af" }

[tasks]
test = "mojo run tests"
"#;

    const OLD: &str = "c3dfb87f5b09fb2559449dcb8b748629da4908af";
    const NEW: &str = "1111111111111111111111111111111111111111";

    #[test]
    fn repo_key_ignores_scheme_suffix_and_case() {
        let want = "github.com/magmalake/deflate.mojo";
        for url in [
            "https://github.com/magmalake/deflate.mojo",
            "https://github.com/magmalake/deflate.mojo.git",
            "https://github.com/Magmalake/deflate.mojo/",
            "git@github.com:magmalake/deflate.mojo.git",
        ] {
            assert_eq!(repo_key(url), want, "{url}");
        }
        assert_eq!(repo_key("file:///tmp/x/deflate.git"), "/tmp/x/deflate");
    }

    #[test]
    fn finds_pins_in_every_dependency_table() {
        let found = pins(MANIFEST).unwrap();
        let mut tables: Vec<(&str, &str)> = found
            .iter()
            .map(|p| (p.table.as_str(), p.dep.as_str()))
            .collect();
        tables.sort();
        assert_eq!(
            tables,
            vec![
                ("dependencies", "lint-mojo"),
                ("feature.bench.dependencies", "bench-mojo"),
                ("feature.bench.dependencies", "deflate-mojo"),
                (
                    "feature.nightly.target.osx-arm64.dependencies",
                    "deflate-shim"
                ),
                ("package.build-dependencies", "deflate-mojo"),
                ("package.host-dependencies", "deflate-mojo"),
                ("package.run-dependencies", "deflate-mojo"),
                ("package.target.linux-64.host-dependencies", "deflate-mojo"),
                ("target.linux-64.dependencies", "deflate-mojo"),
            ]
        );
        let shim = found.iter().find(|p| p.dep == "deflate-shim").unwrap();
        assert_eq!(shim.subdirectory.as_deref(), Some("shim"));
    }

    #[test]
    fn repin_moves_only_the_named_repo_and_keeps_formatting() {
        let deflate = repo_key("https://github.com/magmalake/deflate.mojo");
        let (out, moved) = repin(MANIFEST, |p| {
            (repo_key(&p.git) == deflate).then(|| NEW.to_string())
        })
        .unwrap();
        assert_eq!(moved.len(), 7);
        assert!(moved.iter().all(|(p, new)| p.rev == OLD && new == NEW));
        // Exactly the revs changed: the rest of the file, comments included,
        // is byte for byte what it was.
        assert_eq!(out, MANIFEST.replace(OLD, NEW));
        assert!(out.contains(&format!("rev = \"{NEW}\" }} # published 0.2.0")));
        assert!(out.contains(&format!("rev = \"{NEW}\"  # a regular table")));
        // A second pass has nothing left to move.
        let (again, moved) = repin(&out, |p| {
            (repo_key(&p.git) == deflate).then(|| NEW.to_string())
        })
        .unwrap();
        assert!(moved.is_empty());
        assert_eq!(again, out);
    }

    #[test]
    fn set_version_touches_only_plain_versions_in_named_tables() {
        let out = set_version(MANIFEST, &["package", "workspace"], "0.4.6").unwrap();
        assert!(out.contains("version = \"0.4.6\" # keep in step with shelf.toml"));
        assert_eq!(out.matches("0.4.6").count(), 2);
        let shelf = "name = \"a\"\nversion = \"1.0.0\"  # bump me\ntins = []\n";
        assert_eq!(
            set_version(shelf, &[""], "1.0.1").unwrap(),
            "name = \"a\"\nversion = \"1.0.1\"  # bump me\ntins = []\n"
        );
        let inherited = "[package]\nname = \"x\"\nversion = { workspace = true }\n";
        assert_eq!(
            set_version(inherited, &["package"], "2.0.0").unwrap(),
            inherited
        );
        assert_eq!(version_of(shelf).as_deref(), Some("1.0.0"));
    }

    #[test]
    fn workflow_pins_are_exact_commit_matches_only() {
        let wf = format!(
            "steps:\n  - uses: actions/checkout@v4\n    with:\n      repository: magmalake/deflate.mojo\n      ref: {OLD}\n  - run: echo {OLD}ff\n  - with: {{repository: magmalake/deflate.mojo, ref: {OLD}, path: deflate.mojo}}\n"
        );
        let found = workflow_pins("ci.yml", &wf, &[OLD.to_string(), "c3dfb87".to_string()]);
        assert_eq!(found.iter().map(|p| p.line).collect::<Vec<_>>(), vec![5, 7]);
        let out = replace_sha(&wf, OLD, NEW);
        assert_eq!(out.matches(NEW).count(), 2);
        assert!(
            out.contains(&format!("{OLD}ff")),
            "a longer hex run is not a pin"
        );
        let found = workflow_refs(&wf);
        let refs: Vec<(usize, &str)> = found.iter().map(|r| (r.line, r.repo.as_str())).collect();
        let deflate = "github.com/magmalake/deflate.mojo";
        assert_eq!(refs, vec![(5, deflate), (7, deflate)]);
    }

    /// The forms a CI workflow checks a sibling out at a commit in, with an
    /// unrelated commit each: what `workflow_refs` must find by repository.
    fn checkouts() -> String {
        let (a, b, c, d, e, f) = (
            "a".repeat(40),
            "b".repeat(40),
            "c".repeat(40),
            "d".repeat(40),
            "e".repeat(40),
            "f".repeat(40),
        );
        format!(
            r#"jobs:
  data:
    steps:
      - uses: actions/checkout@v4
        with: {{repository: magmalake/parquet.mojo, ref: {a}, path: parquet.mojo}}
      - uses: actions/checkout@v4
        with:
          repository: Magmalake/Iceberg.mojo
          # pinned by hand
          ref: "{b}"
          path: iceberg.mojo
      - uses: actions/checkout@v4
        with:
          path: thrift.mojo
          ref: {c}
          repository: magmalake/thrift.mojo
      - uses: actions/checkout@v4
        with:
          repository: magmalake/avro.mojo
          ref: main
      - uses: actions/checkout@v4
        with:
          repository: ${{{{ github.repository }}}}
          ref: {f}
      - run: |
          git clone --depth 50 https://github.com/magmalake/deflate.mojo.git deflate
          git -C deflate checkout --quiet {d}
          echo {e}
      - run: git clone https://github.com/magmalake/lz4.mojo lz4 && (cd lz4 && git checkout {e}) && git clone https://github.com/magmalake/zstd.mojo
      - uses: actions/checkout@v4
        with:
          repository: magmalake/roaring.mojo
      - uses: actions/checkout@v4
        with:
          ref: {f}
"#
        )
    }

    #[test]
    fn workflow_refs_find_checkouts_and_clones_by_repository() {
        let wf = checkouts();
        let refs = workflow_refs(&wf);
        let found: Vec<(usize, &str, RefKind, char)> = refs
            .iter()
            .map(|r| {
                (
                    r.line,
                    r.repo.strip_prefix("github.com/magmalake/").unwrap(),
                    r.kind,
                    r.rev.chars().next().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![
                (5, "parquet.mojo", RefKind::Checkout, 'a'),
                (10, "iceberg.mojo", RefKind::Checkout, 'b'),
                (15, "thrift.mojo", RefKind::Checkout, 'c'),
                (27, "deflate.mojo", RefKind::Clone, 'd'),
                (29, "lz4.mojo", RefKind::Clone, 'e'),
            ],
            "a branch ref, an expression, a repository with no ref, a ref \
             with no repository and an `echo` of a commit aren't pins"
        );
        for r in workflow_refs(&wf) {
            assert_eq!(&wf[r.offset..r.offset + 40], r.rev);
        }
    }

    #[test]
    fn repoint_refs_moves_only_the_named_repositories() {
        let wf = checkouts();
        let (out, moved) = repoint_refs(&wf, |repo| match repo {
            "github.com/magmalake/parquet.mojo" | "github.com/magmalake/deflate.mojo" => {
                Some(NEW.to_string())
            }
            "github.com/magmalake/thrift.mojo" => Some("c".repeat(40)),
            _ => None,
        });
        assert_eq!(moved.len(), 2, "thrift is already there");
        let want = wf
            .replacen(&"a".repeat(40), NEW, 1)
            .replacen(&"d".repeat(40), NEW, 1);
        assert_eq!(out, want);
        assert!(repoint_refs(&out, |_| None).1.is_empty());
    }

    const CHANGELOG: &str = "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n### Removed\n\n- `old_api`.\n\n## [0.2.0] - 2026-11-05\n\n### Added\n\n- Streaming.\n";

    #[test]
    fn changelog_unreleased_moves_under_the_new_version() {
        assert!(unreleased_breaks(CHANGELOG));
        let out = release_changelog(CHANGELOG, "0.3.0", "2026-11-06", "unused").unwrap();
        assert_eq!(
            out,
            "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n## [0.3.0] - 2026-11-06\n\n### Removed\n\n- `old_api`.\n\n## [0.2.0] - 2026-11-05\n\n### Added\n\n- Streaming.\n"
        );
        assert!(!unreleased_breaks(&out));
    }

    #[test]
    fn an_empty_unreleased_gets_the_pin_moves() {
        let text = "# Changelog\n\n## Unreleased\n\n## 0.1.0 - 2026-01-01\n\n- First.\n";
        let out = release_changelog(
            text,
            "0.1.1",
            "2026-11-06",
            "### Changed\n\n- Pins deflate-mojo 0.2.1.",
        )
        .unwrap();
        assert_eq!(
            out,
            "# Changelog\n\n## Unreleased\n\n## 0.1.1 - 2026-11-06\n\n### Changed\n\n- Pins deflate-mojo 0.2.1.\n\n## 0.1.0 - 2026-01-01\n\n- First.\n"
        );
        // At the end of the file, and with no Unreleased section at all.
        let last = release_changelog("## [Unreleased]\n- x\n", "1.0.0", "2026-11-06", "").unwrap();
        assert_eq!(last, "## [Unreleased]\n\n## [1.0.0] - 2026-11-06\n\n- x\n");
        assert!(release_changelog("# Changelog\n", "1.0.0", "d", "").is_none());
        assert!(!unreleased_breaks("# nothing\n"));
    }
}
