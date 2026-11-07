//! Preflight's `pixi install` output, cut down to one line per package.
//!
//! pixi prints rattler-build's whole log for every source dependency it
//! builds: thousands of lines, in which a failure is hard to find. Each build
//! is a top-level box,
//!
//! ```text
//!  ╭─ Running build for recipe: lz4-mojo-0.1.4-h60d57d3_0
//!  │ …
//!  │ │ × error Script failed with status 1
//!  │ …
//!  ╰─────────────────── (took 6 seconds)
//! ```
//!
//! so the full output goes to a log file and [`BuildLog`] reports each box
//! as it closes, and on a failure the end of the failed package's box.

const START: &str = "╭─ Running build for recipe: ";

#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    pub name: String,
    pub version: String,
    pub failed: bool,
    /// As pixi printed it, e.g. "6 seconds".
    pub took: Option<String>,
}

impl Build {
    pub fn describe(&self) -> String {
        let took = self
            .took
            .as_deref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();
        format!(
            "{} {} {}{took}",
            if self.failed { "failed" } else { "built " },
            self.name,
            self.version
        )
    }
}

/// Splits `name-version-build` (the recipe's output name) into its name and
/// version; names may hold dashes, versions and build strings don't.
fn split_recipe(id: &str) -> (String, String) {
    let mut parts = id.trim().rsplitn(3, '-');
    let _build = parts.next();
    match (parts.next(), parts.next()) {
        (Some(version), Some(name)) => (name.to_string(), version.to_string()),
        _ => (id.trim().to_string(), String::new()),
    }
}

#[derive(Default)]
pub struct BuildLog {
    current: Option<(Build, Vec<String>)>,
    /// The lines of the last build that failed.
    failed_lines: Vec<String>,
    /// Lines outside any build box since the last one closed (pixi's own
    /// errors, a solve failure).
    outside: Vec<String>,
    pub builds: Vec<Build>,
}

impl BuildLog {
    /// Take one line of output; returns the build that it finished, if any.
    pub fn feed(&mut self, line: &str) -> Option<Build> {
        let line = line.trim_end_matches(['\n', '\r']);
        let top = line.trim_start();
        if let Some(id) = top.strip_prefix(START) {
            let (name, version) = split_recipe(id);
            self.current = Some((
                Build {
                    name,
                    version,
                    failed: false,
                    took: None,
                },
                vec![line.to_string()],
            ));
            self.outside.clear();
            return None;
        }
        let Some((build, lines)) = self.current.as_mut() else {
            self.outside.push(line.to_string());
            return None;
        };
        lines.push(line.to_string());
        if top.starts_with('│') {
            if top.contains("× error") {
                build.failed = true;
            }
            return None;
        }
        if top.starts_with("╰─") {
            build.took = top
                .rsplit_once("(took ")
                .map(|(_, t)| t.trim_end_matches(')').to_string());
            let (build, lines) = self.current.take().expect("in a build");
            if build.failed {
                self.failed_lines = lines;
            }
            self.builds.push(build.clone());
            return Some(build);
        }
        None
    }

    /// What to show of a failed run: the last `tail` lines of the failed
    /// package's build, or of whatever was printed outside the builds (a
    /// solve failure), followed by pixi's own error after the builds.
    pub fn failure(&self, tail: usize) -> Vec<String> {
        let last = |v: &[String]| v[v.len().saturating_sub(tail)..].to_vec();
        let mut out = Vec::new();
        if let Some((_, lines)) = &self.current {
            out.extend(last(lines));
        } else if !self.failed_lines.is_empty() {
            out.extend(last(&self.failed_lines));
        }
        let rest: Vec<String> = self
            .outside
            .iter()
            .filter(|l| !l.trim().is_empty())
            .cloned()
            .collect();
        out.extend(last(&rest));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = " ╭─ Running build for recipe: json_mlake-0.4.3-h60d57d3_0
 │
 │ ╭─ Fetching source code
 │ │ No sources to fetch
 │ ╰─────────────────── (took 0 seconds)
 │
 ╰─────────────────── (took 8 seconds)
";

    const FAILED: &str = " ╭─ Running build for recipe: iceberg-mojo-0.10.0-h60d57d3_0
 │ ╭─ Running build script
 │ │ maintain.mojo:338:20: error: field destroyed out of the middle of a value
 │ │ mojo: error: errors occurred during compilation
 │ │ × error Script failed with status 1
 │ ╰─────────────────── (took 6 seconds)
 │
 ╰─────────────────── (took 6 seconds)
Error:   × failed to build 'iceberg-mojo'
  ╰─▶   × Script failed to execute
";

    fn run(text: &str) -> (BuildLog, Vec<String>) {
        let mut log = BuildLog::default();
        let done = text
            .lines()
            .filter_map(|l| log.feed(l))
            .map(|b| b.describe())
            .collect();
        (log, done)
    }

    #[test]
    fn one_line_per_package() {
        let (_, done) = run(&format!("{OK}{FAILED}"));
        assert_eq!(
            done,
            [
                "built  json_mlake 0.4.3 (8 seconds)",
                "failed iceberg-mojo 0.10.0 (6 seconds)"
            ]
        );
    }

    #[test]
    fn a_failure_shows_its_build_and_pixis_error() {
        let (log, _) = run(&format!("{OK}{FAILED}"));
        let shown = log.failure(4);
        assert_eq!(
            shown,
            [
                " │ │ × error Script failed with status 1",
                " │ ╰─────────────────── (took 6 seconds)",
                " │",
                " ╰─────────────────── (took 6 seconds)",
                "Error:   × failed to build 'iceberg-mojo'",
                "  ╰─▶   × Script failed to execute",
            ]
        );
    }

    #[test]
    fn a_solve_failure_shows_pixis_output() {
        let (log, done) = run("Error:   × failed to solve\n  ╰─▶ no candidates\n");
        assert!(done.is_empty());
        assert_eq!(
            log.failure(40),
            ["Error:   × failed to solve", "  ╰─▶ no candidates"]
        );
    }

    #[test]
    fn recipe_names_keep_their_dashes() {
        assert_eq!(
            split_recipe("memory-region-mojo-0.1.1-h60d57d3_0"),
            ("memory-region-mojo".into(), "0.1.1".into())
        );
    }
}
