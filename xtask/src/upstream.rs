//! Tracking of the original Telegram Desktop client (tdesktop).
//!
//! * `upstream/upstream.lock.toml` records which tdesktop release this port is synced to
//!   and the checksums of files vendored from it (TL schemas, ...).
//! * `upstream/map.toml` maps tdesktop source directories to areas of this project, so the
//!   report shows which of our subsystems an upstream release touches.
//!
//! `check` produces a Markdown report (changelog, TL schema diff, new localization strings,
//! touched areas, submodule changes, commit list). `sync` re-vendors files at a new tag and
//! bumps the lock. `verify` checks that vendored files were not edited by hand.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tdr_tl_parser::Schema;

use crate::github::{GitHub, Release};
use crate::report::{self, Limits, ReportData, SubmoduleChange};

const LOCK_PATH: &str = "upstream/upstream.lock.toml";
const MAP_PATH: &str = "upstream/map.toml";
pub const REPORTS_DIR: &str = "upstream/reports";
const LANG_PATH: &str = "Telegram/Resources/langs/lang.strings";
const CHANGELOG_PATH: &str = "changelog.txt";
const API_TL_LOCAL: &str = "schema/api.tl";
const API_TL_UPSTREAM: &str = "Telegram/SourceFiles/mtproto/scheme/api.tl";
/// GitHub rejects PR/issue bodies longer than 65536 characters.
const PR_BODY_MAX: usize = 60_000;

#[derive(Debug, Serialize, Deserialize)]
struct Lock {
    /// `owner/name` of the upstream repository.
    repo: String,
    /// `stable` — track releases only; `beta` — also prereleases.
    channel: String,
    tag: String,
    commit: String,
    published_at: String,
    layer: Option<u32>,
    #[serde(default)]
    vendored: Vec<Vendored>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Vendored {
    /// Path in this repository.
    path: String,
    /// Path in the upstream repository.
    upstream: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct AreaMap {
    pub area: Vec<Area>,
    #[serde(default)]
    pub submodule: Vec<Submodule>,
}

#[derive(Debug, Deserialize)]
pub struct Area {
    pub name: String,
    /// Crate(s) of this project responsible for the area.
    pub ours: String,
    pub paths: Vec<String>,
}

/// An upstream git submodule (lib_ui, lib_base, ...). Its changes are invisible in the
/// main repository's diff, so they are compared in the submodule's own repository.
#[derive(Debug, Deserialize)]
pub struct Submodule {
    pub path: String,
    pub ours: String,
}

fn root() -> PathBuf {
    // xtask lives in <root>/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent directory")
        .to_path_buf()
}

fn read_lock() -> Result<Lock> {
    let path = root().join(LOCK_PATH);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn write_lock(lock: &Lock) -> Result<()> {
    let header = "# Upstream tdesktop release this port is synced to.\n\
                  # Managed by `cargo xtask upstream sync` — do not edit checksums by hand.\n\n";
    let body = toml::to_string_pretty(lock)?;
    fs::write(root().join(LOCK_PATH), format!("{header}{body}"))?;
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Parses `v7.4.1`, `7.4.1 beta`, `7.4` into comparable numeric parts.
pub fn parse_version(s: &str) -> Option<Vec<u32>> {
    let s = s.trim().trim_start_matches('v');
    let s = s.split_whitespace().next()?;
    let mut parts = s
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Some(parts)
}

pub fn version_gt(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[derive(Default)]
struct Options {
    channel: Option<String>,
    report: Option<PathBuf>,
    pr_body: Option<PathBuf>,
    save: bool,
    github_output: bool,
    tag: Option<String>,
}

fn parse_options(args: &[&str]) -> Result<Options> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut value = || {
            it.next()
                .map(|v| v.to_string())
                .with_context(|| format!("{a} needs a value"))
        };
        match a {
            "--channel" => o.channel = Some(value()?),
            "--report" => o.report = Some(PathBuf::from(value()?)),
            "--pr-body" => o.pr_body = Some(PathBuf::from(value()?)),
            "--tag" => o.tag = Some(value()?),
            "--save" => o.save = true,
            "--github-output" => o.github_output = true,
            _ => bail!("unknown option {a}"),
        }
    }
    if let Some(c) = &o.channel
        && c != "stable"
        && c != "beta"
    {
        bail!("--channel must be `stable` or `beta`");
    }
    Ok(o)
}

/// Releases newer than the locked one, newest first.
fn newer_releases(gh: &GitHub, lock: &Lock, channel: &str) -> Result<Vec<Release>> {
    let mut releases: Vec<Release> = gh
        .releases(&lock.repo)?
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| channel == "beta" || !r.prerelease)
        .filter(|r| version_gt(&r.tag_name, &lock.tag))
        .collect();
    releases.sort_by_key(|r| std::cmp::Reverse(parse_version(&r.tag_name)));
    Ok(releases)
}

pub fn check(args: &[&str]) -> Result<()> {
    let opts = parse_options(args)?;
    let lock = read_lock()?;
    let channel = opts.channel.clone().unwrap_or_else(|| lock.channel.clone());
    let gh = GitHub::new();

    let releases = newer_releases(&gh, &lock, &channel)?;
    let Some(latest) = releases.first() else {
        println!(
            "Up to date: {} {} (channel: {channel}).",
            lock.repo, lock.tag
        );
        if opts.github_output {
            github_output(&[("update", "false")])?;
        }
        return Ok(());
    };
    let head = latest.tag_name.clone();
    eprintln!("New upstream release: {} → {head}", lock.tag);

    let data = collect(&gh, &lock, releases)?;
    let saved_path = format!("{REPORTS_DIR}/{head}.md");
    let full = report::render(&data, &Limits::FULL, None)?;
    let compact = report::render(&data, &Limits::COMPACT, Some(&saved_path))?;
    let compact = report::truncate(compact, PR_BODY_MAX);

    if opts.save {
        fs::create_dir_all(root().join(REPORTS_DIR))?;
        fs::write(root().join(&saved_path), &full)?;
        eprintln!("Full report saved to {saved_path}");
    }
    if let Some(path) = &opts.report {
        fs::write(path, &full)?;
        eprintln!("Full report written to {}", path.display());
    }
    if let Some(path) = &opts.pr_body {
        fs::write(path, &compact)?;
        eprintln!("PR body written to {}", path.display());
    }
    if opts.report.is_none() && opts.pr_body.is_none() && !opts.save {
        println!("{compact}");
    }
    if opts.github_output {
        let title = format!("Upstream: tdesktop {} → {head}", lock.tag);
        github_output(&[("update", "true"), ("tag", &head), ("title", &title)])?;
    }
    Ok(())
}

fn github_output(pairs: &[(&str, &str)]) -> Result<()> {
    use std::io::Write;
    let path = std::env::var("GITHUB_OUTPUT").context("GITHUB_OUTPUT is not set")?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    for (k, v) in pairs {
        writeln!(file, "{k}={v}")?;
    }
    Ok(())
}

/// Downloads everything the report needs.
fn collect(gh: &GitHub, lock: &Lock, releases: Vec<Release>) -> Result<ReportData> {
    let base = lock.tag.clone();
    let head = releases[0].tag_name.clone();
    let text = |reference: &str, path: &str| -> Result<String> {
        Ok(String::from_utf8_lossy(&gh.raw(&lock.repo, reference, path)?).into_owned())
    };

    let map: AreaMap = toml::from_str(&fs::read_to_string(root().join(MAP_PATH))?)
        .context("parsing upstream/map.toml")?;
    let compare = gh.compare(&lock.repo, &base, &head)?;

    let mut submodules = Vec::new();
    for sm in &map.submodule {
        let (Ok(old), Ok(new)) = (
            gh.content_entry(&lock.repo, &sm.path, &base),
            gh.content_entry(&lock.repo, &sm.path, &head),
        ) else {
            submodules.push(SubmoduleChange::Note(format!(
                "`{}`: could not resolve (added or removed?)",
                sm.path
            )));
            continue;
        };
        if old.sha == new.sha || new.kind != "submodule" {
            continue;
        }
        let sub_repo = new
            .submodule_git_url
            .as_deref()
            .and_then(|u| u.strip_prefix("https://github.com/"))
            .map(|u| u.trim_end_matches(".git").to_string());
        let Some(sub_repo) = sub_repo else {
            submodules.push(SubmoduleChange::Note(format!(
                "`{}`: bumped (non-GitHub submodule)",
                sm.path
            )));
            continue;
        };
        let cmp = gh.compare(&sub_repo, &old.sha, &new.sha)?;
        submodules.push(SubmoduleChange::Bumped {
            repo: sub_repo,
            ours: sm.ours.clone(),
            old: old.sha,
            new: new.sha,
            compare: cmp,
        });
    }

    Ok(ReportData {
        repo: lock.repo.clone(),
        changelog: text(&head, CHANGELOG_PATH)?,
        old_api: fs::read_to_string(root().join(API_TL_LOCAL))?,
        new_api: text(&head, API_TL_UPSTREAM)?,
        old_lang: text(&base, LANG_PATH)?,
        new_lang: text(&head, LANG_PATH)?,
        base,
        head,
        releases,
        map,
        compare,
        submodules,
    })
}

pub fn sync(args: &[&str]) -> Result<()> {
    let opts = parse_options(args)?;
    let mut lock = read_lock()?;
    let gh = GitHub::new();

    let (tag, published_at) = match &opts.tag {
        Some(t) => {
            let rel = gh
                .releases(&lock.repo)?
                .into_iter()
                .find(|r| &r.tag_name == t);
            (
                t.clone(),
                rel.and_then(|r| r.published_at).unwrap_or_default(),
            )
        }
        None => {
            let channel = opts.channel.clone().unwrap_or_else(|| lock.channel.clone());
            match newer_releases(&gh, &lock, &channel)?.into_iter().next() {
                Some(r) => (r.tag_name, r.published_at.unwrap_or_default()),
                None => {
                    println!("Already at the latest release ({}).", lock.tag);
                    return Ok(());
                }
            }
        }
    };

    // Download everything first so a network error never leaves a half-updated tree.
    let mut downloads = Vec::new();
    for v in &lock.vendored {
        downloads.push(gh.raw(&lock.repo, &tag, &v.upstream)?);
    }
    let commit = gh.commit_sha(&lock.repo, &tag)?;

    for (v, data) in lock.vendored.iter_mut().zip(downloads) {
        let sha = sha256_hex(&data);
        let status = if sha == v.sha256 {
            "unchanged"
        } else {
            "updated  "
        };
        fs::write(root().join(&v.path), &data).with_context(|| format!("writing {}", v.path))?;
        println!("{status} {}", v.path);
        v.sha256 = sha;
    }

    let api = fs::read_to_string(root().join(API_TL_LOCAL))?;
    lock.layer = Schema::parse(&api)?.layer;
    lock.commit = commit;
    lock.tag = tag;
    lock.published_at = published_at;
    write_lock(&lock)?;
    println!(
        "Synced to {} ({}), layer {}.",
        lock.tag,
        &lock.commit[..9],
        lock.layer.map_or("?".into(), |l| l.to_string())
    );
    Ok(())
}

pub fn verify() -> Result<()> {
    let lock = read_lock()?;
    let mut bad = 0;
    for v in &lock.vendored {
        let data = fs::read(root().join(&v.path)).with_context(|| format!("reading {}", v.path))?;
        if sha256_hex(&data) != v.sha256 {
            eprintln!(
                "MISMATCH {} (edited by hand? run `cargo xtask upstream sync`)",
                v.path
            );
            bad += 1;
        }
    }
    let api = fs::read_to_string(root().join(API_TL_LOCAL))?;
    let layer = Schema::parse(&api)?.layer;
    if layer != lock.layer {
        eprintln!(
            "MISMATCH layer: lock says {:?}, schema/api.tl says {layer:?}",
            lock.layer
        );
        bad += 1;
    }
    if bad > 0 {
        bail!("{bad} problem(s) with files vendored per {LOCK_PATH}");
    }
    println!(
        "OK: {} files match tdesktop {} (layer {}).",
        lock.vendored.len(),
        lock.tag,
        layer.map_or("?".into(), |l| l.to_string())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(version_gt("v7.4.1", "v7.4"));
        assert!(version_gt("v7.10", "v7.9.9"));
        assert!(!version_gt("v7.4.0", "v7.4"));
        assert!(version_gt("7.3.4 beta (29.10.26)", "v7.3.3"));
        assert!(!version_gt("garbage", "v1"));
    }

    #[test]
    fn map_parses() {
        let map: AreaMap =
            toml::from_str(&fs::read_to_string(root().join(MAP_PATH)).unwrap()).unwrap();
        assert!(!map.area.is_empty());
        assert!(!map.submodule.is_empty());
    }
}
