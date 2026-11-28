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
- GPUI 0.2.2's `layout_line` keeps the previous run's font while color, underline
  and strikethrough stay the same: give runs of another font a different color
  (`rich_text::separate_fonts`, alpha off by 1/512) or bold after regular shapes regular.
- The message field (`text_input`) keeps tags (tdesktop's `**`, `__`, ``` etc.) and
  paints through `rich_text::RichText::paint_at`; its menu is `open_field_menu`
  (`ui/menus.rs`). Synthetic Ctrl chords for demo checks need the modifier held
  (`keybd_event` with pauses): `SendKeys "^b"` reaches GPUI as a plain "b". Bindings
  of punctuation depend on the layout; Ctrl+Shift+. is also matched by its physical
  key (`on_key_down`).
- Window controls (folder column, chat search field, "Connecting..." box, chat header
  and message field buttons) live in `ui/chrome.rs`; folders come from
  `tdr_data::filters` (rules and icons ported) through `bridge` (`Update::Folders`,
  `Command::SelectFolder`). Demo flags: `--folders`, `--connecting`.
  `tdr probe folders` prints folder rule sizes and counts (no titles).
- Accounts: `accounts.rs` (registry `data/accounts.txt`, account 1 in `data/`, others in
  `data/accounts/<id>/`; `Accounts` runs a bridge per account and routes updates),
  `root.rs` (the window root, rebuilds `MainView` on switch), `intro.rs` (login screens,
  driven by `tdr_data::login::run`), `ui/main_menu.rs`. Demo: `--accounts` (three fake
  accounts), `--intro` (the login screens; the demo walks through code and password).
  `tdr probe login` checks the login flow (event kinds only).
- Layers (`ui/layers.rs`, tdesktop's `LayerStackWidget`): the main menu, Settings
  (`ui/settings.rs`, the special layer) and boxes (About, the logout confirmation)
  with the stack's animations; they are drawn at the view's root, below the title
  bar. Animation helpers: `anim.rs` (`Simple`, easings). Demo: `--start` (the first
  launch's start screen), `--intro` (the login of "Add Account").
- Countries and phone numbers: `tdr_data::countries` (tdesktop's built-in list in
  `countries_fallback.rs`, generated from `countries_instance.cpp`; `by_code` keeps
  the last country of a shared code like `QHash`). The login's country list and
  settings are layers of `MainView` (`ui/country_box.rs`, `render_intro_settings`),
  asked for through `IntroEvent`.
- Adaptive layout (`Window::Adaptive`): `MainView::one_column` (body under 640 px
  without the folder column), `filters_vertical` (window from 452 px, else folder
  tabs `render_folder_tabs`), `column_slide` (list <-> chat in one column);
  Settings is a section in one column (`render_settings_section`). The window's
  width at the last frame is `window_w`. Demo checks resize the window with
  `MoveWindow` (see the scratchpad's `ad1.ps1`).
- Links to the repository come only from `REPO_URL` (`main.rs`, from Cargo.toml's
  `repository`) through `ui::layers::repo_url`.
- Demo click coordinates are logical: screenshots are device pixels (divide by the
  scale, 1.25 here).
- Demo checks must not run while the user's own window is in front: `uilib.ps1`'s
  `Shot` guards too, a screenshot never captures anything but the demo window.
- GPUI text input: typed characters reach an element through its input handler
  (`window.handle_input` + `EntityInputHandler`), not `KeyDownEvent::key_char` alone
  (IME, unicode SendInput); key listeners handle only the command keys.
- GPUI 0.2.2 samples atlas images with linear filtering: at a fractional scale an image
  touching its edge bleeds its atlas neighbour (a stripe). `icons` pads every mask.
- Drags (scrollbars, the column edge) follow the mouse through window-level listeners
  and a full-window capture layer (`render_drag_capture`), not the element's own
  `on_mouse_move`, which stops when the mouse leaves the element.
- Sizes in `ui` go through `scale::px` (tdesktop's `ConvertScale`: every style size
  becomes whole device pixels on its own) and fonts through `scale::font` (Qt's
  rounded ascent + descent as the line height). Use `gpui::px` only for values
  already in logical pixels. Times on screen are shifted by the server clock offset
  (`present::set_server_time_offset`), dates use the system's short formats.
- Message text is our own element (`rich_text`: entities -> spans and quote
  paragraphs, wrapping, painting, hit tests); `ui/message_text.rs` keeps the layout
  cache, selection, link clicks and toasts. Spoiler particles (`spoiler`) and toast
  icons (`lottie`, a tiny Lottie player) are generated once per scale/color.
- One running copy per data folder (`instance`, a named pipe); `--demo` is exempt.
- Menus: `popup_menu` draws `Ui::PopupMenu` (geometry, unroll animation, ripple)
  for any area; `ui/menus.rs` builds the chat's menus, `tray_menu` shows the same
  menu in a frameless transparent popup window. Its colors are `defaultMenu`'s
  window colors, not `menuBg*` (the night theme's `menuBgOver` is white).
  `--demo --tray-menu` opens the tray menu for screenshots. The popup window is
  `Opaque`: GPUI's `Transparent` adds a tinted Windows backdrop (the frame is cleared
  to transparent either way).
- Code blocks are highlighted by `tdr-prism` (libprisma ported: Prism grammars from
  `assets/libprisma/grammars.dat`, JS regexes translated for `fancy-regex`) on the
  data thread (`bridge::add_syntax`, cached); colors are `Kind::Syntax(1..=8)`.
- `uniform_list` lays out each item as its own root, where a `top` offset is
  ignored: animate an item inside a fixed-size wrapper.
- Live checks against Telegram use the user's session in `data/` (git-ignored):
  `tdr me`, `tdr selftest reconnect|unknown-key|dc N`. Run cargo with
  `--manifest-path` if the shell cannot `cd` into the bracketed folder.
- Channels: posts (`MessageItem.post`: views, signature, comments) are parsed in
  `describe::message_info`; the bottom of a chat that can't be written to is
  `DialogItem.compose` (`Compose::of`, drawn by `render_compose_bar` in `ui/chrome.rs`).
  `--demo --open=4` opens the demo channel.
- Archive: `ui/archive.rs` (its row over "All chats", its menu, its list with the slide);
  the bridge sends `Update::Archive` and the archive's list as `Update::Dialogs` of
  `ARCHIVE_LIST` (-1) while it is open. The per-account "in main menu" setting lives in
  the cache's `meta` (`tdr_data::ARCHIVE_IN_MENU_KEY`). `one_line(..).marked(ranges,
  color)` draws ranges semibold in a color.
- Focus: `root.rs` gives the focus back to `MainView` when the focused element goes
  away (`on_focus_lost`), so window-wide actions (Esc) always reach the view.
- Full info of supergroups/channels (`channelFull`) is stored as a blob in `peer_full`
  (`Storage::peer_full`, `describe::full_info`), loaded by `Sync::load_full` on open.
  A chat's loaded history hangs on its `dialogs` row: a chat without one gets it before
  (or, failing that, from) its first history page.
- The chat's "⋮" menu is `open_chat_menu` (`ui/menus.rs`, `fillHistoryActions`'s
  rules) from `DialogItem.rights`; `popup_menu` actions can have a submenu and a grey
  text at the right. The login screens and the account slide over each other through
  `MainView::intro_slide` (`Root::rebuild` hands over the leaving login view).
- Demo test scripts: `uilib.ps1`'s `Click` waits 350 ms after the press; to catch an
  animation mid-way, press without the wait and shoot at once.
- Replies: `MessageItem.reply` (`bridge::reply_line`; missing replied messages are
  asked for by `resolve_replies` at the top of the data loop), drawn by
  `render_reply_box`; the field's reply bar is `render_reply_bar` (`MainView::reply_to`).
- Threads (comments, forum topics) are `Shown::Thread` in the bridge and
  `MainView::thread` in the UI (`ui/threads.rs`); `Update::History.thread` says
  whose history it is. Forums open their topics list (`MainView::forum`) beside the
  chat list narrowed to userpics. Demo: Telegram Tips has comments, Design Team is a
  forum.
- Markdown typed by hand is applied on send by `markdown.rs` (tdesktop's
  `getTextWithAppliedMarkdown`), called from `text_input`'s `enter`; the field keeps
  the raw text. Animated switches: forum open/close (`MainView::forum_hiding`,
  `forum_shown`), folder tabs (`FolderSlide`, `folder_slide_view`), main menu
  accounts (`menu_accounts_toggled`).
- Emoji are tdesktop's sprite images (`assets/emoji`), never the system font: the list
  and text search are `tdr-emoji` (lib_ui's `emoji.txt` prepared like codegen's
  `emoji/data.cpp`; the index is the sprite position), images `emoji.rs`. In shaped
  text an emoji becomes a space of the in-memory font `emoji_font` (same byte
  length, fillers): use `emoji::shape_text` for any text you shape and paint the
  images with `emoji::paint`; snap hit-test indices with `emoji::snap`. The panel is
  `ui/emoji_panel.rs` (`Area::Emoji` scroll), recent emoji and skin tones
  `tdr_emoji::recent` (`data/emoji.txt`).
- GPUI repaints the whole window every frame, and on the author's laptop (AMD Radeon
  Vega 3) even an empty frame at 60 fps fills the GPU: continuous animations must not
  call `request_animation_frame` every vsync. Spoilers and custom emoji ask
  `repaint::at` for their next frame (a ticker in `MainView::new` notifies then),
  animations pause while the window is inactive, and custom emoji keep only the frame
  on screen as a GPU image (`custom_emoji`: an instance per emoji and size, frames
  packed in memory, rendered ahead by workers that serve the last painted first,
  old images dropped from the atlas each frame). `tdr probe emoji-speed` times
  downloads, decoding and rendering on real sets (counts and timings only). Performance switches: `TDR_PROFILE=1` prints timings to stderr,
  `TDR_OFF=tails,spoilers,emoji,shadows,text,view` turns parts off (`profile.rs`).
- Never call `window.paint_path`: GPUI 0.2.2 draws every batch of paths through a
  window-sized 4x MSAA texture (cleared and resolved per batch), and paths between
  other primitives are a batch each, so a tail per message made every frame cost
  tens of milliseconds on the Vega 3. Strokes go through `vector::paint_strokes`
  (tiny-skia, cached images dropped from the atlas when unused), filled shapes are
  cached images (`icons::bubble_tail_image`).
- Emoji while typing: instant replaces and short codes are `tdr_emoji::suggestions`
  (lib_ui's `emoji_suggestions` + codegen's replaces), language keywords
  `tdr_emoji::keywords` (fetched by `tdr_data::emoji_keywords`, kept in
  `data/emoji-keywords/`). The field (`text_input`) finds the query and forwards
  Enter/Tab/arrows/Esc to the shared `emoji_suggest::Suggest`; `MainView` fills its
  rows and draws them (`ui/emoji_suggest_box.rs`).
- GPUI is vendored in `vendor/gpui` (0.2.2 plus `tdr patch` changes listed in
  `vendor/gpui/TDR-PATCHES.md`: atlas tiles deallocated, Segment Heap in its manifest).
  It is excluded from the workspace and from rustfmt; change it only with a marked patch.
- Memory: `mapped_store` keeps rendered custom emoji frames and cut emoji images in
  memory-mapped files under `data/cache` (`cache-demo` for the demo), outside the
  process's private memory. `TDR_MEMORY=1` prints heap and holders every 5 s
  (`memory.rs`, a counting global allocator), `TDR_MEMORY=big` the stacks of blocks
  of 4 MB and more. Demo load: `TDR_DEMO_EMOJI=N` (copies of the demo animations) or
  `real` (custom emoji from the file cache, read only). Measure the private working set
  (Task Manager's "Memory") with `Win32_PerfFormattedData_PerfProc_Process`.
- Clipboard: `clipboard.rs` writes and reads tdesktop's `application/x-td-field-text` /
  `-tags` formats (copy from messages and the field keeps formatting and custom emoji).
  Demo paste checks overwrite the user's clipboard: save and restore it, skip if it holds
  anything but text.
- PowerShell paths with `[` `]` (this folder) need `-LiteralPath`; copy files with bash.
- Custom emoji show only once every frame is rendered (silhouette until then; asked by
  the user), workers render one emoji to its end in on-screen reading order, and the
  long press preview (`custom_emoji::focus`) goes first, on all cores. `tdr-lottie`
  keeps numbers in `f32` (as rlottie); check renderer changes against reference frames
  with the ignored test `memory::tests::lottie_reference` (`LOTTIE_REF`, `LOTTIE_MODE`).
- Reactions: `describe::Reactions` (parsed, `updateMessageReactions` patches the cached
  message), `tdr_data::reactions` (available/top/send), `Sync::toggle_reaction` (local
  echo, then `messages.sendReaction`), `bridge::MessageReactions` on `MessageItem`; the UI
  is `ui/reactions.rs` (pills under the text, the strip over a message's menu). Reaction
  icons are documents downloaded like custom emoji (`known_docs` in the bridge) and drawn
  with `custom_emoji::paint_still` (first frame). Demo: Alice and the groups have reactions.
- Reaction animations: `ReactionFx` in `ui/reactions.rs` (`diff_reactions` from
  `update_list`: pills grow, go away keeping their place until gone, counts slide; `Fly`
  is `ReactionFlyAnimation`, painted by a `deferred` canvas). One-shot document
  animations are `Clip` (`custom_emoji::paint_frame`/`timeline` at a fixed side, drawn
  scaled: never re-render per hover size). The strip grows with the menu
  (`PopupMenu::appear_state`) and unrolls into the full panel (`expand_strip`,
  `render_strip_panel`); the corner button is `ui/corner_reaction.rs` (`Manager`/`Button`
  states, shown 300 ms after the mouse rests on a bubble). `TDR_DEMO_REACTIONS=anim`
  gives the demo's first reactions the demo animations (strip/fly checks).

