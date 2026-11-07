//! `shelf release`: release several tins that pin each other by git revision,
//! all prepared and tested at once and published parents first
//! (specs/03_release.md).
//!
//! `plan` works out the cascade from the registry's dependents and the
//! repositories' manifests; `apply` cuts a release branch per repository,
//! preflights them together, opens the PRs, and merges and publishes in
//! order, keeping its progress in `.shelf-release.json` so a stopped run can
//! be resumed.

mod apply;
mod gh;
pub mod manifest;
pub mod plan;

use crate::git;
use crate::registry::Registry;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ReleaseCmd {
    /// Work out a cascade: the tins to release, in order, their versions and
    /// every pin that moves. Changes nothing.
    Plan {
        #[command(flatten)]
        what: PlanArgs,
        /// Print the plan as JSON (the input of `apply --from`). Either way
        /// the plan is also saved to .shelf-plan.json, which `apply` with no
        /// tins applies.
        #[arg(long)]
        json: bool,
    },
    /// Cut every release branch of a cascade, preflight them together, open
    /// the PRs, and (with --merge) merge and publish parents first. With no
    /// tins, applies the plan `shelf release plan` saved here, as long as no
    /// repository's default branch has moved since.
    Apply {
        #[command(flatten)]
        what: PlanArgs,
        /// Apply a plan saved with `plan --json` instead of making one.
        #[arg(long, value_name = "PLAN_JSON", conflicts_with = "resume")]
        from: Option<String>,
        /// Carry on from .shelf-release.json: re-pin what's below a release
        /// branch that moved, and go on from the step that stopped.
        #[arg(long)]
        resume: bool,
        /// Run each tin's `test` task against the others' release heads in
        /// preflight.
        #[arg(long)]
        test: bool,
        /// Don't build the release heads together before opening PRs.
        #[arg(long)]
        skip_preflight: bool,
        /// Poll the PRs' required checks until they pass or one fails.
        #[arg(long)]
        wait: bool,
        /// Once every PR is green and every pin is its parent's current release
        /// head, merge them (merge commits, parents first) and publish. Needs gh.
        #[arg(long)]
        merge: bool,
        /// Publish each release whose branch is merged (when the PRs were
        /// merged by hand).
        #[arg(long)]
        publish: bool,
        /// One level of the graph at a time, each published before the next
        /// is cut.
        #[arg(long)]
        waves: bool,
        /// Release even when a dependent pins or checks out a tin outside the
        /// cascade at another commit than its parents pin it at.
        #[arg(long)]
        allow_mismatch: bool,
        /// An app directory to remind about `pixi shelf update` afterwards.
        #[arg(long, value_name = "PATH")]
        consumer: Vec<String>,
    },
}

#[derive(Args, Clone)]
pub struct PlanArgs {
    /// The tins whose changes to release.
    #[arg(value_name = "TIN")]
    tins: Vec<String>,
    /// Every tin checked out here with commits since its published version
    /// (release commits don't count).
    #[arg(long)]
    changed: bool,
    /// A named tin's bump, name=patch|minor|major (default: patch, or what
    /// its CHANGELOG's Unreleased headings or its changes/ fragments say).
    #[arg(long, value_name = "NAME=LEVEL")]
    bump: Vec<String>,
    /// Where a tin's repository is checked out (default: the directory named
    /// after the last segment of its git URL, here).
    #[arg(long, value_name = "NAME=PATH")]
    repo: Vec<String>,
    /// Leave a tin out: its dependents keep its published version.
    #[arg(long, value_name = "TIN")]
    without: Vec<String>,
}

impl PlanArgs {
    fn is_empty(&self) -> bool {
        self.tins.is_empty()
            && !self.changed
            && self.bump.is_empty()
            && self.repo.is_empty()
            && self.without.is_empty()
    }
}

pub fn run(reg: &Registry, cmd: ReleaseCmd) -> Result<()> {
    let cwd = std::env::current_dir()?;
    match cmd {
        ReleaseCmd::Plan { what, json } => {
            let plan = make_plan(reg, &cwd, &what)?;
            let text = serde_json::to_string_pretty(&plan)?;
            let saved = cwd.join(PLAN_FILE);
            std::fs::write(&saved, format!("{text}\n"))
                .with_context(|| format!("could not write {}", saved.display()))?;
            if json {
                println!("{text}");
            } else {
                print!("{}", plan::render(&plan));
                println!("saved to {PLAN_FILE}; `shelf release apply` with no tins applies it");
            }
            Ok(())
        }
        ReleaseCmd::Apply {
            what,
            from,
            resume,
            test,
            skip_preflight,
            wait,
            merge,
            publish,
            waves,
            allow_mismatch,
            consumer,
        } => {
            let opts = apply::Opts {
                test,
                skip_preflight,
                wait,
                merge,
                publish,
                waves,
                allow_mismatch,
                consumers: consumer,
            };
            let ws_root = cwd.clone();
            if resume {
                if !what.is_empty() {
                    bail!(
                        "--resume carries on with the saved plan; it takes no tins or plan options"
                    );
                }
                return apply::resume(reg, &ws_root, &opts);
            }
            let plan = match from {
                Some(path) => {
                    if !what.is_empty() {
                        bail!("--from applies a saved plan; it takes no tins or plan options");
                    }
                    let text = std::fs::read_to_string(&path)
                        .with_context(|| format!("could not read {path}"))?;
                    serde_json::from_str(&text).with_context(|| {
                        format!("{path} is not a plan from `shelf release plan --json`")
                    })?
                }
                None if what.is_empty() => saved_plan(&cwd)?,
                None => make_plan(reg, &cwd, &what)?,
            };
            apply::start(reg, &ws_root, plan, &opts)
        }
    }
}

/// Where `shelf release plan` saves the plan it printed, beside
/// `.shelf-release.json`.
pub const PLAN_FILE: &str = ".shelf-plan.json";

/// The plan `shelf release plan` saved here, for `apply` with no tins; refused
/// when a repository's default branch has moved since it was made.
fn saved_plan(cwd: &Path) -> Result<plan::Plan> {
    let path = cwd.join(PLAN_FILE);
    if !path.exists() {
        bail!(
            "name the tins to release, or pass --changed; or run `shelf release plan` first, \
             and `apply` with no tins applies the plan it saves"
        );
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let plan: plan::Plan = serde_json::from_str(&text)
        .with_context(|| format!("{PLAN_FILE} is not a plan from `shelf release plan`"))?;
    let mut stale = Vec::new();
    for r in &plan.releases {
        if !r.path.join(".git").exists() {
            stale.push(format!("  {}: no checkout at {}", r.repo, r.path.display()));
            continue;
        }
        let c = Checkout::open(&r.path)?;
        if c.default_branch != r.default_branch {
            stale.push(format!(
                "  {}: default branch is {}, not {}",
                r.repo, c.default_branch, r.default_branch
            ));
        } else if c.base != r.base {
            stale.push(format!(
                "  {}: {} is at {}, the plan was made at {}",
                r.repo,
                r.default_branch,
                crate::short_sha(&c.base),
                crate::short_sha(&r.base)
            ));
        }
    }
    if !stale.is_empty() {
        bail!(
            "the plan in {PLAN_FILE} is stale:\n{}\nre-run `shelf release plan` (with its tins \
             and options) to make a new one",
            stale.join("\n")
        );
    }
    let tins: Vec<String> = plan
        .releases
        .iter()
        .flat_map(|r| r.tins.iter().map(|t| format!("{} {}", t.name, t.next)))
        .collect();
    println!("applying the plan in {PLAN_FILE}: {}", tins.join(", "));
    Ok(plan)
}

fn make_plan(reg: &Registry, cwd: &Path, what: &PlanArgs) -> Result<plan::Plan> {
    if what.changed && !what.tins.is_empty() {
        bail!("name the tins or pass --changed, not both");
    }
    let mut overrides = Vec::new();
    for r in &what.repo {
        let (name, path) = r
            .split_once('=')
            .ok_or_else(|| anyhow!("--repo takes name=path, not '{r}'"))?;
        let path = cwd.join(path);
        if !path.join(".git").exists() {
            bail!("--repo {name}={}: not a git checkout", path.display());
        }
        overrides.push((name.to_string(), path));
    }
    let ws = Workspace {
        root: cwd.to_path_buf(),
        overrides,
    };
    let req = plan::Request {
        tins: what.tins.clone(),
        changed: what.changed,
        bumps: what
            .bump
            .iter()
            .map(|b| plan::parse_bump(b))
            .collect::<Result<_>>()?,
        without: what.without.clone(),
    };
    plan::make(&ws, reg, &req)
}

/// The directory holding the tins' repositories side by side.
pub struct Workspace {
    pub root: PathBuf,
    /// `--repo name=path`, by tin name or checkout directory name.
    pub overrides: Vec<(String, PathBuf)>,
}

/// A git URL's last path segment, which names its checkout:
/// `https://github.com/magmalake/deflate.mojo.git` is `deflate.mojo`.
pub fn checkout_dir_name(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    url.rsplit(['/', ':']).next().unwrap_or(url).to_string()
}

impl Workspace {
    pub fn locate(&self, url: &str, tins: &[&str]) -> Option<PathBuf> {
        let dir = checkout_dir_name(url);
        if let Some((_, path)) = self
            .overrides
            .iter()
            .find(|(n, _)| tins.contains(&n.as_str()) || *n == dir)
        {
            return Some(path.clone());
        }
        let path = self.root.join(&dir);
        path.join(".git").exists().then_some(path)
    }
}

/// A repository checked out in the workspace, read at its default branch's
/// remote head (fetched when opened), not at whatever is checked out.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub path: PathBuf,
    pub default_branch: String,
    /// `origin/<default_branch>`'s commit.
    pub base: String,
}

impl Checkout {
    pub fn open(path: &Path) -> Result<Self> {
        git::git(path, &["fetch", "--quiet", "origin"])
            .with_context(|| format!("could not fetch {}", path.display()))?;
        let default_branch = default_branch(path)?;
        let base = git::git(path, &["rev-parse", &format!("origin/{default_branch}")])?;
        Ok(Self {
            path: path.to_path_buf(),
            default_branch,
            base,
        })
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// A file at the base commit; `None` when it isn't there.
    pub fn read(&self, file: &str) -> Result<Option<String>> {
        read_at(&self.path, &self.base, file)
    }

    /// The changelog fragments under `dir`'s `changes/` at the base commit.
    pub fn fragments(&self, dir: Option<&str>) -> Result<Option<Vec<manifest::Fragment>>> {
        fragments_at(&self.path, &self.base, dir)
    }

    /// The CI workflow files at the base commit.
    pub fn workflows(&self) -> Result<Vec<String>> {
        workflows_at(&self.path, &self.base)
    }

    /// Commits on the default branch since `published` that aren't release
    /// commits: merges and commits whose subject starts with "Release" don't
    /// count.
    pub fn unreleased_commits(&self, published: &str) -> Result<usize> {
        let range = format!("{published}..{}", self.base);
        let log = git::git(&self.path, &["log", "--no-merges", "--format=%s", &range])
            .with_context(|| {
                format!(
                    "{}: is {} in its history?",
                    self.name(),
                    crate::short_sha(published)
                )
            })?;
        Ok(log
            .lines()
            .filter(|s| !s.trim().is_empty() && !s.trim_start().starts_with("Release"))
            .count())
    }
}

pub fn default_branch(path: &Path) -> Result<String> {
    let head = |p: &Path| git::git(p, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
    let full = match head(path) {
        Ok(h) => h,
        Err(_) => {
            git::git(path, &["remote", "set-head", "origin", "--auto"]).with_context(|| {
                format!("{}: could not find origin's default branch", path.display())
            })?;
            head(path)?
        }
    };
    Ok(full.strip_prefix("origin/").unwrap_or(&full).to_string())
}

pub fn read_at(path: &Path, commit: &str, file: &str) -> Result<Option<String>> {
    let listed = git::git(path, &["ls-tree", "--name-only", commit, "--", file])?;
    if listed.is_empty() {
        return Ok(None);
    }
    Ok(Some(git::git_raw(
        path,
        &["show", &format!("{commit}:{file}")],
    )?))
}

/// The changelog fragments in `changes/` (under a tin's subdirectory, if it
/// has one) at `commit`, in release order. `None` when there is no such
/// directory: the repository doesn't keep fragments.
pub fn fragments_at(
    path: &Path,
    commit: &str,
    dir: Option<&str>,
) -> Result<Option<Vec<manifest::Fragment>>> {
    let changes = match dir {
        Some(d) => format!("{d}/{}/", manifest::CHANGES_DIR),
        None => format!("{}/", manifest::CHANGES_DIR),
    };
    let listed = git::git(path, &["ls-tree", "--name-only", commit, "--", &changes])?;
    if listed.is_empty() {
        return Ok(None);
    }
    let mut found = Vec::new();
    for file in listed.lines() {
        let name = file.strip_prefix(&changes).unwrap_or(file);
        if !manifest::is_fragment_file(name) {
            continue;
        }
        let text = git::git_raw(path, &["show", &format!("{commit}:{file}")])?;
        found.push(manifest::fragment(name, &text)?);
    }
    manifest::sort_fragments(&mut found);
    Ok(Some(found))
}

pub fn workflows_at(path: &Path, commit: &str) -> Result<Vec<String>> {
    let listed = git::git(
        path,
        &["ls-tree", "--name-only", commit, "--", ".github/workflows/"],
    )?;
    Ok(listed
        .lines()
        .filter(|f| f.ends_with(".yml") || f.ends_with(".yaml"))
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_names_come_from_the_url() {
        assert_eq!(
            checkout_dir_name("https://github.com/magmalake/deflate.mojo.git"),
            "deflate.mojo"
        );
        assert_eq!(
            checkout_dir_name("https://github.com/magmalake/flare_mlake"),
            "flare_mlake"
        );
        assert_eq!(checkout_dir_name("git@github.com:o/r.git"), "r");
        assert_eq!(checkout_dir_name("file:///tmp/remotes/base1/"), "base1");
    }
}
