//! tdesktop's icons (`Telegram/Resources/icons`, vendored in `assets/tdesktop-icons`).
//!
//! They are grayscale masks in three scales; tdesktop tints them with a palette color at
//! draw time. GPUI draws images as they are, so a tinted copy is made once per
//! (icon, color, scale) and cached.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{Img, RenderImage, Window, img, prelude::*, px};
use tdr_theme::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    DialogsMenu,
    DialogsPinned,
    NightMode,
    Send,
    DownArrow,
    /// The chat list's "scroll to top" arrow: `DownArrow` flipped (`dialogsToUpAbove`).
    UpArrow,
    DownCircle,
    DownShadow,
    /// Before a group's name in the chat list.
    ChatGroup,
    ChatChannel,
    ChatBot,
    /// Send state of the last message in the chat list: sent (✓), read (✓✓).
    DialogsSent,
    DialogsReceived,
    /// The same in an outgoing bubble.
    HistorySent,
    HistoryReceived,
}

macro_rules! icon_bytes {
    ($name:literal) => {
        [
            include_bytes!(concat!("../../../assets/tdesktop-icons/", $name, ".png")).as_slice(),
            include_bytes!(concat!("../../../assets/tdesktop-icons/", $name, "@2x.png")).as_slice(),
            include_bytes!(concat!("../../../assets/tdesktop-icons/", $name, "@3x.png")).as_slice(),
        ]
    };
}

impl Icon {
    fn files(self) -> [&'static [u8]; 3] {
        match self {
            Icon::DialogsMenu => icon_bytes!("dialogs__dialogs_menu"),
            Icon::DialogsPinned => icon_bytes!("dialogs__dialogs_pinned"),
            Icon::NightMode => icon_bytes!("menu__night_mode"),
            Icon::Send => icon_bytes!("chat__input_send"),
            Icon::DownArrow | Icon::UpArrow => icon_bytes!("history_down_arrow"),
            Icon::DownCircle => icon_bytes!("history_down_circle"),
            Icon::DownShadow => icon_bytes!("history_down_shadow"),
            Icon::ChatGroup => icon_bytes!("dialogs__dialogs_chat"),
            Icon::ChatChannel => icon_bytes!("dialogs__dialogs_channel"),
            Icon::ChatBot => icon_bytes!("dialogs__dialogs_bot"),
            Icon::DialogsSent => icon_bytes!("dialogs__dialogs_sent"),
            Icon::DialogsReceived => icon_bytes!("dialogs__dialogs_received"),
            Icon::HistorySent => icon_bytes!("history_sent"),
            Icon::HistoryReceived => icon_bytes!("history_received"),
        }
    }
}

struct Tinted {
    image: Arc<RenderImage>,
    /// Size in logical pixels (the @1x size).
    width: f32,
    height: f32,
    /// `PAD` in logical pixels: drawn outside the icon's box.
    pad: f32,
}

/// (icon, color, scale index) -> tinted image.
type Cache = HashMap<(Icon, u32, usize), Arc<Tinted>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

/// The smallest mask at least as large as the device pixels (125 % uses @2x shrunk,
/// not @1x blown up).
fn scale_index(scale_factor: f32) -> usize {
    if scale_factor > 2. {
        2
    } else if scale_factor > 1. {
        1
    } else {
        0
    }
}

/// Transparent border around every tinted mask, in mask pixels. GPUI keeps images in a
/// texture atlas and samples them with linear filtering: scaled to a fractional display
/// scale, an icon touching its edge picks up a line of its atlas neighbour (a stripe
/// above the "scroll down" button at 125 %).
const PAD: u32 = 2;

fn tinted(icon: Icon, color: Color, scale: usize) -> Arc<Tinted> {
    let key = (icon, color.rgba(), scale);
    if let Some(t) = CACHE.lock().unwrap().get(&key) {
        return t.clone();
    }
    let files = icon.files();
    let decode = |bytes: &[u8]| {
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .expect("bundled icon")
            .to_rgba8()
    };
    let base = decode(files[0]);
    let mut mask = decode(files[scale]);
    if icon == Icon::UpArrow {
        image::imageops::flip_vertical_in_place(&mut mask);
    }
    let rgba = color.rgba();
    let (r, g, b, a) = (
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        rgba as u8,
    );
    // The masks are opaque grayscale: the gray level is the coverage (tdesktop's
    // `colorizeImage` reads the low byte). GPUI expects BGRA.
    let mut padded = image::RgbaImage::new(mask.width() + 2 * PAD, mask.height() + 2 * PAD);
    for (x, y, px) in mask.enumerate_pixels() {
        let alpha = (px[2] as u16 * a as u16 / 255) as u8;
        padded.put_pixel(x + PAD, y + PAD, image::Rgba([b, g, r, alpha]));
    }
    let pad = PAD as f32 * base.width() as f32 / mask.width() as f32;
    let frame = image::Frame::new(padded);
    let tinted = Arc::new(Tinted {
        image: Arc::new(RenderImage::new(smallvec::smallvec![frame])),
        width: base.width() as f32,
        height: base.height() as f32,
        pad,
    });
    CACHE.lock().unwrap().insert(key, tinted.clone());
    tinted
}

/// The icon tinted with `color`, at its natural (tdesktop @1x) size. The transparent
/// border hangs outside that box (negative margins), so layout sees the bare icon.
pub fn icon(icon: Icon, color: Color, window: &Window) -> Img {
    let t = tinted(icon, color, scale_index(window.scale_factor()));
    img(t.image.clone())
        .w(px(t.width + 2. * t.pad))
        .h(px(t.height + 2. * t.pad))
        .m(px(-t.pad))
        .flex_none()
}
