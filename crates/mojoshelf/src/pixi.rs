//! Pixi mode: install books as registry-pinned git source dependencies,
//! delegating the pixi.toml ceremony to `pixi add --git`. The dependency set
//! is flattened: every book in the transitive resolve set becomes a
//! top-level entry pinned by the registry, mirroring submodule mode.

use crate::registry::Registry;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn run_pixi(args: &[&str]) -> Result<()> {
    let status = Command::new("pixi")
        .args(args)
        .status()
        .context("failed to run pixi; is it installed?")?;
    if !status.success() {
        bail!("pixi {} failed", args.join(" "));
    }
    Ok(())
}

fn find_pixi_toml() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = dir.join("pixi.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            bail!(
                "no pixi.toml found; run inside a pixi workspace, \
                 or use submodule mode (plain `shelf add`)"
            );
        }
    }
}

/// Source dependencies need the pixi-build preview feature in the consumer
/// workspace; fail with the exact snippet instead of a cryptic pixi error.
fn ensure_pixi_build_preview() -> Result<()> {
    let path = find_pixi_toml()?;
    let text = std::fs::read_to_string(&path)?;
    if !text.contains("pixi-build") {
        bail!(
            "pixi source dependencies need the pixi-build preview feature.\n\
             Add this line to the [workspace] section of {}:\n\n    \
             preview = [\"pixi-build\"]\n",
            path.display()
        );
    }
    Ok(())
}

pub fn add(reg: &Registry, spec: &str, dry_run: bool) -> Result<()> {
    let (name, version) = crate::split_spec(spec);
    let set = crate::install_set(reg, name, version)?;
    if dry_run {
        println!("would run:");
        for b in &set {
            println!("  pixi add --git {} --rev {} {}", b.url, b.commit_sha, b.name);
        }
        return Ok(());
    }
    ensure_pixi_build_preview()?;
    for b in &set {
        run_pixi(&["add", "--git", &b.url, "--rev", &b.commit_sha, &b.name])?;
        println!(
            "added {} {} ({}) as a pixi git dependency",
            b.name,
            b.version,
            &b.commit_sha[..12]
        );
    }
    Ok(())
}

pub fn update(reg: &Registry, name: Option<&str>) -> Result<()> {
    // Re-resolving and re-adding moves the pinned revs; `pixi add` replaces
    // the existing entries.
    let Some(name) = name else {
        bail!("pixi mode updates one book at a time: shelf update <name>");
    };
    add(reg, name, false)
}

pub fn remove(name: &str) -> Result<()> {
    ensure_pixi_build_preview()?;
    run_pixi(&["remove", name])?;
    println!(
        "removed {name}; note: its dependencies were added as top-level entries \
         and are not removed automatically (`pixi remove <dep>`)"
    );
    Ok(())
}

pub fn list() -> Result<()> {
    run_pixi(&["list"])
}
