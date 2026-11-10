//! Minimal GitHub REST client for the upstream tracker.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const API: &str = "https://api.github.com";
const RAW: &str = "https://raw.githubusercontent.com";

pub struct GitHub {
    agent: ureq::Agent,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub prerelease: bool,
    pub draft: bool,
    pub published_at: Option<String>,
    pub html_url: String,
}

#[derive(Debug, Deserialize)]
pub struct Compare {
    pub total_commits: u64,
    #[serde(default)]
    pub commits: Vec<CommitEntry>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Deserialize)]
pub struct CommitEntry {
    pub sha: String,
    pub commit: CommitInner,
}

#[derive(Debug, Deserialize)]
pub struct CommitInner {
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct FileEntry {
    pub filename: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Deserialize)]
pub struct ContentEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: String,
    pub submodule_git_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommitRef {
    sha: String,
}

impl GitHub {
    pub fn new() -> Self {
        let token = ["GITHUB_TOKEN", "GH_TOKEN"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .or_else(gh_cli_token);
        Self {
            agent: ureq::Agent::new_with_defaults(),
            token,
        }
    }

    fn get(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let mut req = self
            .agent
            .get(url)
            .header("User-Agent", "tdr-xtask")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        req
    }

    fn api<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{API}{path}");
        self.get(&url)
            .call()
            .with_context(|| format!("GET {url}"))?
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_json()
            .with_context(|| format!("decoding {url}"))
    }

    pub fn releases(&self, repo: &str) -> Result<Vec<Release>> {
        self.api(&format!("/repos/{repo}/releases?per_page=50"))
    }

    pub fn commit_sha(&self, repo: &str, reference: &str) -> Result<String> {
        Ok(self
            .api::<CommitRef>(&format!("/repos/{repo}/commits/{reference}"))?
            .sha)
    }

    /// Metadata of a path at a ref; for submodules gives the pinned commit.
    pub fn content_entry(&self, repo: &str, path: &str, reference: &str) -> Result<ContentEntry> {
        self.api(&format!("/repos/{repo}/contents/{path}?ref={reference}"))
    }

    /// Compares two refs. Files come from the first page only (GitHub caps them at 300);
    /// commits are paginated.
    pub fn compare(&self, repo: &str, base: &str, head: &str) -> Result<Compare> {
        let mut result: Compare = self.api(&format!(
            "/repos/{repo}/compare/{base}...{head}?per_page=100&page=1"
        ))?;
        let mut page = 2;
        while (result.commits.len() as u64) < result.total_commits && page <= 20 {
            let next: Compare = self.api(&format!(
                "/repos/{repo}/compare/{base}...{head}?per_page=100&page={page}"
            ))?;
            if next.commits.is_empty() {
                break;
            }
            result.commits.extend(next.commits);
            page += 1;
        }
        Ok(result)
    }

    /// Raw file contents at a given ref.
    pub fn raw(&self, repo: &str, reference: &str, path: &str) -> Result<Vec<u8>> {
        let url = format!("{RAW}/{repo}/{reference}/{path}");
        self.get(&url)
            .call()
            .with_context(|| format!("GET {url}"))?
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .with_context(|| format!("reading {url}"))
    }
}

/// Falls back to the GitHub CLI's token for local runs (raises the API rate limit).
fn gh_cli_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}
