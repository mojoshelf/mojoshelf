mod git;
mod graduate;
mod pixi;
mod registry;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use registry::Registry;
use shelf_core::{Manifest, PublishRequest, ResolvedTin};
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "shelf",
    version,
    about = "mojoshelf: a registry of Mojo tins",
    long_about = "mojoshelf: a registry of Mojo tins.\n\n\
        Two install modes:\n  \
        - submodule mode (default as `shelf`): tins become git submodules under shelf/<name>\n  \
        - pixi mode (default as `pixi shelf`, or --pixi): tins become registry-pinned\n    \
        git source dependencies via `pixi add --git`, built by pixi-build-mojo"
)]
struct Cli {
    /// Registry base URL.
    #[arg(long, global = true, env = "SHELF_REGISTRY", default_value = "https://mojoshelf.org")]
    registry: String,
    /// Install via pixi git source dependencies instead of git submodules
    /// (the default when invoked as `pixi shelf`).
    #[arg(long, global = true)]
    pixi: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a tin (and its dependencies) as submodules under shelf/.
    Add {
        /// Tin name, optionally with a version: name[@version].
        spec: String,
        /// Print the install set without touching git.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a tin's submodule.
    Remove { name: String },
    /// Re-pin a tin (or all installed tins) to its latest published version.
    Update { name: Option<String> },
    /// List installed tins with their pinned versions.
    List,
    /// Search registry tin names and descriptions.
    Search { term: Option<String> },
    /// Show a tin's description, URL, versions, and dependencies.
    Info { name: String },
    /// Publish the version in ./shelf.toml to the registry.
    Publish,
    /// Generate a modular-community channel recipe from this tin (the
    /// graduation path): preflight checks, recipe.yaml, submission steps.
    Graduate {
        /// GitHub username for extra.maintainers (default: the repo owner).
        #[arg(long)]
        maintainer: Option<String>,
        /// SPDX license id (default: detected from the LICENSE file).
        #[arg(long)]
        license: Option<String>,
        /// Output directory for the generated recipe.
        #[arg(long, default_value = "community-recipe")]
        out: String,
    },
}

/// True when running as the pixi extension (`pixi shelf …` dispatches to a
/// binary named pixi-shelf).
fn invoked_as_pixi_extension() -> bool {
    std::env::args()
        .next()
        .map(|argv0| {
            Path::new(&argv0)
                .file_stem()
                .map(|s| s.to_string_lossy().starts_with("pixi-"))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn main() {
    let cli = Cli::parse();
    let reg = Registry::new(&cli.registry);
    let pixi_mode = cli.pixi || invoked_as_pixi_extension();
    let result = match cli.cmd {
        Cmd::Add { spec, dry_run } if pixi_mode => pixi::add(&reg, &spec, dry_run),
        Cmd::Add { spec, dry_run } => add(&reg, &spec, dry_run),
        Cmd::Remove { name } if pixi_mode => pixi::remove(&name),
        Cmd::Remove { name } => remove(&reg, &name),
        Cmd::Update { name } if pixi_mode => pixi::update(&reg, name.as_deref()),
        Cmd::Update { name } => update(&reg, name.as_deref()),
        Cmd::List if pixi_mode => pixi::list(),
        Cmd::List => list(&reg),
        Cmd::Search { term } => search(&reg, term.as_deref().unwrap_or("")),
        Cmd::Info { name } => info(&reg, &name),
        Cmd::Publish => publish(&reg),
        Cmd::Graduate { maintainer, license, out } => {
            graduate::run(maintainer.as_deref(), license.as_deref(), &out)
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use shelf_core::Manifest;

    #[test]
    fn manifest_accepts_legacy_books_alias() {
        let m: Manifest =
            toml::from_str("name = \"a\"\nversion = \"1.0.0\"\nbooks = [\"zlib-mojo\"]").unwrap();
        assert_eq!(m.tins, vec!["zlib-mojo"]);
        let m2: Manifest =
            toml::from_str("name = \"a\"\nversion = \"1.0.0\"\ntins = [\"csv\"]").unwrap();
        assert_eq!(m2.tins, vec!["csv"]);
    }
}

/// Converts an ssh-style remote (git@host:owner/repo) to https so that
/// consumers without ssh access can clone the submodule.
pub(crate) fn https_url(origin: &str) -> String {
    match origin.strip_prefix("git@").and_then(|rest| rest.split_once(':')) {
        Some((host, path)) => format!("https://{host}/{path}"),
        None => origin.to_string(),
    }
}

fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (spec, None),
    }
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Warns when a tin's git URL changed within the last month
/// (shelf_core::URL_CHANGE_WARN_DAYS) — the repo behind the name may no
/// longer be the one the consumer vetted.
fn warn_recent_url_change(name: &str, url: &str, prev_url: Option<&str>, changed_at: Option<&str>) {
    let Some(changed) = changed_at else { return };
    if !shelf_core::url_change_is_recent(changed, now_unix_secs()) {
        return;
    }
    let from = prev_url.map(|p| format!(" from {p}")).unwrap_or_default();
    eprintln!(
        "warning: the git repository behind '{name}' changed{from} to {url} on {}; \
         review it before trusting the code",
        changed.get(..10).unwrap_or(changed),
    );
}

fn install_set(reg: &Registry, name: &str, version: Option<&str>) -> Result<Vec<ResolvedTin>> {
    let set = reg
        .resolve(name, version)
        .with_context(|| format!("could not resolve '{name}'"))?;
    for tin in &set {
        warn_recent_url_change(&tin.name, &tin.url, tin.prev_url.as_deref(), tin.url_changed_at.as_deref());
    }
    Ok(set)
}

fn install(root: &Path, tin: &ResolvedTin) -> Result<()> {
    let path = format!("shelf/{}", tin.name);
    git::add_submodule(root, &tin.url, &path)?;
    git::pin_submodule(root, &path, &tin.commit_sha)?;
    println!("added {} {} ({})", tin.name, tin.version, &tin.commit_sha[..12]);
    Ok(())
}

fn add(reg: &Registry, spec: &str, dry_run: bool) -> Result<()> {
    let (name, version) = split_spec(spec);
    let set = install_set(reg, name, version)?;
    if dry_run {
        println!("would install into shelf/:");
        for b in &set {
            println!("  {} {} ({})", b.name, b.version, &b.commit_sha[..12]);
        }
        return Ok(());
    }
    let root = git::repo_root()?;
    let installed: Vec<String> = git::installed_tins(&root)?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for tin in &set {
        if tin.kind == "channel" {
            bail!(
                "'{}' comes from the modular-community channel (binary \
                 package); submodule mode needs a source tin. Use pixi mode \
                 instead: pixi shelf add {}",
                tin.name,
                tin.name
            );
        }
        if installed.contains(&tin.name) {
            println!("skipping {} (already installed)", tin.name);
            continue;
        }
        install(&root, tin)?;
    }
    println!("done; commit the submodule changes when ready");
    Ok(())
}

fn remove(reg: &Registry, name: &str) -> Result<()> {
    let root = git::repo_root()?;
    let installed = git::installed_tins(&root)?;
    if !installed.iter().any(|(n, _)| n == name) {
        bail!("'{name}' is not installed under shelf/");
    }
    // Warn if any other installed tin's pinned version depends on it.
    for (other, sha) in installed.iter().filter(|(n, _)| n != name) {
        if let Ok(detail) = reg.info(other) {
            let depends = detail
                .versions
                .iter()
                .find(|v| v.commit_sha == *sha)
                .map(|v| v.dependencies.iter().any(|d| d == name))
                .unwrap_or(false);
            if depends {
                println!("warning: installed tin '{other}' depends on '{name}'");
            }
        }
    }
    git::remove_submodule(&root, &format!("shelf/{name}"))?;
    println!("removed {name}; commit the change when ready");
    Ok(())
}

fn update(reg: &Registry, name: Option<&str>) -> Result<()> {
    let root = git::repo_root()?;
    let installed = git::installed_tins(&root)?;
    if installed.is_empty() {
        bail!("nothing installed under shelf/");
    }
    let targets: Vec<&(String, String)> = match name {
        Some(n) => {
            let found = installed
                .iter()
                .find(|(bn, _)| bn == n)
                .ok_or_else(|| anyhow!("'{n}' is not installed under shelf/"))?;
            vec![found]
        }
        None => installed.iter().collect(),
    };
    for (tin_name, _) in targets {
        for resolved in install_set(reg, tin_name, None)? {
            let path = format!("shelf/{}", resolved.name);
            match installed.iter().find(|(n, _)| *n == resolved.name) {
                None => install(&root, &resolved)?,
                Some((_, sha)) if *sha != resolved.commit_sha => {
                    git::pin_submodule(&root, &path, &resolved.commit_sha)?;
                    println!(
                        "updated {} -> {} ({})",
                        resolved.name,
                        resolved.version,
                        &resolved.commit_sha[..12]
                    );
                }
                Some(_) => println!("{} is up to date ({})", resolved.name, resolved.version),
            }
        }
    }
    println!("done; commit the submodule changes when ready");
    Ok(())
}

fn list(reg: &Registry) -> Result<()> {
    let root = git::repo_root()?;
    let installed = git::installed_tins(&root)?;
    if installed.is_empty() {
        println!("nothing installed under shelf/");
        return Ok(());
    }
    for (name, sha) in installed {
        let version = reg
            .info(&name)
            .ok()
            .and_then(|d| {
                d.versions
                    .iter()
                    .find(|v| v.commit_sha == sha)
                    .map(|v| v.version.clone())
            })
            .unwrap_or_else(|| "?".into());
        let short = sha.get(..12).unwrap_or(&sha);
        println!("{name} {version} ({short})");
    }
    Ok(())
}

fn search(reg: &Registry, term: &str) -> Result<()> {
    let tins = reg.search(term)?;
    if tins.is_empty() {
        println!("no tins found");
        return Ok(());
    }
    for b in tins {
        println!(
            "{} {} — {}",
            b.name,
            b.latest_version.as_deref().unwrap_or("(unpublished)"),
            b.description.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn info(reg: &Registry, name: &str) -> Result<()> {
    let d = reg.info(name)?;
    println!("{}", d.name);
    println!("  url: {}", d.url);
    warn_recent_url_change(&d.name, &d.url, d.prev_url.as_deref(), d.url_changed_at.as_deref());
    if let Some(author) = &d.author {
        println!("  author: {author}");
    }
    if let Some(desc) = &d.description {
        println!("  description: {desc}");
    }
    if !d.tags.is_empty() {
        println!("  tags: {}", d.tags.join(", "));
    }
    if !d.dependents.is_empty() {
        println!("  depended on by: {}", d.dependents.join(", "));
    }
    if let (Some(stars), Some(push)) = (d.stars, d.last_push.as_deref()) {
        let commits = match (d.commits_month, d.commits_year) {
            (Some(m), Some(y)) => format!("; {m} commits last month, {y} last year"),
            _ => String::new(),
        };
        println!("  activity: {stars} stars; last push {push}{commits}");
    }
    if d.kind == "channel" {
        println!(
            "  kind: modular-community channel package (latest {})",
            d.channel_version.as_deref().unwrap_or("?")
        );
    } else if let Some(cv) = &d.channel_version {
        println!("  graduated: also on the modular-community channel as {cv}");
        if d.versions.is_empty() {
            println!("  no published versions");
        }
    } else if d.versions.is_empty() {
        println!("  no published versions");
    }
    for v in &d.versions {
        let deps = if v.dependencies.is_empty() {
            String::new()
        } else {
            format!("  deps: {}", v.dependencies.join(", "))
        };
        println!(
            "  {} ({}) published {}{deps}",
            v.version,
            &v.commit_sha[..12],
            v.published_at
        );
    }
    Ok(())
}

/// A tin is pixi-consumable when its pixi.toml declares a [package] built
/// by pixi-build-mojo. Warn (not fail) otherwise: FFI tins legitimately
/// stay submodule-only until the backend supports their build steps.
fn warn_if_not_pixi_consumable(manifest: &Manifest) {
    let ok = std::fs::read_to_string("pixi.toml")
        .map(|text| text.contains("[package]") && text.contains("pixi-build-mojo"))
        .unwrap_or(false);
    if ok {
        return;
    }
    let name = &manifest.name;
    let version = &manifest.version;
    eprintln!(
        "warning: '{name}' is not consumable as a pixi source dependency — \
pixi.toml has no [package] section with the pixi-build-mojo backend, so \
consumers can only install it in submodule mode.\n\
To fix: make the library a Mojo package (src/{name}/__init__.mojo — \
`from {name} import …` keeps working for -I consumers) and add to pixi.toml:\n\
\n\
    [workspace]                     # existing section\n\
    preview = [\"pixi-build\"]\n\
\n\
    [package]\n\
    name = \"{name}\"\n\
    version = \"{version}\"\n\
\n\
    [package.build]\n\
    backend = {{ name = \"pixi-build-mojo\", version = \"0.*\" }}\n\
\n\
    [package.build.config.pkg]\n\
    path = \"src/{name}\"\n\
    name = \"{name}\"\n\
\n\
    [package.host-dependencies]\n\
    mojo-compiler = \"==1.0.0\"\n\
\n\
    [package.build-dependencies]\n\
    mojo-compiler = \"==1.0.0\"\n\
\n\
    [package.run-dependencies]\n\
    mojo-compiler = \"==1.0.0\"\n\
\n\
Then verify with `pixi build` before publishing."
    );
}

fn publish(reg: &Registry) -> Result<()> {
    let manifest_path = Path::new("shelf.toml");
    let raw = std::fs::read_to_string(manifest_path)
        .context("no shelf.toml here; run publish from the tin's repo root")?;
    let manifest: Manifest = toml::from_str(&raw).context("could not parse shelf.toml")?;
    semver::Version::parse(&manifest.version)
        .with_context(|| format!("'{}' in shelf.toml is not valid semver", manifest.version))?;

    let cwd = std::env::current_dir()?;
    if !git::working_tree_clean(&cwd)? {
        bail!("working tree is dirty; commit or stash before publishing");
    }
    if !git::head_is_pushed(&cwd)? {
        bail!("HEAD is not on any remote branch; push before publishing");
    }
    let commit_sha = git::head_commit(&cwd)?;
    let origin = git::git(&cwd, &["remote", "get-url", "origin"])
        .context("no 'origin' remote; publishing needs a public repo URL")?;
    warn_if_not_pixi_consumable(&manifest);
    reg.publish(&PublishRequest {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        commit_sha: commit_sha.clone(),
        url: https_url(&origin),
        description: manifest.description.clone(),
        tags: manifest.tags.clone(),
        dependencies: manifest.tins,
    })?;
    println!(
        "published {} {} ({})",
        manifest.name,
        manifest.version,
        &commit_sha[..12]
    );
    Ok(())
}
