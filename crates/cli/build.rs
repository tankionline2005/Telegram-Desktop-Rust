//! Embeds the Telegram API credentials at build time.
//!
//! Sources, in order: environment variables `TDR_API_ID` / `TDR_API_HASH` (CI secrets),
//! then the git-ignored `.env` file in the workspace root. Without them the binary still
//! builds, but commands that talk to Telegram explain how to configure credentials.

use std::path::Path;
use std::{env, fs};

fn main() {
    let env_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    println!("cargo:rerun-if-changed={}", env_file.display());
    println!("cargo:rerun-if-env-changed=TDR_API_ID");
    println!("cargo:rerun-if-env-changed=TDR_API_HASH");

    let from_file = fs::read_to_string(&env_file).unwrap_or_default();
    let lookup = |key: &str| -> Option<String> {
        env::var(key).ok().filter(|v| !v.is_empty()).or_else(|| {
            from_file.lines().find_map(|line| {
                let (k, v) = line.split_once('=')?;
                (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
            })
        })
    };
    for key in ["TDR_API_ID", "TDR_API_HASH"] {
        if let Some(v) = lookup(key).filter(|v| !v.is_empty()) {
            println!("cargo:rustc-env={key}={v}");
        }
    }
}
