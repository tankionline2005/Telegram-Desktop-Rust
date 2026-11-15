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
- Profiles: `release` is the fast everyday optimized build (incremental, no LTO);
  `dist` (`--profile dist`) is the shipping build with fat LTO. Keep generic code thin:
  e.g. `tdr_api::Client::invoke<R>` only (de)serializes and calls the non-generic
  `invoke_raw`, so per-request-type monomorphization stays small.
- `tdr-updates` is a pure state machine (no I/O): keep it that way and test it there;
  network and storage glue lives in `tdr-data`. Its test checks that every sequenced
  `Update` in the schema is classified — extend `classify` when a new layer adds one.
- Cache rows store our own columns plus the TL blob tagged with its layer, so schema
  updates never break the cache. Don't print cached message texts or chat names in
  tool output: they are the user's private data.
- Run clippy as `cargo clippy --workspace --all-targets -- -D warnings`: setting
  `RUSTFLAGS=-D warnings` locally changes every dependency's fingerprint and rebuilds
  GPUI (tens of minutes).
- UI checks: run the app with `--demo [--light|--dark] [--open=N]` (fake chats) and
  capture only its window (bring it to front, copy its rect; `PrintWindow` renders the
  custom-frame window wrongly); never screenshot the real mode (the user's chats).
  Synthetic mouse/keyboard input (SendKeys, mouse_event) is allowed for demo checks,
  but before every input action verify that the foreground window belongs to the demo
  process (`GetForegroundWindow` + `GetWindowThreadProcessId`) and abort otherwise:
  `SetForegroundWindow` can silently fail, and the input then lands in the user's own
  window (once it typed into the user's real Telegram client). Maximize the window for
  tests.
- GPUI 0.2.2 applies `hover(...)` styles at paint time, after text is shaped, so a
  hover `text_color` never shows; track the hover with `on_hover` instead.
- GPUI dispatches keys along the focused element's path, or to the view root only when
  nothing is focused: the main view focuses itself on start; window-wide actions (Esc)
  sit on the root element.
- Panics are appended to `data/crash.log` (the release build has no console). It may
  contain the user's text: read only the location lines.
- Demo runs keep their window placement in `data/window-demo.txt`, the real app in
  `data/window.txt`. Files that must survive a crash are written with
  `tdr_api::write_atomic`.
- Message/peer details for the UI (media kind, service action, edited, peer kind, color,
  members, status, userpic) are parsed from the cached TL blobs by `tdr_data::describe`;
  the English strings with tdesktop's rules live in `crates/app/src/present.rs`.
- Proven improvements over tdesktop go to `docs/IMPROVEMENTS.md`.
- Drafts: `tdr_data::drafts` is a pure state machine ported from tdesktop, with one
  intended difference (an empty cloud draft never wipes the local one), see
  `docs/ARCHITECTURE.md`.
- Give the user the path of the built exe (`target/release/unofficial-telegram-desktop-rust.exe`)
  after each build: they test it directly instead of rebuilding.
- Never put `track_focus` on an ancestor of the title bar: focusable elements
  prevent_default mouse downs and GPUI then skips the native caption buttons.
- GPUI 0.2.2 measures wrapped text at a different width than it paints it: message
  bubbles get an explicit width from `text_width` (see `render_message`).
- GPUI 0.2.2 never truncates `nowrap` text (stale measure cache), and its
  `text_ellipsis()` panics on multi-byte text (it shortens runs in place across measure
  passes): never use either; use `one_line::one_line` for cut single lines.
- Live checks against Telegram use the user's session in `data/` (git-ignored):
  `tdr me`, `tdr selftest reconnect|unknown-key|dc N`. Run cargo with
  `--manifest-path` if the shell cannot `cd` into the bracketed folder.
