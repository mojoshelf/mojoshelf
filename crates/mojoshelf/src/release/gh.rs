//! Thin wrappers over the `gh` binary, for release PRs.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

fn gh(args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .context("failed to run gh; is it installed?")?;
    if !out.status.success() {
        bail!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether `gh` is installed and signed in.
pub fn available() -> bool {
    Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub url: String,
}

/// Open a PR labelled `release` (making the label if the repo lacks it).
pub fn create(slug: &str, base: &str, head: &str, title: &str, body: &str) -> Result<Pr> {
    let _ = gh(&[
        "label",
        "create",
        "release",
        "--repo",
        slug,
        "--color",
        "0e8a16",
        "--description",
        "A release made by shelf release",
        "--force",
    ]);
    let url = gh(&[
        "pr", "create", "--repo", slug, "--base", base, "--head", head, "--title", title, "--body",
        body, "--label", "release",
    ])?;
    let url = url.lines().last().unwrap_or_default().trim().to_string();
    let number = url
        .rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .with_context(|| format!("gh pr create printed '{url}', not a PR URL"))?;
    Ok(Pr { number, url })
}

/// The open PR for `head`, if there is one (a rerun after a PR was opened
/// but before the state file recorded it).
pub fn find(slug: &str, head: &str) -> Result<Option<Pr>> {
    let out = gh(&[
        "pr",
        "list",
        "--repo",
        slug,
        "--head",
        head,
        "--state",
        "open",
        "--json",
        "number,url",
    ])?;
    let prs: Vec<Pr> = serde_json::from_str(&out).context("could not parse gh pr list")?;
    Ok(prs.into_iter().next())
}

pub fn edit_body(slug: &str, number: u64, body: &str) -> Result<()> {
    gh(&[
        "pr",
        "edit",
        &number.to_string(),
        "--repo",
        slug,
        "--body",
        body,
    ])?;
    Ok(())
}

/// How a PR's checks stand.
#[derive(Debug, Clone, PartialEq)]
pub enum Checks {
    Green,
    Pending(Vec<String>),
    Failed(Vec<String>),
}

#[derive(Deserialize)]
struct Check {
    name: String,
    bucket: String,
}

/// The PR's required checks, or all of them when the repo requires none.
pub fn checks(slug: &str, number: u64) -> Result<Checks> {
    let n = number.to_string();
    let fields = "name,bucket";
    let run = |required: bool| -> Result<String> {
        let mut args = vec!["pr", "checks", &n, "--repo", slug, "--json", fields];
        if required {
            args.push("--required");
        }
        // `gh pr checks` exits 8 while checks are pending and 1 when one
        // failed, printing the JSON either way.
        let out = Command::new("gh")
            .args(&args)
            .output()
            .context("failed to run gh; is it installed?")?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if stdout.starts_with('[') {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.contains("no required checks") || stderr.contains("no checks reported") {
            return Ok(String::new());
        }
        bail!("gh pr checks failed: {stderr}");
    };
    let mut out = run(true)?;
    if out.is_empty() {
        out = run(false)?;
    }
    if out.is_empty() {
        return Ok(Checks::Green);
    }
    let list: Vec<Check> = serde_json::from_str(&out).context("could not parse gh pr checks")?;
    Ok(summarize(list.into_iter().map(|c| (c.name, c.bucket))))
}

fn summarize(list: impl Iterator<Item = (String, String)>) -> Checks {
    let (mut failed, mut pending) = (Vec::new(), Vec::new());
    for (name, bucket) in list {
        match bucket.as_str() {
            "fail" | "cancel" => failed.push(name),
            "pending" => pending.push(name),
            _ => {}
        }
    }
    if !failed.is_empty() {
        Checks::Failed(failed)
    } else if !pending.is_empty() {
        Checks::Pending(pending)
    } else {
        Checks::Green
    }
}

pub fn merge(slug: &str, number: u64) -> Result<()> {
    gh(&[
        "pr",
        "merge",
        &number.to_string(),
        "--repo",
        slug,
        "--merge",
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_summary_puts_failures_first() {
        let s =
            |v: &[(&str, &str)]| summarize(v.iter().map(|(a, b)| (a.to_string(), b.to_string())));
        assert_eq!(s(&[("test", "pass"), ("lint", "skipping")]), Checks::Green);
        assert_eq!(
            s(&[("test", "pending"), ("lint", "pass")]),
            Checks::Pending(vec!["test".into()])
        );
        assert_eq!(
            s(&[("test", "pending"), ("lint", "fail"), ("x", "cancel")]),
            Checks::Failed(vec!["lint".into(), "x".into()])
        );
    }
}
