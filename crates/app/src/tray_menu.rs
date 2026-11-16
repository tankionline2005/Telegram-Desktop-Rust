//! The tray icon's menu, as in tdesktop (`Tray::createMenu`: a `Ui::PopupMenu` of
//! its own, `defaultPopupMenu`): "Open Telegram" / "Minimize to Tray", "Disable
//! notifications", "Quit Telegram". It is a borderless popup window with the menu
//! drawn in it, so it looks and animates like every other menu. A chosen item acts
//! once the menu has faded out (`Tray::addAction`'s `callbackLater`).

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point, px, size,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::popup_menu::{Entry, MenuAction, MenuColors, MenuEvent, PopupMenu};
use crate::tray::TrayEvent;

/// The theme the menu follows (the main window keeps it current).
pub static DARK: AtomicBool = AtomicBool::new(false);

/// Room around the menu for its shadow.
const MARGIN: f32 = 12.;

pub struct TrayMenu {
    menu: PopupMenu,
    events: UnboundedSender<TrayEvent>,
    chosen: Option<TrayEvent>,
}

impl TrayMenu {
    fn on_event(&mut self, event: MenuEvent, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let area = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        match self.menu.handle(event, area, window) {
            Some(MenuAction::TrayShow) => self.chosen = Some(TrayEvent::Show),
            Some(MenuAction::TrayHide) => self.chosen = Some(TrayEvent::Hide),
            Some(MenuAction::TrayQuit) => self.chosen = Some(TrayEvent::Quit),
            // Notifications come later.
            _ => {}
        }
        cx.notify();
    }
}

impl Render for TrayMenu {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.menu.gone() {
            if let Some(event) = self.chosen.take() {
                let _ = self.events.send(event);
            }
            window.remove_window();
            return div();
        }
        let area = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let colors = MenuColors::from(tdr_theme::palette(DARK.load(Ordering::Relaxed)));
        let view = cx.entity().downgrade();
        let menu = crate::popup_menu::render(
            &self.menu,
            &colors,
            area,
            window,
            None,
            move |event, window, cx| {
                let _ = view.update(cx, |this, cx| this.on_event(event, window, cx));
            },
        );
        div()
            .size_full()
            .font_family("Open Sans")
            .text_size(crate::scale::font(13.).size)
            .line_height(crate::scale::font(13.).line_height())
            .child(menu)
    }
}

/// Windows draws a frame (shadow, a line at the top, a border, round corners) around
/// popup windows: the menu draws its own corners and shadow.
fn no_frame(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmExtendFrameIntoClientArea,
        DwmSetWindowAttribute,
    };
    let Ok(RawWindowHandle::Win32(h)) = HasWindowHandle::window_handle(window).map(|h| h.as_raw())
    else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    // DWMWA_COLOR_NONE.
    let none: u32 = 0xFFFF_FFFE;
    unsafe {
        // No frame of Windows' own: its shadow around the whole window and the line
        // along its top (the menu draws its own shadow).
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            &DWMNCRP_DISABLED as *const _ as *const _,
            size_of_val(&DWMNCRP_DISABLED) as u32,
        );
        let _ =
            DwmExtendFrameIntoClientArea(hwnd, &windows::Win32::UI::Controls::MARGINS::default());
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &DWMWCP_DONOTROUND as *const _ as *const _,
            size_of_val(&DWMWCP_DONOTROUND) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &none as *const _ as *const _,
            size_of::<u32>() as u32,
        );
    }
}

/// Exactly at `rect` (device pixels) and above everything, the taskbar's popups
/// included (`Platform::ShowOverAll`).
fn place_on_top(window: &gpui::Window, (x, y, w, h): (i32, i32, i32, i32)) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_SHOWWINDOW, SetWindowPos};
    let Ok(RawWindowHandle::Win32(handle)) =
        HasWindowHandle::window_handle(window).map(|h| h.as_raw())
    else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW);
    }
}

/// Opens the menu at `cursor` (device pixels), inside `work` (the work area of
/// that screen: left, top, right, bottom in device pixels).
pub fn open(
    cursor: (i32, i32),
    work: (i32, i32, i32, i32),
    window_shown: bool,
    events: UnboundedSender<TrayEvent>,
    cx: &mut App,
) {
    use crate::scale::lg;
    let scale = crate::scale::get();
    let action = |label: &str, action| Entry::Action {
        label: label.to_string().into(),
        icon: None,
        action,
    };
    let entries = vec![
        // The window on screen: it goes to the tray; in the tray: it comes back.
        if window_shown {
            action("Minimize to Tray", MenuAction::TrayHide)
        } else {
            action("Open Telegram", MenuAction::TrayShow)
        },
        action("Disable notifications", MenuAction::ToggleNotifications),
        action("Quit Telegram", MenuAction::TrayQuit),
    ];
    // The menu's size, to place the window (`prepareGeometryFor` in the work
    // area); the menu measures itself exactly inside it.
    let text_system = cx.text_system().clone();
    let font_id = text_system.resolve_font(&gpui::font("Open Sans"));
    let font_size = crate::scale::font(13.).size;
    let text_width = |text: &str| -> f32 {
        text.chars()
            .map(|ch| {
                text_system
                    .advance(font_id, font_size, ch)
                    .map_or(0., |a| f32::from(a.width))
            })
            .sum()
    };
    let width = entries
        .iter()
        .map(|e| match e {
            Entry::Action { label, .. } => lg(17.) + text_width(label) + lg(17.),
            _ => 0.,
        })
        .fold(lg(156.), f32::max)
        .min(lg(300.));
    let item = lg(8.) + crate::scale::font(13.).height + lg(7.);
    let height = 2. * lg(8.) + item * entries.len() as f32;
    let logical = |v: i32| v as f32 / scale;
    let (cursor_x, cursor_y) = (logical(cursor.0), logical(cursor.1));
    let (left, top, right, bottom) = (
        logical(work.0),
        logical(work.1),
        logical(work.2),
        logical(work.3),
    );
    let from_right = cursor_x + width > right;
    let from_bottom = cursor_y + height > bottom;
    let x = if from_right {
        cursor_x - width
    } else {
        cursor_x
    }
    .clamp(left, (right - width).max(left));
    let y = if from_bottom {
        cursor_y - height
    } else {
        cursor_y
    }
    .clamp(top, (bottom - height).max(top));
    let margin = MARGIN;
    let slack = 24.;
    let (win_w, win_h) = (width + slack + 2. * margin, height + 2. * margin);
    let win_x = if from_right {
        x + width - (win_w - margin)
    } else {
        x - margin
    };
    let win_y = if from_bottom {
        y + height - (win_h - margin)
    } else {
        y - margin
    };
    let anchor = point(
        px(if from_right { win_w - margin } else { margin }),
        px(if from_bottom { win_h - margin } else { margin }),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(win_x), px(win_y)),
            size(px(win_w), px(win_h)),
        ))),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // Not `Transparent`: on Windows that adds a tinted backdrop (a light box around
        // the menu). The frame is cleared to transparent anyway (DirectComposition).
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some("unofficial-telegram-desktop-rust".into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            // A click anywhere else takes the focus away: the menu goes.
            cx.observe_window_activation(window, |this: &mut TrayMenu, window, cx| {
                if !window.is_window_active() {
                    this.menu.hide();
                    cx.notify();
                }
            })
            .detach();
            TrayMenu {
                menu: PopupMenu::new(entries, false, anchor),
                events,
                chosen: None,
            }
        })
    });
    match opened {
        Ok(handle) => {
            let device = |v: f32| (v * scale).round() as i32;
            let rect = (device(win_x), device(win_y), device(win_w), device(win_h));
            let _ = handle.update(cx, |_, window, _| {
                no_frame(window);
                place_on_top(window, rect);
                window.activate_window();
            });
        }
        Err(e) => tracing::warn!("tray menu: {e}"),
    }
}
