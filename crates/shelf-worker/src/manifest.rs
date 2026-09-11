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
}
