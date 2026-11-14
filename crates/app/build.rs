//! * Embeds the Telegram API credentials (environment variables `TDR_API_ID` /
//!   `TDR_API_HASH`, or the git-ignored `.env` in the workspace root).
//! * On Windows, embeds the app icon as resource 1, where GPUI looks for the window icon.

use std::path::Path;
use std::{env, fs};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env_file = root.join(".env");
    println!("cargo:rerun-if-changed={}", env_file.display());
    println!("cargo:rerun-if-env-changed=TDR_API_ID");
    println!("cargo:rerun-if-env-changed=TDR_API_HASH");
    let from_file = fs::read_to_string(&env_file).unwrap_or_default();
    for key in ["TDR_API_ID", "TDR_API_HASH"] {
        let value = env::var(key).ok().filter(|v| !v.is_empty()).or_else(|| {
            from_file.lines().find_map(|line| {
                let (k, v) = line.split_once('=')?;
                (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
            })
        });
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            println!("cargo:rustc-env={key}={v}");
        }
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = root.join("assets/icons/app-icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let rc = Path::new(&env::var("OUT_DIR").unwrap()).join("app.rc");
        let icon_path = icon
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
            .replace('\\', "\\\\");
        let icon_path = icon_path.trim_start_matches("\\\\\\\\?\\\\");
        fs::write(&rc, format!("1 ICON \"{icon_path}\"\n")).unwrap();
        embed_resource::compile(&rc, embed_resource::NONE)
            .manifest_optional()
            .expect("embed the app icon");
    }
}
