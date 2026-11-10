# Notes for AI assistants

- Rust port of Telegram Desktop (tdesktop). Plan and decisions: `docs/ARCHITECTURE.md`;
  upstream tracking: `docs/UPSTREAM.md`. Docs are in Russian, code and comments in English.
- Workspace members in `Cargo.toml` are listed explicitly: cargo glob patterns fail when
  the checkout path contains `[` / `]` (the author's local folder does). Add new crates there.
- Crate names use the `tdr-` prefix, directories under `crates/` drop it.
- `schema/*.tl` are byte-exact copies from tdesktop; never edit them. Use
  `cargo xtask upstream sync`; `cargo xtask upstream verify` runs in CI.
- Before finishing: `cargo fmt --all`, `cargo clippy --workspace --all-targets`
  (CI uses `-D warnings`), `cargo test --workspace`.
- License is GPL-3.0-or-later; porting tdesktop code is allowed and expected.
- Never commit `api_id`/`api_hash`; they come from environment variables at build time.
