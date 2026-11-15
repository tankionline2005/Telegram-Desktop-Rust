//! Unofficial Telegram Desktop Rust — the desktop application.
//!
//! The window (GPUI) runs on the main thread; network and cache live on the data thread
//! (`bridge`). The account is the one logged in with the console client (`tdr login`):
//! both use `data/session.bin` and `data/cache.sqlite` in the workspace for now.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod icons;
mod one_line;
mod placement;
mod present;
mod text_input;
#[cfg(windows)]
mod tray;
mod ui;
mod wallpaper;

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

/// Appends every panic (with a backtrace) to `data/crash.log`: the release build has no
/// console, and a panic inside a GPUI callback aborts the process.
fn install_crash_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let report = format!(
            "--- {} (+{:?} since start), thread '{}' ---
{info}
{backtrace}
",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            STARTED.elapsed(),
            thread.name().unwrap_or("?"),
        );
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir().join("crash.log"))
        {
            let _ = f.write_all(report.as_bytes());
        }
        previous(info);
    }));
}

/// Saves the window placement shortly after it stops changing.
fn watch_placement(path: PathBuf, window: &mut gpui::Window, cx: &mut gpui::Context<ui::MainView>) {
    let latest = std::rc::Rc::new(std::cell::Cell::new(None::<WindowBounds>));
    cx.observe_window_bounds(window, move |_, window, cx| {
        let first = latest.replace(Some(window.window_bounds())).is_none();
        if first {
            let latest = latest.clone();
            let path = path.clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if let Some(bounds) = latest.take() {
                    placement::save(&path, &bounds);
                }
            })
            .detach();
        }
    })
    .detach();
}

/// The tray icon; the window's close button hides it there, as in tdesktop.
#[cfg(windows)]
fn setup_tray(window: gpui::WindowHandle<ui::MainView>, placement_path: PathBuf, cx: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let hwnd = window
        .update(cx, |_, window, _| {
            match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => {
                    Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _))
                }
                _ => None,
            }
        })
        .ok()
        .flatten();
    let Some(hwnd) = hwnd else { return };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let Some(icon) = tray::Tray::new(hwnd, APP_NAME, tx) else {
        tracing::warn!("tray icon could not be created");
        return;
    };
    let icon = std::rc::Rc::new(std::cell::RefCell::new(Some(icon)));

    let save_path = placement_path.clone();
    let _ = window.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, move |window, _| {
            placement::save(&save_path, &window.window_bounds());
            tray::Tray::hide_window();
            false
        });
    });

    cx.spawn(async move |cx| {
        while let Some(event) = rx.recv().await {
            match event {
                tray::TrayEvent::Toggle => {
                    if tray::Tray::window_is_active() {
                        tray::Tray::hide_window();
                    } else {
                        tray::Tray::show_window();
                    }
                }
                tray::TrayEvent::Quit => {
                    let _ = window.update(cx, |_, window, _| {
                        placement::save(&placement_path, &window.window_bounds());
                    });
                    // Remove the icon now: a dead process leaves a ghost in the tray.
                    icon.borrow_mut().take();
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        }
    })
    .detach();
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
    install_crash_log();

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
                files: data_dir().join("files"),
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
        ui::bind_keys(cx);

        let dark = !force_light
            && (force_dark
                || matches!(
                    cx.window_appearance(),
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                ));
        // Demo runs (tests, screenshots) keep their own placement.
        let placement_path = data_dir().join(if demo {
            "window-demo.txt"
        } else {
            "window.txt"
        });
        let window_bounds = placement::load(&placement_path, cx).unwrap_or_else(|| {
            WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(720.)), cx))
        });
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
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
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = ui::MainView::new(commands, updates, dark, cx);
                        view.open_on_start = open_on_start.map(|n| n.saturating_sub(1));
                        // Keys (Esc) need a focused element from the start.
                        window.focus(&gpui::Focusable::focus_handle(&view, cx));
                        watch_placement(placement_path.clone(), window, cx);
                        cx.observe_window_activation(
                            window,
                            |view: &mut ui::MainView, window, _| {
                                let active = window.is_window_active();
                                #[cfg(windows)]
                                tray::Tray::window_activation_changed(active);
                                view.window_activation_changed(active);
                            },
                        )
                        .detach();
                        view
                    })
                },
            )
            .expect("open the main window");
        tracing::info!(elapsed = ?STARTED.elapsed(), "ui: window opened");
        #[cfg(windows)]
        setup_tray(window, placement_path, cx);
        cx.activate(true);
    });
}
