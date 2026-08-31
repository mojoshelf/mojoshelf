//! Agent-facing "tin cards": one precomputed markdown document per tin that
//! answers what an agent needs before using it — import name vs package
//! name, install commands, API surface, a usage snippet, health signals.
//! Pure functions here; the Worker cron does the fetching and storage.

use crate::TinDetail;

/// Total card size cap; the API-surface section is truncated to fit.
pub const CARD_MAX_BYTES: usize = 8 * 1024;
const SNIPPET_MAX_LINES: usize = 30;
const SIGS_PER_FILE: usize = 20;

/// Top-level (unindented) `fn` / `struct` / `trait` / `alias` signatures
/// from a Mojo source file, trailing `:` stripped. A crude line parse — it
/// exists to give agents a scent of the API, not to be a parser.
pub fn extract_signatures(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|line| {
            ["fn ", "struct ", "trait ", "alias "]
                .iter()
                .any(|kw| line.starts_with(kw))
        })
        .take(SIGS_PER_FILE)
        .map(|line| line.trim_end().trim_end_matches(':').to_string())
        .collect()
}

/// First fenced code block from a README — a ```mojo block if any, else the
/// first untagged block. Skips blocks tagged with other languages.
pub fn extract_snippet(readme: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    let mut lines = readme.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(tag) = trimmed.strip_prefix("```") else {
            continue;
        };
        let tag = tag.trim().to_lowercase();
        let mut block = Vec::new();
        for body_line in lines.by_ref() {
            if body_line.trim_start().starts_with("```") {
                break;
            }
            if block.len() < SNIPPET_MAX_LINES {
                block.push(body_line.to_string());
            }
        }
        if block.is_empty() {
            continue;
        }
        let text = block.join("\n");
        if tag == "mojo" {
            return Some(text);
        }
        if tag.is_empty() && fallback.is_none() {
            fallback = Some(text);
        }
    }
    fallback
}

/// The Mojo import name from a tin repo's pixi.toml:
/// `[package.build.config.pkg]` → `name = "..."`.
pub fn pixi_import_name(pixi_toml: &str) -> Option<String> {
    let mut in_pkg_section = false;
    for line in pixi_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_pkg_section = trimmed == "[package.build.config.pkg]";
            continue;
        }
        if in_pkg_section {
            if let Some(rest) = trimmed.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                let value = value.trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Guessed Mojo import name when the repo's pixi.toml doesn't say:
/// mojo affixes stripped, `-` → `_` (zlib-mojo → zlib, small-time → small_time).
pub fn guess_import_name(tin_name: &str) -> String {
    tin_name
        .strip_suffix("-mojo")
        .or_else(|| tin_name.strip_prefix("mojo-"))
        .unwrap_or(tin_name)
        .replace('-', "_")
}

/// Repo-derived extras the cron feeds into a card; all optional so a card
/// degrades gracefully when fetches fail.
#[derive(Default)]
pub struct CardExtras {
    /// From pixi.toml when Some; guessed otherwise.
    pub import_name: Option<String>,
    /// (file path, signatures) per scanned source file.
    pub api: Vec<(String, Vec<String>)>,
    /// README code block.
    pub snippet: Option<String>,
}

/// Assembles the markdown card for one tin. Works for both kinds; `extras`
/// is empty for channel tins and for on-the-fly fallback cards.
pub fn assemble_card(d: &TinDetail, extras: &CardExtras) -> String {
    let mut out = String::new();
    let kind_label = if d.kind == "channel" {
        "binary package from the modular-community channel, mirrored on mojoshelf"
    } else {
        "source tin on mojoshelf, git-pinned per published version"
    };
    out.push_str(&format!("# {} ({})\n\n", d.name, kind_label));
    if let Some(desc) = d.description.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("{desc}\n\n"));
    }

    out.push_str(&format!("- package name (registry/conda): `{}`\n", d.name));
    let (import, guessed) = match &extras.import_name {
        Some(n) => (n.clone(), false),
        None => (guess_import_name(&d.name), true),
    };
    out.push_str(&format!(
        "- Mojo import name: `{import}` (e.g. `from {import} import …`){}\n",
        if guessed {
            " — guessed from the tin name, verify in the repo"
        } else {
            ""
        }
    ));
    if let Some(author) = d.author.as_deref() {
        out.push_str(&format!("- author: {author}\n"));
    }
    if !d.tags.is_empty() {
        out.push_str(&format!("- tags: {}\n", d.tags.join(", ")));
    }
    out.push_str(&format!("- repository: {}\n", d.url));
    match d.kind.as_str() {
        "channel" => {
            if let Some(v) = d.channel_version.as_deref() {
                out.push_str(&format!("- latest channel version: {v}\n"));
            }
        }
        _ => {
            if let Some(v) = d.versions.first() {
                out.push_str(&format!(
                    "- latest version: {} (commit {})\n",
                    v.version,
                    &v.commit_sha[..12.min(v.commit_sha.len())]
                ));
                if !v.dependencies.is_empty() {
                    out.push_str(&format!("- depends on: {}\n", v.dependencies.join(", ")));
                }
            } else {
                out.push_str("- no published versions yet\n");
            }
            if !d.dependents.is_empty() {
                out.push_str(&format!("- depended on by: {}\n", d.dependents.join(", ")));
            }
        }
    }
    if let (Some(stars), Some(push)) = (d.stars, d.last_push.as_deref()) {
        out.push_str(&format!("- activity: {stars} stars, last push {push}"));
        if let (Some(m), Some(y)) = (d.commits_month, d.commits_year) {
            out.push_str(&format!(", {m} commits last month / {y} last year"));
        }
        out.push('\n');
    }
    match (d.verified_ok, d.verified_at.as_deref()) {
        (Some(true), Some(at)) => {
            let compiler = d
                .verified_compiler
                .as_deref()
                .map(|c| format!(" with mojo-compiler {c}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- verified: consumer smoke build passed{compiler} (checked {at})\n"
            ));
        }
        (Some(false), Some(at)) => {
            out.push_str(&format!(
                "- verified: consumer smoke build FAILING (checked {at})\n"
            ));
        }
        _ => {}
    }

    out.push_str("\n## Install\n\n");
    if d.kind == "channel" {
        out.push_str(&format!(
            "```sh\npixi shelf add {n}    # or plain: pixi add {n}\n```\n\n\
             A binary conda package; the solver picks the version — no registry \
             pinning, and submodule mode does not apply.\n",
            n = d.name
        ));
    } else {
        out.push_str(&format!(
            "```sh\npixi shelf add {n}    # pixi mode: registry-pinned git source dependency\nshelf add {n}         # submodule mode: pinned source under shelf/{n}\n```\n",
            n = d.name
        ));
        if let Some(v) = d.versions.first() {
            out.push_str(&format!(
                "\nOr with plain pixi (no shelf CLI):\n```sh\npixi add --git {} --rev {} {}\n```\n",
                d.url, v.commit_sha, d.name
            ));
        }
    }

    if !extras.api.is_empty() {
        let mut api_section = String::from("\n## API surface\n");
        for (path, sigs) in &extras.api {
            if sigs.is_empty() {
                continue;
            }
            api_section.push_str(&format!("\n### {path}\n\n"));
            for sig in sigs {
                api_section.push_str(&format!("- `{sig}`\n"));
            }
        }
        let remaining = CARD_MAX_BYTES.saturating_sub(out.len() + 1024);
        if api_section.len() > remaining {
            let mut cut = remaining.min(api_section.len());
            while cut > 0 && !api_section.is_char_boundary(cut) {
                cut -= 1;
            }
            api_section.truncate(cut);
            api_section.push_str("\n- … (truncated)\n");
        }
        out.push_str(&api_section);
    }

    if let Some(snippet) = extras.snippet.as_deref() {
        if out.len() + snippet.len() + 64 < CARD_MAX_BYTES {
            out.push_str(&format!("\n## Usage (from the README)\n\n```mojo\n{snippet}\n```\n"));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VersionInfo;

    fn detail() -> TinDetail {
        TinDetail {
            nightly_at: None,
            nightly_ok: None,
            nightly_compiler: None,
            verified_run_url: None,
            nightly_run_url: None,
            name: "zlib-mojo".into(),
            url: "https://github.com/o/zlib.mojo.git".into(),
            description: Some("zlib bindings".into()),
            author: Some("someone".into()),
            tags: vec!["compression".into()],
            versions: vec![VersionInfo {
                version: "0.2.0".into(),
                commit_sha: "a".repeat(40),
                published_at: "2026-08-01T00:00:00Z".into(),
                dependencies: vec![],
            }],
            dependents: vec!["docx".into()],
            kind: "source".into(),
            channel_version: None,
            stars: Some(4),
            last_push: Some("2026-08-20T00:00:00Z".into()),
            commits_month: Some(3),
            commits_year: Some(30),
            prev_url: None,
            url_changed_at: None,
            verified_at: Some("2026-08-25T00:00:00Z".into()),
            verified_ok: Some(true),
            verified_compiler: Some("1.0.0".into()),
        }
    }

    #[test]
    fn signatures_top_level_only() {
        let src = "struct Reader:\n    fn read(self):\n        pass\nfn parse(s: String) raises -> Int:\ntrait Writable:\nalias Byte = UInt8\n# fn commented\n";
        assert_eq!(
            extract_signatures(src),
            vec![
                "struct Reader",
                "fn parse(s: String) raises -> Int",
                "trait Writable",
                "alias Byte = UInt8",
            ]
        );
    }

    #[test]
    fn snippet_prefers_mojo_block() {
        let readme = "Intro\n```sh\npixi add x\n```\n```mojo\nfrom zlib import inflate\n```\n";
        assert_eq!(extract_snippet(readme).as_deref(), Some("from zlib import inflate"));
        let plain = "```\ngeneric block\n```\n```python\nnope\n```";
        assert_eq!(extract_snippet(plain).as_deref(), Some("generic block"));
        assert_eq!(extract_snippet("no code here"), None);
    }

    #[test]
    fn import_name_from_pixi_toml_and_guess() {
        let toml = "[package]\nname = \"zlib-mojo\"\n[package.build.config.pkg]\npath = \"src/zlib\"\nname = \"zlib\"\n";
        assert_eq!(pixi_import_name(toml).as_deref(), Some("zlib"));
        assert_eq!(pixi_import_name("[package]\nname = \"x\"\n"), None);
        assert_eq!(guess_import_name("zlib-mojo"), "zlib");
        assert_eq!(guess_import_name("mojo-libc"), "libc");
        assert_eq!(guess_import_name("small-time"), "small_time");
    }

    #[test]
    fn card_has_the_load_bearing_facts() {
        let extras = CardExtras {
            import_name: Some("zlib".into()),
            api: vec![("src/zlib/inflate.mojo".into(), vec!["fn inflate(data: List[UInt8]) raises -> List[UInt8]".into()])],
            snippet: Some("from zlib import inflate".into()),
        };
        let card = assemble_card(&detail(), &extras);
        for needle in [
            "# zlib-mojo",
            "package name (registry/conda): `zlib-mojo`",
            "Mojo import name: `zlib`",
            "pixi shelf add zlib-mojo",
            "shelf add zlib-mojo",
            "--rev aaaa",
            "fn inflate",
            "from zlib import inflate",
            "smoke build passed with mojo-compiler 1.0.0",
            "depended on by: docx",
        ] {
            assert!(card.contains(needle), "card missing {needle:?}:\n{card}");
        }
        assert!(!card.contains("guessed from the tin name"));
        assert!(card.len() <= CARD_MAX_BYTES);
    }

    #[test]
    fn card_guesses_import_and_caps_size() {
        let sigs: Vec<String> = (0..200).map(|i| format!("fn f{i}(x: Int) -> Int")).collect();
        let extras = CardExtras {
            import_name: None,
            api: (0..40).map(|i| (format!("src/m/f{i}.mojo"), sigs.clone())).collect(),
            snippet: None,
        };
        let card = assemble_card(&detail(), &extras);
        assert!(card.contains("guessed from the tin name"));
        assert!(card.contains("(truncated)"));
        assert!(card.len() <= CARD_MAX_BYTES, "card is {} bytes", card.len());
    }
}
