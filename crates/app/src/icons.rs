//! tdesktop's icons (`Telegram/Resources/icons`, vendored in `assets/tdesktop-icons`).
//!
//! They are grayscale masks in three scales; tdesktop tints them with a palette color at
//! draw time. GPUI draws images as they are, so a tinted copy is made once per
//! (icon, color, scale) and cached.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{Div, RenderImage, Window, div, img, prelude::*, px};
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
    /// The round "go to original" button of forwarded messages
    /// (`historyGoToOriginalIcon`).
    GoToOriginal,
    /// The chat header's back arrow (`infoTopBarBackIcon`).
    Back,
    /// The shadow at the edge of a chat sliding in (`slideShadow`), stretched.
    SlideShadow,
    /// Send state of the last message in the chat list: sent (✓), read (✓✓).
    DialogsSent,
    DialogsReceived,
    /// The same in an outgoing bubble.
    HistorySent,
    HistoryReceived,
    /// In the top right corner of a block quote (`chat/mini_quote`).
    MiniQuote,
    /// In the top right corner of a code block (`chat/mini_copy`).
    MiniCopy,
    // Popup menu items (`menuIcon*`, `whenEdited`, `whenOriginal`).
    MenuReply,
    MenuEdit,
    MenuPin,
    MenuCopy,
    MenuForward,
    MenuDelete,
    MenuSelect,
    MenuInvite,
    MenuCalendar,
    MenuPayment,
    MenuEdited,
    MenuForwarded,
    MenuNotifications,
    /// A submenu's arrow (`defaultMenuArrow`).
    MenuSubmenuArrow,
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
            Icon::GoToOriginal => icon_bytes!("filled_go_to_message"),
            Icon::Back => icon_bytes!("info__info_back"),
            Icon::SlideShadow => icon_bytes!("slide_shadow"),
            Icon::MiniQuote => icon_bytes!("chat__mini_quote"),
            Icon::MiniCopy => icon_bytes!("chat__mini_copy"),
            Icon::MenuReply => icon_bytes!("menu__reply"),
            Icon::MenuEdit => icon_bytes!("menu__edit"),
            Icon::MenuPin => icon_bytes!("menu__pin"),
            Icon::MenuCopy => icon_bytes!("menu__copy"),
            Icon::MenuForward => icon_bytes!("menu__forward"),
            Icon::MenuDelete => icon_bytes!("menu__delete"),
            Icon::MenuSelect => icon_bytes!("menu__select"),
            Icon::MenuInvite => icon_bytes!("menu__invite"),
            Icon::MenuCalendar => icon_bytes!("menu__calendar"),
            Icon::MenuPayment => icon_bytes!("payments__payment_card"),
            Icon::MenuEdited => icon_bytes!("menu__edited_status"),
            Icon::MenuForwarded => icon_bytes!("menu__forwarded_status"),
            Icon::MenuNotifications => icon_bytes!("menu__info_notifications"),
            Icon::MenuSubmenuArrow => icon_bytes!("menu__submenu_arrow"),
        }
    }
}

struct Tinted {
    image: Arc<RenderImage>,
    /// Device pixels of the icon itself (tdesktop's `ConvertScale` of the @1x size).
    width: f32,
    height: f32,
}

/// (icon, color, scale factor bits) -> tinted image.
type Cache = HashMap<(Icon, u32, u32), Arc<Tinted>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
static BASE_SIZES: LazyLock<Mutex<HashMap<Icon, (u32, u32)>>> = LazyLock::new(Default::default);

/// The mask to scale from: the smallest at least as large as the device pixels
/// (tdesktop takes @2x for 125 % and scales it down).
fn scale_index(scale_factor: f32) -> usize {
    if scale_factor > 2. {
        2
    } else if scale_factor > 1. {
        1
    } else {
        0
    }
}

/// Transparent border around every tinted mask, in device pixels. GPUI keeps images in
/// a texture atlas and samples them with linear filtering: an icon touching its edge
/// can pick up a line of its atlas neighbour.
const PAD: u32 = 2;

fn decode(bytes: &[u8]) -> image::RgbaImage {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("bundled icon")
        .to_rgba8()
}

/// The @1x size (tdesktop's style size of the icon).
fn base_size(icon: Icon) -> (u32, u32) {
    if let Some(size) = BASE_SIZES.lock().unwrap().get(&icon) {
        return *size;
    }
    let base = decode(icon.files()[0]);
    let size = (base.width(), base.height());
    BASE_SIZES.lock().unwrap().insert(icon, size);
    size
}

/// The icon's width in logical pixels at the current scale.
pub fn icon_width(icon: Icon, _window: &Window) -> f32 {
    crate::scale::lg(base_size(icon).0 as f32)
}

fn tinted(icon: Icon, color: Color, scale_factor: f32) -> Arc<Tinted> {
    let key = (icon, color.rgba(), scale_factor.to_bits());
    if let Some(t) = CACHE.lock().unwrap().get(&key) {
        return t.clone();
    }
    let (base_w, base_h) = base_size(icon);
    // Exactly the device pixels tdesktop gives the icon, drawn 1:1.
    let width = crate::scale::device(base_w as f32).max(1.);
    let height = crate::scale::device(base_h as f32).max(1.);
    let mut mask = decode(icon.files()[scale_index(scale_factor)]);
    if mask.width() != width as u32 || mask.height() != height as u32 {
        mask = image::imageops::resize(
            &mask,
            width as u32,
            height as u32,
            image::imageops::FilterType::Triangle,
        );
    }
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
    let frame = image::Frame::new(padded);
    let tinted = Arc::new(Tinted {
        image: Arc::new(RenderImage::new(smallvec::smallvec![frame])),
        width,
        height,
    });
    CACHE.lock().unwrap().insert(key, tinted.clone());
    tinted
}

/// The tinted icon for painting by hand: the image, the icon's logical width and
/// height, and the transparent border around it (logical pixels).
pub fn tinted_image(
    icon: Icon,
    color: Color,
    scale_factor: f32,
) -> Option<(Arc<RenderImage>, f32, f32, f32)> {
    let t = tinted(icon, color, scale_factor);
    Some((
        t.image.clone(),
        t.width / scale_factor,
        t.height / scale_factor,
        PAD as f32 / scale_factor,
    ))
}

/// The icon tinted with `color`, at tdesktop's size for the current scale: a box
/// exactly the icon's size, the image's transparent border hanging outside it.
pub fn icon(icon: Icon, color: Color, window: &Window) -> Div {
    let scale = window.scale_factor();
    let t = tinted(icon, color, scale);
    let pad = PAD as f32 / scale;
    div()
        .flex_none()
        .relative()
        .w(px(t.width / scale))
        .h(px(t.height / scale))
        .child(
            img(t.image.clone())
                .absolute()
                .left(px(-pad))
                .top(px(-pad))
                .w(px(t.width / scale + 2. * pad))
                .h(px(t.height / scale + 2. * pad)),
        )
}

/// The bubble tail (`bubble_tail`, 6 x 10 style px) as an outline in style pixels,
/// traced from the @3x mask: drawn as a path it is antialiased like the bubble it
/// joins at any (fractional) scroll position, where an image would snap to pixels
/// and sometimes stand a pixel off.
pub fn bubble_tail_outline() -> &'static [(f32, f32)] {
    static OUTLINE: LazyLock<Vec<(f32, f32)>> = LazyLock::new(|| {
        let mask = decode(include_bytes!(
            "../../../assets/tdesktop-icons/bubble_tail@3x.png"
        ));
        let (w, h) = (mask.width() as f32, mask.height() as f32);
        let scale = 3.;
        // The shape fills each row from its left edge to the right side: the row's
        // coverage gives where that edge is.
        let left = |y: u32| {
            let covered: f32 = (0..mask.width())
                .map(|x| mask.get_pixel(x, y)[2] as f32 / 255.)
                .sum();
            (w - covered) / scale
        };
        let mut points = vec![(w / scale, 0.), (w / scale, h / scale)];
        points.push((left(mask.height() - 1), h / scale));
        for y in (0..mask.height()).rev() {
            points.push((left(y), (y as f32 + 0.5) / scale));
        }
        points.push((left(0), 0.));
        points
    });
    &OUTLINE
}

/// An icon stretched over its parent's height (the 1 px tall `slide_shadow`).
pub fn stretched(icon: Icon, color: Color, window: &Window) -> gpui::Img {
    let scale = window.scale_factor();
    let t = tinted(icon, color, scale);
    img(t.image.clone())
        .w(px(t.width / scale))
        .h_full()
        .object_fit(gpui::ObjectFit::Fill)
}
