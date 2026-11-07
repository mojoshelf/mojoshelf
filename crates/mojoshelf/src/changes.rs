//! `shelf changes`: changelog fragments, one file per change under
//! `changes/`, which `shelf release apply` gathers into CHANGELOG.md. Two
//! PRs never edit the same file, so they never conflict over the changelog.

use crate::git;
use crate::release::manifest::{self, Level, CHANGES_DIR};
use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use std::path::Path;

/// The label that lets a PR change code without a fragment.
const NO_CHANGELOG: &str = "no-changelog";
/// The label `shelf release apply` puts on its PRs, which release the
/// fragments rather than add one.
const RELEASE: &str = "release";

#[derive(Subcommand)]
pub enum ChangesCmd {
    /// Write a fragment, changes/<slug>[.<level>].md, from a template.
    New {
        /// The file's name: the PR number and a word or two, `155-fills`.
        slug: String,
        /// patch|minor|breaking|major (default: patch). `breaking` is minor
        /// below 1.0 and major after.
        #[arg(long, value_name = "LEVEL")]
        bump: Option<Level>,
    },
    /// For PR CI: fail when the branch changes code but adds no fragment.
    /// Prose-only changes (*.md, docs/, LICENSE, NOTICE) and PRs labelled
    /// `no-changelog` (or `release`) pass.
    Check {
        /// What the branch is compared with (default: origin/$GITHUB_BASE_REF,
        /// else origin's default branch).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// A label the PR carries (default: read from $GITHUB_EVENT_PATH).
        #[arg(long, value_name = "NAME")]
        label: Vec<String>,
    },
}

pub fn run(cmd: ChangesCmd) -> Result<()> {
    match cmd {
        ChangesCmd::New { slug, bump } => new(&std::env::current_dir()?, &slug, bump),
        ChangesCmd::Check { base, label } => check(&git::repo_root()?, base, label),
    }
}

const TEMPLATE: &str = "- **What changed, in one line.** What a user of the tin notices and how\n  to use it; the issue or PR it closes.\n";

fn new(dir: &Path, slug: &str, bump: Option<Level>) -> Result<()> {
    let slug = slug.strip_suffix(".md").unwrap_or(slug);
    if slug.is_empty() || slug.contains(['/', '\\']) || slug.starts_with('.') {
        bail!("'{slug}' is not a fragment name; use something like 155-fills");
    }
    let file = match bump {
        None | Some(Level::Patch) => format!("{slug}.md"),
        Some(level) => format!("{slug}.{}.md", level.as_str()),
    };
    let changes = dir.join(CHANGES_DIR);
    let path = changes.join(&file);
    if path.exists() {
        bail!("{CHANGES_DIR}/{file} already exists");
    }
    std::fs::create_dir_all(&changes)
        .with_context(|| format!("could not create {}", changes.display()))?;
    std::fs::write(&path, TEMPLATE)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("wrote {CHANGES_DIR}/{file}; replace the template with the entry");
    Ok(())
}

/// Whether a changed file is prose that no build or test reads, the same
/// rule as the magmalake CI change filters.
fn is_prose(file: &str) -> bool {
    file.ends_with(".md") || file.starts_with("docs/") || file == "LICENSE" || file == "NOTICE"
}

/// A fragment file anywhere in the tree: `changes/<name>.md`, or under a
/// subdirectory tin's `changes/`.
fn fragment_name(file: &str) -> Option<&str> {
    let (dir, name) = file.rsplit_once('/')?;
    let is_changes = dir == CHANGES_DIR || dir.ends_with(&format!("/{CHANGES_DIR}"));
    (is_changes && manifest::is_fragment_file(name)).then_some(name)
}

/// The PR's labels from the GitHub Actions event payload, if there is one.
fn event_labels() -> Result<Vec<String>> {
    let Ok(path) = std::env::var("GITHUB_EVENT_PATH") else {
        return Ok(vec![]);
    };
    let text = std::fs::read_to_string(&path).with_context(|| format!("could not read {path}"))?;
    let event: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{path} is not JSON"))?;
    Ok(event["pull_request"]["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(str::to_string))
        .collect())
}

fn check(root: &Path, base: Option<String>, mut labels: Vec<String>) -> Result<()> {
    labels.extend(event_labels()?);
    if let Some(l) = labels.iter().find(|l| *l == NO_CHANGELOG || *l == RELEASE) {
        println!("labelled {l}: no changelog fragment needed");
        return Ok(());
    }
    let base = match base {
        Some(b) => b,
        None => match std::env::var("GITHUB_BASE_REF") {
            Ok(b) if !b.is_empty() => format!("origin/{b}"),
            _ => format!("origin/{}", crate::release::default_branch(root)?),
        },
    };
    let range = format!("{base}...HEAD");
    let diff =
        git::git(root, &["diff", "--name-status", "--no-renames", &range]).with_context(|| {
            format!("could not diff {range}; fetch the base (actions/checkout with fetch-depth: 0)")
        })?;
    let mut code = Vec::new();
    let mut added = Vec::new();
    for line in diff.lines() {
        let (status, file) = line
            .split_once('\t')
            .ok_or_else(|| anyhow!("unexpected git diff line '{line}'"))?;
        if let Some(name) = fragment_name(file) {
            if status.starts_with(['A', 'M']) {
                added.push((file, name));
            }
        } else if !is_prose(file) {
            code.push(file);
        }
    }
    // A fragment that `shelf release` would refuse fails here, not at release.
    for (file, name) in &added {
        let text = std::fs::read_to_string(root.join(file))
            .with_context(|| format!("could not read {file}"))?;
        let f = manifest::fragment(name, &text)?;
        if f.body.is_empty() {
            bail!("{file} is empty");
        }
        println!("{file} ({})", f.level.as_str());
    }
    match (code.is_empty(), added.is_empty()) {
        (true, _) => {
            println!("only prose changed since {base}: no changelog fragment needed");
            Ok(())
        }
        (false, false) => Ok(()),
        (false, true) => bail!(
            "{} code file{} changed since {base} ({}{}) but no changelog fragment was added: \
             run `shelf changes new <pr>-<slug> [--bump minor]` and commit it, or label the PR \
             {NO_CHANGELOG}",
            code.len(),
            if code.len() == 1 { "" } else { "s" },
            code.iter().take(3).copied().collect::<Vec<_>>().join(", "),
            if code.len() > 3 { ", …" } else { "" },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_and_fragment_paths() {
        assert!(is_prose("README.md") && is_prose("docs/guide.html") && is_prose("LICENSE"));
        assert!(!is_prose("src/scan.mojo") && !is_prose("pixi.toml") && !is_prose("x/docs/a"));
        assert_eq!(fragment_name("changes/155-fills.md"), Some("155-fills.md"));
        assert_eq!(
            fragment_name("full/changes/1-a.minor.md"),
            Some("1-a.minor.md")
        );
        assert_eq!(fragment_name("changes/README.md"), None);
        assert_eq!(fragment_name("notchanges/1-a.md"), None);
        assert_eq!(fragment_name("155-fills.md"), None);
    }
}
