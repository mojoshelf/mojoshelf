//! `shelf graduate` — the graduation path as a command: generate a
//! modular-community channel recipe from this tin, preflight-checked and
//! ready to submit upstream by PR.

use crate::git;
use anyhow::{anyhow, bail, Context, Result};
use shelf_core::Manifest;
use std::path::Path;

pub fn run(maintainer: Option<&str>, license: Option<&str>, out: &str) -> Result<()> {
    // ── the tin ────────────────────────────────────────────────────────────
    let raw = std::fs::read_to_string("shelf.toml")
        .context("no shelf.toml here; run graduate from the tin's repo root")?;
    let manifest: Manifest = toml::from_str(&raw).context("could not parse shelf.toml")?;
    let description = manifest
        .description
        .clone()
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("shelf.toml needs a description — the channel requires a summary")
        })?;

    // ── git state: the recipe pins a pushed commit ─────────────────────────
    let cwd = std::env::current_dir()?;
    if !git::working_tree_clean(&cwd)? {
        bail!("working tree is dirty; commit before graduating");
    }
    if !git::head_is_pushed(&cwd)? {
        bail!("HEAD is not on any remote branch; push before graduating");
    }
    let rev = git::head_commit(&cwd)?;
    let origin = git::git(&cwd, &["remote", "get-url", "origin"])
        .context("no 'origin' remote; the recipe needs a public git URL")?;
    let https = crate::https_url(&origin);
    let git_url = if https.ends_with(".git") {
        https.clone()
    } else {
        format!("{https}.git")
    };
    let homepage = https.trim_end_matches(".git").to_string();

    // ── pixi.toml: package layout + compiler pin ───────────────────────────
    let pixi_raw = std::fs::read_to_string("pixi.toml")
        .context("no pixi.toml here; the tin must be a pixi package to graduate")?;
    let pixi: toml::Value = toml::from_str(&pixi_raw).context("could not parse pixi.toml")?;
    let package = pixi.get("package").ok_or_else(|| {
        anyhow!(
            "pixi.toml has no [package] section — make the tin pixi-consumable \
             first (see mojoshelf.org/packaging)"
        )
    })?;
    let backend = package
        .get("build")
        .and_then(|b| b.get("backend"))
        .and_then(|b| b.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    if backend != "pixi-build-mojo" {
        bail!(
            "the [package] build backend is '{backend}', expected pixi-build-mojo \
             — FFI-heavy tins need a hand-written channel recipe"
        );
    }
    let pkg_cfg = package.get("build").and_then(|b| b.get("config")).and_then(|c| c.get("pkg"));
    let pkg_path = pkg_cfg
        .and_then(|p| p.get("path"))
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("src/{}", manifest.name));
    let mojopkg_name = pkg_cfg
        .and_then(|p| p.get("name"))
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| manifest.name.clone());
    if !Path::new(&pkg_path).join("__init__.mojo").exists() {
        bail!("'{pkg_path}/__init__.mojo' not found — the package path must be a Mojo package");
    }
    let compiler_pin = package
        .get("host-dependencies")
        .and_then(|d| d.get("mojo-compiler"))
        .and_then(|v| v.as_str())
        .unwrap_or("==1.0.0");
    let mojo_version = mojo_range(compiler_pin);

    // Shim run-dependencies mean native code the channel recipe must build
    // itself — generate anyway, but say so loudly.
    let has_shim = package
        .get("run-dependencies")
        .and_then(|d| d.as_table())
        .map(|t| t.iter().any(|(k, v)| k != "mojo-compiler" && v.get("path").is_some()))
        .unwrap_or(false);

    // ── license ────────────────────────────────────────────────────────────
    let license_file = ["LICENSE", "LICENSE.md", "LICENSE.txt", "LICENSE-APACHE"]
        .iter()
        .find(|f| Path::new(f).exists())
        .map(|f| f.to_string())
        .ok_or_else(|| anyhow!("no LICENSE file found — the channel requires one"))?;
    let license_id = match license {
        Some(l) => l.to_string(),
        None => detect_license(&license_file)?,
    };

    // ── maintainer ─────────────────────────────────────────────────────────
    let owner = homepage
        .split("github.com/")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("")
        .to_string();
    let maintainer = maintainer.map(str::to_string).unwrap_or_else(|| owner.clone());

    // ── dependencies on other tins ─────────────────────────────────────────
    let mut dep_lines = String::new();
    for dep in &manifest.tins {
        dep_lines.push_str(&format!("    - {dep}\n"));
    }
    if !manifest.tins.is_empty() {
        println!(
            "note: this tin depends on {:?} — each dependency must already be \
             on the modular-community channel under that exact name, or the \
             channel build will not solve.",
            manifest.tins
        );
    }
    if has_shim {
        println!(
            "warning: this tin has a native shim subpackage; the generated \
             recipe only builds the Mojo package. Port the shim build into \
             the recipe's build script by hand before submitting."
        );
    }

    // ── recipe ─────────────────────────────────────────────────────────────
    let name = &manifest.name;
    let version = &manifest.version;
    let deps_block = if dep_lines.is_empty() {
        String::new()
    } else {
        format!("{dep_lines}")
    };
    let recipe = format!(
        r#"# Generated by `shelf graduate` from the mojoshelf tin '{name}'.
context:
  version: "{version}"
  mojo_version: "{mojo_version}"

package:
  name: "{name}"
  version: ${{{{ version }}}}

source:
  - git: {git_url}
    rev: {rev}

build:
  number: 0
  script:
    - mkdir -p ${{PREFIX}}/lib/mojo
    - mojo precompile {pkg_path} -o ${{{{ PREFIX }}}}/lib/mojo/{mojopkg_name}.mojoc

requirements:
  host:
    - mojo-compiler ${{{{ mojo_version }}}}
{deps_block}  build:
    - mojo-compiler ${{{{ mojo_version }}}}
  run:
    - mojo-compiler ${{{{ mojo_version }}}}
{deps_block}
tests:
  - script:
      - printf 'import {mojopkg_name}\ndef main():\n    pass\n' > graduate_smoke.mojo
      - mojo build graduate_smoke.mojo -o graduate_smoke
    requirements:
      run:
        - mojo-compiler ${{{{ mojo_version }}}}

about:
  homepage: {homepage}
  license: {license_id}
  license_file: {license_file}
  summary: {description}
  repository: {git_url}

extra:
  project_name: {name}
  maintainers:
    - {maintainer}
"#
    );

    let out_dir = Path::new(out).join(name);
    std::fs::create_dir_all(&out_dir)?;
    let recipe_path = out_dir.join("recipe.yaml");
    std::fs::write(&recipe_path, recipe)?;

    println!("wrote {}", recipe_path.display());
    println!();
    println!("Submit it to the modular-community channel:");
    println!("  1. Fork https://github.com/modular/modular-community");
    println!("  2. Copy {} to recipes/{name}/ in the fork", out_dir.display());
    println!("  3. Open a PR — their CI builds the recipe on every platform");
    println!();
    println!("With the gh CLI, roughly:");
    println!("  gh repo fork modular/modular-community --clone");
    println!("  cp -R {} modular-community/recipes/{name}", out_dir.display());
    println!("  cd modular-community && git checkout -b add-{name} && git add recipes/{name}");
    println!("  git commit -m 'Add {name} {version}' && git push -u origin add-{name}");
    println!("  gh pr create --title 'Add {name} {version}'");
    if maintainer == owner {
        println!();
        println!(
            "note: extra.maintainers defaulted to the repo owner '{owner}'; \
             pass --maintainer <github-user> if that should be a person."
        );
    }
    Ok(())
}

/// "==1.0.0" -> ">=1.0.0, <1.1.0"; anything else passes through unchanged.
fn mojo_range(pin: &str) -> String {
    if let Some(v) = pin.strip_prefix("==") {
        if let Ok(parsed) = semver::Version::parse(v) {
            return format!(">={v}, <{}.{}.0", parsed.major, parsed.minor + 1);
        }
    }
    pin.to_string()
}

fn detect_license(path: &str) -> Result<String> {
    let text = std::fs::read_to_string(path)?.to_lowercase();
    let head: String = text.chars().take(600).collect();
    if head.contains("mit license") {
        Ok("MIT".into())
    } else if head.contains("apache license") && head.contains("version 2.0") {
        if text.contains("llvm exception") {
            Ok("Apache-2.0 WITH LLVM-exception".into())
        } else {
            Ok("Apache-2.0".into())
        }
    } else if head.contains("bsd 3-clause") || head.contains("bsd-3-clause") {
        Ok("BSD-3-Clause".into())
    } else {
        bail!("could not detect the license in {path}; pass --license <SPDX id>")
    }
}
