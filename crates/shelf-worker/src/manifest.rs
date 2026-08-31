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
pub async fn fetch(url: &str, commit_sha: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.trim_end_matches(".git").split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    // mojoproject.toml is the same manifest under pixi's Mojo-flavoured name.
    for file in ["pixi.toml", "mojoproject.toml"] {
        let raw = format!("https://raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{file}");
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
/// `{ path = "shim" }` — so only `../` and absolute paths are reported.
pub fn escaping_path_deps(manifest: &str) -> Vec<Escaping> {
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        // Unparsable manifests are not this check's business; the build will
        // have plenty to say about them.
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&doc, "", &mut found);
    found
}

fn walk(table: &toml::Table, prefix: &str, found: &mut Vec<Escaping>) {
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
                if escapes(dep_path) {
                    found.push(Escaping {
                        table: path.clone(),
                        name: name.clone(),
                        path: dep_path.to_string(),
                    });
                }
            }
        }
        walk(child, &path, found);
    }
}

/// Whether a dependency path leaves the repository: absolute, or climbing out
/// with `..` before it descends far enough to come back.
fn escapes(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with("~/") {
        return true;
    }
    let mut depth: i32 = 0;
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
        let found = escaping_path_deps(manifest);
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
        assert!(escaping_path_deps("[tasks]\nbuild = { path = \"../x\" }").is_empty());
        assert!(escaping_path_deps("this is not toml {{{").is_empty());
    }
}
