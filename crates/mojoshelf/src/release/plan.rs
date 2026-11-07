//! `shelf release plan`: which tins a change ripples up to, in what order, at
//! what versions, and every pin that moves.

use super::manifest::{self, repo_key, Level, Pin, RefKind, WorkflowPin};
use super::{Checkout, Workspace};
use crate::registry::Registry;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use shelf_core::{TinDetail, TinSummary};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

/// What the registry knows, behind a trait so the cascade can be computed
/// over a made-up graph in tests.
pub trait Catalog {
    fn all(&self) -> Result<Vec<TinSummary>>;
    fn detail(&self, name: &str) -> Result<TinDetail>;
}

impl Catalog for Registry {
    fn all(&self) -> Result<Vec<TinSummary>> {
        self.search("")
    }
    fn detail(&self, name: &str) -> Result<TinDetail> {
        self.info(name)
    }
}

/// A whole cascade, parents first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub releases: Vec<RepoRelease>,
}

/// One repository's release: one branch, one commit, and every tin published
/// from it (the root and any subdirectory tins, all at that commit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRelease {
    /// The checkout's directory name, which is how the plan names a repo.
    pub repo: String,
    pub path: PathBuf,
    pub url: String,
    pub default_branch: String,
    /// The default branch's commit the plan was made from.
    pub base: String,
    pub branch: String,
    /// 0 for a repo that pins nothing in the cascade; one more than its
    /// highest parent otherwise. `--waves` releases a level at a time.
    pub level: usize,
    pub parents: Vec<String>,
    pub tins: Vec<TinRelease>,
    pub pins: Vec<PinMove>,
    #[serde(default)]
    pub workflow_pins: Vec<WorkflowPinMove>,
    /// CI checkouts of repositories below this one in the cascade, found by
    /// the repository they name, at whatever commit.
    #[serde(default)]
    pub refs: Vec<RefMove>,
    /// Commits of tins outside the cascade that this repository builds with
    /// and a parent pins at another commit. `apply` refuses them unless
    /// `--allow-mismatch`.
    #[serde(default)]
    pub mismatches: Vec<Mismatch>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinRelease {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    /// The version in shelf.toml on the default branch.
    pub current: String,
    pub next: String,
    /// The commit its latest published version is at, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_commit: Option<String>,
    pub reason: String,
    /// The changelog fragments (`changes/<file>`) the release gathers, with
    /// the level each asks for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<(String, Level)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinMove {
    /// The manifest, relative to the repository root.
    pub file: String,
    #[serde(flatten)]
    pub pin: Pin,
    /// The repo (a plan `repo` name) whose release head the pin moves to.
    pub parent: String,
    /// The old rev isn't the parent's latest published commit.
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPinMove {
    #[serde(flatten)]
    pub pin: WorkflowPin,
    pub parent: String,
}

/// A CI workflow's checkout of a repository the cascade releases before this
/// one (a parent, or a parent's parent, …): it moves to that repository's
/// release head, whatever commit it was at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefMove {
    pub file: String,
    pub line: usize,
    pub kind: RefKind,
    /// The repo (a plan `repo` name) whose release head it moves to.
    pub repo: String,
    pub rev: String,
}

/// A tin outside the cascade that a repository pins or checks out at one
/// commit while one of its parents pins it at another: after the release the
/// repository would build that parent next to a sibling the parent wasn't
/// built with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mismatch {
    /// `file:line` of a workflow, or `file [table] dep` of a manifest pin.
    pub at: String,
    /// The tin's repository, by its checkout name.
    pub tin: String,
    pub rev: String,
    /// The parent (a plan `repo` name) and the commits it pins the tin at.
    pub parent: String,
    pub parent_revs: Vec<String>,
}

impl Mismatch {
    pub fn describe(&self) -> String {
        let theirs: Vec<&str> = self
            .parent_revs
            .iter()
            .map(|r| crate::short_sha(r))
            .collect();
        format!(
            "{} has {} at {}, but {} pins it at {}",
            self.at,
            self.tin,
            crate::short_sha(&self.rev),
            self.parent,
            theirs.join(", ")
        )
    }
}

/// Two revisions name the same commit: equal, or one a hex prefix (7 or more
/// characters) of the other.
fn same_commit(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    a == b
        || (short.len() >= 7
            && short.bytes().all(|c| c.is_ascii_hexdigit())
            && long.starts_with(short))
}

impl Plan {
    /// Every mismatch in the plan, described, with the repo it's in.
    pub fn mismatches(&self) -> Vec<String> {
        self.releases
            .iter()
            .flat_map(|r| {
                r.mismatches
                    .iter()
                    .map(move |m| format!("{}: {}", r.repo, m.describe()))
            })
            .collect()
    }

    pub fn get(&self, repo: &str) -> Option<&RepoRelease> {
        self.releases.iter().find(|r| r.repo == repo)
    }
}

impl RepoRelease {
    /// The tin published from the repository root (or the first one, for a
    /// repo that only publishes from subdirectories).
    pub fn root(&self) -> &TinRelease {
        self.tins
            .iter()
            .find(|t| t.subdirectory.is_none())
            .unwrap_or(&self.tins[0])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl std::str::FromStr for Bump {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "patch" => Ok(Bump::Patch),
            "minor" => Ok(Bump::Minor),
            "major" => Ok(Bump::Major),
            _ => bail!("'{s}' is not a bump; use patch, minor or major"),
        }
    }
}

pub fn bump(version: &str, how: Bump) -> Result<String> {
    let mut v = semver::Version::parse(version)
        .with_context(|| format!("'{version}' is not valid semver"))?;
    v.pre = semver::Prerelease::EMPTY;
    v.build = semver::BuildMetadata::EMPTY;
    match how {
        Bump::Patch => v.patch += 1,
        Bump::Minor => {
            v.minor += 1;
            v.patch = 0;
        }
        Bump::Major => {
            v.major += 1;
            v.minor = 0;
            v.patch = 0;
        }
    }
    Ok(v.to_string())
}

/// The bump a breaking change needs: minor below 1.0, major after.
pub fn breaking(version: &str) -> Bump {
    match semver::Version::parse(version) {
        Ok(v) if v.major == 0 => Bump::Minor,
        _ => Bump::Major,
    }
}

/// The bump a fragment's level makes from `version`.
pub fn bump_for(level: Level, version: &str) -> Bump {
    match level {
        Level::Patch => Bump::Patch,
        Level::Minor => Bump::Minor,
        Level::Breaking => breaking(version),
        Level::Major => Bump::Major,
    }
}

/// What the user asked for.
#[derive(Debug, Default, Clone)]
pub struct Request {
    pub tins: Vec<String>,
    pub changed: bool,
    pub bumps: Vec<(String, Bump)>,
    pub without: Vec<String>,
}

pub fn parse_bump(arg: &str) -> Result<(String, Bump)> {
    let (name, how) = arg
        .split_once('=')
        .ok_or_else(|| anyhow!("--bump takes name=patch|minor|major, not '{arg}'"))?;
    Ok((name.to_string(), how.parse()?))
}

/// The cascade: `seeds`, and every repo whose manifests pin one already in it
/// by `rev`, recursively, in the order they were found. `dependents` lists
/// the registry's dependents of a repo's tins (as repos); `pins_to(child,
/// parent)` says whether the child's manifests pin the parent. A dependent
/// that pins nothing in the cascade isn't in it.
pub fn cascade(
    seeds: &[String],
    mut dependents: impl FnMut(&str) -> Result<Vec<String>>,
    mut pins_to: impl FnMut(&str, &str) -> Result<bool>,
) -> Result<Vec<String>> {
    let mut found: Vec<String> = Vec::new();
    for s in seeds {
        if !found.contains(s) {
            found.push(s.clone());
        }
    }
    let mut next = 0;
    while next < found.len() {
        let parent = found[next].clone();
        next += 1;
        for child in dependents(&parent)? {
            if found.contains(&child) {
                continue;
            }
            if pins_to(&child, &parent)? {
                found.push(child);
            }
        }
    }
    Ok(found)
}

/// Parents-first order and each node's level, ties kept in the given order.
/// `edges` are (child, parent).
pub fn order(nodes: &[String], edges: &[(String, String)]) -> Result<Vec<(String, usize)>> {
    let mut level: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(nodes.len());
    while out.len() < nodes.len() {
        let ready: Vec<&String> = nodes
            .iter()
            .filter(|n| !level.contains_key(n.as_str()))
            .filter(|n| {
                edges
                    .iter()
                    .filter(|(c, _)| c == *n)
                    .all(|(_, p)| level.contains_key(p.as_str()))
            })
            .collect();
        if ready.is_empty() {
            let stuck: Vec<&str> = nodes
                .iter()
                .filter(|n| !level.contains_key(n.as_str()))
                .map(String::as_str)
                .collect();
            bail!(
                "these repositories pin each other in a cycle: {}",
                stuck.join(", ")
            );
        }
        let mut placed = Vec::new();
        for n in ready {
            let l = edges
                .iter()
                .filter(|(c, _)| c == n)
                .map(|(_, p)| level[p.as_str()] + 1)
                .max()
                .unwrap_or(0);
            placed.push((n.as_str(), l));
        }
        for (n, l) in placed {
            level.insert(n, l);
            out.push((n.to_string(), l));
        }
    }
    Ok(out)
}

/// A repository as the registry sees it: its URL and every source tin
/// published from it, root first.
struct Repo {
    url: String,
    tins: Vec<TinSummary>,
}

fn group_by_repo(all: Vec<TinSummary>) -> BTreeMap<String, Repo> {
    let mut repos: BTreeMap<String, Repo> = BTreeMap::new();
    for t in all.into_iter().filter(|t| t.kind == "source") {
        let repo = repos.entry(repo_key(&t.url)).or_insert_with(|| Repo {
            url: t.url.clone(),
            tins: Vec::new(),
        });
        repo.tins.push(t);
    }
    for repo in repos.values_mut() {
        repo.tins.sort_by(|a, b| {
            (a.subdirectory.is_some(), &a.name).cmp(&(b.subdirectory.is_some(), &b.name))
        });
    }
    repos
}

/// The manifests a repository's tins build from: the root pixi.toml and each
/// subdirectory tin's own.
fn manifest_files(repo: &Repo) -> Vec<String> {
    let mut files = vec!["pixi.toml".to_string()];
    for t in &repo.tins {
        if let Some(dir) = &t.subdirectory {
            let f = format!("{dir}/pixi.toml");
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files
}

fn file_in(dir: Option<&str>, name: &str) -> String {
    match dir {
        Some(d) => format!("{d}/{name}"),
        None => name.to_string(),
    }
}

/// Pins with the manifest each is in.
type FilePins = Vec<(String, Pin)>;

/// The registry and the checkouts, looked up once each.
struct Ctx<'a> {
    ws: &'a Workspace,
    cat: &'a dyn Catalog,
    repos: BTreeMap<String, Repo>,
    repo_of_tin: HashMap<String, String>,
    details: RefCell<HashMap<String, TinDetail>>,
    checkouts: RefCell<HashMap<String, Option<Checkout>>>,
    pins: RefCell<HashMap<String, Option<FilePins>>>,
}

impl Ctx<'_> {
    fn lookup(&self, name: &str) -> Result<String> {
        self.repo_of_tin.get(name).cloned().ok_or_else(|| {
            anyhow!("'{name}' is not a source tin in the registry; `shelf search {name}` lists close names")
        })
    }

    fn detail(&self, name: &str) -> Result<TinDetail> {
        if let Some(d) = self.details.borrow().get(name) {
            return Ok(d.clone());
        }
        let d = self
            .cat
            .detail(name)
            .with_context(|| format!("could not look up '{name}' in the registry"))?;
        self.details
            .borrow_mut()
            .insert(name.to_string(), d.clone());
        Ok(d)
    }

    fn locate(&self, key: &str) -> Option<PathBuf> {
        let repo = &self.repos[key];
        let names: Vec<&str> = repo.tins.iter().map(|t| t.name.as_str()).collect();
        self.ws.locate(&repo.url, &names)
    }

    /// The checkout of a repo, opened (and fetched) the first time it's
    /// asked for; `None` when it isn't checked out here.
    fn open(&self, key: &str) -> Result<Option<Checkout>> {
        if let Some(c) = self.checkouts.borrow().get(key) {
            return Ok(c.clone());
        }
        let c = match self.locate(key) {
            Some(path) => Some(Checkout::open(&path)?),
            None => None,
        };
        self.checkouts
            .borrow_mut()
            .insert(key.to_string(), c.clone());
        Ok(c)
    }

    /// Every rev pin in a repo's manifests, with the file it's in.
    fn pins_of(&self, key: &str) -> Result<Option<FilePins>> {
        if let Some(p) = self.pins.borrow().get(key) {
            return Ok(p.clone());
        }
        let found = match self.open(key)? {
            None => None,
            Some(c) => {
                let mut found = Vec::new();
                for file in manifest_files(&self.repos[key]) {
                    let Some(text) = c.read(&file)? else { continue };
                    for pin in
                        manifest::pins(&text).with_context(|| format!("{}/{file}", c.name()))?
                    {
                        found.push((file.clone(), pin));
                    }
                }
                Some(found)
            }
        };
        self.pins
            .borrow_mut()
            .insert(key.to_string(), found.clone());
        Ok(found)
    }

    fn name_of(&self, key: &str) -> String {
        self.checkouts
            .borrow()
            .get(key)
            .and_then(|c| c.as_ref().map(|c| c.name()))
            .unwrap_or_else(|| key.to_string())
    }
}

pub fn make(ws: &Workspace, cat: &dyn Catalog, req: &Request) -> Result<Plan> {
    let repos = group_by_repo(cat.all()?);
    let repo_of_tin: HashMap<String, String> = repos
        .iter()
        .flat_map(|(k, r)| r.tins.iter().map(move |t| (t.name.clone(), k.clone())))
        .collect();
    let cx = Ctx {
        ws,
        cat,
        repos,
        repo_of_tin,
        details: RefCell::default(),
        checkouts: RefCell::default(),
        pins: RefCell::default(),
    };
    let mut without = BTreeSet::new();
    for name in &req.without {
        without.insert(cx.lookup(name)?);
    }

    // 1. Which tins.
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut seeds: Vec<String> = Vec::new();
    let mut changed_note: HashMap<String, String> = HashMap::new();
    for name in &req.tins {
        let key = cx.lookup(name)?;
        if without.contains(&key) {
            bail!("'{name}' is both named and left out with --without");
        }
        named.insert(name.clone());
        if !seeds.contains(&key) {
            seeds.push(key);
        }
    }
    if req.changed {
        for (key, repo) in &cx.repos {
            if without.contains(key) || seeds.contains(key) || cx.locate(key).is_none() {
                continue;
            }
            let Some(c) = cx.open(key)? else { continue };
            let d = cx.detail(&repo.tins[0].name)?;
            let Some(published) = d.versions.first() else {
                continue;
            };
            let n = c.unreleased_commits(&published.commit_sha)?;
            if n > 0 {
                changed_note.insert(
                    key.clone(),
                    format!(
                        "{n} commit{} since {}",
                        if n == 1 { "" } else { "s" },
                        published.version
                    ),
                );
                for t in &repo.tins {
                    named.insert(t.name.clone());
                }
                seeds.push(key.clone());
            }
        }
        if seeds.is_empty() {
            bail!("no tin checked out here has commits since its published version");
        }
    }
    if seeds.is_empty() {
        bail!("name the tins to release, or pass --changed");
    }

    // 2. The cascade.
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for key in &seeds {
        if cx.pins_of(key)?.is_none() {
            missing.insert(key.clone());
        }
    }
    let nodes = cascade(
        &seeds,
        |parent| {
            let mut out = Vec::new();
            for t in &cx.repos[parent].tins {
                for dep in cx.detail(&t.name)?.dependents {
                    let Some(k) = cx.repo_of_tin.get(&dep) else {
                        continue;
                    };
                    if k != parent && !without.contains(k) && !out.contains(k) {
                        out.push(k.clone());
                    }
                }
            }
            Ok(out)
        },
        |child, parent| match cx.pins_of(child)? {
            Some(pins) => Ok(pins.iter().any(|(_, p)| repo_key(&p.git) == parent)),
            None => {
                missing.insert(child.to_string());
                Ok(false)
            }
        },
    )?;
    if !missing.is_empty() {
        let lines: Vec<String> = missing
            .iter()
            .map(|k| {
                let r = &cx.repos[k];
                format!("  git clone {}   # {}", r.url, r.tins[0].name)
            })
            .collect();
        bail!(
            "the cascade reaches repositories that aren't checked out here; clone them \
             next to the others, point at them with --repo <tin>=<path>, or leave them \
             out with --without <tin>:\n{}",
            lines.join("\n")
        );
    }

    let mut edges = Vec::new();
    for child in &nodes {
        let pins = cx.pins_of(child)?.unwrap_or_default();
        for parent in &nodes {
            if parent != child && pins.iter().any(|(_, p)| repo_key(&p.git) == *parent) {
                edges.push((child.clone(), parent.clone()));
            }
        }
    }
    let ordered = order(&nodes, &edges)?;
    // What each repo is released after: its parents, theirs, and so on.
    let mut ancestors: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (key, _) in &ordered {
        let mut below = BTreeSet::new();
        for (_, p) in edges.iter().filter(|(c, _)| c == key) {
            below.insert(p.clone());
            below.extend(ancestors.get(p).cloned().unwrap_or_default());
        }
        ancestors.insert(key.clone(), below);
    }

    // 3. and 4. Versions, pins and workflow pins, per repo.
    let mut releases: Vec<RepoRelease> = Vec::new();
    for (key, level) in &ordered {
        let repo = &cx.repos[key];
        let c = cx.open(key)?.expect("checked out");
        let parents: Vec<String> = edges
            .iter()
            .filter(|(ch, _)| ch == key)
            .map(|(_, p)| p.clone())
            .collect();
        // The commit each parent repo's tins are published at, by subdirectory.
        let mut published_at: HashMap<(String, Option<String>), String> = HashMap::new();
        for p in &parents {
            for t in &cx.repos[p].tins {
                if let Some(v) = cx.detail(&t.name)?.versions.first() {
                    published_at.insert((p.clone(), t.subdirectory.clone()), v.commit_sha.clone());
                }
            }
        }
        let mut pins = Vec::new();
        for (file, pin) in cx.pins_of(key)?.unwrap_or_default() {
            let pk = repo_key(&pin.git);
            if !parents.contains(&pk) {
                continue;
            }
            let published = published_at
                .get(&(pk.clone(), pin.subdirectory.clone()))
                .or_else(|| published_at.get(&(pk.clone(), None)));
            pins.push(PinMove {
                file,
                stale: published.is_some_and(|sha| *sha != pin.rev),
                pin,
                parent: cx.name_of(&pk),
            });
        }

        let below = &ancestors[key];
        let mut workflow_pins = Vec::new();
        let mut refs = Vec::new();
        let mut warnings = Vec::new();
        // What this repo builds with from outside the cascade, for the
        // consistency check: (where, repository, commit).
        let mut outside: Vec<(String, String, String)> = Vec::new();
        for (file, pin) in cx.pins_of(key)?.unwrap_or_default() {
            let pk = repo_key(&pin.git);
            if !nodes.contains(&pk) {
                outside.push((format!("{file} [{}] {}", pin.table, pin.dep), pk, pin.rev));
            }
        }
        for file in c.workflows()? {
            let Some(text) = c.read(&file)? else { continue };
            // Checkouts by repository: every one of a repo released before
            // this one moves to its head, whatever commit it's at now.
            for r in manifest::workflow_refs(&text) {
                if r.repo == *key {
                    continue;
                }
                if below.contains(&r.repo) {
                    refs.push(RefMove {
                        file: file.clone(),
                        line: r.line,
                        kind: r.kind,
                        repo: cx.name_of(&r.repo),
                        rev: r.rev,
                    });
                } else if nodes.contains(&r.repo) {
                    warnings.push(format!(
                        "{file}:{} checks out {} at {}, which this cascade releases but not \
                         before {}; left as it is",
                        r.line,
                        cx.name_of(&r.repo),
                        crate::short_sha(&r.rev),
                        c.name()
                    ));
                } else {
                    outside.push((format!("{file}:{}", r.line), r.repo, r.rev));
                }
            }
            for p in &parents {
                // Any other mention of a commit this cascade knows is a pin
                // of the parent, matched exactly.
                let mut old: Vec<String> = published_at
                    .iter()
                    .filter(|((k, _), _)| k == p)
                    .map(|(_, sha)| sha.clone())
                    .collect();
                for m in pins.iter().filter(|m| repo_key(&m.pin.git) == *p) {
                    old.push(m.pin.rev.clone());
                }
                old.sort();
                old.dedup();
                workflow_pins.extend(manifest::workflow_pins(&file, &text, &old).into_iter().map(
                    |pin| WorkflowPinMove {
                        pin,
                        parent: cx.name_of(p),
                    },
                ));
            }
        }
        workflow_pins.retain(|w| {
            !refs
                .iter()
                .any(|m| m.file == w.pin.file && m.line == w.pin.line)
        });
        workflow_pins.sort_by(|a, b| (&a.pin.file, a.pin.line).cmp(&(&b.pin.file, b.pin.line)));
        refs.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

        // Transitive consistency: a tin outside the cascade that this repo
        // builds with must be at the commit its parents' packages pin it at,
        // which the release doesn't change.
        let mut mismatches = Vec::new();
        for p in &parents {
            let mut theirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (_, pin) in cx.pins_of(p)?.unwrap_or_default() {
                if pin.table.starts_with("package.") {
                    let revs = theirs.entry(repo_key(&pin.git)).or_default();
                    if !revs.contains(&pin.rev) {
                        revs.push(pin.rev);
                    }
                }
            }
            for (at, x, rev) in &outside {
                let Some(revs) = theirs.get(x) else { continue };
                if !revs.iter().any(|r| same_commit(r, rev)) {
                    mismatches.push(Mismatch {
                        at: at.clone(),
                        tin: super::checkout_dir_name(
                            cx.repos.get(x).map(|r| r.url.as_str()).unwrap_or(x),
                        ),
                        rev: rev.clone(),
                        parent: cx.name_of(p),
                        parent_revs: revs.clone(),
                    });
                }
            }
        }

        let mut tins = Vec::new();
        for t in &repo.tins {
            let dir = t.subdirectory.as_deref();
            let shelf_file = file_in(dir, "shelf.toml");
            let shelf = c
                .read(&shelf_file)?
                .ok_or_else(|| anyhow!("{}: no {shelf_file} on {}", c.name(), c.default_branch))?;
            let current = manifest::version_of(&shelf)
                .ok_or_else(|| anyhow!("{}: {shelf_file} has no version", c.name()))?;
            let d = cx.detail(&t.name)?;
            let published =
                shelf_core::latest_version(d.versions.iter().map(|v| v.version.as_str()));
            let published_commit = published.and_then(|p| {
                d.versions
                    .iter()
                    .find(|v| v.version == p)
                    .map(|v| v.commit_sha.clone())
            });
            let is_named = named.contains(&t.name);
            let explicit = req
                .bumps
                .iter()
                .find(|(n, _)| *n == t.name)
                .map(|(_, b)| *b);
            let base = published.unwrap_or(&current).to_string();
            // An author who already bumped shelf.toml (and didn't publish)
            // chose the version; keep it.
            let ahead = match (semver::Version::parse(&current), published) {
                (Ok(cur), Some(p)) => semver::Version::parse(p).is_ok_and(|p| cur > p),
                _ => false,
            };
            let mut reason = if is_named {
                changed_note
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| "named".to_string())
            } else if t.subdirectory.is_some() {
                format!("released with {} at the same commit", repo.tins[0].name)
            } else {
                let mut names: Vec<&str> = Vec::new();
                for m in &pins {
                    if !names.contains(&m.pin.dep.as_str()) {
                        names.push(&m.pin.dep);
                    }
                }
                format!("pins {}", names.join(", "))
            };
            let fragments = c.fragments(dir)?.unwrap_or_default();
            let next = match explicit {
                Some(b) => bump(&base, b)?,
                None if ahead => {
                    reason.push_str("; version already set in shelf.toml");
                    current.clone()
                }
                None => {
                    let changelog = c.read(&file_in(dir, "CHANGELOG.md"))?;
                    let mut how = Bump::Patch;
                    if is_named
                        && changelog
                            .as_deref()
                            .is_some_and(manifest::unreleased_breaks)
                    {
                        reason.push_str("; breaking per CHANGELOG");
                        how = breaking(&base);
                    }
                    // The highest any fragment asks for; they're released
                    // with this version whichever way the tin got here.
                    if let Some(f) = fragments.iter().max_by_key(|f| f.level) {
                        let b = bump_for(f.level, &base);
                        if b > how {
                            how = b;
                            let file = format!("{}/{}", manifest::CHANGES_DIR, f.file);
                            reason.push_str(&format!(
                                "; {} per {}",
                                f.level.as_str(),
                                file_in(dir, &file)
                            ));
                        }
                    }
                    bump(&base, how)?
                }
            };
            tins.push(TinRelease {
                name: t.name.clone(),
                subdirectory: t.subdirectory.clone(),
                current,
                next,
                published_commit,
                reason,
                changes: fragments.into_iter().map(|f| (f.file, f.level)).collect(),
            });
        }
        let root = tins
            .iter()
            .find(|t| t.subdirectory.is_none())
            .unwrap_or(&tins[0]);
        releases.push(RepoRelease {
            repo: c.name(),
            path: c.path.clone(),
            url: repo.url.clone(),
            default_branch: c.default_branch.clone(),
            base: c.base.clone(),
            branch: format!("release/{}-{}", root.name, root.next),
            level: *level,
            parents: parents.iter().map(|p| cx.name_of(p)).collect(),
            tins,
            pins,
            workflow_pins,
            refs,
            mismatches,
            warnings,
        });
    }

    for (name, _) in &req.bumps {
        if !releases
            .iter()
            .any(|r| r.tins.iter().any(|t| t.name == *name))
        {
            bail!("--bump names '{name}', which isn't in the cascade");
        }
    }
    Ok(Plan { releases })
}

/// `owner/name` of a GitHub URL.
pub fn github_slug(url: &str) -> Option<String> {
    let key = repo_key(url);
    let rest = key.strip_prefix("github.com/")?;
    let mut parts = rest.splitn(3, '/');
    let (owner, name) = (parts.next()?, parts.next()?);
    Some(format!("{owner}/{name}"))
}

/// The human form of the plan.
pub fn render(plan: &Plan) -> String {
    let tins: usize = plan.releases.iter().map(|r| r.tins.len()).sum();
    let mut out = format!(
        "release plan, parents first ({} repositor{}, {tins} tin{}):\n",
        plan.releases.len(),
        if plan.releases.len() == 1 { "y" } else { "ies" },
        if tins == 1 { "" } else { "s" },
    );
    let width = plan
        .releases
        .iter()
        .flat_map(|r| r.tins.iter().map(|t| t.name.len()))
        .max()
        .unwrap_or(0);
    for (i, r) in plan.releases.iter().enumerate() {
        out.push_str(&format!("\n{:>2}. {}  {}\n", i + 1, r.repo, r.branch));
        for t in &r.tins {
            let from = t
                .subdirectory
                .as_deref()
                .map(|d| format!("{d}/: "))
                .unwrap_or_default();
            out.push_str(&format!(
                "    {:width$}  {} -> {}  {from}{}\n",
                t.name, t.current, t.next, t.reason
            ));
            for (file, level) in &t.changes {
                let file = format!("{}/{file}", manifest::CHANGES_DIR);
                out.push_str(&format!(
                    "    {:width$}  {} ({})\n",
                    "",
                    file_in(t.subdirectory.as_deref(), &file),
                    level.as_str()
                ));
            }
        }
        let head = |parent: &str| {
            plan.get(parent)
                .map(|p| format!("{} head", p.branch))
                .unwrap_or_else(|| format!("{parent} release head"))
        };
        for m in &r.pins {
            let stale = if m.stale {
                " (not its published commit)"
            } else {
                ""
            };
            out.push_str(&format!(
                "    {} [{}] {}  {}{stale} -> {}\n",
                m.file,
                m.pin.table,
                m.pin.dep,
                crate::short_sha(&m.pin.rev),
                head(&m.parent)
            ));
        }
        for w in &r.workflow_pins {
            out.push_str(&format!(
                "    {}:{}  {} -> {}\n",
                w.pin.file,
                w.pin.line,
                crate::short_sha(&w.pin.rev),
                head(&w.parent)
            ));
        }
        for m in &r.refs {
            out.push_str(&format!(
                "    {}:{}  {} of {}  {} -> {}\n",
                m.file,
                m.line,
                m.kind.describe(),
                m.repo,
                crate::short_sha(&m.rev),
                head(&m.repo)
            ));
        }
        for m in &r.mismatches {
            out.push_str(&format!("    mismatch: {}\n", m.describe()));
        }
        for w in &r.warnings {
            out.push_str(&format!("    warning: {w}\n"));
        }
    }
    let n = plan.mismatches().len();
    if n > 0 {
        out.push_str(&format!(
            "\n{n} mismatch{}: a tin outside the cascade at another commit than a parent \
             pins it at; `apply` refuses unless --allow-mismatch\n",
            if n == 1 { "" } else { "es" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The registry's dependents, and the (child, parent) rev pins.
    type Graph = (
        HashMap<&'static str, Vec<&'static str>>,
        Vec<(&'static str, &'static str)>,
    );

    /// deflate <- avro <- iceberg <- orca and deflate <- parquet <- iceberg;
    /// `docs` depends on deflate without a rev pin (and `site` pins docs);
    /// `lonely` is pinned by avro.
    fn graph() -> Graph {
        let dependents = HashMap::from([
            ("deflate", vec!["avro", "parquet", "iceberg", "docs"]),
            ("avro", vec!["iceberg", "parquet"]),
            ("parquet", vec!["iceberg"]),
            ("iceberg", vec!["orca"]),
            ("orca", vec![]),
            ("docs", vec!["site"]),
            ("lonely", vec!["avro"]),
        ]);
        // (child, parent) pins by rev. parquet lists avro as a dependent
        // edge in the registry (an old version) but no longer pins it.
        let pins = vec![
            ("avro", "deflate"),
            ("parquet", "deflate"),
            ("iceberg", "deflate"),
            ("iceberg", "avro"),
            ("iceberg", "parquet"),
            ("orca", "iceberg"),
            ("site", "docs"),
            ("avro", "lonely"),
        ];
        (dependents, pins)
    }

    fn run(seeds: &[&str]) -> Vec<String> {
        let (deps, pins) = graph();
        cascade(
            &s(seeds),
            |p| Ok(deps.get(p).map(|v| s(v)).unwrap_or_default()),
            |c, p| Ok(pins.contains(&(c, p))),
        )
        .unwrap()
    }

    #[test]
    fn cascade_follows_rev_pins_only() {
        assert_eq!(
            run(&["deflate"]),
            s(&["deflate", "avro", "parquet", "iceberg", "orca"])
        );
        // docs depends on deflate without a rev pin: not touched, and neither
        // is what pins docs.
        assert!(!run(&["deflate"]).contains(&"docs".to_string()));
        assert_eq!(run(&["iceberg"]), s(&["iceberg", "orca"]));
        assert_eq!(run(&["orca"]), s(&["orca"]));
        assert_eq!(
            run(&["lonely", "deflate"]),
            s(&["lonely", "deflate", "avro", "parquet", "iceberg", "orca"])
        );
    }

    #[test]
    fn order_is_parents_first_with_levels() {
        let nodes = run(&["deflate"]);
        let (_, pins) = graph();
        let edges: Vec<(String, String)> = pins
            .iter()
            .filter(|(c, p)| nodes.contains(&c.to_string()) && nodes.contains(&p.to_string()))
            .map(|(c, p)| (c.to_string(), p.to_string()))
            .collect();
        let ordered = order(&nodes, &edges).unwrap();
        assert_eq!(
            ordered,
            vec![
                ("deflate".into(), 0),
                ("avro".into(), 1),
                ("parquet".into(), 1),
                ("iceberg".into(), 2),
                ("orca".into(), 3)
            ]
        );
        let cycle = order(
            &s(&["a", "b"]),
            &[("a".into(), "b".into()), ("b".into(), "a".into())],
        );
        assert!(cycle.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn versions_bump_and_breaking_depends_on_major() {
        assert_eq!(bump("0.2.0", Bump::Patch).unwrap(), "0.2.1");
        assert_eq!(bump("0.2.7", Bump::Minor).unwrap(), "0.3.0");
        assert_eq!(bump("1.4.2", Bump::Major).unwrap(), "2.0.0");
        assert_eq!(bump("1.0.0-rc.1", Bump::Patch).unwrap(), "1.0.1");
        assert!(bump("one", Bump::Patch).is_err());
        assert_eq!(breaking("0.9.1"), Bump::Minor);
        assert_eq!(breaking("1.2.0"), Bump::Major);
        assert_eq!(bump_for(Level::Breaking, "0.9.1"), Bump::Minor);
        assert_eq!(bump_for(Level::Breaking, "1.2.0"), Bump::Major);
        assert_eq!(bump_for(Level::Major, "0.9.1"), Bump::Major);
        assert!(Bump::Patch < Bump::Minor && Bump::Minor < Bump::Major);
        assert_eq!(
            parse_bump("avro-mojo=minor").unwrap(),
            ("avro-mojo".into(), Bump::Minor)
        );
        assert!(parse_bump("avro-mojo").is_err());
        assert!(parse_bump("avro-mojo=huge").is_err());
    }

    #[test]
    fn same_commit_takes_a_prefix() {
        let full = "4cf043712aa386c1ed2403771969fb3e4c79ea2d";
        assert!(same_commit(full, full));
        assert!(same_commit("4cf0437", full) && same_commit(full, "4cf0437"));
        assert!(!same_commit("4cf04", full), "too short to be sure");
        assert!(!same_commit("5cf0437", full));
        assert!(!same_commit("main", "main-2"));
    }

    #[test]
    fn github_slugs() {
        assert_eq!(
            github_slug("https://github.com/magmalake/deflate.mojo.git").as_deref(),
            Some("magmalake/deflate.mojo")
        );
        assert_eq!(github_slug("file:///tmp/deflate.git"), None);
    }
}
