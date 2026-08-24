//! Thin wrappers over the `git` binary.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("failed to run git; is it installed?")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Root of the repository the user runs `shelf` inside.
pub fn repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let root = git(&cwd, &["rev-parse", "--show-toplevel"])
        .context("shelf must be run inside a git repository")?;
    Ok(PathBuf::from(root))
}

/// Submodule paths under shelf/ with their pinned commits, from `.gitmodules`.
pub fn installed_tins(root: &Path) -> Result<Vec<(String, String)>> {
    if !root.join(".gitmodules").exists() {
        return Ok(vec![]);
    }
    let paths = git(
        root,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    )
    .unwrap_or_default();
    let mut tins = Vec::new();
    for line in paths.lines() {
        let Some(path) = line.split_whitespace().nth(1) else {
            continue;
        };
        let Some(name) = path.strip_prefix("shelf/") else {
            continue;
        };
        let status = git(root, &["submodule", "status", "--", path])?;
        let sha = status
            .trim_start_matches(['+', '-', 'U', ' '])
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        tins.push((name.to_string(), sha));
    }
    Ok(tins)
}

/// Checks out `commit` inside the submodule at `path` and stages the pin.
pub fn pin_submodule(root: &Path, path: &str, commit: &str) -> Result<()> {
    let sub = root.join(path);
    if git(&sub, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_err() {
        // The pinned commit may not be on the default branch we cloned.
        let _ = git(&sub, &["fetch", "origin", commit]);
        if git(&sub, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_err() {
            git(&sub, &["fetch", "origin"])?;
        }
    }
    git(&sub, &["checkout", "--detach", commit])
        .with_context(|| format!("could not check out {commit} in {path}"))?;
    git(root, &["add", path])?;
    Ok(())
}

pub fn add_submodule(root: &Path, url: &str, path: &str) -> Result<()> {
    git(root, &["submodule", "add", "--force", url, path])?;
    Ok(())
}

pub fn remove_submodule(root: &Path, path: &str) -> Result<()> {
    git(root, &["submodule", "deinit", "-f", "--", path])?;
    git(root, &["rm", "-f", "--", path])?;
    let module_dir = root.join(".git").join("modules").join(path);
    if module_dir.exists() {
        std::fs::remove_dir_all(&module_dir)
            .with_context(|| format!("could not remove {}", module_dir.display()))?;
    }
    Ok(())
}

pub fn head_commit(dir: &Path) -> Result<String> {
    git(dir, &["rev-parse", "HEAD"])
}

pub fn working_tree_clean(dir: &Path) -> Result<bool> {
    // Untracked files don't affect the committed content being pinned.
    Ok(git(dir, &["status", "--porcelain", "-uno"])?.is_empty())
}

pub fn head_is_pushed(dir: &Path) -> Result<bool> {
    Ok(!git(dir, &["branch", "-r", "--contains", "HEAD"])?.is_empty())
}
