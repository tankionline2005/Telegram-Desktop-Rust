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
    DownCircle,
    DownShadow,
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
            Icon::DownArrow => icon_bytes!("history_down_arrow"),
            Icon::DownCircle => icon_bytes!("history_down_circle"),
            Icon::DownShadow => icon_bytes!("history_down_shadow"),
        }
    }
}

struct Tinted {
    image: Arc<RenderImage>,
    /// Size in logical pixels (the @1x size).
    width: f32,
    height: f32,
}

/// (icon, color, scale index) -> tinted image.
type Cache = HashMap<(Icon, u32, usize), Arc<Tinted>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

fn scale_index(scale_factor: f32) -> usize {
    if scale_factor > 2.25 {
        2
    } else if scale_factor > 1.25 {
        1
    } else {
        0
    }
}

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
    let rgba = color.rgba();
    let (r, g, b, a) = (
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        rgba as u8,
    );
    // The masks are opaque grayscale: the gray level is the coverage (tdesktop's
    // `colorizeImage` reads the low byte). GPUI expects BGRA.
    for px in mask.pixels_mut() {
        let alpha = (px[2] as u16 * a as u16 / 255) as u8;
        px.0 = [b, g, r, alpha];
    }
    let frame = image::Frame::new(mask);
    let tinted = Arc::new(Tinted {
        image: Arc::new(RenderImage::new(smallvec::smallvec![frame])),
        width: base.width() as f32,
        height: base.height() as f32,
    });
    CACHE.lock().unwrap().insert(key, tinted.clone());
    tinted
}

/// The icon tinted with `color`, at its natural (tdesktop @1x) size.
pub fn icon(icon: Icon, color: Color, window: &Window) -> Img {
    let t = tinted(icon, color, scale_index(window.scale_factor()));
    img(t.image.clone())
        .w(px(t.width))
        .h(px(t.height))
        .flex_none()
}
