//! Markdown rendering of the upstream report. Data is collected once and rendered twice:
//! in full (saved to `upstream/reports/`) and compact (PR body, limited by GitHub).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context, Result};
use tdr_tl_parser::{Change, Schema};

use crate::github::{Compare, Release};
use crate::upstream::{AreaMap, version_gt};

pub struct ReportData {
    pub repo: String,
    pub base: String,
    pub head: String,
    pub releases: Vec<Release>,
    pub changelog: String,
    pub old_api: String,
    pub new_api: String,
    pub old_lang: String,
    pub new_lang: String,
    pub map: AreaMap,
    pub compare: Compare,
    pub submodules: Vec<SubmoduleChange>,
}

pub enum SubmoduleChange {
    Bumped {
        repo: String,
        ours: String,
        old: String,
        new: String,
        compare: Compare,
    },
    Note(String),
}

pub struct Limits {
    schema_changes: usize,
    lang_added: usize,
    lang_removed: usize,
    commits: usize,
    submodule_commits: usize,
}

impl Limits {
    pub const FULL: Limits = Limits {
        schema_changes: usize::MAX,
        lang_added: usize::MAX,
        lang_removed: usize::MAX,
        commits: usize::MAX,
        submodule_commits: usize::MAX,
    };
    pub const COMPACT: Limits = Limits {
        schema_changes: 60,
        lang_added: 60,
        lang_removed: 20,
        commits: 60,
        submodule_commits: 15,
    };
}

/// Appends "… and N more" when `total` exceeds `shown`.
fn more(r: &mut String, total: usize, shown: usize) -> std::fmt::Result {
    if total > shown {
        writeln!(r, "- … and {} more", total - shown)?;
    }
    Ok(())
}

pub fn render(d: &ReportData, l: &Limits, full_report_path: Option<&str>) -> Result<String> {
    let mut r = String::new();
    let (base, head) = (&d.base, &d.head);

    writeln!(r, "# tdesktop {base} → {head}\n")?;
    if let Some(path) = full_report_path {
        writeln!(r, "> Compact version. The full report is in `{path}`.\n")?;
    }
    writeln!(r, "New releases since the last sync:\n")?;
    for rel in &d.releases {
        let date = rel
            .published_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .unwrap_or("?");
        let beta = if rel.prerelease { " *(beta)*" } else { "" };
        writeln!(r, "- [{}]({}){beta} — {date}", rel.tag_name, rel.html_url)?;
    }

    write_changelog(&mut r, &d.changelog, base)?;
    write_schema(&mut r, &d.old_api, &d.new_api, l)?;
    write_lang(&mut r, &d.old_lang, &d.new_lang, l)?;
    write_areas(&mut r, &d.compare, &d.map)?;
    write_submodules(&mut r, &d.submodules, l)?;
    write_commits(&mut r, &d.compare, &d.repo, l)?;

    writeln!(
        r,
        "\n---\nFull diff: https://github.com/{}/compare/{base}...{head}",
        d.repo
    )?;
    Ok(r)
}

/// Hard cap for bodies with a length limit; cuts at a line boundary.
pub fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let note = "\n\n_…truncated: see the full report._\n";
    let mut cut = max - note.len();
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let cut = s[..cut].rfind('\n').unwrap_or(cut);
    s.truncate(cut);
    s.push_str(note);
    s
}

fn write_changelog(r: &mut String, changelog: &str, since: &str) -> Result<()> {
    writeln!(r, "\n## Changelog\n")?;
    let sections = changelog_since(changelog, since);
    if sections.is_empty() {
        writeln!(r, "_No changelog entries found._")?;
    }
    for (header, body) in sections {
        writeln!(r, "### {header}\n\n{}\n", body.trim())?;
    }
    Ok(())
}

/// Changelog sections (header, body) for versions newer than `since`.
pub fn changelog_since<'a>(changelog: &'a str, since: &str) -> Vec<(&'a str, String)> {
    let mut out: Vec<(&str, String)> = Vec::new();
    let mut current: Option<(&str, String)> = None;
    for line in changelog.lines() {
        let is_header = line.contains('(')
            && line
                .split_whitespace()
                .next()
                .and_then(crate::upstream::parse_version)
                .is_some();
        if is_header {
            if let Some(sec) = current.take() {
                out.push(sec);
            }
            if !version_gt(line, since) {
                break; // The changelog is newest-first.
            }
            current = Some((line.trim(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out.extend(current);
    out
}

fn join_params(params: &[&tdr_tl_parser::Param]) -> String {
    params
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn write_schema(r: &mut String, old: &str, new: &str, l: &Limits) -> Result<()> {
    let old = Schema::parse(old).context("parsing local schema/api.tl")?;
    let new = Schema::parse(new).context("parsing upstream api.tl")?;
    let d = tdr_tl_parser::diff(&old, &new);

    let layer = |l: Option<u32>| l.map_or("?".into(), |l| l.to_string());
    writeln!(r, "\n## TL schema (api.tl)\n")?;
    if d.is_empty() {
        writeln!(r, "No changes (layer {}).", layer(new.layer))?;
        return Ok(());
    }
    writeln!(
        r,
        "Layer {} → {}: **{}** added, **{}** changed, **{}** removed.\n",
        layer(d.old_layer),
        layer(d.new_layer),
        d.count(|c| matches!(c, Change::Added(_))),
        d.count(|c| matches!(c, Change::Changed { .. })),
        d.count(|c| matches!(c, Change::Removed(_))),
    )?;
    writeln!(r, "<details><summary>Schema changes</summary>\n\n```diff")?;
    for c in d.changes.iter().take(l.schema_changes) {
        match c {
            Change::Added(def) => writeln!(r, "+ {def}")?,
            Change::Removed(def) => writeln!(r, "- {def}")?,
            Change::Changed {
                old,
                new,
                added_params,
                removed_params,
            } => {
                writeln!(r, "- {old}\n+ {new}")?;
                if !added_params.is_empty() {
                    writeln!(r, "#   new params: {}", join_params(added_params))?;
                }
                if !removed_params.is_empty() {
                    writeln!(r, "#   dropped params: {}", join_params(removed_params))?;
                }
            }
        }
    }
    if d.changes.len() > l.schema_changes {
        writeln!(r, "# … and {} more", d.changes.len() - l.schema_changes)?;
    }
    writeln!(r, "```\n</details>")?;
    Ok(())
}

/// `"lng_key" = "value";` pairs from a tdesktop `lang.strings` file.
pub fn parse_lang(text: &str) -> BTreeMap<&str, &str> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('"')?;
            let (key, rest) = rest.split_once('"')?;
            let value = rest.trim_start().strip_prefix('=')?.trim();
            let value = value.strip_suffix(';').unwrap_or(value).trim();
            let value = value.strip_prefix('"').unwrap_or(value);
            let value = value.strip_suffix('"').unwrap_or(value);
            Some((key, value))
        })
        .collect()
}

fn write_lang(r: &mut String, old: &str, new: &str, l: &Limits) -> Result<()> {
    let (old, new) = (parse_lang(old), parse_lang(new));
    let added: Vec<_> = new.iter().filter(|(k, _)| !old.contains_key(*k)).collect();
    let removed: Vec<_> = old.keys().filter(|k| !new.contains_key(*k)).collect();
    let changed = new
        .iter()
        .filter(|(k, v)| old.get(*k).is_some_and(|o| o != *v))
        .count();

    writeln!(r, "\n## Localization (lang.strings)\n")?;
    writeln!(
        r,
        "**{}** added, **{changed}** changed, **{}** removed. \
         New strings are the quickest hint of new user-facing features.",
        added.len(),
        removed.len()
    )?;
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    writeln!(r, "\n<details><summary>Added / removed keys</summary>\n")?;
    for (k, v) in added.iter().take(l.lang_added) {
        let v: String = v.chars().take(100).collect();
        writeln!(
            r,
            "- `+ {k}` — {}",
            v.replace('|', "\\|").replace('<', "&lt;")
        )?;
    }
    more(r, added.len(), l.lang_added)?;
    for k in removed.iter().take(l.lang_removed) {
        writeln!(r, "- `- {k}`")?;
    }
    more(r, removed.len(), l.lang_removed)?;
    writeln!(r, "\n</details>")?;
    Ok(())
}

fn write_areas(r: &mut String, cmp: &Compare, map: &AreaMap) -> Result<()> {
    #[derive(Default)]
    struct Stat {
        files: u64,
        add: u64,
        del: u64,
    }
    let mut stats: BTreeMap<usize, Stat> = BTreeMap::new();
    let mut unmapped: BTreeMap<&str, u64> = BTreeMap::new();
    for f in &cmp.files {
        // The longest matching prefix wins.
        let area = map
            .area
            .iter()
            .enumerate()
            .flat_map(|(i, a)| a.paths.iter().map(move |p| (i, p)))
            .filter(|(_, p)| f.filename.starts_with(p.as_str()))
            .max_by_key(|(_, p)| p.len())
            .map(|(i, _)| i);
        match area {
            Some(i) => {
                let s = stats.entry(i).or_default();
                s.files += 1;
                s.add += f.additions;
                s.del += f.deletions;
            }
            None => {
                let dir = f.filename.rsplit_once('/').map_or("(root)", |(d, _)| d);
                *unmapped.entry(dir).or_default() += 1;
            }
        }
    }

    writeln!(r, "\n## Affected areas\n")?;
    let truncated = cmp.files.len() >= 300;
    writeln!(
        r,
        "{} commits, {} files changed{}.\n",
        cmp.total_commits,
        cmp.files.len(),
        if truncated {
            " (GitHub truncates the file list at 300)"
        } else {
            ""
        }
    )?;
    if !stats.is_empty() {
        writeln!(r, "| Area | Our crates | Files | Lines + / − |")?;
        writeln!(r, "|---|---|---:|---:|")?;
        let mut rows: Vec<_> = stats.into_iter().collect();
        rows.sort_by_key(|(_, s)| std::cmp::Reverse((s.files, s.add + s.del)));
        for (i, s) in rows {
            let a = &map.area[i];
            // GitHub omits line stats for big diffs.
            let lines = if s.add + s.del == 0 {
                "n/a".to_string()
            } else {
                format!("+{} / −{}", s.add, s.del)
            };
            writeln!(r, "| {} | `{}` | {} | {lines} |", a.name, a.ours, s.files)?;
        }
    }
    if !unmapped.is_empty() {
        let list: Vec<_> = unmapped
            .iter()
            .map(|(d, n)| format!("`{d}` ({n})"))
            .collect();
        writeln!(
            r,
            "\nNot covered by `upstream/map.toml` (consider adding): {}",
            list.join(", ")
        )?;
    }
    Ok(())
}

fn write_submodules(r: &mut String, subs: &[SubmoduleChange], l: &Limits) -> Result<()> {
    writeln!(r, "\n## Submodules (desktop-app libraries)\n")?;
    if subs.is_empty() {
        writeln!(r, "No tracked submodules changed.")?;
    }
    for s in subs {
        match s {
            SubmoduleChange::Note(n) => writeln!(r, "- {n}")?,
            SubmoduleChange::Bumped {
                repo,
                ours,
                old,
                new,
                compare,
            } => {
                writeln!(
                    r,
                    "- **[{repo}](https://github.com/{repo}/compare/{}...{})** → `{ours}`: \
                     {} commits, {} files",
                    &old[..9],
                    &new[..9],
                    compare.total_commits,
                    compare.files.len()
                )?;
                for c in compare.commits.iter().take(l.submodule_commits) {
                    let subject = c.commit.message.lines().next().unwrap_or("");
                    writeln!(r, "  - {}", subject.replace('<', "&lt;"))?;
                }
                if compare.commits.len() > l.submodule_commits {
                    writeln!(
                        r,
                        "  - … and {} more",
                        compare.commits.len() - l.submodule_commits
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn write_commits(r: &mut String, cmp: &Compare, repo: &str, l: &Limits) -> Result<()> {
    writeln!(
        r,
        "\n<details><summary>Commits ({})</summary>\n",
        cmp.total_commits
    )?;
    // Newest first is more useful when the list is cut.
    for c in cmp.commits.iter().rev().take(l.commits) {
        let subject = c.commit.message.lines().next().unwrap_or("");
        writeln!(
            r,
            "- [`{}`](https://github.com/{repo}/commit/{}) {}",
            &c.sha[..9],
            c.sha,
            subject.replace('<', "&lt;")
        )?;
    }
    more(r, cmp.commits.len(), l.commits)?;
    writeln!(r, "\n</details>")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_sections() {
        let log =
            "7.4.1 (06.11.26)\n\n- Fix A.\n\n7.4 (05.11.26)\n\n- B.\n\n7.3.5 (03.11.26)\n\n- C.\n";
        let s = changelog_since(log, "v7.3.5");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0, "7.4.1 (06.11.26)");
        assert!(s[1].1.contains("- B."));
        assert!(changelog_since(log, "v7.4.1").is_empty());
    }

    #[test]
    fn lang_strings() {
        let m = parse_lang("\"lng_a\" = \"Hello\";\n// c\n\"lng_b\" = \"Say \\\"hi\\\"\";\n");
        assert_eq!(m["lng_a"], "Hello");
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn truncation() {
        let s = "line one\nline two\nline three\n".repeat(10);
        let t = truncate(s.clone(), 100);
        assert!(t.len() <= 100);
        assert!(t.ends_with("full report._\n"));
        assert_eq!(truncate(s.clone(), 10_000), s);
    }
}
