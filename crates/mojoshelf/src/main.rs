mod git;
mod registry;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use registry::Registry;
use shelf_core::{Manifest, PublishRequest, ResolvedBook};
use std::path::Path;

#[derive(Parser)]
#[command(name = "shelf", version, about = "mojoshelf: a git-submodule-based registry of Mojo books")]
struct Cli {
    /// Registry base URL.
    #[arg(long, global = true, env = "SHELF_REGISTRY", default_value = "https://mojoshelf.org")]
    registry: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a book (and its dependencies) as submodules under shelf/.
    Add {
        /// Book name, optionally with a version: name[@version].
        spec: String,
        /// Print the install set without touching git.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a book's submodule.
    Remove { name: String },
    /// Re-pin a book (or all installed books) to its latest published version.
    Update { name: Option<String> },
    /// List installed books with their pinned versions.
    List,
    /// Search registry book names and descriptions.
    Search { term: Option<String> },
    /// Show a book's description, URL, versions, and dependencies.
    Info { name: String },
    /// Publish the version in ./shelf.toml to the registry.
    Publish,
}

fn main() {
    let cli = Cli::parse();
    let reg = Registry::new(&cli.registry);
    let result = match cli.cmd {
        Cmd::Add { spec, dry_run } => add(&reg, &spec, dry_run),
        Cmd::Remove { name } => remove(&reg, &name),
        Cmd::Update { name } => update(&reg, name.as_deref()),
        Cmd::List => list(&reg),
        Cmd::Search { term } => search(&reg, term.as_deref().unwrap_or("")),
        Cmd::Info { name } => info(&reg, &name),
        Cmd::Publish => publish(&reg),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Converts an ssh-style remote (git@host:owner/repo) to https so that
/// consumers without ssh access can clone the submodule.
fn https_url(origin: &str) -> String {
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

fn install_set(reg: &Registry, name: &str, version: Option<&str>) -> Result<Vec<ResolvedBook>> {
    reg.resolve(name, version)
        .with_context(|| format!("could not resolve '{name}'"))
}

fn install(root: &Path, book: &ResolvedBook) -> Result<()> {
    let path = format!("shelf/{}", book.name);
    git::add_submodule(root, &book.url, &path)?;
    git::pin_submodule(root, &path, &book.commit_sha)?;
    println!("added {} {} ({})", book.name, book.version, &book.commit_sha[..12]);
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
    let installed: Vec<String> = git::installed_books(&root)?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for book in &set {
        if installed.contains(&book.name) {
            println!("skipping {} (already installed)", book.name);
            continue;
        }
        install(&root, book)?;
    }
    println!("done; commit the submodule changes when ready");
    Ok(())
}

fn remove(reg: &Registry, name: &str) -> Result<()> {
    let root = git::repo_root()?;
    let installed = git::installed_books(&root)?;
    if !installed.iter().any(|(n, _)| n == name) {
        bail!("'{name}' is not installed under shelf/");
    }
    // Warn if any other installed book's pinned version depends on it.
    for (other, sha) in installed.iter().filter(|(n, _)| n != name) {
        if let Ok(detail) = reg.info(other) {
            let depends = detail
                .versions
                .iter()
                .find(|v| v.commit_sha == *sha)
                .map(|v| v.dependencies.iter().any(|d| d == name))
                .unwrap_or(false);
            if depends {
                println!("warning: installed book '{other}' depends on '{name}'");
            }
        }
    }
    git::remove_submodule(&root, &format!("shelf/{name}"))?;
    println!("removed {name}; commit the change when ready");
    Ok(())
}

fn update(reg: &Registry, name: Option<&str>) -> Result<()> {
    let root = git::repo_root()?;
    let installed = git::installed_books(&root)?;
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
    for (book_name, _) in targets {
        for resolved in install_set(reg, book_name, None)? {
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
    let installed = git::installed_books(&root)?;
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
    let books = reg.search(term)?;
    if books.is_empty() {
        println!("no books found");
        return Ok(());
    }
    for b in books {
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
    if let Some(author) = &d.author {
        println!("  author: {author}");
    }
    if let Some(desc) = &d.description {
        println!("  description: {desc}");
    }
    if d.versions.is_empty() {
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

fn publish(reg: &Registry) -> Result<()> {
    let manifest_path = Path::new("shelf.toml");
    let raw = std::fs::read_to_string(manifest_path)
        .context("no shelf.toml here; run publish from the book's repo root")?;
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
    reg.publish(&PublishRequest {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        commit_sha: commit_sha.clone(),
        url: https_url(&origin),
        dependencies: manifest.books,
    })?;
    println!(
        "published {} {} ({})",
        manifest.name,
        manifest.version,
        &commit_sha[..12]
    );
    Ok(())
}
