//! HTTP client for the registry API.

use anyhow::{anyhow, bail, Result};
use serde::de::DeserializeOwned;
use shelf_core::{ApiError, BookDetail, BookSummary, PublishRequest, ResolvedBook};

pub struct Registry {
    base: String,
    agent: ureq::Agent,
}

impl Registry {
    pub fn new(base: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .new_agent();
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }

    fn parse<T: DeserializeOwned>(mut res: ureq::http::Response<ureq::Body>) -> Result<T> {
        let status = res.status().as_u16();
        if (300..400).contains(&status) {
            let target = res
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            if target.contains("cloudflareaccess.com") {
                bail!(
                    "the registry route is gated by Cloudflare Access; \
                     remove the Access application covering it and retry"
                );
            }
            bail!("registry unexpectedly redirected to {target}");
        }
        if status >= 400 {
            let msg = res
                .body_mut()
                .read_json::<ApiError>()
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("registry returned HTTP {status}"));
            bail!(msg);
        }
        res.body_mut()
            .read_json::<T>()
            .map_err(|e| anyhow!("could not parse registry response: {e}"))
    }

    pub fn search(&self, term: &str) -> Result<Vec<BookSummary>> {
        let url = format!("{}/api/books?q={}", self.base, urlencode(term));
        Self::parse(self.agent.get(&url).call()?)
    }

    pub fn info(&self, name: &str) -> Result<BookDetail> {
        let url = format!("{}/api/books/{}", self.base, urlencode(name));
        Self::parse(self.agent.get(&url).call()?)
    }

    pub fn resolve(&self, name: &str, version: Option<&str>) -> Result<Vec<ResolvedBook>> {
        let mut url = format!("{}/api/books/{}/resolve", self.base, urlencode(name));
        if let Some(v) = version {
            url.push_str(&format!("?version={}", urlencode(v)));
        }
        Self::parse(self.agent.get(&url).call()?)
    }

    pub fn publish(&self, req: &PublishRequest) -> Result<()> {
        let token = std::env::var("SHELF_TOKEN").map_err(|_| {
            anyhow!("SHELF_TOKEN is not set; sign in at {}/authors to get one", self.base)
        })?;
        let url = format!("{}/api/publish", self.base);
        let res = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(req)?;
        let _: serde_json::Value = Self::parse(res)?;
        Ok(())
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
