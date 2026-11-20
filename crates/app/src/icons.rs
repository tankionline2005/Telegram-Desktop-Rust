//! tdesktop's icons (`Telegram/Resources/icons`, vendored in `assets/tdesktop-icons`).
//!
//! They are grayscale masks in three scales; tdesktop tints them with a palette color at
//! draw time. GPUI draws images as they are, so a tinted copy is made once per
//! (icon, color, scale) and cached.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{Div, RenderImage, Window, div, img, prelude::*, px};
use tdr_data::filters::FilterIcon;
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
    /// Folder icons (`Ui::FilterIcon`, `folders/folders_*`).
    Folder(FilterIcon),
    // The chat header (`topBarSearch`, `topBarCall`, `topBarInfo`, `menuToggleIcon`).
    TopBarSearch,
    TopBarCall,
    TopBarProfile,
    MenuDots,
    // The message field (`historyAttach`, `historyAttachEmoji`, the record button).
    InputAttach,
    InputSmile,
    InputRecord,
    // `historyAiComposeButton` (three layers) and `historyExpandComposeButton`: SVGs.
    AiLetters,
    AiStar1,
    AiStar2,
    Expand,
    // `Window::ConnectionState`: the box's round ends and their shadow (the body
    // between them is drawn).
    ConnectingLeft,
    ConnectingRight,
    ConnectingLeftShadow,
    ConnectingRightShadow,
    // The main menu (`menuIcon*`, `settingsIconAdd`) and the menu button's dot.
    MenuProfile,
    MenuGroups,
    MenuChannel,
    MenuContacts,
    MenuPhone,
    MenuSaved,
    MenuSettings,
    MenuArchive,
    SettingsAdd,
    DialogsMenuUnread,
    DialogsMenuUnreadDot,
    // The login screens.
    IntroQrPlane,
    IntroCountryDropdown,
    BoxBack,
}

/// Where an icon's mask comes from.
enum Source {
    /// Grayscale masks @1x, @2x, @3x.
    Png([&'static [u8]; 3]),
    /// A vector icon drawn at this style size (`name-WxH` in tdesktop's styles).
    Svg(&'static [u8], u32, u32),
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

macro_rules! svg_bytes {
    ($name:literal, $w:literal, $h:literal) => {
        Source::Svg(
            include_bytes!(concat!("../../../assets/tdesktop-icons/", $name, ".svg")).as_slice(),
            $w,
            $h,
        )
    };
}

fn folder_files(icon: FilterIcon) -> [&'static [u8]; 3] {
    use FilterIcon as F;
    match icon {
        F::Cat => icon_bytes!("folders__folders_cat"),
        F::Book => icon_bytes!("folders__folders_book"),
        F::Money => icon_bytes!("folders__folders_money"),
        F::Game => icon_bytes!("folders__folders_game"),
        F::Light => icon_bytes!("folders__folders_light"),
        F::Like => icon_bytes!("folders__folders_like"),
        F::Note => icon_bytes!("folders__folders_note"),
        F::Palette => icon_bytes!("folders__folders_palette"),
        F::Travel => icon_bytes!("folders__folders_travel"),
        F::Sport => icon_bytes!("folders__folders_sport"),
        F::Favorite => icon_bytes!("folders__folders_favorite"),
        F::Study => icon_bytes!("folders__folders_study"),
        F::Airplane => icon_bytes!("folders__folders_airplane"),
        F::Private => icon_bytes!("folders__folders_private"),
        F::Groups => icon_bytes!("folders__folders_group"),
        F::All => icon_bytes!("folders__folders_all"),
        F::Unread => icon_bytes!("folders__folders_unread"),
        F::Bots => icon_bytes!("folders__folders_bots"),
        F::Crown => icon_bytes!("folders__folders_crown"),
        F::Flower => icon_bytes!("folders__folders_flower"),
        F::Home => icon_bytes!("folders__folders_home"),
        F::Love => icon_bytes!("folders__folders_love"),
        F::Mask => icon_bytes!("folders__folders_mask"),
        F::Party => icon_bytes!("folders__folders_party"),
        F::Trade => icon_bytes!("folders__folders_trade"),
        F::Work => icon_bytes!("folders__folders_work"),
        F::Unmuted => icon_bytes!("folders__folders_unmuted"),
        F::Channels => icon_bytes!("folders__folders_channels"),
        F::Custom => icon_bytes!("folders__folders_custom"),
        F::Setup => icon_bytes!("folders__folders_setup"),
        F::Edit => icon_bytes!("folders__folders_edit"),
    }
}

impl Icon {
    fn source(self) -> Source {
        match self {
            Icon::Folder(f) => Source::Png(folder_files(f)),
            Icon::TopBarSearch => Source::Png(icon_bytes!("top_bar_search")),
            Icon::TopBarCall => Source::Png(icon_bytes!("top_bar_call")),
            Icon::TopBarProfile => Source::Png(icon_bytes!("top_bar_profile")),
            Icon::MenuDots => Source::Png(icon_bytes!("title_menu_dots")),
            Icon::InputAttach => Source::Png(icon_bytes!("chat__input_attach")),
            Icon::InputSmile => Source::Png(icon_bytes!("chat__input_smile_face")),
            Icon::InputRecord => Source::Png(icon_bytes!("chat__input_record")),
            Icon::AiLetters => svg_bytes!("chat__ai_letters", 20, 20),
            Icon::AiStar1 => svg_bytes!("chat__ai_star1", 20, 20),
            Icon::AiStar2 => svg_bytes!("chat__ai_star2", 20, 20),
            Icon::Expand => svg_bytes!("menu__rich__expand", 20, 20),
            Icon::ConnectingLeft => Source::Png(icon_bytes!("connecting_left")),
            Icon::ConnectingRight => Source::Png(icon_bytes!("connecting_right")),
            Icon::ConnectingLeftShadow => Source::Png(icon_bytes!("connecting_left_shadow")),
            Icon::ConnectingRightShadow => Source::Png(icon_bytes!("connecting_right_shadow")),
            Icon::MenuProfile => Source::Png(icon_bytes!("menu__profile")),
            Icon::MenuGroups => Source::Png(icon_bytes!("menu__groups")),
            Icon::MenuChannel => Source::Png(icon_bytes!("menu__channel")),
            Icon::MenuContacts => Source::Png(icon_bytes!("menu__name_show")),
            Icon::MenuPhone => Source::Png(icon_bytes!("menu__phone")),
            Icon::MenuSaved => Source::Png(icon_bytes!("menu__saved_messages")),
            Icon::MenuSettings => Source::Png(icon_bytes!("menu__settings")),
            Icon::MenuArchive => Source::Png(icon_bytes!("menu__archive_open")),
            Icon::SettingsAdd => Source::Png(icon_bytes!("settings__add")),
            Icon::DialogsMenuUnread => Source::Png(icon_bytes!("dialogs__dialogs_menu_unread")),
            Icon::DialogsMenuUnreadDot => {
                Source::Png(icon_bytes!("dialogs__dialogs_menu_unread_dot"))
            }
            Icon::IntroQrPlane => Source::Png(icon_bytes!("intro_qr_plane")),
            Icon::IntroCountryDropdown => Source::Png(icon_bytes!("intro_country_dropdown")),
            Icon::BoxBack => Source::Png(icon_bytes!("box_button_back")),
            other => Source::Png(other.files()),
        }
    }

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
            _ => unreachable!("icons with their own source"),
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

/// A vector icon as a mask like the PNG ones (coverage in every channel), drawn to
/// `width` x `height` device pixels.
fn render_svg(svg: &[u8], width: u32, height: u32) -> image::RgbaImage {
    let mut out = image::RgbaImage::new(width.max(1), height.max(1));
    let Ok(tree) = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()) else {
        return out;
    };
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(width.max(1), height.max(1)) else {
        return out;
    };
    let size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    for (px, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let a = src.alpha();
        *px = image::Rgba([a, a, a, 255]);
    }
    out
}

/// The @1x size (tdesktop's style size of the icon).
fn base_size(icon: Icon) -> (u32, u32) {
    if let Some(size) = BASE_SIZES.lock().unwrap().get(&icon) {
        return *size;
    }
    let size = match icon.source() {
        Source::Png(files) => {
            let base = decode(files[0]);
            (base.width(), base.height())
        }
        Source::Svg(_, w, h) => (w, h),
    };
    BASE_SIZES.lock().unwrap().insert(icon, size);
    size
}

/// The icon's height in logical pixels at the current scale.
pub fn icon_height(icon: Icon, _window: &Window) -> f32 {
    crate::scale::lg(base_size(icon).1 as f32)
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
    let mut mask = match icon.source() {
        Source::Png(files) => decode(files[scale_index(scale_factor)]),
        Source::Svg(svg, _, _) => render_svg(svg, width as u32, height as u32),
    };
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
