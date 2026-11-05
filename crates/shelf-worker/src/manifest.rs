//! Reading a tin's pixi manifest well enough to reject one nobody can install.
//!
//! A dependency declared as `{ path = "../sibling" }` resolves in the author's
//! checkout, where the siblings sit next to each other, and nowhere else: a
//! consumer fetches the tin's git tree alone, so the path points outside
//! anything they have. Pixi says as much — "every source dependency of a
//! published package has to opt in as well" — and offers no equivalent of
//! Cargo's `{ path = "..", version = ".." }`, where the path is a local
//! convenience and the version is what everyone else resolves. A single pixi
//! entry is exactly one of path, git, url or version.
//!
//! So an escaping path dependency makes a tin permanently unconsumable, and
//! the publish that creates it is the moment to say so.
//!
//! The same manifest carries the other way to build a tin nobody can install
//! alongside its siblings: pinning a registered tin at a commit the registry
//! never published. A consumer's solve then sees two source records for one
//! package, so `pin_check` compares each such pin with the registry's record.

use worker::*;

/// A dependency that cannot be resolved outside the author's own checkout.
pub struct Escaping {
    pub table: String,
    pub name: String,
    pub path: String,
}

/// Fetches a tin's manifest at the published commit, or `None` when there is
/// nothing to read: a non-GitHub host, a missing file, an unreachable API.
///
/// Publishing must not fail because this lookup did — the check refuses a tin
/// on evidence, never on the absence of it.
pub async fn fetch(url: &str, commit_sha: &str, subdirectory: Option<&str>) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.trim_end_matches(".git").split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    // A tin published from a subdirectory keeps its manifest there.
    let dir = match subdirectory {
        Some(d) if !d.is_empty() => format!("{}/", d.trim_matches('/')),
        _ => String::new(),
    };
    // mojoproject.toml is the same manifest under pixi's Mojo-flavoured name.
    for file in ["pixi.toml", "mojoproject.toml"] {
        let raw =
            format!("https://raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{dir}{file}");
        let headers = Headers::new();
        headers.set("User-Agent", "mojoshelf-publish").ok()?;
        let mut init = RequestInit::new();
        init.with_headers(headers);
        let Ok(req) = Request::new_with_init(&raw, &init) else {
            continue;
        };
        let Ok(mut res) = Fetch::Request(req).send().await else {
            continue;
        };
        if res.status_code() == 200 {
            if let Ok(text) = res.text().await {
                return Some(text);
            }
        }
    }
    None
}

/// Path dependencies pointing outside the repository, found anywhere in the
/// manifest.
///
/// A path *inside* the repo is fine and common — a tin's own FFI shim lives at
/// `{ path = "shim" }` — so only paths that leave the repository are reported.
///
/// `depth` is how far the manifest itself sits below the repository root, so a
/// tin published from a subdirectory may climb that far and no further: the
/// `full/` package of a two-tin repo depends on its own root as
/// `{ path = ".." }`, which is inside the repo and resolves for everyone.
pub fn escaping_path_deps(manifest: &str, depth: usize) -> Vec<Escaping> {
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        // Unparsable manifests are not this check's business; the build will
        // have plenty to say about them.
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&doc, "", depth, &mut found);
    found
}

/// How far below the repository root a subdirectory sits: `None` and `""` are
/// the root, `"full"` is one, `"a/b"` is two.
pub fn subdirectory_depth(subdirectory: Option<&str>) -> usize {
    subdirectory
        .unwrap_or("")
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .count()
}

fn walk(table: &toml::Table, prefix: &str, depth: usize, found: &mut Vec<Escaping>) {
    for (key, value) in table {
        let Some(child) = value.as_table() else {
            continue;
        };
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        // Any table whose name ends in "dependencies" holds dependency specs:
        // [dependencies], [package.host-dependencies], [feature.x.dependencies].
        if key.ends_with("dependencies") {
            for (name, spec) in child {
                let Some(spec) = spec.as_table() else {
                    continue;
                };
                let Some(dep_path) = spec.get("path").and_then(|p| p.as_str()) else {
                    continue;
                };
                if escapes(dep_path, depth) {
                    found.push(Escaping {
                        table: path.clone(),
                        name: name.clone(),
                        path: dep_path.to_string(),
                    });
                }
            }
        }
        walk(child, &path, depth, found);
    }
}

/// Whether a dependency path leaves the repository: absolute, or climbing out
/// with `..` past the repository root.
///
/// `from_root` is the manifest's own depth, which is how much headroom `..`
/// has before it escapes. At the root (0) a single `..` already leaves; from
/// `full/` (1) it lands on the root and is fine.
fn escapes(path: &str, from_root: usize) -> bool {
    if path.starts_with('/') || path.starts_with("~/") {
        return true;
    }
    let mut depth = from_root as i32;
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// A registered source tin, as far as the pin check needs to know it.
pub struct Published {
    pub name: String,
    pub url: String,
    pub subdirectory: Option<String>,
    /// (version, commit sha) for every published version.
    pub versions: Vec<(String, String)>,
}

/// What the pin check found: pins to refuse the publish over, and pins to
/// mention to the author.
#[derive(Debug, Default, PartialEq)]
pub struct PinReport {
    pub refusals: Vec<String>,
    pub warnings: Vec<String>,
}

/// Checks every git dependency that pins a registered tin by `rev`.
///
/// A rev the registry never published for that tin is a refusal; a published
/// rev that isn't the tin's latest version's commit is a warning. Only
/// commits can be judged: a `rev` that isn't hex (pixi also accepts a branch
/// or tag there) is left alone, as are `branch`/`tag` pins and git
/// dependencies on anything that isn't a registered tin.
///
/// A rev may be abbreviated. Pixi hands `rev` to git, which resolves any
/// unambiguous prefix (the tool's own minimum is 4 characters, the
/// abbreviation `git log` prints is 7), so a prefix of at least 7 hex
/// characters counts when exactly one published commit starts with it.
///
/// `own_url` is the repository being published. A dependency on it (a git
/// dependency on the same repository, which is what a tin in a subdirectory
/// has on its sibling) belongs to the path rules above, not to this check.
pub fn pin_check(manifest: &str, own_url: &str, published: &[Published]) -> PinReport {
    let mut report = PinReport::default();
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        return report;
    };
    let own = normalize_url(own_url);
    let mut pins = Vec::new();
    collect_pins(&doc, "", &mut pins);
    for pin in pins {
        let url = normalize_url(&pin.git);
        if url == own {
            continue;
        }
        let sub = pin.subdirectory.as_deref().map(|d| d.trim_matches('/'));
        let Some(tin) = published.iter().find(|t| {
            normalize_url(&t.url) == url
                && t.subdirectory
                    .as_deref()
                    .map(|d| d.trim_matches('/'))
                    .filter(|d| !d.is_empty())
                    == sub.filter(|d| !d.is_empty())
        }) else {
            continue;
        };
        if pin.rev.len() < 7 || !pin.rev.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let rev = pin.rev.to_ascii_lowercase();
        let short = &pin.rev[..7];
        let latest = shelf_core::latest_version(tin.versions.iter().map(|(v, _)| v.as_str()))
            .and_then(|l| tin.versions.iter().find(|(v, _)| v == l));
        // Distinct commits the rev could mean; two versions may share one.
        let mut matches: Vec<&(String, String)> = Vec::new();
        for v in &tin.versions {
            if v.1.to_ascii_lowercase().starts_with(&rev)
                && !matches.iter().any(|m| m.1.eq_ignore_ascii_case(&v.1))
            {
                matches.push(v);
            }
        }
        let name = &tin.name;
        let table = &pin.table;
        let tail = format!("Consumers will see two source records for {name}.");
        match (matches.as_slice(), latest) {
            ([hit], Some(latest)) => {
                if !hit.1.eq_ignore_ascii_case(&latest.1) {
                    report.warnings.push(format!(
                        "{name} is pinned at {short} in [{table}], which is {name} {}, but \
                         its latest is {} at {}. {tail}",
                        hit.0,
                        latest.0,
                        &latest.1[..7],
                    ));
                }
            }
            (_, Some(latest)) => report.refusals.push(format!(
                "{name} is pinned at {short} in [{table}], which isn't a published \
                 version; its latest is {} at {}. {tail}",
                latest.0,
                &latest.1[..7],
            )),
            (_, None) => report.refusals.push(format!(
                "{name} is pinned at {short} in [{table}], which isn't a published \
                 version; {name} has no published versions yet. {tail}"
            )),
        }
    }
    report
}

struct Pin {
    table: String,
    git: String,
    rev: String,
    subdirectory: Option<String>,
}

/// Every `{ git = .., rev = .. }` entry in any table named `*dependencies`,
/// found the way `walk` finds paths.
fn collect_pins(table: &toml::Table, prefix: &str, found: &mut Vec<Pin>) {
    for (key, value) in table {
        let Some(child) = value.as_table() else {
            continue;
        };
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if key.ends_with("dependencies") {
            for spec in child.values().filter_map(|s| s.as_table()) {
                let get = |k: &str| spec.get(k).and_then(|v| v.as_str());
                if let (Some(git), Some(rev)) = (get("git"), get("rev")) {
                    found.push(Pin {
                        table: path.clone(),
                        git: git.to_string(),
                        rev: rev.to_string(),
                        subdirectory: get("subdirectory").map(str::to_string),
                    });
                }
            }
        }
        collect_pins(child, &path, found);
    }
}

/// Shared with the CLI's `shelf release`, which matches pins to repositories
/// the same way.
pub use shelf_core::normalize_url;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_sibling_paths_but_not_inner_ones() {
        let manifest = r#"
[package.host-dependencies]
mojo-compiler = "==1.0.0"
avro-mojo = { path = "../avro.mojo" }
zlib-shim = { path = "shim" }

[dependencies]
nested = { path = "vendor/../vendor/thing" }
absolute = { path = "/opt/thing" }
"#;
        let found = escaping_path_deps(manifest, 0);
        // Tables come out of toml::Table in name order, so compare as a set.
        let mut names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["absolute", "avro-mojo"]);
        let avro = found.iter().find(|e| e.name == "avro-mojo").unwrap();
        assert_eq!(avro.table, "package.host-dependencies");
        assert_eq!(avro.path, "../avro.mojo");
    }

    #[test]
    fn ignores_non_dependency_tables_and_bad_toml() {
        assert!(escaping_path_deps("[tasks]\nbuild = { path = \"../x\" }", 0).is_empty());
        assert!(escaping_path_deps("this is not toml {{{", 0).is_empty());
    }

    #[test]
    fn a_subdirectory_tin_may_depend_on_its_own_root() {
        // What the `full/` half of a two-tin repo declares: one level up is
        // the repository root, which every consumer fetches.
        let manifest = r#"
[package.host-dependencies]
parquet-mojo = { path = ".." }
"#;
        assert!(escaping_path_deps(manifest, 1).is_empty());
        // The same line from a root manifest really does leave the repo.
        assert_eq!(escaping_path_deps(manifest, 0).len(), 1);
    }

    #[test]
    fn a_subdirectory_tin_cannot_climb_past_the_root() {
        let manifest = r#"
[package.host-dependencies]
sibling = { path = "../../other.mojo" }
"#;
        assert_eq!(escaping_path_deps(manifest, 1).len(), 1);
        // Two levels down, the same path lands inside the repo.
        assert!(escaping_path_deps(manifest, 2).is_empty());
    }

    #[test]
    fn subdirectory_depth_counts_components() {
        assert_eq!(subdirectory_depth(None), 0);
        assert_eq!(subdirectory_depth(Some("")), 0);
        assert_eq!(subdirectory_depth(Some("full")), 1);
        assert_eq!(subdirectory_depth(Some("/full/")), 1);
        assert_eq!(subdirectory_depth(Some("a/b")), 2);
    }

    const SHA_OLD: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
    const SHA_NEW: &str = "6639b18aa0b1c2d3e4f5061728394a5b6c7d8e9f";

    fn deflate() -> Published {
        Published {
            name: "deflate-mojo".into(),
            url: "https://github.com/magmalake/deflate.mojo".into(),
            subdirectory: None,
            versions: vec![
                ("0.1.0".into(), SHA_OLD.into()),
                ("0.2.0".into(), SHA_NEW.into()),
            ],
        }
    }

    fn pin(table: &str, git: &str, rev: &str) -> String {
        format!("[{table}]\ndeflate-mojo = {{ git = \"{git}\", rev = \"{rev}\" }}\n")
    }

    const REPO: &str = "https://github.com/magmalake/deflate.mojo";
    const ME: &str = "https://github.com/magmalake/avro.mojo";

    #[test]
    fn an_unpublished_rev_is_refused() {
        let m = pin("package.host-dependencies", REPO, "deadbeefcafe");
        let r = pin_check(&m, ME, &[deflate()]);
        assert!(r.warnings.is_empty());
        assert_eq!(
            r.refusals,
            [
                "deflate-mojo is pinned at deadbee in [package.host-dependencies], which \
              isn't a published version; its latest is 0.2.0 at 6639b18. Consumers will \
              see two source records for deflate-mojo."
            ]
        );
    }

    #[test]
    fn a_published_rev_that_is_not_the_latest_warns() {
        let m = pin("package.host-dependencies", REPO, SHA_OLD);
        let r = pin_check(&m, ME, &[deflate()]);
        assert!(r.refusals.is_empty());
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("deflate-mojo 0.1.0"));
        assert!(r.warnings[0].contains("latest is 0.2.0 at 6639b18"));
    }

    #[test]
    fn the_latest_rev_is_accepted_silently() {
        let m = pin("package.host-dependencies", REPO, SHA_NEW);
        assert_eq!(pin_check(&m, ME, &[deflate()]), PinReport::default());
    }

    #[test]
    fn latest_is_by_semver_not_by_order() {
        let mut t = deflate();
        t.versions.reverse();
        t.versions
            .push(("0.10.0".into(), "abcdef0123456789".repeat(2) + "abcdef01"));
        let m = pin("dependencies", REPO, SHA_NEW);
        let r = pin_check(&m, ME, &[t]);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("latest is 0.10.0 at abcdef0"));
    }

    #[test]
    fn short_revs_match_a_unique_prefix_of_seven_or_more() {
        let ok = pin("dependencies", REPO, "6639b18");
        assert_eq!(pin_check(&ok, ME, &[deflate()]), PinReport::default());
        let upper = pin("dependencies", REPO, "6639B18AA");
        assert_eq!(pin_check(&upper, ME, &[deflate()]), PinReport::default());
        // Too short to trust: not judged at all.
        let short = pin("dependencies", REPO, "6639b1");
        assert!(pin_check(&short, ME, &[deflate()]).refusals.is_empty());
        // Ambiguous: two published commits share the prefix.
        let mut t = deflate();
        t.versions
            .push(("0.3.0".into(), format!("6639b18{}", "0".repeat(33))));
        let amb = pin("dependencies", REPO, "6639b18");
        assert_eq!(pin_check(&amb, ME, &[t]).refusals.len(), 1);
    }

    #[test]
    fn a_pin_by_a_tin_with_no_versions_is_refused() {
        let mut t = deflate();
        t.versions.clear();
        let r = pin_check(&pin("dependencies", REPO, SHA_NEW), ME, &[t]);
        assert_eq!(r.refusals.len(), 1);
        assert!(r.refusals[0].contains("no published versions yet"));
    }

    #[test]
    fn urls_compare_across_spellings() {
        for url in [
            "https://github.com/magmalake/deflate.mojo.git",
            "https://GitHub.com/magmalake/deflate.mojo/",
            "git@github.com:magmalake/deflate.mojo.git",
            "ssh://git@github.com/magmalake/deflate.mojo",
            "ssh://git@github.com:22/magmalake/deflate.mojo.git",
            "git+https://github.com/magmalake/deflate.mojo",
            "https://github.com/MagmaLake/Deflate.mojo",
        ] {
            let r = pin_check(&pin("dependencies", url, "deadbeefcafe"), ME, &[deflate()]);
            assert_eq!(r.refusals.len(), 1, "{url}");
        }
        assert_eq!(normalize_url("git@github.com:a/b.git"), "github.com/a/b");
    }

    #[test]
    fn every_dependency_table_is_checked() {
        for table in [
            "dependencies",
            "package.host-dependencies",
            "package.run-dependencies",
            "package.build-dependencies",
            "feature.test.dependencies",
            "target.linux-64.dependencies",
            "feature.test.target.osx-arm64.dependencies",
            "workspace.dependencies",
        ] {
            let m = pin(table, REPO, "deadbeefcafe");
            let r = pin_check(&m, ME, &[deflate()]);
            assert_eq!(r.refusals.len(), 1, "{table}");
            assert!(r.refusals[0].contains(&format!("in [{table}]")), "{table}");
        }
    }

    #[test]
    fn git_deps_on_other_things_are_ignored() {
        // Not a registered tin; a branch pin; a tag pin; a non-commit rev;
        // a path dependency; a plain version.
        let m = r#"
[dependencies]
other = { git = "https://github.com/someone/else.mojo", rev = "deadbeefcafe" }
a = { git = "https://github.com/magmalake/deflate.mojo", branch = "main" }
b = { git = "https://github.com/magmalake/deflate.mojo", tag = "v1" }
c = { git = "https://github.com/magmalake/deflate.mojo", rev = "main" }
d = { path = "shim" }
e = "==1.0"
"#;
        assert_eq!(pin_check(m, ME, &[deflate()]), PinReport::default());
        assert_eq!(
            pin_check("not toml {{{", ME, &[deflate()]),
            PinReport::default()
        );
    }

    #[test]
    fn a_tins_own_repository_is_left_to_the_path_rules() {
        // A subdirectory tin depending on its repo's root by git, whatever
        // the rev: not this check's business.
        let m = pin("package.host-dependencies", REPO, "deadbeefcafe");
        assert_eq!(pin_check(&m, REPO, &[deflate()]), PinReport::default());
        assert_eq!(
            pin_check(
                &m,
                "git@github.com:magmalake/deflate.mojo.git",
                &[deflate()]
            ),
            PinReport::default()
        );
        // A path dependency on one's own root is no pin at all.
        let p = "[package.host-dependencies]\ndeflate-mojo = { path = \"..\" }\n";
        assert_eq!(pin_check(p, REPO, &[deflate()]), PinReport::default());
    }

    #[test]
    fn a_subdirectory_selects_the_tin_in_a_shared_repository() {
        let root = deflate();
        let full = Published {
            name: "deflate-full-mojo".into(),
            subdirectory: Some("full".into()),
            versions: vec![("0.1.0".into(), SHA_OLD.into())],
            ..deflate()
        };
        let m = format!(
            "[dependencies]\nx = {{ git = \"{REPO}\", rev = \"deadbeefcafe\", \
             subdirectory = \"full\" }}\n"
        );
        let r = pin_check(&m, ME, &[root, full]);
        assert_eq!(r.refusals.len(), 1);
        assert!(r.refusals[0].starts_with("deflate-full-mojo is pinned"));
    }

    /// Runs the check over manifests fetched by `scripts/pin-dryrun.sh`, to
    /// see what the registry would say about versions already published.
    /// `PIN_DRYRUN_DIR` holds `tins/<name>.json` (the registry's detail
    /// answer) and `manifests/<name>.toml` (the manifest at the latest
    /// version's commit).
    #[test]
    #[ignore = "needs the data scripts/pin-dryrun.sh fetches"]
    fn dry_run_over_published_manifests() {
        let dir = std::path::PathBuf::from(std::env::var("PIN_DRYRUN_DIR").unwrap());
        let mut published = Vec::new();
        for entry in std::fs::read_dir(dir.join("tins")).unwrap() {
            let d: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap())
                    .unwrap();
            published.push(Published {
                name: d["name"].as_str().unwrap().into(),
                url: d["url"].as_str().unwrap().into(),
                subdirectory: d["subdirectory"].as_str().map(str::to_string),
                versions: d["versions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| {
                        (
                            v["version"].as_str().unwrap().to_string(),
                            v["commit_sha"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect(),
            });
        }
        let mut refused = 0;
        for tin in &published {
            let Ok(text) =
                std::fs::read_to_string(dir.join(format!("manifests/{}.toml", tin.name)))
            else {
                continue;
            };
            let report = pin_check(&text, &tin.url, &published);
            println!(
                "{}: {} refusals, {} warnings",
                tin.name,
                report.refusals.len(),
                report.warnings.len()
            );
            for m in report.refusals.iter().chain(&report.warnings) {
                println!("  {m}");
            }
            refused += report.refusals.len();
        }
        assert_eq!(refused, 0);
    }
}
