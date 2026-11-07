//! `shelf release apply`: carry out a plan. Every step records what it did in
//! `.shelf-release.json` beside the repositories, so a run that stops (a
//! failed check, a merge left for later) carries on with `--resume`.

use super::gh::{self, Checks, Pr};
use super::manifest::{self, repo_key};
use super::plan::{github_slug, Plan, RepoRelease};
use super::{fragments_at, read_at, workflows_at};
use crate::git;
use crate::registry::Registry;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use shelf_core::ResolvedTin;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const STATE_FILE: &str = ".shelf-release.json";

/// The last line of every release commit `apply` writes.
const RELEASE_COMMIT_MARK: &str = "Made by `shelf release apply`.";

pub struct Opts {
    pub test: bool,
    pub skip_preflight: bool,
    pub wait: bool,
    pub merge: bool,
    pub publish: bool,
    pub waves: bool,
    pub allow_mismatch: bool,
    pub consumers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    pub plan: Plan,
    /// By the plan's repo name.
    pub repos: BTreeMap<String, RepoState>,
    #[serde(default)]
    pub waves: bool,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RepoState {
    /// The commit the release branch was cut from, recorded as soon as the
    /// local branch exists and before the release commit is written, so a run
    /// stopped before the push leaves a trace `--resume` can act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_from: Option<String>,
    /// The release branch's head, as pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Heads the branch had before, oldest first; a CI workflow that pinned
    /// one of them moves with the pins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_heads: Vec<String>,
    /// The head preflight last passed at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<Pr>,
    #[serde(default)]
    pub merged: bool,
    /// The tins published from the head so far.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub published: Vec<String>,
}

impl State {
    fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("{} is not a shelf release state file", path.display()))
    }

    fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)? + "\n";
        std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
    }

    fn release(&self, repo: &str) -> &RepoRelease {
        self.plan.get(repo).expect("repo in plan")
    }

    fn of(&mut self, repo: &str) -> &mut RepoState {
        self.repos.entry(repo.to_string()).or_default()
    }

    fn head(&self, repo: &str) -> Option<&str> {
        self.repos.get(repo).and_then(|r| r.head.as_deref())
    }

    fn is_published(&self, repo: &str) -> bool {
        let done = self
            .repos
            .get(repo)
            .map(|r| r.published.clone())
            .unwrap_or_default();
        self.release(repo)
            .tins
            .iter()
            .all(|t| done.contains(&t.name))
    }

    /// Where each repo's release stopped, one line each, for the error that
    /// refuses to start a second release over this one.
    fn progress(&self) -> Vec<String> {
        self.plan
            .releases
            .iter()
            .map(|r| {
                let s = self.repos.get(&r.repo).cloned().unwrap_or_default();
                let step = if self.is_published(&r.repo) {
                    "published".to_string()
                } else if s.merged {
                    "merged, not yet published".to_string()
                } else if let Some(pr) = &s.pr {
                    format!("PR {} open", pr.url)
                } else if let Some(head) = &s.head {
                    match &s.preflight {
                        Some(p) if p == head => {
                            format!(
                                "pushed ({}), preflight passed, no PR",
                                crate::short_sha(head)
                            )
                        }
                        _ => format!("pushed ({}), not yet preflighted", crate::short_sha(head)),
                    }
                } else if s.cut_from.is_some() {
                    "branch cut locally, stopped before it was pushed".to_string()
                } else {
                    "not cut yet".to_string()
                };
                format!("  {}: {}: {step}", r.repo, r.branch)
            })
            .collect()
    }

    /// Each parent repo's current release head, keyed by its repo URL.
    fn parent_heads(&self, r: &RepoRelease) -> Result<HashMap<String, String>> {
        let mut heads = HashMap::new();
        for p in &r.parents {
            let head = self
                .head(p)
                .ok_or_else(|| anyhow!("{}: its parent {p} has no release branch yet", r.repo))?;
            heads.insert(repo_key(&self.release(p).url), head.to_string());
        }
        Ok(heads)
    }
}

pub fn start(reg: &Registry, root: &Path, plan: Plan, opts: &Opts) -> Result<()> {
    let path = root.join(STATE_FILE);
    if path.exists() {
        match State::load(&path) {
            Ok(st) if st.done => {}
            Ok(st) => bail!(
                "a release is under way here ({STATE_FILE}):\n{}\n`shelf release apply --resume` \
                 carries on from there: it cuts and pushes the branches not pushed yet (reusing a \
                 local one that holds only its release commit), re-pins below any branch that \
                 moved, then preflights, opens the PRs, and with --merge merges and publishes. \
                 Or delete {STATE_FILE} to start over",
                st.progress().join("\n")
            ),
            Err(_) => bail!(
                "a release is under way here ({STATE_FILE}); `shelf release apply --resume` \
                 carries on with it, or delete the file to start over"
            ),
        }
    }
    if plan.releases.is_empty() {
        bail!("the plan has nothing to release");
    }
    let mismatches = plan.mismatches();
    if !mismatches.is_empty() {
        let list: Vec<String> = mismatches.iter().map(|m| format!("  {m}")).collect();
        if !opts.allow_mismatch {
            bail!(
                "tins outside the cascade disagree with what a parent pins them at:\n{}\n\
                 move them to the parent's commit (or put the tin in the cascade), or pass \
                 --allow-mismatch to release anyway",
                list.join("\n")
            );
        }
        println!(
            "releasing despite these mismatches (--allow-mismatch):\n{}",
            list.join("\n")
        );
    }
    for r in &plan.releases {
        if !git::working_tree_clean(&r.path)? {
            bail!(
                "{}: working tree is dirty; commit or stash before releasing",
                r.repo
            );
        }
    }
    let mut st = State {
        repos: plan
            .releases
            .iter()
            .map(|r| (r.repo.clone(), RepoState::default()))
            .collect(),
        plan,
        waves: opts.waves,
        done: false,
    };
    st.save(&path)?;
    run(reg, &path, &mut st, opts, false)
}

pub fn resume(reg: &Registry, root: &Path, opts: &Opts) -> Result<()> {
    let path = root.join(STATE_FILE);
    if !path.exists() {
        bail!("no release to resume here (no {STATE_FILE}); start one with `shelf release apply <tin>…`");
    }
    let mut st = State::load(&path)?;
    if st.done {
        println!("the release in {STATE_FILE} is finished; nothing to resume");
        return Ok(());
    }
    st.waves |= opts.waves;
    run(reg, &path, &mut st, opts, true)
}

fn run(reg: &Registry, path: &Path, st: &mut State, opts: &Opts, resuming: bool) -> Result<()> {
    let all: Vec<String> = st.plan.releases.iter().map(|r| r.repo.clone()).collect();
    let groups: Vec<Vec<String>> = if st.waves {
        let levels: BTreeSet<usize> = st.plan.releases.iter().map(|r| r.level).collect();
        levels
            .into_iter()
            .map(|l| {
                st.plan
                    .releases
                    .iter()
                    .filter(|r| r.level == l)
                    .map(|r| r.repo.clone())
                    .collect()
            })
            .collect()
    } else {
        vec![all.clone()]
    };
    for group in &groups {
        if group.iter().all(|r| st.is_published(r)) {
            continue;
        }
        if resuming {
            refresh(path, st, group)?;
        }
        sync(path, st, group, resuming)?;
        if opts.skip_preflight {
            println!("preflight skipped (--skip-preflight)");
        } else {
            preflight(reg, path, st, group, opts.test)?;
        }
        let with_prs = prs(path, st, group)?;
        if with_prs && (opts.wait || opts.merge) {
            wait(st, group)?;
        } else if opts.wait {
            println!("no PRs to wait for");
        }
        if opts.merge {
            if !with_prs {
                bail!(
                    "--merge needs gh and GitHub remotes; merge the release branches yourself \
                     (merge commits, parents first), then `shelf release apply --resume --publish`"
                );
            }
            merge(path, st, group)?;
        }
        if opts.merge || opts.publish {
            publish(reg, path, st, group)?;
        }
        if !group.iter().all(|r| st.is_published(r)) {
            println!();
            if with_prs {
                println!(
                    "next: once the checks pass, `shelf release apply --resume --merge` merges and \
                     publishes, parents first"
                );
            } else {
                println!(
                    "next: merge each branch into its default branch (merge commits, parents first), \
                     then `shelf release apply --resume --publish`"
                );
            }
            return Ok(());
        }
    }
    st.done = true;
    st.save(path)?;
    println!("\nreleased:");
    for r in &st.plan.releases {
        for t in &r.tins {
            println!(
                "  {} {} ({})",
                t.name,
                t.next,
                crate::short_sha(st.head(&r.repo).unwrap_or(""))
            );
        }
    }
    for c in &opts.consumers {
        println!("next, in the app at {c}: pixi shelf update");
    }
    Ok(())
}

// ── branches ────────────────────────────────────────────────────────────────

/// Notice release branches that moved on origin since the last run: a fix
/// pushed to a parent's branch. The new head is taken; `sync` then re-pins
/// what's below it.
fn refresh(path: &Path, st: &mut State, group: &[String]) -> Result<()> {
    let mut moved = BTreeSet::new();
    for repo in group {
        let r = st.release(repo).clone();
        let Some(head) = st.head(repo).map(str::to_string) else {
            continue;
        };
        git::git(&r.path, &["fetch", "--quiet", "origin"])?;
        let remote = git::git(&r.path, &["rev-parse", &format!("origin/{}", r.branch)])
            .with_context(|| format!("{}: {} is gone from origin", r.repo, r.branch))?;
        if remote != head {
            println!(
                "{} moved: {} -> {}",
                r.branch,
                crate::short_sha(&head),
                crate::short_sha(&remote)
            );
            let s = st.of(repo);
            s.previous_heads.push(head);
            s.head = Some(remote);
            moved.insert(repo.clone());
        }
    }
    let below = repin_order(&st.plan, &moved);
    if !below.is_empty() {
        println!("re-pinning below it: {}", below.join(", "));
    }
    st.save(path)
}

/// The repos to re-pin after the heads in `moved` changed, in plan order:
/// every repo below one of them (a re-pinned repo's head moves too, so its
/// children follow), and only those.
pub fn repin_order(plan: &Plan, moved: &BTreeSet<String>) -> Vec<String> {
    let mut moved = moved.clone();
    let mut out = Vec::new();
    for r in &plan.releases {
        if r.parents.iter().any(|p| moved.contains(p)) {
            out.push(r.repo.clone());
            moved.insert(r.repo.clone());
        }
    }
    out
}

/// Cut each release branch that doesn't exist yet, and bring every pin on
/// the existing ones to its parent's current head, parents first.
fn sync(path: &Path, st: &mut State, group: &[String], resuming: bool) -> Result<()> {
    for repo in group {
        if st.is_published(repo) {
            continue;
        }
        let r = st.release(repo).clone();
        let heads = st.parent_heads(&r)?;
        // CI checkouts by repository: parents' and their parents' heads.
        let mut ref_heads = heads.clone();
        for m in &r.refs {
            let head = st
                .head(&m.repo)
                .ok_or_else(|| anyhow!("{}: {} has no release branch yet", r.repo, m.repo))?;
            ref_heads.insert(repo_key(&st.release(&m.repo).url), head.to_string());
        }
        // Workflows pin the old commits exactly; these are the ones to move.
        let mut old_revs: HashMap<String, Vec<String>> = HashMap::new();
        for p in &r.parents {
            let key = repo_key(&st.release(p).url);
            let mut olds: Vec<String> = r
                .workflow_pins
                .iter()
                .filter(|w| w.parent == *p)
                .map(|w| w.pin.rev.clone())
                .collect();
            olds.extend(
                st.repos
                    .get(p)
                    .map(|s| s.previous_heads.clone())
                    .unwrap_or_default(),
            );
            old_revs.insert(key, olds);
        }
        if !git::working_tree_clean(&r.path)? {
            let cut = st.repos.get(repo).is_some_and(|s| s.cut_from.is_some());
            if cut && st.head(repo).is_none() {
                bail!(
                    "{}: working tree is dirty; an apply stopped while writing the release \
                     commit on {}. Discard those edits (they are remade), then --resume",
                    r.repo,
                    r.branch
                );
            }
            bail!(
                "{}: working tree is dirty; commit or stash before releasing",
                r.repo
            );
        }
        let remote_branch = format!("origin/{}", r.branch);
        match st.head(repo).map(str::to_string) {
            None => {
                git::git(&r.path, &["fetch", "--quiet", "origin"])?;
                let exists =
                    !git::git(&r.path, &["ls-remote", "--heads", "origin", &r.branch])?.is_empty();
                if exists && !resuming {
                    bail!(
                        "{}: {} is already on origin; delete it, or carry on with --resume",
                        r.repo,
                        r.branch
                    );
                }
                if exists {
                    // Pushed by a run that stopped before saving it.
                    checkout_branch(&r)?;
                    let head = git::head_commit(&r.path)?;
                    println!(
                        "{}: carrying on from {} ({})",
                        r.repo,
                        r.branch,
                        crate::short_sha(&head)
                    );
                    st.of(repo).head = Some(head);
                    st.save(path)?;
                } else {
                    let base = format!("origin/{}", r.default_branch);
                    let base_sha = git::git(&r.path, &["rev-parse", &base])?;
                    if base_sha != r.base {
                        println!(
                            "{}: {} moved since the plan ({} -> {}); branching from its new head",
                            r.repo,
                            r.default_branch,
                            crate::short_sha(&r.base),
                            crate::short_sha(&base_sha)
                        );
                    }
                    let flag = if reusable_local_branch(&r, &base)? {
                        "-B"
                    } else {
                        "-b"
                    };
                    git::git(&r.path, &["checkout", "--quiet", flag, &r.branch, &base])?;
                    st.of(repo).cut_from = Some(base_sha);
                    st.save(path)?;
                    let mut changed = release_edits(&st.plan, &r)?;
                    changed.extend(pin_edits(&r, &heads, &ref_heads, &old_revs)?);
                    let head = commit_and_push(&r, &changed, &release_message(&st.plan, &r))?;
                    println!("cut {} ({})", r.branch, crate::short_sha(&head));
                    st.of(repo).head = Some(head);
                    st.save(path)?;
                    continue;
                }
            }
            Some(_) => {
                git::git(&r.path, &["fetch", "--quiet", "origin"])?;
                let remote = git::git(&r.path, &["rev-parse", &remote_branch])?;
                if Some(remote.as_str()) != st.head(repo) {
                    // Moved since `refresh` looked; take it like refresh would.
                    let s = st.of(repo);
                    if let Some(old) = s.head.replace(remote) {
                        s.previous_heads.push(old);
                    }
                }
                checkout_branch(&r)?;
            }
        }
        let changed = pin_edits(&r, &heads, &ref_heads, &old_revs)?;
        if changed.is_empty() {
            continue;
        }
        let parents: Vec<String> = r
            .parents
            .iter()
            .map(|p| {
                let h = st.head(p).unwrap_or("");
                format!("{} ({})", st.release(p).root().name, crate::short_sha(h))
            })
            .collect();
        let msg = format!("Pin {} at the new release head", parents.join(", "));
        let head = commit_and_push(&r, &changed, &msg)?;
        println!("re-pinned {} ({})", r.branch, crate::short_sha(&head));
        let s = st.of(repo);
        if let Some(old) = s.head.replace(head) {
            s.previous_heads.push(old);
        }
        st.save(path)?;
    }
    Ok(())
}

/// Whether a local release branch is left over from a run that stopped
/// before pushing it, holding nothing but the default branch and shelf's own
/// release commit, so it can be cut again from `base` (`checkout -B`).
/// `false` when there is no such branch; an error when the branch has
/// anything else on it.
fn reusable_local_branch(r: &RepoRelease, base: &str) -> Result<bool> {
    let local = format!("refs/heads/{}", r.branch);
    if git::git(&r.path, &["rev-parse", "--verify", "--quiet", &local]).is_err() {
        return Ok(false);
    }
    let range = format!("{base}..{local}");
    let commits = git::git(&r.path, &["log", "--format=%h %s", &range])?;
    let ours = git::git(&r.path, &["log", "--format=%B%x00", &range])?;
    let bodies: Vec<&str> = ours
        .split('\0')
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    let only_release_commit = bodies.len() <= 1
        && bodies
            .iter()
            .all(|b| b.starts_with("Release ") && b.ends_with(RELEASE_COMMIT_MARK));
    if !only_release_commit {
        bail!(
            "{}: a local {} exists, not on origin, with commits other than shelf's release \
             commit:\n{}\npush it, or delete it (`git branch -D {}`), then --resume",
            r.repo,
            r.branch,
            commits
                .lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            r.branch
        );
    }
    println!(
        "{}: cutting {} again over the local one an earlier run left unpushed",
        r.repo, r.branch
    );
    Ok(true)
}

/// Check out the release branch at origin's head, refusing to drop local
/// commits that aren't pushed.
fn checkout_branch(r: &RepoRelease) -> Result<()> {
    let remote = format!("origin/{}", r.branch);
    let local = format!("refs/heads/{}", r.branch);
    if git::git(&r.path, &["rev-parse", "--verify", "--quiet", &local]).is_ok()
        && git::git(&r.path, &["merge-base", "--is-ancestor", &local, &remote]).is_err()
    {
        bail!(
            "{}: the local {} has commits that aren't on origin; push them (or drop them) first",
            r.repo,
            r.branch
        );
    }
    git::git(&r.path, &["checkout", "--quiet", "-B", &r.branch, &remote])?;
    Ok(())
}

/// The manifests a repo's tins build from, relative to its root.
fn manifest_files(r: &RepoRelease) -> Vec<String> {
    let mut files = vec!["pixi.toml".to_string()];
    for t in &r.tins {
        if let Some(d) = &t.subdirectory {
            files.push(format!("{d}/pixi.toml"));
        }
    }
    files.dedup();
    files
}

fn in_dir(dir: Option<&str>, file: &str) -> String {
    match dir {
        Some(d) => format!("{d}/{file}"),
        None => file.to_string(),
    }
}

fn edit(repo: &Path, file: &str, f: impl FnOnce(&str) -> Result<Option<String>>) -> Result<bool> {
    let full = repo.join(file);
    let Ok(before) = std::fs::read_to_string(&full) else {
        return Ok(false);
    };
    match f(&before)? {
        Some(after) if after != before => {
            std::fs::write(&full, after)
                .with_context(|| format!("could not write {}", full.display()))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Versions and changelogs: the part of a release commit that's only made
/// once, when the branch is cut.
fn release_edits(plan: &Plan, r: &RepoRelease) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    let date = today();
    for t in &r.tins {
        let dir = t.subdirectory.as_deref();
        let shelf = in_dir(dir, "shelf.toml");
        if edit(&r.path, &shelf, |s| {
            manifest::set_version(s, &[""], &t.next).map(Some)
        })? {
            changed.push(shelf);
        }
        let pixi = in_dir(dir, "pixi.toml");
        if edit(&r.path, &pixi, |s| {
            manifest::set_version(s, &["package", "workspace"], &t.next).map(Some)
        })? {
            changed.push(pixi);
        }
        let changelog = in_dir(dir, "CHANGELOG.md");
        let fallback = if r.parents.is_empty() {
            String::new()
        } else {
            let pins: Vec<String> = r
                .parents
                .iter()
                .filter_map(|p| plan.get(p))
                .map(|p| format!("{} {}", p.root().name, p.root().next))
                .collect();
            format!("### Changed\n\n- Pins {}.", pins.join(", "))
        };
        // The fragments are read at HEAD, which is the branch just cut from
        // the default branch, and deleted in the release commit.
        let fragments = fragments_at(&r.path, "HEAD", dir)?;
        let entry = fragments.as_deref().map(manifest::fragments_entry);
        let full = r.path.join(&changelog);
        if entry.as_deref().is_some_and(|e| !e.is_empty()) && !full.exists() {
            std::fs::write(&full, "# Changelog\n")
                .with_context(|| format!("could not write {}", full.display()))?;
        }
        if edit(&r.path, &changelog, |s| {
            Ok(manifest::release_changelog(
                s,
                &t.next,
                &date,
                entry.as_deref(),
                &fallback,
            ))
        })? {
            changed.push(changelog);
        }
        for f in fragments.iter().flatten() {
            let file = in_dir(dir, &format!("{}/{}", manifest::CHANGES_DIR, f.file));
            std::fs::remove_file(r.path.join(&file))
                .with_context(|| format!("{}: could not delete {file}", r.repo))?;
            changed.push(file);
        }
    }
    Ok(changed)
}

/// Pins (and the workflows that pin by commit) to each parent's head, and
/// workflow checkouts of any repo below this one to its head; relocks when a
/// manifest changed. Returns the files changed.
fn pin_edits(
    r: &RepoRelease,
    heads: &HashMap<String, String>,
    ref_heads: &HashMap<String, String>,
    old_revs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for file in manifest_files(r) {
        if edit(&r.path, &file, |s| {
            manifest::repin(s, |p| heads.get(&repo_key(&p.git)).cloned()).map(|(out, _)| Some(out))
        })? {
            changed.push(file);
        }
    }
    let manifests_changed = !changed.is_empty();
    let head = git::head_commit(&r.path)?;
    for file in workflows_at(&r.path, &head)? {
        if edit(&r.path, &file, |s| {
            let mut out = s.to_string();
            for (key, new) in heads {
                for old in old_revs.get(key).into_iter().flatten() {
                    if old != new {
                        out = manifest::replace_sha(&out, old, new);
                    }
                }
            }
            let (out, _) = manifest::repoint_refs(&out, |repo| ref_heads.get(repo).cloned());
            Ok(Some(out))
        })? {
            changed.push(file);
        }
    }
    if manifests_changed && r.path.join("pixi.lock").exists() {
        relock(&r.path)?;
        changed.push("pixi.lock".into());
    }
    Ok(changed)
}

fn relock(repo: &Path) -> Result<()> {
    let manifest = repo.join("pixi.toml");
    let status = Command::new("pixi")
        .args(["lock", "--manifest-path"])
        .arg(&manifest)
        .current_dir(repo)
        .status()
        .context("failed to run pixi; is it installed?")?;
    if !status.success() {
        bail!("pixi lock failed in {}", repo.display());
    }
    Ok(())
}

fn commit_and_push(r: &RepoRelease, files: &[String], message: &str) -> Result<String> {
    if files.is_empty() {
        bail!(
            "{}: the release changes nothing; is the plan stale?",
            r.repo
        );
    }
    let mut add = vec!["add", "--"];
    add.extend(files.iter().map(String::as_str));
    git::git(&r.path, &add)?;
    git::git(&r.path, &["commit", "--quiet", "-m", message])?;
    git::git(&r.path, &["push", "--quiet", "-u", "origin", &r.branch])
        .with_context(|| format!("{}: could not push {}", r.repo, r.branch))?;
    let head = git::head_commit(&r.path)?;
    // A quiet push that didn't happen says nothing; check it landed.
    let remote = git::git(&r.path, &["rev-parse", &format!("origin/{}", r.branch)])?;
    if remote != head {
        bail!(
            "{}: pushed {}, but origin has {}",
            r.branch,
            crate::short_sha(&head),
            crate::short_sha(&remote)
        );
    }
    Ok(head)
}

fn release_message(plan: &Plan, r: &RepoRelease) -> String {
    let tins: Vec<String> = r
        .tins
        .iter()
        .map(|t| format!("{} {}", t.name, t.next))
        .collect();
    let mut msg = format!("Release {}", tins.join(" and "));
    if !r.parents.is_empty() {
        let parents: Vec<String> = r
            .parents
            .iter()
            .filter_map(|p| plan.get(p))
            .map(|p| format!("{} {}", p.root().name, p.root().next))
            .collect();
        msg.push_str(&format!(
            "\n\nPins {} at {} release branch head.",
            parents.join(", "),
            if parents.len() == 1 { "its" } else { "their" }
        ));
    }
    msg.push_str("\n\n");
    msg.push_str(RELEASE_COMMIT_MARK);
    msg
}

/// Today's UTC date, YYYY-MM-DD.
fn today() -> String {
    let days = crate::now_unix_secs().div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a calendar date (the inverse of shelf_core's
/// days-from-civil).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// ── preflight ───────────────────────────────────────────────────────────────

fn pixi_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "osx-arm64",
        ("macos", _) => "osx-64",
        ("linux", "aarch64") => "linux-aarch64",
        ("windows", _) => "win-64",
        _ => "linux-64",
    }
}

/// The scratch project's pixi.toml: every tin of `set` in `[dependencies]`,
/// written the way `pixi shelf add` writes them, in one edit.
fn preflight_manifest(set: &[ResolvedTin], channels: &[String], platform: &str) -> Result<String> {
    let mut doc = toml_edit::DocumentMut::new();
    let mut ws = toml_edit::Table::new();
    ws.insert("name", toml_edit::value("shelf-release-preflight"));
    ws.insert(
        "channels",
        toml_edit::value(channels.iter().collect::<toml_edit::Array>()),
    );
    ws.insert(
        "platforms",
        toml_edit::value([platform].into_iter().collect::<toml_edit::Array>()),
    );
    ws.insert(
        "preview",
        toml_edit::value(["pixi-build"].into_iter().collect::<toml_edit::Array>()),
    );
    doc.insert("workspace", toml_edit::Item::Table(ws));
    Ok(crate::pixi::write_pins(&doc.to_string(), set, true)?.0)
}

/// One scratch project that adds every release head the way a consumer
/// would, in one edit, then solves and builds; with `test`, each tin's own
/// `test` task too. Only the repos whose head preflight hasn't passed at run.
fn preflight(
    reg: &Registry,
    path: &Path,
    st: &mut State,
    group: &[String],
    test: bool,
) -> Result<()> {
    let targets: Vec<String> = group
        .iter()
        .filter(|r| !st.is_published(r))
        .filter(|r| {
            let s = st.repos.get(r.as_str()).cloned().unwrap_or_default();
            s.head.is_some() && s.preflight != s.head
        })
        .cloned()
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    if Command::new("pixi").arg("--version").output().is_err() {
        bail!("preflight needs pixi on PATH (or pass --skip-preflight)");
    }
    // Every tin in the cascade at its release head, by name.
    let mut heads: HashMap<String, ResolvedTin> = HashMap::new();
    for r in &st.plan.releases {
        let Some(head) = st.head(&r.repo) else {
            continue;
        };
        for t in &r.tins {
            heads.insert(
                t.name.clone(),
                ResolvedTin {
                    name: t.name.clone(),
                    url: r.url.clone(),
                    version: t.next.clone(),
                    commit_sha: head.to_string(),
                    kind: "source".into(),
                    prev_url: None,
                    url_changed_at: None,
                    subdirectory: t.subdirectory.clone(),
                },
            );
        }
    }
    let mut set: Vec<ResolvedTin> = Vec::new();
    let mut channels: Vec<String> = Vec::new();
    for repo in &targets {
        let r = st.release(repo);
        for t in &r.tins {
            // The tin's published closure, with every cascade tin in it moved
            // to its release head.
            let closure = reg.resolve(&t.name, None).unwrap_or_default();
            for dep in closure
                .into_iter()
                .chain(std::iter::once(heads[&t.name].clone()))
            {
                let dep = heads.get(&dep.name).cloned().unwrap_or(dep);
                if !set.iter().any(|d| d.name == dep.name) {
                    set.push(dep);
                }
            }
        }
        let text = std::fs::read_to_string(r.path.join("pixi.toml")).unwrap_or_default();
        if let Ok(doc) = text.parse::<toml_edit::DocumentMut>() {
            let list = doc
                .get("workspace")
                .and_then(|w| w.get("channels"))
                .and_then(|c| c.as_array());
            for c in list.into_iter().flatten().filter_map(|c| c.as_str()) {
                if !channels.iter().any(|x| x == c) {
                    channels.push(c.to_string());
                }
            }
        }
    }
    if channels.is_empty() {
        channels.push("conda-forge".into());
    }
    let dir = std::env::temp_dir().join(format!("shelf-release-preflight-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let text = preflight_manifest(&set, &channels, pixi_platform())?;
    let manifest = dir.join("pixi.toml");
    std::fs::write(&manifest, &text)?;
    println!(
        "preflight: solving and building {} release head{} together in {}",
        targets.len(),
        if targets.len() == 1 { "" } else { "s" },
        dir.display()
    );
    let status = Command::new("pixi")
        .args(["install", "--manifest-path"])
        .arg(&manifest)
        .status()
        .context("failed to run pixi; is it installed?")?;
    if !status.success() {
        bail!(
            "preflight failed: the release heads don't solve and build together (scratch project in \
             {}); no PRs were opened. Fix the release branch with a new commit, push it, and \
             `shelf release apply --resume`",
            dir.display()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    if test {
        for repo in &targets {
            let r = st.release(repo).clone();
            let text = std::fs::read_to_string(r.path.join("pixi.toml")).unwrap_or_default();
            let has_test = text
                .parse::<toml_edit::DocumentMut>()
                .ok()
                .is_some_and(|d| d.get("tasks").and_then(|t| t.get("test")).is_some());
            if !has_test {
                println!("preflight: {} has no `test` task; skipped", r.repo);
                continue;
            }
            println!("preflight: {}: pixi run test", r.repo);
            let status = Command::new("pixi")
                .args(["run", "test"])
                .current_dir(&r.path)
                .status()
                .context("failed to run pixi; is it installed?")?;
            if !status.success() {
                bail!(
                    "preflight failed: {}'s tests fail against the other release heads; no PRs were \
                     opened",
                    r.repo
                );
            }
        }
    }
    for repo in &targets {
        let s = st.of(repo);
        s.preflight = s.head.clone();
    }
    st.save(path)?;
    println!("preflight passed");
    Ok(())
}

// ── pull requests ───────────────────────────────────────────────────────────

/// Open a PR per branch that has none, then give every PR the cascade's
/// table with links to its siblings. False when PRs can't be made here (no
/// gh, or not GitHub), after printing the branches instead.
fn prs(path: &Path, st: &mut State, group: &[String]) -> Result<bool> {
    let pending: Vec<&String> = group.iter().filter(|r| !st.is_published(r)).collect();
    let github = pending
        .iter()
        .all(|r| github_slug(&st.release(r).url).is_some());
    if !github || !gh::available() {
        let why = if github {
            "gh isn't available"
        } else {
            "not GitHub repositories"
        };
        println!("no PRs opened ({why}); the release branches are pushed:");
        for r in &pending {
            let rel = st.release(r);
            println!(
                "  {}  {}  {}",
                rel.repo,
                rel.branch,
                crate::short_sha(st.head(r).unwrap_or(""))
            );
        }
        return Ok(false);
    }
    let mut opened = false;
    for repo in &pending {
        if st.repos.get(repo.as_str()).is_some_and(|s| s.pr.is_some()) {
            continue;
        }
        let r = st.release(repo).clone();
        let slug = github_slug(&r.url).expect("github");
        let pr = match gh::find(&slug, &r.branch)? {
            Some(pr) => pr,
            None => {
                let title = format!(
                    "Release {}",
                    r.tins
                        .iter()
                        .map(|t| format!("{} {}", t.name, t.next))
                        .collect::<Vec<_>>()
                        .join(" and ")
                );
                gh::create(
                    &slug,
                    &r.default_branch,
                    &r.branch,
                    &title,
                    &pr_body(st, &r),
                )?
            }
        };
        println!("{}: {}", r.repo, pr.url);
        st.of(repo).pr = Some(pr);
        st.save(path)?;
        opened = true;
    }
    if opened {
        // Now every PR exists, link them to each other.
        for repo in &pending {
            let r = st.release(repo).clone();
            if let Some(pr) = st.repos.get(repo.as_str()).and_then(|s| s.pr.clone()) {
                gh::edit_body(
                    &github_slug(&r.url).expect("github"),
                    pr.number,
                    &pr_body(st, &r),
                )?;
            }
        }
    }
    Ok(true)
}

fn pr_body(st: &State, this: &RepoRelease) -> String {
    let mut body = String::from(
        "A release made by `shelf release apply`, one of a cascade of tins that pin each other. \
         Each tin pins its parents at their release branch heads, so all of them are tested at once.\n\n\
         | | repository | release | PR |\n|---|---|---|---|\n",
    );
    for (i, r) in st.plan.releases.iter().enumerate() {
        let tins: Vec<String> = r
            .tins
            .iter()
            .map(|t| format!("{} {}", t.name, t.next))
            .collect();
        let pr = st
            .repos
            .get(&r.repo)
            .and_then(|s| s.pr.as_ref())
            .map(|p| p.url.clone())
            .unwrap_or_else(|| "(not opened yet)".into());
        let me = if r.repo == this.repo {
            " **(this PR)**"
        } else {
            ""
        };
        body.push_str(&format!(
            "| {} | {}{me} | {} | {pr} |\n",
            i + 1,
            r.repo,
            tins.join(", ")
        ));
    }
    if !this.parents.is_empty() {
        body.push_str("\nPins moved here:\n\n");
        for m in &this.pins {
            let head = st.head(&m.parent).unwrap_or("");
            body.push_str(&format!(
                "- `{}` [{}] {}: `{}` -> `{}` ({})\n",
                m.file,
                m.pin.table,
                m.pin.dep,
                crate::short_sha(&m.pin.rev),
                crate::short_sha(head),
                st.release(&m.parent).branch
            ));
        }
    }
    if !this.refs.is_empty() {
        body.push_str("\nCI checkouts moved here:\n\n");
        for m in &this.refs {
            body.push_str(&format!(
                "- `{}:{}` {} of {}: `{}` -> `{}` ({})\n",
                m.file,
                m.line,
                m.kind.describe(),
                m.repo,
                crate::short_sha(&m.rev),
                crate::short_sha(st.head(&m.repo).unwrap_or("")),
                st.release(&m.repo).branch
            ));
        }
    }
    body.push_str(
        "\nMerge with `shelf release apply --resume --merge`: it merges (merge commits, parents first) \
         only when every PR is green and every pin is its parent's current release head, then \
         publishes each tin at its release head.\n",
    );
    body
}

// ── checks and merging ──────────────────────────────────────────────────────

fn wait(st: &State, group: &[String]) -> Result<()> {
    let mut last: HashMap<String, String> = HashMap::new();
    loop {
        let mut pending = false;
        for repo in group.iter().filter(|r| !st.is_published(r)) {
            let r = st.release(repo);
            let Some(pr) = st.repos.get(repo).and_then(|s| s.pr.as_ref()) else {
                continue;
            };
            let checks = gh::checks(&github_slug(&r.url).expect("github"), pr.number)?;
            let line = match &checks {
                Checks::Green => "green".to_string(),
                Checks::Pending(n) => format!("waiting on {}", n.join(", ")),
                Checks::Failed(n) => format!("failed: {}", n.join(", ")),
            };
            if last.get(repo) != Some(&line) {
                println!("{}: {line}", r.repo);
                last.insert(repo.clone(), line);
            }
            match checks {
                Checks::Failed(names) => bail!(
                    "{} failed {} on {}; nothing was merged or published. A flaky check: rerun it. A \
                     fix: push it to {} as a new commit, then `shelf release apply --resume` re-pins \
                     what's below it",
                    r.repo,
                    names.join(", "),
                    pr.url,
                    r.branch
                ),
                Checks::Pending(_) => pending = true,
                Checks::Green => {}
            }
        }
        if !pending {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}

/// What the merge rule looks at for one release.
#[derive(Debug, Clone)]
pub struct MergeInput {
    pub repo: String,
    pub checks: Checks,
    /// Every pin to a parent in the cascade at the release head: (parent
    /// repo, rev).
    pub pins: Vec<(String, String)>,
}

/// Why the cascade can't merge yet; empty when it can. Every PR must be
/// green **and** every pin must be its parent's current release head: a
/// child tested against a parent commit that has since been replaced can't
/// merge, so nothing is published against a commit that isn't.
pub fn merge_blockers(inputs: &[MergeInput], heads: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for i in inputs {
        match &i.checks {
            Checks::Green => {}
            Checks::Pending(n) => out.push(format!(
                "{}: checks still running ({})",
                i.repo,
                n.join(", ")
            )),
            Checks::Failed(n) => out.push(format!("{}: checks failed ({})", i.repo, n.join(", "))),
        }
        for (parent, rev) in &i.pins {
            match heads.get(parent) {
                Some(head) if head == rev => {}
                Some(head) => out.push(format!(
                    "{}: pins {parent} at {}, but its release head is {}; `shelf release apply --resume` \
                     re-pins it",
                    i.repo,
                    crate::short_sha(rev),
                    crate::short_sha(head)
                )),
                None => out.push(format!("{}: {parent} has no release head", i.repo)),
            }
        }
    }
    out
}

/// The pins to cascade parents in the manifests at `commit`.
fn pins_at(st: &State, r: &RepoRelease, commit: &str) -> Result<Vec<(String, String)>> {
    let parents: HashMap<String, String> = r
        .parents
        .iter()
        .map(|p| (repo_key(&st.release(p).url), p.clone()))
        .collect();
    let mut out = Vec::new();
    for file in manifest_files(r) {
        let Some(text) = read_at(&r.path, commit, &file)? else {
            continue;
        };
        for pin in manifest::pins(&text)? {
            if let Some(p) = parents.get(&repo_key(&pin.git)) {
                out.push((p.clone(), pin.rev));
            }
        }
    }
    Ok(out)
}

fn merge(path: &Path, st: &mut State, group: &[String]) -> Result<()> {
    let todo: Vec<String> = group
        .iter()
        .filter(|r| !st.is_published(r) && !st.repos.get(r.as_str()).is_some_and(|s| s.merged))
        .cloned()
        .collect();
    let heads: HashMap<String, String> = st
        .repos
        .iter()
        .filter_map(|(k, s)| s.head.clone().map(|h| (k.clone(), h)))
        .collect();
    let mut inputs = Vec::new();
    for repo in &todo {
        let r = st.release(repo);
        let pr = st.repos[repo]
            .pr
            .clone()
            .ok_or_else(|| anyhow!("{}: no PR to merge", r.repo))?;
        let head = st.head(repo).unwrap_or_default().to_string();
        inputs.push(MergeInput {
            repo: repo.clone(),
            checks: gh::checks(&github_slug(&r.url).expect("github"), pr.number)?,
            pins: pins_at(st, r, &head)?,
        });
    }
    let blockers = merge_blockers(&inputs, &heads);
    if !blockers.is_empty() {
        bail!("not merging yet:\n  {}", blockers.join("\n  "));
    }
    for repo in &todo {
        let r = st.release(repo).clone();
        let head = st.head(repo).unwrap_or_default().to_string();
        git::git(&r.path, &["fetch", "--quiet", "origin"])?;
        let base = format!("origin/{}", r.default_branch);
        if git::git(&r.path, &["merge-base", "--is-ancestor", &base, &head]).is_err() {
            bail!(
                "{}: {} moved since {} was cut. Merge {base} into the release branch as a new commit, \
                 push it, and `shelf release apply --resume` (which re-pins what's below it)",
                r.repo,
                r.default_branch,
                r.branch
            );
        }
        let pr = st.repos[repo].pr.clone().expect("checked above");
        gh::merge(&github_slug(&r.url).expect("github"), pr.number)?;
        println!("merged {}", pr.url);
        st.of(repo).merged = true;
        st.save(path)?;
    }
    Ok(())
}

// ── publishing ──────────────────────────────────────────────────────────────

/// Publish each merged release, parents first, from a detached checkout of
/// its head, then check the registry has it at that commit. Stops at the
/// first release that isn't merged yet.
fn publish(reg: &Registry, path: &Path, st: &mut State, group: &[String]) -> Result<()> {
    for repo in group {
        if st.is_published(repo) {
            continue;
        }
        let r = st.release(repo).clone();
        let head = st
            .head(repo)
            .ok_or_else(|| anyhow!("{}: no release branch to publish", r.repo))?
            .to_string();
        git::git(&r.path, &["fetch", "--quiet", "origin"])?;
        let base = format!("origin/{}", r.default_branch);
        if git::git(&r.path, &["merge-base", "--is-ancestor", &head, &base]).is_err() {
            let same_tree = git::git(&r.path, &["rev-parse", &format!("{head}^{{tree}}")])?
                == git::git(&r.path, &["rev-parse", &format!("{base}^{{tree}}")])?;
            if !same_tree {
                // Parents first: nothing below an unpublished release goes out.
                println!(
                    "{}: {} isn't merged into {} yet",
                    r.repo, r.branch, r.default_branch
                );
                return Ok(());
            }
            eprintln!(
                "warning: {} was squashed or rebased into {}; publishing {} (same tree), which isn't \
                 on the default branch",
                r.branch,
                r.default_branch,
                crate::short_sha(&head)
            );
        }
        let back = git::git(&r.path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let back = if back == "HEAD" {
            git::head_commit(&r.path)?
        } else {
            back
        };
        git::git(&r.path, &["checkout", "--quiet", "--detach", &head])?;
        let result = publish_tins(reg, path, st, &r, &head);
        git::git(&r.path, &["checkout", "--quiet", &back])?;
        result?;
    }
    Ok(())
}

fn publish_tins(
    reg: &Registry,
    path: &Path,
    st: &mut State,
    r: &RepoRelease,
    head: &str,
) -> Result<()> {
    for t in &r.tins {
        if st
            .repos
            .get(&r.repo)
            .is_some_and(|s| s.published.contains(&t.name))
        {
            continue;
        }
        let at = |reg: &Registry| -> Result<Option<String>> {
            Ok(reg
                .info(&t.name)
                .ok()
                .and_then(|d| d.versions.into_iter().find(|v| v.version == t.next))
                .map(|v| v.commit_sha))
        };
        match at(reg)? {
            Some(sha) if sha == head => println!("{} {} is already published", t.name, t.next),
            Some(sha) => bail!(
                "{} {} is already published at {}, not at the release head {}",
                t.name,
                t.next,
                crate::short_sha(&sha),
                crate::short_sha(head)
            ),
            None => {
                let dir: PathBuf = match &t.subdirectory {
                    Some(d) => r.path.join(d),
                    None => r.path.clone(),
                };
                crate::publish_from(reg, &dir)?;
                if at(reg)?.as_deref() != Some(head) {
                    bail!(
                        "published {} {}, but the registry doesn't show it at {}",
                        t.name,
                        t.next,
                        crate::short_sha(head)
                    );
                }
            }
        }
        st.of(&r.repo).published.push(t.name.clone());
        st.save(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::plan::TinRelease;
    use super::*;

    fn release(repo: &str, level: usize, parents: &[&str]) -> RepoRelease {
        RepoRelease {
            repo: repo.into(),
            path: PathBuf::from(repo),
            url: format!("https://github.com/o/{repo}"),
            default_branch: "main".into(),
            base: "b".repeat(40),
            branch: format!("release/{repo}-1.0.1"),
            level,
            parents: parents.iter().map(|p| p.to_string()).collect(),
            tins: vec![TinRelease {
                name: repo.into(),
                subdirectory: None,
                current: "1.0.0".into(),
                next: "1.0.1".into(),
                published_commit: None,
                reason: "named".into(),
                changes: vec![],
            }],
            pins: vec![],
            workflow_pins: vec![],
            refs: vec![],
            mismatches: vec![],
            warnings: vec![],
        }
    }

    /// base1 and base2 named; mid pins base1; top pins mid and base2; side
    /// pins base2 only.
    fn plan() -> Plan {
        Plan {
            releases: vec![
                release("base1", 0, &[]),
                release("base2", 0, &[]),
                release("mid", 1, &["base1"]),
                release("side", 1, &["base2"]),
                release("top", 2, &["mid", "base2"]),
            ],
        }
    }

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_moved_head_re_pins_only_its_descendants() {
        let p = plan();
        assert_eq!(repin_order(&p, &set(&["base1"])), vec!["mid", "top"]);
        assert_eq!(repin_order(&p, &set(&["base2"])), vec!["side", "top"]);
        assert_eq!(repin_order(&p, &set(&["mid"])), vec!["top"]);
        assert!(repin_order(&p, &set(&["top"])).is_empty());
        assert!(repin_order(&p, &set(&[])).is_empty());
    }

    fn heads() -> HashMap<String, String> {
        HashMap::from([("base1".into(), "h1".into()), ("mid".into(), "h2".into())])
    }

    #[test]
    fn merge_needs_green_checks_and_current_pins() {
        let ok = vec![
            MergeInput {
                repo: "base1".into(),
                checks: Checks::Green,
                pins: vec![],
            },
            MergeInput {
                repo: "mid".into(),
                checks: Checks::Green,
                pins: vec![("base1".into(), "h1".into())],
            },
        ];
        assert!(merge_blockers(&ok, &heads()).is_empty());

        // Green everywhere, but mid was tested against base1's old head.
        let mut stale = ok.clone();
        stale[1].pins = vec![("base1".into(), "h0".into())];
        let b = merge_blockers(&stale, &heads());
        assert_eq!(b.len(), 1);
        assert!(b[0].contains("mid: pins base1 at h0"), "{b:?}");

        let mut red = ok.clone();
        red[0].checks = Checks::Failed(vec!["test".into()]);
        red[1].checks = Checks::Pending(vec!["build".into()]);
        let b = merge_blockers(&red, &heads());
        assert_eq!(b.len(), 2, "{b:?}");
        assert!(b[0].contains("failed") && b[1].contains("still running"));
    }

    #[test]
    fn preflight_adds_every_head_in_one_manifest() {
        let tin = |name: &str, sha: &str, sub: Option<&str>| ResolvedTin {
            name: name.into(),
            url: "https://github.com/o/parquet.mojo".into(),
            version: "1.0.1".into(),
            commit_sha: sha.into(),
            kind: "source".into(),
            prev_url: None,
            url_changed_at: None,
            subdirectory: sub.map(str::to_string),
        };
        let text = preflight_manifest(
            &[
                tin("parquet-mojo", "abc", None),
                tin("parquet-full-mojo", "abc", Some("full")),
            ],
            &["https://conda.modular.com/max".into(), "conda-forge".into()],
            "linux-64",
        )
        .unwrap();
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(doc["workspace"]["platforms"].as_array().unwrap().len(), 1);
        assert!(text.contains("preview = [\"pixi-build\"]"), "{text}");
        assert!(text.contains(
            "parquet-full-mojo = { git = \"https://github.com/o/parquet.mojo\", rev = \"abc\", subdirectory = \"full\" }"
        ), "{text}");
    }

    #[test]
    fn dates_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        let secs = shelf_core::iso_to_unix_secs("2026-11-05T12:00:00Z").unwrap();
        assert_eq!(civil_from_days(secs.div_euclid(86_400)), (2026, 11, 5));
        let leap = shelf_core::iso_to_unix_secs("2024-02-29T00:00:00Z").unwrap();
        assert_eq!(civil_from_days(leap / 86_400), (2024, 2, 29));
    }
}
