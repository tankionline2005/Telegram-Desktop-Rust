//! Unofficial Telegram Desktop Rust — the desktop application.
//!
//! The window (GPUI) runs on the main thread; network and cache live on the data thread
//! (`bridge`). The account is the one logged in with the console client (`tdr login`):
//! both use `data/session.bin` and `data/cache.sqlite` in the workspace for now.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod text_input;
mod ui;

use std::borrow::Cow;
use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowAppearance, WindowBounds,
    WindowOptions, px, size,
};
use tdr_api::Config;

const APP_NAME: &str = "Unofficial Telegram Desktop Rust";

/// Process start, for startup timing logs.
pub static STARTED: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn config() -> Option<Config> {
    let id = option_env!("TDR_API_ID")?.parse().ok()?;
    let hash = option_env!("TDR_API_HASH")?;
    let mut c = Config::new(id, hash);
    c.app_version = env!("CARGO_PKG_VERSION").into();
    Some(c)
}

fn main() {
    std::sync::LazyLock::force(&STARTED);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TDR_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let demo = std::env::args().any(|a| a == "--demo");
    let force_dark = std::env::args().any(|a| a == "--dark");
    let force_light = std::env::args().any(|a| a == "--light");
    let open_on_start = std::env::args().find_map(|a| {
        a.strip_prefix("--open=")
            .and_then(|n| n.parse::<usize>().ok())
    });
    let (commands, updates) = if demo {
        bridge::start_demo()
    } else {
        let Some(config) = config() else {
            eprintln!("API credentials are missing: fill in .env (see .env.example) and rebuild");
            std::process::exit(1);
        };
        bridge::start(
            config,
            bridge::Paths {
                session: data_dir().join("session.bin"),
                cache: data_dir().join("cache.sqlite"),
            },
        )
    };

    Application::new().run(move |cx: &mut App| {
        tracing::info!(elapsed = ?STARTED.elapsed(), "ui: platform ready");
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(
                    include_bytes!("../../../assets/fonts/OpenSans-Regular.ttf").as_slice(),
                ),
                Cow::Borrowed(
                    include_bytes!("../../../assets/fonts/OpenSans-SemiBold.ttf").as_slice(),
                ),
            ])
            .expect("bundled fonts are valid");
        tracing::info!(elapsed = ?STARTED.elapsed(), "ui: fonts loaded");
        text_input::bind_keys(cx);

        let dark = !force_light
            && (force_dark
                || matches!(
                    cx.window_appearance(),
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                ));
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    // The title is shown in the taskbar only: the window draws tdesktop's
                    // custom title bar itself.
                    title: Some(APP_NAME.into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                app_id: Some("unofficial-telegram-desktop-rust".into()),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = ui::MainView::new(commands, updates, dark, cx);
                    view.open_on_start = open_on_start.map(|n| n.saturating_sub(1));
                    view
                })
            },
        )
        .expect("open the main window");
        tracing::info!(elapsed = ?STARTED.elapsed(), "ui: window opened");
        cx.activate(true);
    });
}
