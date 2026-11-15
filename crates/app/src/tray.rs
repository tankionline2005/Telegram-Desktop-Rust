//! The notification-area (tray) icon on Windows, as in tdesktop (`platform/win/tray_win`):
//! the window closes to the tray, a click on the icon shows or hides it, the right
//! button opens the app's own menu (`tray_menu`).
//!
//! The icon is tdesktop's monochrome tray plane (`tray_monochrome.svg`) painted in our
//! brand orange. It is drawn at `SM_CXSMICON`, like tdesktop's.
//!
//! Plain Win32: a message-only window receives the icon's callbacks on the UI thread
//! (GPUI's message loop dispatches them) and forwards them to the app over a channel.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, GetMonitorInfoW, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, GetCursorPos,
    GetForegroundWindow, GetSystemMetrics, HICON, HWND_MESSAGE, ICONINFO, IsIconic,
    IsWindowVisible, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SW_HIDE, SW_RESTORE,
    SW_SHOW, SetForegroundWindow, ShowWindow, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WNDCLASSW,
    WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// What the user did with the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Click, or the first menu item: show the window, or hide it when it is active.
    Toggle,
    Quit,
    /// The right button: our menu at the cursor (device pixels), inside the work
    /// area of its screen (left, top, right, bottom).
    Menu {
        cursor: (i32, i32),
        work: (i32, i32, i32, i32),
    },
}

const CALLBACK: u32 = WM_APP + 1;
const NIN_SELECT: u32 = 0x400; // WM_USER
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;
const ICON_ID: u32 = 1;

/// tdesktop's `Telegram/Resources/icons/tray_monochrome.svg` (vendored).
const TRAY_SVG: &str = include_str!("../../../assets/tdesktop-icons/tray_monochrome.svg");
/// The brand orange of the app icon (top and bottom of its gradient).
const ORANGE_TOP: &str = "#EA6A29";
const ORANGE_BOTTOM: &str = "#DC5F23";

thread_local! {
    static EVENTS: RefCell<Option<UnboundedSender<TrayEvent>>> = const { RefCell::new(None) };
    static ADD: RefCell<Option<NOTIFYICONDATAW>> = const { RefCell::new(None) };
}
/// The main window (for "is it active?" when the menu opens).
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
/// When the main window last lost focus (ms since the epoch): clicking the tray icon
/// activates the taskbar first, so "active" means "was active a moment ago".
static DEACTIVATED_AT: AtomicU64 = AtomicU64::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn main_hwnd() -> Option<HWND> {
    let raw = MAIN_WINDOW.load(Ordering::Relaxed);
    (raw != 0).then_some(HWND(raw as *mut _))
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

impl Tray {
    /// Adds the icon. Events arrive on `events`; `main` is the app window.
    pub fn new(main: HWND, tooltip: &str, events: UnboundedSender<TrayEvent>) -> Option<Self> {
        MAIN_WINDOW.store(main.0 as isize, Ordering::Relaxed);
        EVENTS.with(|e| *e.borrow_mut() = Some(events));
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("UnofficialTelegramDesktopRustTray"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class.lpszClassName,
                PCWSTR::null(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            TASKBAR_CREATED.store(
                RegisterWindowMessageW(w!("TaskbarCreated")),
                Ordering::Relaxed,
            );

            let size = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
            let icon = create_icon(&render_icon(size)?, size)?;
            let mut data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: ICON_ID,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
                uCallbackMessage: CALLBACK,
                hIcon: icon,
                ..Default::default()
            };
            let tip: Vec<u16> = tooltip.encode_utf16().collect();
            let n = tip.len().min(data.szTip.len() - 1);
            data.szTip[..n].copy_from_slice(&tip[..n]);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            add_icon(&data);
            ADD.with(|a| *a.borrow_mut() = Some(data));
            Some(Self { hwnd, icon })
        }
    }

    /// Hides the main window to the tray.
    pub fn hide_window() {
        if let Some(main) = main_hwnd() {
            unsafe {
                let _ = ShowWindow(main, SW_HIDE);
            }
        }
    }

    /// Shows the main window from the tray and brings it to the front.
    pub fn show_window() {
        if let Some(main) = main_hwnd() {
            unsafe {
                let _ = ShowWindow(
                    main,
                    if IsIconic(main).as_bool() {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                let _ = SetForegroundWindow(main);
            }
        }
    }

    /// The main window is visible and was active a moment ago.
    pub fn window_is_active() -> bool {
        let Some(main) = main_hwnd() else {
            return false;
        };
        unsafe {
            IsWindowVisible(main).as_bool()
                && !IsIconic(main).as_bool()
                && (GetForegroundWindow() == main
                    || now_ms().saturating_sub(DEACTIVATED_AT.load(Ordering::Relaxed)) < 500)
        }
    }

    /// Call when the main window gains or loses focus.
    pub fn window_activation_changed(active: bool) {
        if !active {
            DEACTIVATED_AT.store(now_ms(), Ordering::Relaxed);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            ADD.with(|a| {
                if let Some(data) = a.borrow_mut().take() {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                }
            });
            let _ = DestroyIcon(self.icon);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe fn add_icon(data: &NOTIFYICONDATAW) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, data);
        let _ = Shell_NotifyIconW(NIM_SETVERSION, data);
    }
}

fn send(event: TrayEvent) {
    EVENTS.with(|e| {
        if let Some(tx) = e.borrow().as_ref() {
            let _ = tx.send(event);
        }
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == CALLBACK {
        // NOTIFYICON_VERSION_4: the event is in the low word of lparam.
        match (lparam.0 as u32) & 0xffff {
            NIN_SELECT | NIN_KEYSELECT => send(TrayEvent::Toggle),
            WM_CONTEXTMENU => unsafe { request_menu() },
            _ => {}
        }
        return LRESULT(0);
    }
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
        // Explorer restarted: the icon is gone, add it again.
        ADD.with(|a| {
            if let Some(data) = a.borrow().as_ref() {
                unsafe { add_icon(data) };
            }
        });
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The right button on the icon: the app shows its menu (`tray_menu`).
unsafe fn request_menu() {
    unsafe {
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        let monitor = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let r = info.rcWork;
            (r.left, r.top, r.right, r.bottom)
        } else {
            (0, 0, i32::MAX / 4, i32::MAX / 4)
        };
        send(TrayEvent::Menu {
            cursor: (at.x, at.y),
            work,
        });
    }
}

/// The tray plane at `size` px, straight (not premultiplied) RGBA.
fn render_icon(size: u32) -> Option<Vec<u8>> {
    let svg = TRAY_SVG.replace("fill=\"#FFFFFF\"", "fill=\"url(#brand)\"");
    let svg = svg.replacen(
        "<g ",
        &format!(
            "<defs><linearGradient id=\"brand\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\">\
             <stop offset=\"0\" stop-color=\"{ORANGE_TOP}\"/>\
             <stop offset=\"1\" stop-color=\"{ORANGE_BOTTOM}\"/></linearGradient></defs><g "
        ),
        1,
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

/// A 32-bit icon with alpha from straight RGBA pixels.
unsafe fn create_icon(rgba: &[u8], size: u32) -> Option<HICON> {
    unsafe {
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        let (dst, _) = dst.as_chunks_mut::<4>();
        let (src, _) = rgba.as_chunks::<4>();
        for (d, s) in dst.iter_mut().zip(src) {
            *d = [s[2], s[1], s[0], s[3]]; // BGRA
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon.ok()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_renders_in_orange() {
        let rgba = super::render_icon(32).expect("svg renders");
        assert_eq!(rgba.len(), 32 * 32 * 4);
        let opaque: Vec<_> = rgba.chunks(4).filter(|p| p[3] == 255).collect();
        assert!(opaque.len() > 100, "the plane covers part of the icon");
        // Orange: red high, blue low.
        assert!(opaque.iter().all(|p| p[0] > 200 && p[2] < 60));
    }
}
