//! Pixi mode: install tins as registry-pinned git source dependencies,
//! written into pixi.toml as the entries `pixi add --git` would make and
//! solved once. The dependency set is flattened: every tin in the transitive
//! resolve set becomes a top-level entry pinned by the registry, mirroring
//! submodule mode.

use crate::registry::Registry;
use anyhow::{bail, Context, Result};
use shelf_core::{ResolvedTin, TinSummary};
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
    let set = crate::resolve_all(reg, specs)?;
    if dry_run {
        print_plan(&set);
        return Ok(());
    }
    // `add` keeps the rev of a tin that is already in pixi.toml; only `update`
    // moves pins.
    apply(&set, false)
}

pub fn update(reg: &Registry, name: Option<&str>) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => vec![n.to_string()],
        None => {
            let text = std::fs::read_to_string(find_pixi_toml()?)?;
            let names = installed_tins(reg, &text)?;
            if names.is_empty() {
                bail!("no tins in pixi.toml's [dependencies]");
            }
            names.into_iter().map(|(n, _)| n).collect()
        }
    };
    // The closures of every target, merged: the registry resolves each
    // dependency to its latest version, so they agree on every shared tin.
    let set = crate::resolve_all(reg, &names)?;
    apply(&set, true)
}

fn print_plan(set: &[ResolvedTin]) {
    println!("would write to pixi.toml [dependencies], then run `pixi install` once:");
    for b in set {
        match pin_value(b) {
            Some(v) => println!("  {} = {}", b.name, v.to_string().trim()),
            None => println!("  {} = \"*\"  # modular-community channel", b.name),
        }
    }
}

/// The `[dependencies]` value pinning a source tin to its registry rev, the
/// same inline table `pixi add --git` writes. `None` for a channel package.
fn pin_value(b: &ResolvedTin) -> Option<toml_edit::Value> {
    if b.kind == "channel" {
        return None;
    }
    let mut t = toml_edit::InlineTable::new();
    t.insert("git", b.url.as_str().into());
    t.insert("rev", b.commit_sha.as_str().into());
    if let Some(dir) = b.subdirectory.as_deref() {
        t.insert("subdirectory", dir.into());
    }
    t.fmt();
    Some(toml_edit::Value::InlineTable(t))
}

/// What writing the install set did to one entry.
#[derive(Debug, PartialEq)]
pub(crate) enum Change {
    Added,
    /// Re-pinned; holds the previous rev when there was one.
    Repinned(Option<String>),
    Unchanged,
    /// Already present and left alone: `add` on an installed tin, or a
    /// channel package whose version the solver owns.
    Kept,
}

/// The git pin of an existing `[dependencies]` entry: (git, rev, subdirectory).
fn existing_pin(item: &toml_edit::Item) -> Option<(String, String, Option<String>)> {
    let t = item.as_table_like()?;
    let get = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some((
        get("git")?,
        get("rev").unwrap_or_default(),
        get("subdirectory"),
    ))
}

/// Write every tin of the install set into `[dependencies]` in one edit.
///
/// Entries are changed in place, so the order of the table and the comments
/// around it survive. With `overwrite`, existing source pins move to the
/// registry's rev; without it, they stay as they are.
pub(crate) fn write_pins(
    text: &str,
    set: &[ResolvedTin],
    overwrite: bool,
) -> Result<(String, Vec<Change>)> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("could not parse pixi.toml")?;
    let deps = doc
        .entry("dependencies")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .context("pixi.toml's `dependencies` is not a table")?;
    let mut changes = Vec::with_capacity(set.len());
    for b in set {
        let pin = pin_value(b);
        let change = match (deps.get_mut(&b.name), pin) {
            (None, Some(pin)) => {
                deps.insert(&b.name, toml_edit::Item::Value(pin));
                Change::Added
            }
            (None, None) => {
                deps.insert(&b.name, toml_edit::value("*"));
                Change::Added
            }
            (Some(_), None) => Change::Kept,
            (Some(_), Some(_)) if !overwrite => Change::Kept,
            (Some(item), Some(mut pin)) => {
                let old = existing_pin(item);
                let same = old.as_ref().is_some_and(|(git, rev, sub)| {
                    *git == b.url && *rev == b.commit_sha && *sub == b.subdirectory
                });
                if same {
                    Change::Unchanged
                } else {
                    // Keep a trailing comment on the line.
                    if let Some(v) = item.as_value() {
                        *pin.decor_mut() = v.decor().clone();
                    }
                    *item = toml_edit::Item::Value(pin);
                    Change::Repinned(old.map(|(_, rev, _)| rev))
                }
            }
        };
        changes.push(change);
    }
    Ok((doc.to_string(), changes))
}

/// Pin the whole install set in one edit of pixi.toml and solve once.
///
/// Adding the tins one `pixi add` at a time solves after each, and a solve
/// half-way through can see two revs of a tin's closure at once — a shim
/// subpackage that kept its name, version and build string across revs
/// becomes two identical source records and the solve fails. One edit, one
/// solve never has a half-updated closure. If the solve fails anyway,
/// pixi.toml and pixi.lock are put back as they were.
fn apply(set: &[ResolvedTin], overwrite: bool) -> Result<()> {
    ensure_pixi_build_preview()?;
    let manifest = find_pixi_toml()?;
    let lock = manifest.with_file_name("pixi.lock");
    let before = std::fs::read_to_string(&manifest)?;
    let lock_before = std::fs::read(&lock).ok();
    let (after, changes) = write_pins(&before, set, overwrite)?;

    for (b, change) in set.iter().zip(&changes) {
        let what = if b.kind == "channel" {
            format!(
                "{} (modular-community channel, latest {})",
                b.name, b.version
            )
        } else {
            let from = b
                .subdirectory
                .as_deref()
                .map(|d| format!(", from {d}/"))
                .unwrap_or_default();
            format!(
                "{} {} ({}{from})",
                b.name,
                b.version,
                crate::short_sha(&b.commit_sha)
            )
        };
        match change {
            Change::Added => println!("adding {what}"),
            Change::Repinned(Some(rev)) => {
                println!("updating {what}, was {}", crate::short_sha(rev))
            }
            Change::Repinned(None) => println!("pinning {what}"),
            Change::Unchanged => println!("{what} is up to date"),
            Change::Kept if b.kind == "channel" => {
                println!(
                    "{} is already in pixi.toml; the solver owns its version",
                    b.name
                )
            }
            Change::Kept => println!("skipping {} (already in pixi.toml)", b.name),
        }
    }
    if after == before {
        println!("nothing to change");
        return Ok(());
    }

    std::fs::write(&manifest, &after)
        .with_context(|| format!("could not write {}", manifest.display()))?;
    let manifest_arg = manifest.to_string_lossy().into_owned();
    if let Err(e) = run_pixi(&["install", "--manifest-path", &manifest_arg]) {
        std::fs::write(&manifest, &before)
            .with_context(|| format!("could not restore {}", manifest.display()))?;
        match &lock_before {
            Some(bytes) => std::fs::write(&lock, bytes)
                .with_context(|| format!("could not restore {}", lock.display()))?,
            None => {
                let _ = std::fs::remove_file(&lock);
            }
        }
        bail!(
            "{e:#}; pixi.toml and pixi.lock were restored.\n\
             A tin's own pixi.toml can pin an older rev of a dependency than the \
             registry's latest; `pixi shelf update` with no name re-pins every \
             installed tin together"
        );
    }
    println!("done");
    Ok(())
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

/// The `[dependencies]` entries that are registry tins, in pixi.toml order,
/// with the registry's summary of each.
fn installed_tins(reg: &Registry, text: &str) -> Result<Vec<(String, TinSummary)>> {
    let doc: toml_edit::DocumentMut = text.parse().context("could not parse pixi.toml")?;
    let Some(deps) = doc.get("dependencies").and_then(|d| d.as_table_like()) else {
        return Ok(Vec::new());
    };
    let mut registry: Vec<TinSummary> = reg.search("")?;
    Ok(deps
        .iter()
        .filter_map(|(name, _)| {
            let i = registry.iter().position(|t| t.name == name)?;
            Some((name.to_string(), registry.swap_remove(i)))
        })
        .collect())
}

/// `shelf list` in pixi mode: the tins in pixi.toml and the versions their
/// revs are, not the whole environment.
pub fn list(reg: &Registry) -> Result<()> {
    let text = std::fs::read_to_string(find_pixi_toml()?)?;
    let tins = installed_tins(reg, &text)?;
    if tins.is_empty() {
        println!("no tins in pixi.toml's [dependencies]");
        return Ok(());
    }
    let doc: toml_edit::DocumentMut = text.parse()?;
    for (name, summary) in tins {
        let pin = doc["dependencies"].get(&name).and_then(existing_pin);
        let Some((_, rev, _)) = pin else {
            let spec = doc["dependencies"]
                .get(&name)
                .and_then(|i| i.as_str())
                .unwrap_or("*");
            println!("{name} {spec} (modular-community channel)");
            continue;
        };
        let version = reg
            .info(&name)
            .ok()
            .and_then(|d| {
                d.versions
                    .into_iter()
                    .find(|v| v.commit_sha == rev)
                    .map(|v| v.version)
            })
            .unwrap_or_else(|| "?".into());
        let newer = match summary.latest_version {
            Some(latest) if latest != version => format!("; {latest} available"),
            _ => String::new(),
        };
        println!("{name} {version} ({}){newer}", crate::short_sha(&rev));
    }
    Ok(())
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
            let tin = tin.to_string_lossy().into_owned();
            let src = format!("shelf/{tin}/src");
            if root.join(&src).is_dir() {
                dirs.push(src);
            }
            // A repo that publishes more than one tin keeps the others in
            // subdirectories, and a submodule clones the whole repo — so a
            // sibling's source root is sitting right there, and the tin the
            // consumer actually asked for may be the one inside it.
            let mut subs: Vec<String> = std::fs::read_dir(root.join("shelf").join(&tin))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|sub| !sub.starts_with('.') && sub != "src")
                .filter(|sub| root.join(format!("shelf/{tin}/{sub}/src")).is_dir())
                .map(|sub| format!("shelf/{tin}/{sub}/src"))
                .collect();
            subs.sort();
            dirs.extend(subs);
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

    fn tin(name: &str, sha: &str) -> ResolvedTin {
        ResolvedTin {
            name: name.into(),
            url: format!("https://example.test/{name}.git"),
            version: "1.0.0".into(),
            commit_sha: sha.into(),
            kind: "source".into(),
            prev_url: None,
            url_changed_at: None,
            subdirectory: None,
        }
    }

    const MANIFEST: &str = r#"[workspace]
preview = ["pixi-build"]

[dependencies]
mojo = "==1.0.0"
# the store
store = { git = "https://example.test/store.git", rev = "old1" } # keep me
orca = { git = "https://example.test/orca.git", rev = "old2" }
python = "3.12.*"

[tasks]
run = "mojo run main.mojo"
"#;

    #[test]
    fn update_repins_the_whole_closure_in_place() {
        let mut full = tin("full", "new3");
        full.subdirectory = Some("full".into());
        let set = vec![tin("store", "new1"), full, tin("orca", "new2")];
        let (out, changes) = write_pins(MANIFEST, &set, true).unwrap();
        assert_eq!(
            changes,
            vec![
                Change::Repinned(Some("old1".into())),
                Change::Added,
                Change::Repinned(Some("old2".into())),
            ]
        );
        assert_eq!(
            out,
            r#"[workspace]
preview = ["pixi-build"]

[dependencies]
mojo = "==1.0.0"
# the store
store = { git = "https://example.test/store.git", rev = "new1" } # keep me
orca = { git = "https://example.test/orca.git", rev = "new2" }
python = "3.12.*"
full = { git = "https://example.test/full.git", rev = "new3", subdirectory = "full" }

[tasks]
run = "mojo run main.mojo"
"#
        );
    }

    #[test]
    fn add_keeps_existing_pins_and_update_skips_current_ones() {
        let set = vec![tin("store", "new1"), tin("orca", "old2")];
        let (out, changes) = write_pins(MANIFEST, &set, false).unwrap();
        assert_eq!(out, MANIFEST);
        assert_eq!(changes, vec![Change::Kept, Change::Kept]);
        let (_, changes) = write_pins(MANIFEST, &set, true).unwrap();
        assert_eq!(changes[1], Change::Unchanged);
    }

    #[test]
    fn channel_tins_are_added_unpinned_and_never_overwritten() {
        let mut python = tin("python", "");
        python.kind = "channel".into();
        let mut fresh = tin("fresh", "");
        fresh.kind = "channel".into();
        let (out, changes) = write_pins(MANIFEST, &[python, fresh], true).unwrap();
        assert_eq!(changes, vec![Change::Kept, Change::Added]);
        assert!(
            out.contains("python = \"3.12.*\"\nfresh = \"*\"\n"),
            "{out}"
        );
    }

    #[test]
    fn a_manifest_without_dependencies_gets_the_table() {
        let (out, _) = write_pins("[workspace]\n", &[tin("store", "s")], false).unwrap();
        assert!(out.contains("[dependencies]\nstore = { git = "), "{out}");
    }

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
