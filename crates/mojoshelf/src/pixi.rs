//! Pixi mode: install tins as registry-pinned git source dependencies,
//! delegating the pixi.toml ceremony to `pixi add --git`. The dependency set
//! is flattened: every tin in the transitive resolve set becomes a
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

pub fn add(reg: &Registry, specs: &[String], dry_run: bool) -> Result<()> {
    add_inner(reg, specs, dry_run, false)
}

fn add_inner(reg: &Registry, specs: &[String], dry_run: bool, force: bool) -> Result<()> {
    let set = crate::resolve_all(reg, specs)?;
    if dry_run {
        println!("would run:");
        for b in &set {
            if b.kind == "channel" {
                println!("  pixi add {}  # modular-community channel", b.name);
            } else {
                println!(
                    "  pixi add --git {} --rev {} {}",
                    b.url, b.commit_sha, b.name
                );
            }
        }
        return Ok(());
    }
    ensure_pixi_build_preview()?;
    for b in &set {
        if force {
            // `pixi add` keeps an existing entry's rev; drop it first so the
            // new pin actually lands.
            let _ = std::process::Command::new("pixi")
                .args(["remove", &b.name])
                .output();
        }
        if b.kind == "channel" {
            // Binary modular-community package: plain conda dependency; the
            // solver owns version + dependency resolution.
            run_pixi(&["add", &b.name])?;
            println!(
                "added {} (modular-community channel, latest {})",
                b.name, b.version
            );
        } else {
            run_pixi(&["add", "--git", &b.url, "--rev", &b.commit_sha, &b.name])?;
            println!(
                "added {} {} ({}) as a pixi git dependency",
                b.name,
                b.version,
                &b.commit_sha[..12]
            );
        }
    }
    Ok(())
}

pub fn update(reg: &Registry, name: Option<&str>) -> Result<()> {
    // Re-resolving and re-adding moves the pinned revs; `pixi add` replaces
    // the existing entries.
    let Some(name) = name else {
        bail!("pixi mode updates one tin at a time: shelf update <name>");
    };
    add_inner(reg, std::slice::from_ref(&name.to_string()), false, true)
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

// ── shelf lint ───────────────────────────────────────────────────────────────
//
// `mojolint` (the lint-mojo tin) is a Mojo executable that lives in the
// consumer's pixi environment, next to the `mojo-lsp-server` its `--lsp`
// mode runs — that is what keeps the linter and the compiler the same
// version. shelf adds what the tin cannot know on its own: which files a
// workspace lints by default and where its imports resolve from.

/// Directories a workspace walk never descends into.
const SKIPPED_DIRS: &[&str] = &[".pixi", ".git", "build", "shelf", "community-recipe"];

fn is_mojo_source(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("mojo") | Some("🔥")
    )
}

fn walk_mojo(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("could not read {}", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk_mojo(&path, out)?;
        } else if is_mojo_source(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// The files to lint: the given paths (directories walked), or `src/` and
/// `tests/` when none are given. Returned relative to `root`, which is where
/// `pixi run` executes the command and how the findings are printed.
fn lint_files(root: &std::path::Path, paths: &[String]) -> Result<Vec<String>> {
    let cwd = std::env::current_dir()?;
    let mut files = Vec::new();
    if paths.is_empty() {
        for default in ["src", "tests"] {
            let dir = root.join(default);
            if dir.is_dir() {
                walk_mojo(&dir, &mut files)?;
            }
        }
        if files.is_empty() {
            bail!(
                "no .mojo files under {}/src or {}/tests; name the files or directories to lint",
                root.display(),
                root.display()
            );
        }
    } else {
        for p in paths {
            let path = cwd.join(p);
            if path.is_dir() {
                walk_mojo(&path, &mut files)?;
            } else if path.is_file() {
                files.push(path);
            } else {
                bail!("{p}: no such file or directory");
            }
        }
    }
    Ok(files
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| f.to_string_lossy().into_owned())
        })
        .collect())
}

/// Where `--lsp` resolves the workspace's imports from: its own `src/`, and
/// the `src/` of every submodule-mode tin under `shelf/`. Tins installed in
/// pixi mode need nothing — their `.mojopkg` sits in the environment's
/// `lib/mojo`, which the compiler already searches.
fn include_dirs(root: &std::path::Path) -> Vec<String> {
    let mut dirs = Vec::new();
    if root.join("src").is_dir() {
        dirs.push("src".to_string());
    }
    if let Ok(entries) = std::fs::read_dir(root.join("shelf")) {
        let mut tins: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        tins.sort();
        for tin in tins {
            let src = format!("shelf/{}/src", tin.to_string_lossy());
            if root.join(&src).is_dir() {
                dirs.push(src);
            }
        }
    }
    dirs
}

/// `shelf lint`: run the workspace's `mojolint` over its sources.
///
/// Exits with mojolint's own status — 1 when there are findings, so the
/// command works as a CI gate — and turns "command not found" into the
/// install hint, since the linter is a tin like any other.
pub fn lint(env: Option<&str>, lsp: bool, paths: &[String]) -> Result<()> {
    let manifest = find_pixi_toml()?;
    let root = manifest
        .parent()
        .context("pixi.toml has no parent directory")?;
    let files = lint_files(root, paths)?;
    let mut args: Vec<String> = vec!["run".into()];
    if let Some(env) = env {
        args.push("-e".into());
        args.push(env.into());
    }
    args.push("--".into());
    args.push("mojolint".into());
    if lsp {
        args.push("--lsp".into());
        for dir in include_dirs(root) {
            args.push("-I".into());
            args.push(dir);
        }
    }
    args.extend(files);
    let status = Command::new("pixi")
        .args(&args)
        .current_dir(root)
        .status()
        .context("failed to run pixi; is it installed?")?;
    match status.code() {
        Some(0) => Ok(()),
        Some(1) => std::process::exit(1),
        Some(127) => bail!(
            "mojolint is not in this workspace's environment; add the linter tin first:\n\n    \
             pixi shelf add lint-mojo\n"
        ),
        Some(code) => bail!("mojolint exited with status {code}"),
        None => bail!("mojolint was killed by a signal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_files_walks_src_and_tests_by_default() {
        let dir = std::env::temp_dir().join(format!("shelf-lint-{}", std::process::id()));
        for sub in ["src/pkg", "tests", "build", "shelf/dep/src", ".pixi/envs"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for f in [
            "src/main.mojo",
            "src/pkg/__init__.🔥",
            "tests/test_x.mojo",
            "src/notes.md",
            "build/out.mojo",
            "shelf/dep/src/dep.mojo",
        ] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let files = lint_files(&dir, &[]).unwrap();
        assert_eq!(
            files,
            vec!["src/main.mojo", "src/pkg/__init__.🔥", "tests/test_x.mojo"]
        );
        assert_eq!(include_dirs(&dir), vec!["src", "shelf/dep/src"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
