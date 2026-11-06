//! `shelf release plan` and `apply` end to end, against local bare git
//! repositories (file:// remotes, so no PRs) and a small in-process stand-in
//! for the registry API. Preflight needs pixi and real tins, so these runs
//! pass --skip-preflight.
//!
//! The cascade: base1 and base2 are released; mid pins base1 and publishes a
//! second tin from full/; top pins mid, mid-full and base2, and its CI checks
//! mid out by commit; docs depends on base1 without a rev pin and is left out.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

// ── the registry stand-in ───────────────────────────────────────────────────

#[derive(Clone)]
struct StubTin {
    name: String,
    url: String,
    subdirectory: Option<String>,
    /// (version, commit), newest first.
    versions: Vec<(String, String)>,
    dependents: Vec<String>,
}

type Tins = Arc<Mutex<Vec<StubTin>>>;
type Publishes = Arc<Mutex<Vec<Value>>>;

fn detail(t: &StubTin) -> Value {
    json!({
        "name": t.name, "url": t.url, "description": null, "author": null,
        "subdirectory": t.subdirectory, "dependents": t.dependents,
        "versions": t.versions.iter().map(|(v, sha)| json!({
            "version": v, "commit_sha": sha, "published_at": "2026-11-05T00:00:00Z", "dependencies": []
        })).collect::<Vec<_>>(),
    })
}

fn serve(tins: Tins, publishes: Publishes) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, target) = (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            );
            let mut len = 0;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let path = target.split('?').next().unwrap_or("").to_string();
            let (status, reply) = {
                let mut tins = tins.lock().unwrap();
                match (method.as_str(), path.trim_start_matches("/api/").split('/').collect::<Vec<_>>().as_slice()) {
                    ("GET", ["tins"]) => (
                        200,
                        Value::Array(
                            tins.iter()
                                .map(|t| json!({"name": t.name, "url": t.url, "description": null, "author": null,
                                    "latest_version": t.versions.first().map(|v| &v.0), "subdirectory": t.subdirectory}))
                                .collect(),
                        ),
                    ),
                    ("GET", ["tins", name]) => match tins.iter().find(|t| t.name == *name) {
                        Some(t) => (200, detail(t)),
                        None => (404, json!({"error": "no such tin"})),
                    },
                    ("GET", ["tins", _, "resolve"]) => (200, json!([])),
                    ("POST", ["publish"]) => {
                        let req: Value = serde_json::from_slice(&body).unwrap();
                        let t = tins.iter_mut().find(|t| t.name == req["name"]).unwrap();
                        t.versions.insert(
                            0,
                            (req["version"].as_str().unwrap().into(), req["commit_sha"].as_str().unwrap().into()),
                        );
                        publishes.lock().unwrap().push(req);
                        (200, json!({"ok": true, "warnings": []}))
                    }
                    _ => (404, json!({"error": "not found"})),
                }
            };
            let text = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    format!("http://{addr}")
}

// ── git fixtures ────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .envs(git_env())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GIT_AUTHOR_NAME", "Test"),
        ("GIT_AUTHOR_EMAIL", "test@example.test"),
        ("GIT_COMMITTER_NAME", "Test"),
        ("GIT_COMMITTER_EMAIL", "test@example.test"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ]
}

struct Fixture {
    root: PathBuf,
    ws: PathBuf,
    registry: String,
    tins: Tins,
    publishes: Publishes,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn url(root: &Path, repo: &str) -> String {
    format!("file://{}/remotes/{repo}.git", root.display())
}

fn package(name: &str, version: &str, deps: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[package.build]\nbackend = {{ name = \"pixi-build-mojo\", version = \"0.*\" }}\n{deps}"
    )
}

impl Fixture {
    /// Make a repository with `files`, push it to its bare remote, clone it
    /// into the workspace, and return the commit.
    fn repo(&self, name: &str, files: &[(&str, String)]) -> String {
        let bare = self.root.join("remotes").join(format!("{name}.git"));
        git(
            &self.root,
            &[
                "init",
                "--quiet",
                "--bare",
                "-b",
                "main",
                bare.to_str().unwrap(),
            ],
        );
        let seed = self.root.join("seed").join(name);
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "--quiet", "-b", "main"]);
        for (f, text) in files {
            let p = seed.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "--quiet", "-m", "first"]);
        git(&seed, &["push", "--quiet", &url(&self.root, name), "main"]);
        git(
            &self.ws,
            &["clone", "--quiet", &url(&self.root, name), name],
        );
        git(&seed, &["rev-parse", "HEAD"])
    }

    fn shelf(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_shelf"))
            .args(args)
            .current_dir(&self.ws)
            .env("SHELF_REGISTRY", &self.registry)
            .env("SHELF_TOKEN", "test")
            .envs(git_env())
            // Never reach a real GitHub account, whatever the URLs say.
            .env("GH_CONFIG_DIR", self.root.join("no-gh"))
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, text) = self.shelf(args);
        assert!(ok, "shelf {args:?} failed:\n{text}");
        text
    }

    /// A file on a remote branch.
    fn show(&self, repo: &str, branch: &str, file: &str) -> String {
        let dir = self.ws.join(repo);
        git(&dir, &["fetch", "--quiet", "origin"]);
        git(&dir, &["show", &format!("origin/{branch}:{file}")])
    }

    fn remote_head(&self, repo: &str, branch: &str) -> String {
        let out = git(
            &self.root,
            &[
                "ls-remote",
                &url(&self.root, repo),
                &format!("refs/heads/{branch}"),
            ],
        );
        out.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Push a commit to `branch` from a separate clone, as a fix pushed to a
    /// release PR would be.
    fn push_fix(&self, repo: &str, branch: &str) -> String {
        let dir = self.root.join("fix").join(repo);
        git(
            &self.root,
            &[
                "clone",
                "--quiet",
                "-b",
                branch,
                &url(&self.root, repo),
                dir.to_str().unwrap(),
            ],
        );
        std::fs::write(dir.join("FIX.md"), "a fix\n").unwrap();
        git(&dir, &["add", "FIX.md"]);
        git(&dir, &["commit", "--quiet", "-m", "Fix the release"]);
        git(&dir, &["push", "--quiet", "origin", branch]);
        git(&dir, &["rev-parse", "HEAD"])
    }

    /// Merge a release branch into main with a merge commit, as its PR would.
    fn merge(&self, repo: &str, branch: &str) {
        let dir = self.root.join("merge").join(repo);
        git(
            &self.root,
            &[
                "clone",
                "--quiet",
                &url(&self.root, repo),
                dir.to_str().unwrap(),
            ],
        );
        git(
            &dir,
            &[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                "Merge release",
                &format!("origin/{branch}"),
            ],
        );
        git(&dir, &["push", "--quiet", "origin", "main"]);
    }
}

/// A commit that's nobody's pin or published version.
const UNRELATED: &str = "4cf043712aa386c1ed2403771969fb3e4c79ea2d";

fn empty() -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("shelf-release-it-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(root.join("remotes")).unwrap();
    let tins: Tins = Arc::default();
    let publishes: Publishes = Arc::default();
    let registry = serve(tins.clone(), publishes.clone());
    Fixture {
        root,
        ws,
        registry,
        tins,
        publishes,
    }
}

fn fixture() -> Fixture {
    let fx = empty();
    let r = |name: &str| url(&fx.root, name);
    let pin = |dep: &str, repo: &str, sha: &str, extra: &str| {
        format!(
            "{dep} = {{ git = \"{}\", rev = \"{sha}\"{extra} }}\n",
            r(repo)
        )
    };

    let base1 = fx.repo(
        "base1",
        &[
            ("shelf.toml", "name = \"base1-mojo\"\nversion = \"0.1.0\"\n".into()),
            ("pixi.toml", format!("[workspace]\nname = \"base1\"\nversion = \"0.1.0\"\npreview = [\"pixi-build\"]\n\n{}", package("base1-mojo", "0.1.0", ""))),
            (
                "CHANGELOG.md",
                "# Changelog\n\n## [Unreleased]\n\n### Breaking\n\n- `open` takes a path.\n\n## [0.1.0] - 2026-11-01\n\n- First.\n".into(),
            ),
        ],
    );
    let base2 = fx.repo(
        "base2",
        &[
            (
                "shelf.toml",
                "name = \"base2-mojo\"\nversion = \"1.0.0\"\n".into(),
            ),
            ("pixi.toml", package("base2-mojo", "1.0.0", "")),
        ],
    );
    let mid = fx.repo(
        "mid",
        &[
            ("shelf.toml", "name = \"mid-mojo\"\nversion = \"0.3.0\"\ntins = [\"base1-mojo\"]\n".into()),
            (
                "pixi.toml",
                package(
                    "mid-mojo",
                    "0.3.0",
                    &format!(
                        "\n[package.host-dependencies]\n# the one sibling\n{}\n[feature.bench.dependencies]\n{}",
                        pin("base1-mojo", "base1", &base1, ""),
                        pin("base1-mojo", "base1", &base1, "")
                    ),
                ),
            ),
            ("full/shelf.toml", "name = \"mid-full-mojo\"\nversion = \"0.1.0\"\n".into()),
            (
                "full/pixi.toml",
                package("mid-full-mojo", "0.1.0", "\n[package.host-dependencies]\nmid-mojo = { path = \"..\" }\n"),
            ),
        ],
    );
    let top = fx.repo(
        "top",
        &[
            ("shelf.toml", "name = \"top-mojo\"\nversion = \"2.1.0\"\n".into()),
            (
                "pixi.toml",
                package(
                    "top-mojo",
                    "2.1.0",
                    &format!(
                        "\n[package.run-dependencies]\n{}{}{}\n[target.linux-64.dependencies]\n{}",
                        pin("mid-mojo", "mid", &mid, ""),
                        pin("mid-full-mojo", "mid", &mid, ", subdirectory = \"full\""),
                        pin("base2-mojo", "base2", &base2, ""),
                        pin("base2-mojo", "base2", &base2, ""),
                    ),
                ),
            ),
            (
                ".github/workflows/ci.yml",
                format!(
                    "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          path: mid\n          ref: {mid}\n      - run: |\n          git clone --quiet {} base1\n          git -C base1 checkout --quiet {UNRELATED}\n",
                    r("base1")
                ),
            ),
        ],
    );
    let docs = fx.repo(
        "docs",
        &[
            (
                "shelf.toml",
                "name = \"docs-mojo\"\nversion = \"0.1.0\"\n".into(),
            ),
            (
                "pixi.toml",
                package(
                    "docs-mojo",
                    "0.1.0",
                    "\n[dependencies]\nbase1-mojo = \"*\"\n",
                ),
            ),
        ],
    );
    let t =
        |name: &str, repo: &str, sub: Option<&str>, v: &str, sha: &str, deps: &[&str]| StubTin {
            name: name.into(),
            url: r(repo),
            subdirectory: sub.map(str::to_string),
            versions: vec![(v.into(), sha.into())],
            dependents: deps.iter().map(|d| d.to_string()).collect(),
        };
    *fx.tins.lock().unwrap() = vec![
        t(
            "base1-mojo",
            "base1",
            None,
            "0.1.0",
            &base1,
            &["mid-mojo", "docs-mojo"],
        ),
        t("base2-mojo", "base2", None, "1.0.0", &base2, &["top-mojo"]),
        t(
            "mid-mojo",
            "mid",
            None,
            "0.3.0",
            &mid,
            &["top-mojo", "mid-full-mojo"],
        ),
        t(
            "mid-full-mojo",
            "mid",
            Some("full"),
            "0.1.0",
            &mid,
            &["top-mojo"],
        ),
        t("top-mojo", "top", None, "2.1.0", &top, &[]),
        t("docs-mojo", "docs", None, "0.1.0", &docs, &[]),
    ];
    fx
}

fn pins_of(text: &str, dep: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with(&format!("{dep} = ")))
        .map(|l| {
            l.split("rev = \"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn plan_finds_the_cascade_versions_and_pins() {
    let fx = fixture();
    let out = fx.ok(&[
        "release",
        "plan",
        "base1-mojo",
        "base2-mojo",
        "--bump",
        "base2-mojo=major",
        "--json",
    ]);
    let plan: Value = serde_json::from_str(&out).unwrap();
    let releases = plan["releases"].as_array().unwrap();
    let repos: Vec<&str> = releases
        .iter()
        .map(|r| r["repo"].as_str().unwrap())
        .collect();
    assert_eq!(
        repos,
        vec!["base1", "base2", "mid", "top"],
        "docs pins nothing by rev"
    );
    let levels: Vec<u64> = releases
        .iter()
        .map(|r| r["level"].as_u64().unwrap())
        .collect();
    assert_eq!(levels, vec![0, 0, 1, 2]);
    let next = |i: usize, j: usize| releases[i]["tins"][j]["next"].as_str().unwrap().to_string();
    assert_eq!(
        next(0, 0),
        "0.2.0",
        "### Breaking below 1.0 is a minor bump"
    );
    assert_eq!(next(1, 0), "2.0.0", "--bump major");
    assert_eq!(
        (next(2, 0), next(2, 1)),
        ("0.3.1".into(), "0.1.1".into()),
        "mid and full/ together"
    );
    assert_eq!(releases[2]["branch"], "release/mid-mojo-0.3.1");
    assert_eq!(releases[2]["pins"].as_array().unwrap().len(), 2);
    assert_eq!(releases[3]["pins"].as_array().unwrap().len(), 4);
    assert_eq!(releases[3]["parents"], json!(["base2", "mid"]));
    assert_eq!(releases[3]["workflow_pins"].as_array().unwrap().len(), 1);
    // top's CI clones base1, its parent's parent, at an unrelated commit:
    // found by the repository it names, and moved.
    let refs = releases[3]["refs"].as_array().unwrap();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(
        (
            &refs[0]["repo"],
            &refs[0]["kind"],
            &refs[0]["rev"],
            &refs[0]["line"]
        ),
        (
            &json!("base1"),
            &json!("clone"),
            &json!(UNRELATED),
            &json!(10)
        )
    );
    assert!(releases.iter().all(|r| r["mismatches"] == json!([])));

    let human = fx.ok(&["release", "plan", "base1-mojo"]);
    assert!(
        human.contains("release plan, parents first (3 repositories, 4 tins)"),
        "{human}"
    );
    assert!(
        human.contains("full/: released with mid-mojo at the same commit"),
        "{human}"
    );
    assert!(human.contains(".github/workflows/ci.yml:7"), "{human}");
    assert!(
        human.contains(".github/workflows/ci.yml:10  clone of base1  4cf043712aa3 -> release/base1-mojo-0.2.0 head"),
        "{human}"
    );

    // --without mid: top no longer pins anything in the cascade.
    let out = fx.ok(&[
        "release",
        "plan",
        "base1-mojo",
        "--without",
        "mid-mojo",
        "--json",
    ]);
    let plan: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(plan["releases"].as_array().unwrap().len(), 1);

    // A dependent that isn't checked out stops the plan with the clone line.
    std::fs::rename(fx.ws.join("mid"), fx.root.join("mid-elsewhere")).unwrap();
    let (ok, err) = fx.shelf(&["release", "plan", "base1-mojo"]);
    assert!(
        !ok && err.contains("git clone") && err.contains("mid.git"),
        "{err}"
    );
    let elsewhere = format!("mid-mojo={}", fx.root.join("mid-elsewhere").display());
    fx.ok(&["release", "plan", "base1-mojo", "--repo", &elsewhere]);
}

#[test]
fn changed_skips_release_commits() {
    let fx = fixture();
    let (ok, err) = fx.shelf(&["release", "plan", "--changed"]);
    assert!(
        !ok && err.contains("no tin checked out here has commits"),
        "{err}"
    );
    let dir = fx.ws.join("base2");
    std::fs::write(dir.join("NOTES.md"), "x\n").unwrap();
    git(&dir, &["add", "NOTES.md"]);
    git(
        &dir,
        &["commit", "--quiet", "-m", "Release notes are not a release"],
    );
    git(&dir, &["push", "--quiet", "origin", "main"]);
    let (ok, err) = fx.shelf(&["release", "plan", "--changed"]);
    assert!(
        !ok,
        "a commit whose subject starts with Release doesn't count: {err}"
    );
    std::fs::write(dir.join("NOTES.md"), "y\n").unwrap();
    git(&dir, &["commit", "--quiet", "-am", "Faster open"]);
    git(&dir, &["push", "--quiet", "origin", "main"]);
    let out = fx.ok(&["release", "plan", "--changed", "--json"]);
    let plan: Value = serde_json::from_str(&out).unwrap();
    let repos: Vec<&str> = plan["releases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, vec!["base2", "top"]);
    assert_eq!(
        plan["releases"][0]["tins"][0]["reason"],
        "1 commit since 1.0.0"
    );
}

#[test]
fn apply_cuts_resumes_after_a_fix_and_publishes_parents_first() {
    let fx = fixture();
    let mid_published = fx.tins.lock().unwrap()[2].versions[0].1.clone();
    let out = fx.ok(&[
        "release",
        "apply",
        "base1-mojo",
        "base2-mojo",
        "--skip-preflight",
    ]);
    assert!(
        out.contains("no PRs opened (not GitHub repositories)"),
        "{out}"
    );
    assert!(fx.ws.join(".shelf-release.json").exists());

    let b1 = "release/base1-mojo-0.2.0";
    let b2 = "release/base2-mojo-1.0.1";
    let bm = "release/mid-mojo-0.3.1";
    let bt = "release/top-mojo-2.1.1";
    let (h1, h2, hm, ht) = (
        fx.remote_head("base1", b1),
        fx.remote_head("base2", b2),
        fx.remote_head("mid", bm),
        fx.remote_head("top", bt),
    );
    assert!([&h1, &h2, &hm, &ht].iter().all(|h| h.len() == 40));

    // Versions, changelog, pins and the CI pin, each to the parent's head.
    let changelog = fx.show("base1", b1, "CHANGELOG.md");
    assert!(
        changelog.contains("## [Unreleased]\n\n## [0.2.0] - "),
        "{changelog}"
    );
    assert!(fx
        .show("base1", b1, "pixi.toml")
        .contains("version = \"0.2.0\"\npreview"));
    assert_eq!(
        fx.show("mid", bm, "full/shelf.toml"),
        "name = \"mid-full-mojo\"\nversion = \"0.1.1\""
    );
    let mid_manifest = fx.show("mid", bm, "pixi.toml");
    assert!(
        mid_manifest.contains("# the one sibling\nbase1-mojo"),
        "comments survive"
    );
    assert_eq!(
        pins_of(&mid_manifest, "base1-mojo"),
        vec![h1.clone(), h1.clone()]
    );
    let top_manifest = fx.show("top", bt, "pixi.toml");
    assert_eq!(pins_of(&top_manifest, "mid-mojo"), vec![hm.clone()]);
    assert_eq!(pins_of(&top_manifest, "mid-full-mojo"), vec![hm.clone()]);
    assert_eq!(
        pins_of(&top_manifest, "base2-mojo"),
        vec![h2.clone(), h2.clone()]
    );
    let ci = fx.show("top", bt, ".github/workflows/ci.yml");
    assert!(
        ci.contains(&format!("ref: {hm}")) && !ci.contains(&mid_published),
        "{ci}"
    );
    assert!(ci.contains(&format!("checkout --quiet {h1}")), "{ci}");

    // A second apply refuses while this one is under way.
    let (ok, err) = fx.shelf(&["release", "apply", "base1-mojo", "--skip-preflight"]);
    assert!(!ok && err.contains("--resume"), "{err}");

    // A fix lands on base1's release branch: resume re-pins mid and top,
    // and leaves base2 alone. Nothing is force-pushed.
    let fixed = fx.push_fix("base1", b1);
    let out = fx.ok(&["release", "apply", "--resume", "--skip-preflight"]);
    assert!(out.contains("re-pinning below it: mid, top"), "{out}");
    let (hm2, ht2) = (fx.remote_head("mid", bm), fx.remote_head("top", bt));
    assert_eq!(fx.remote_head("base2", b2), h2);
    assert_ne!(hm2, hm);
    assert_ne!(ht2, ht);
    git(
        &fx.ws.join("mid"),
        &["merge-base", "--is-ancestor", &hm, &hm2],
    );
    git(
        &fx.ws.join("top"),
        &["merge-base", "--is-ancestor", &ht, &ht2],
    );
    assert_eq!(
        pins_of(&fx.show("mid", bm, "pixi.toml"), "base1-mojo"),
        vec![fixed.clone(), fixed.clone()]
    );
    let top_manifest = fx.show("top", bt, "pixi.toml");
    assert_eq!(pins_of(&top_manifest, "mid-mojo"), vec![hm2.clone()]);
    assert_eq!(
        pins_of(&top_manifest, "base2-mojo"),
        vec![h2.clone(), h2.clone()]
    );
    let ci = fx.show("top", bt, ".github/workflows/ci.yml");
    assert!(ci.contains(&format!("ref: {hm2}")), "{ci}");
    assert!(ci.contains(&format!("checkout --quiet {fixed}")), "{ci}");

    // Nothing merged yet: --publish refuses.
    let out = fx.ok(&[
        "release",
        "apply",
        "--resume",
        "--publish",
        "--skip-preflight",
    ]);
    assert!(
        out.contains("base1: release/base1-mojo-0.2.0 isn't merged into main yet"),
        "{out}"
    );
    assert!(fx.publishes.lock().unwrap().is_empty());

    for (repo, branch) in [("base1", b1), ("base2", b2), ("mid", bm), ("top", bt)] {
        fx.merge(repo, branch);
    }
    let out = fx.ok(&[
        "release",
        "apply",
        "--resume",
        "--publish",
        "--skip-preflight",
    ]);
    assert!(out.contains("released:"), "{out}");
    let published: Vec<(String, String, String)> = fx
        .publishes
        .lock()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().into(),
                p["version"].as_str().unwrap().into(),
                p["commit_sha"].as_str().unwrap().into(),
            )
        })
        .collect();
    let want = |n: &str, v: &str, h: &str| (n.to_string(), v.to_string(), h.to_string());
    assert_eq!(
        published,
        vec![
            want("base1-mojo", "0.2.0", &fixed),
            want("base2-mojo", "1.0.1", &h2),
            want("mid-mojo", "0.3.1", &hm2),
            want("mid-full-mojo", "0.1.1", &hm2),
            want("top-mojo", "2.1.1", &ht2),
        ]
    );
    assert_eq!(fx.publishes.lock().unwrap()[3]["subdirectory"], "full");
    let out = fx.ok(&["release", "apply", "--resume"]);
    assert!(out.contains("is finished"), "{out}");
}

#[test]
fn waves_cut_one_level_at_a_time() {
    let fx = fixture();
    fx.ok(&[
        "release",
        "apply",
        "base1-mojo",
        "--waves",
        "--skip-preflight",
    ]);
    assert_eq!(
        fx.remote_head("base1", "release/base1-mojo-0.2.0").len(),
        40
    );
    assert!(
        fx.remote_head("mid", "release/mid-mojo-0.3.1").is_empty(),
        "mid waits for base1's publish"
    );
    fx.merge("base1", "release/base1-mojo-0.2.0");
    let out = fx.ok(&[
        "release",
        "apply",
        "--resume",
        "--publish",
        "--skip-preflight",
    ]);
    assert!(out.contains("published base1-mojo 0.2.0"), "{out}");
    assert_eq!(fx.remote_head("mid", "release/mid-mojo-0.3.1").len(), 40);
    assert!(fx.remote_head("top", "release/top-mojo-2.1.1").is_empty());
}

#[test]
fn a_sibling_outside_the_cascade_at_another_commit_stops_apply() {
    let fx = fixture();
    // Releasing mid alone: base1 stays at its published commit, which mid
    // pins, but top's CI clones base1 at an unrelated commit.
    let out = fx.ok(&["release", "plan", "mid-mojo", "--json"]);
    let plan: Value = serde_json::from_str(&out).unwrap();
    let top = &plan["releases"][1];
    assert_eq!(top["repo"], "top");
    assert_eq!(top["refs"], json!([]), "base1 isn't in the cascade");
    let mismatches = top["mismatches"].as_array().unwrap();
    assert_eq!(mismatches.len(), 1, "{mismatches:?}");
    assert_eq!(mismatches[0]["at"], ".github/workflows/ci.yml:10");
    assert_eq!(mismatches[0]["tin"], "base1");
    assert_eq!(mismatches[0]["rev"], UNRELATED);
    assert_eq!(mismatches[0]["parent"], "mid");
    let human = fx.ok(&["release", "plan", "mid-mojo"]);
    assert!(
        human.contains(
            "mismatch: .github/workflows/ci.yml:10 has base1 at 4cf043712aa3, but mid pins it at"
        ) && human.contains("1 mismatch: "),
        "{human}"
    );

    let (ok, err) = fx.shelf(&["release", "apply", "mid-mojo", "--skip-preflight"]);
    assert!(!ok && err.contains("--allow-mismatch"), "{err}");
    assert!(
        !fx.ws.join(".shelf-release.json").exists(),
        "refused before anything was cut"
    );
    assert!(fx.remote_head("mid", "release/mid-mojo-0.3.1").is_empty());

    let out = fx.ok(&[
        "release",
        "apply",
        "mid-mojo",
        "--skip-preflight",
        "--allow-mismatch",
    ]);
    assert!(out.contains("releasing despite these mismatches"), "{out}");
    let ci = fx.show("top", "release/top-mojo-2.1.1", ".github/workflows/ci.yml");
    assert!(ci.contains(UNRELATED), "left as it was: {ci}");
}

/// magmalake/orca.mojo#161: parquet -> iceberg -> orca, where orca's CI
/// builds iceberg from source next to a checkout of parquet at a commit
/// that's neither parquet's published one nor what iceberg pins. GitHub URLs
/// in the registry and the manifests, file:// remotes for the checkouts.
fn magmalake() -> (Fixture, String, String) {
    let fx = empty();
    let gh = |repo: &str| format!("https://github.com/magmalake/{repo}");
    let thrift = "21cdbce67d22d72e88e88a60e2f168b05b4ef9fa";
    let parquet = fx.repo(
        "parquet.mojo",
        &[
            (
                "shelf.toml",
                "name = \"parquet-mojo\"\nversion = \"0.12.0\"\n".into(),
            ),
            ("pixi.toml", package("parquet-mojo", "0.12.0", "")),
        ],
    );
    let iceberg = fx.repo(
        "iceberg.mojo",
        &[
            ("shelf.toml", "name = \"iceberg-mojo\"\nversion = \"0.10.1\"\n".into()),
            (
                "pixi.toml",
                package(
                    "iceberg-mojo",
                    "0.10.1",
                    &format!(
                        "\n[package.host-dependencies]\nparquet-mojo = {{ git = \"{}\", rev = \"{parquet}\" }}\nthrift-mojo = {{ git = \"{}\", rev = \"{thrift}\" }}\n",
                        gh("parquet.mojo"),
                        gh("thrift.mojo")
                    ),
                ),
            ),
            (
                ".github/workflows/ci.yml",
                "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          repository: magmalake/parquet.mojo\n          path: parquet.mojo\n".into(),
            ),
        ],
    );
    let orca = fx.repo(
        "orca.mojo",
        &[
            ("shelf.toml", "name = \"orca-mojo\"\nversion = \"0.8.3\"\n".into()),
            (
                "pixi.toml",
                package(
                    "orca-mojo",
                    "0.8.3",
                    &format!(
                        "\n[package.run-dependencies]\niceberg-mojo = {{ git = \"{}\", rev = \"{iceberg}\" }}\n",
                        gh("iceberg.mojo")
                    ),
                ),
            ),
            (
                ".github/workflows/ci.yml",
                format!(
                    "jobs:\n  data:\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/checkout@v4\n        with: {{repository: magmalake/parquet.mojo, ref: {UNRELATED}, path: parquet.mojo}}\n      - uses: actions/checkout@v4\n        with: {{repository: magmalake/iceberg.mojo, ref: {iceberg}, path: iceberg.mojo}}\n      - uses: actions/checkout@v4\n        with:\n          repository: magmalake/thrift.mojo\n          ref: {thrift}\n          path: thrift.mojo\n"
                ),
            ),
        ],
    );
    let t = |name: &str, repo: &str, v: &str, sha: &str, deps: &[&str]| StubTin {
        name: name.into(),
        url: gh(repo),
        subdirectory: None,
        versions: vec![(v.into(), sha.into())],
        dependents: deps.iter().map(|d| d.to_string()).collect(),
    };
    *fx.tins.lock().unwrap() = vec![
        t(
            "parquet-mojo",
            "parquet.mojo",
            "0.12.0",
            &parquet,
            &["iceberg-mojo"],
        ),
        t(
            "iceberg-mojo",
            "iceberg.mojo",
            "0.10.1",
            &iceberg,
            &["orca-mojo"],
        ),
        t("orca-mojo", "orca.mojo", "0.8.3", &orca, &[]),
    ];
    (fx, parquet, iceberg)
}

#[test]
fn plan_moves_every_checkout_of_a_cascade_tin_by_repository() {
    let (fx, _, iceberg) = magmalake();
    let args = [
        "release",
        "plan",
        "parquet-mojo",
        "--bump",
        "parquet-mojo=minor",
    ];
    let out = fx.ok(&[&args[..], &["--json"]].concat());
    let plan: Value = serde_json::from_str(&out).unwrap();
    let repos: Vec<&str> = plan["releases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, vec!["parquet.mojo", "iceberg.mojo", "orca.mojo"]);
    let orca = &plan["releases"][2];
    assert_eq!(orca["parents"], json!(["iceberg.mojo"]));
    let refs: Vec<(&str, &str, u64)> = orca["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["repo"].as_str().unwrap(),
                r["rev"].as_str().unwrap(),
                r["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        refs,
        vec![
            ("parquet.mojo", UNRELATED, 6),
            ("iceberg.mojo", iceberg.as_str(), 8)
        ],
        "parquet, which orca doesn't pin, moves too"
    );
    assert_eq!(
        orca["workflow_pins"],
        json!([]),
        "iceberg's line is listed once, as a checkout"
    );
    assert_eq!(orca["mismatches"], json!([]), "thrift agrees with iceberg");
    assert_eq!(orca["warnings"], json!([]));

    let human = fx.ok(&args);
    assert!(
        human.contains(
            ".github/workflows/ci.yml:6  checkout of parquet.mojo  4cf043712aa3 -> release/parquet-mojo-0.13.0 head"
        ),
        "{human}"
    );
    assert!(!human.contains("mismatch"), "{human}");
}

#[test]
fn plan_reports_a_sibling_checkout_its_parent_doesnt_pin() {
    let (fx, parquet, _) = magmalake();
    // Releasing iceberg alone: parquet stays at what iceberg pins, and
    // orca's CI would build iceberg against a parquet it wasn't built with.
    let out = fx.ok(&["release", "plan", "iceberg-mojo", "--json"]);
    let plan: Value = serde_json::from_str(&out).unwrap();
    let orca = &plan["releases"][1];
    assert_eq!(
        orca["refs"].as_array().unwrap().len(),
        1,
        "iceberg's checkout"
    );
    assert_eq!(
        orca["mismatches"],
        json!([{
            "at": ".github/workflows/ci.yml:6",
            "tin": "parquet.mojo",
            "rev": UNRELATED,
            "parent": "iceberg.mojo",
            "parent_revs": [parquet],
        }])
    );
    let (ok, err) = fx.shelf(&["release", "apply", "iceberg-mojo", "--skip-preflight"]);
    assert!(
        !ok && err
            .contains("orca.mojo: .github/workflows/ci.yml:6 has parquet.mojo at 4cf043712aa3")
            && err.contains("--allow-mismatch"),
        "{err}"
    );
    assert!(fx
        .remote_head("iceberg.mojo", "release/iceberg-mojo-0.10.2")
        .is_empty());
}
