//! Project automation. Run with `cargo xtask <command>`.
//!
//! ```text
//! cargo xtask upstream check  [--channel stable|beta] [--save] [--report FILE] [--pr-body FILE] [--github-output]
//! cargo xtask upstream sync   [--channel stable|beta] [--tag vX.Y.Z]
//! cargo xtask upstream verify
//! ```

mod github;
mod report;
mod upstream;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage:
  cargo xtask upstream check  [--channel stable|beta] [--save] [--report FILE] [--pr-body FILE] [--github-output]
      Is there a tdesktop release newer than upstream/upstream.lock.toml? If so, build a report:
      changelog, TL schema diff, new localization strings, affected areas, submodules, commits.
        --save           write the full report to upstream/reports/<tag>.md
        --report FILE    write the full report to FILE
        --pr-body FILE   write the compact report (fits a GitHub PR body) to FILE
        --github-output  export update/tag/title for GitHub Actions
  cargo xtask upstream sync   [--channel stable|beta] [--tag vX.Y.Z]
      Re-vendor schema files from the newest (or given) release and update the lock.
  cargo xtask upstream verify
      Check that vendored files match the checksums in the lock.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["upstream", "check", rest @ ..] => upstream::check(rest),
        ["upstream", "sync", rest @ ..] => upstream::sync(rest),
        ["upstream", "verify"] => upstream::verify(),
        [] | ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            bail!("unknown command: {}", args.join(" "))
        }
    }
}
