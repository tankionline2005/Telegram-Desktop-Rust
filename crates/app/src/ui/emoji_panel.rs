//! The emoji panel (`ChatHelpers::TabbedPanel` with `TabbedSelector` and
//! `EmojiListWidget`): shown while the mouse is over the field's emoji button or
//! the panel, or docked as the third column (`TabbedSection`) by a click on the
//! button. Tabs "Emoji / Stickers / GIFs" (the last two come later), a search
//! field, the recent emoji and the seven sections with their headers, the skin
//! tone picker (`EmojiColorPicker`), the recent emoji's menu and the footer with
//! the sections' icons.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AppContext, Bounds, Context, Corners, Entity, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, TextRun, Window, canvas, div, font, point,
    prelude::FluentBuilder, px as gpx,
};
use tdr_emoji::Section;

use super::{MainView, c};
use crate::anim::ease_out_circ;
use crate::icons::{Icon, icon};
use crate::popup_menu::{Entry, MenuAction, PopupMenu};
use crate::scale::{self, lg, px};
use crate::text_input::TextInput;

// chat_helpers.style
const PAN_WIDTH: f32 = 345.;
const PAN_MARGIN: f32 = 10.;
const PAN_MIN_HEIGHT: f32 = 278.;
const PAN_MAX_HEIGHT: f32 = 640.;
const PAN_HEIGHT_RATIO: f32 = 0.75;
const PAN_RADIUS: f32 = 8.;
const PAN_SHOW_MS: f32 = 200.;
const PAN_FADE_MS: f32 = 200.;
const HIDE_TIMEOUT: Duration = Duration::from_millis(300);
/// `columnMinimalWidthThird`.
pub(super) const SECTION_WIDTH: f32 = 292.;
// defaultTabsSlider
const TABS_HEIGHT: f32 = 43.;
const TABS_BAR_TOP: f32 = 40.;
const TABS_LABEL_TOP: f32 = 12.;
// defaultEmojiPan
const MARGIN_LEFT: f32 = 7.;
const MARGIN_RIGHT: f32 = 7.;
const PADDING_LEFT: f32 = 7.;
const PADDING_RIGHT: f32 = 4.;
const PADDING_BOTTOM: f32 = 7.;
const DESIRED_SIZE: f32 = 37.;
const VERTICAL_SIZE_SUB: f32 = 1.;
const HEADER: f32 = 33.;
const HEADER_LEFT: f32 = 14.;
const HEADER_TOP: f32 = 10.;
const FOOTER: f32 = 36.;
const ICON_SKIP: f32 = 3.;
const ICON_AREA: f32 = 28.;
const AREA: (f32, f32) = (34., 32.);
const SCROLL_WIDTH: f32 = 7.;
// defaultTabbedSearch, searchMargin
const SEARCH_HEIGHT: f32 = 33.;
const SEARCH_MARGIN: [f32; 4] = [1., 11., 2., 5.];
// The color picker.
const COLORS_PADDING: f32 = 8.;
const COLORS_SEP: f32 = 1.;
const COLOR_ALL_SKIP: f32 = 9.;
const COLOR_ALL_TOP: f32 = 10.;
const COLOR_ALL_SIZE: f32 = 24.;
const COLOR_ALL_PADDING: [f32; 4] = [10., 6., 10., -1.];
const PICKER_DELAY: Duration = Duration::from_millis(500);
const SELECTION_MS: f32 = 200.;

const SECTION_TITLES: [&str; 7] = [
    "Emoji & People",
    "Nature",
    "Food & Drink",
    "Activity",
    "Travel & Places",
    "Objects",
    "Symbols & Flags",
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Show {
    Hidden,
    /// `PanelAnimation` from the bottom right corner.
    Unrolling(Instant),
    Shown,
    /// The opacity animation to `to` from `from`.
    Fading {
        start: Instant,
        from: f32,
        to: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Over {
    #[default]
    None,
    Emoji {
        section: usize,
        index: usize,
    },
    ColorAll,
}

struct Picker {
    variants: Vec<u16>,
    all: bool,
    /// Top left inside the list (its margins included).
    at: (f32, f32),
    shown: Instant,
    hiding: Option<Instant>,
    selected: Option<usize>,
    /// What it is for: an emoji of a section, or the "color all" button.
    target: Over,
}

/// The panel's and the third column's state.
pub(super) struct EmojiPanel {
    show: Show,
    /// Docked as the third column.
    pub(super) section: bool,
    hide_at: Option<Instant>,
    pub(super) scroll: ScrollHandle,
    /// The recent emoji and the sections, with the chosen skin tones.
    lists: Vec<Vec<u16>>,
    over: Over,
    pressed: Over,
    press_at: Option<Instant>,
    picker: Option<Picker>,
    /// The footer's selected icon and where its background slides from.
    footer: usize,
    footer_from: (f32, Instant),
    pub(super) search: Option<Entity<TextInput>>,
    query: String,
    results: Option<Vec<u16>>,
    /// The list's top left in the window at the last paint.
    origin: Rc<Cell<Point<Pixels>>>,
    recent: tdr_emoji::recent::Recent,
    recent_path: Option<std::path::PathBuf>,
    /// The list's layout and width at the last frame.
    layout: Option<ListLayout>,
    list_width: f32,
    /// A popup menu is open over the panel.
    menu_open: bool,
}

impl EmojiPanel {
    pub(super) fn new(recent_path: Option<std::path::PathBuf>) -> Self {
        let recent = recent_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| tdr_emoji::recent::Recent::parse(&t))
            .unwrap_or_default();
        Self {
            show: Show::Hidden,
            section: false,
            hide_at: None,
            scroll: ScrollHandle::new(),
            lists: Vec::new(),
            over: Over::None,
            pressed: Over::None,
            press_at: None,
            picker: None,
            footer: 0,
            footer_from: (0., Instant::now()),
            search: None,
            query: String::new(),
            results: None,
            origin: Rc::new(Cell::new(Point::default())),
            recent,
            recent_path,
            layout: None,
            list_width: 0.,
            menu_open: false,
        }
    }

    fn save_recent(&self) {
        if let Some(path) = &self.recent_path {
            let _ = tdr_api::write_atomic(path, self.recent.serialize().as_bytes());
        }
    }

    /// `fillRecent` and the sections with `lookupEmojiVariant`.
    fn refresh(&mut self) {
        let mut lists = vec![self.recent.list()];
        for s in Section::ALL {
            lists.push(
                tdr_emoji::section(s)
                    .iter()
                    .map(|e| self.recent.variant_of(*e))
                    .collect(),
            );
        }
        self.lists = lists;
    }

    /// The panel takes the mouse (not while it fades out).
    fn opacity(&self) -> f32 {
        match self.show {
            Show::Hidden => 0.,
            Show::Unrolling(_) | Show::Shown => 1.,
            Show::Fading { start, from, to } => {
                let t = (start.elapsed().as_secs_f32() * 1000. / PAN_FADE_MS).min(1.);
                from + (to - from) * t
            }
        }
    }

    /// `showAnimated`.
    fn show_animated(&mut self) {
        self.hide_at = None;
        match self.show {
            Show::Hidden => {
                self.refresh();
                self.show = Show::Unrolling(Instant::now());
            }
            Show::Fading { to: 0., .. } => {
                let from = self.opacity();
                self.show = Show::Fading {
                    start: Instant::now(),
                    from,
                    to: 1.,
                };
            }
            _ => {}
        }
    }

    /// `hideAnimated`.
    fn hide_animated(&mut self) {
        self.hide_at = None;
        match self.show {
            Show::Hidden => {}
            Show::Fading { to: 0., .. } => {}
            _ => {
                let from = self.opacity();
                self.show = Show::Fading {
                    start: Instant::now(),
                    from,
                    to: 0.,
                };
                self.picker = None;
            }
        }
    }

    /// `preventAutoHide`: while the skin tone picker or a menu is open.
    fn prevent_auto_hide(&self) -> bool {
        self.menu_open || self.picker.as_ref().is_some_and(|p| p.hiding.is_none())
    }

    /// Advances the animations; true while one runs.
    fn step(&mut self) -> bool {
        if let Some(at) = self.hide_at
            && Instant::now() >= at
        {
            self.hide_at = None;
            if !self.prevent_auto_hide() {
                self.hide_animated();
            }
        }
        let mut running = self.hide_at.is_some();
        match self.show {
            Show::Unrolling(start) => {
                if start.elapsed().as_secs_f32() * 1000. >= PAN_SHOW_MS {
                    self.show = Show::Shown;
                } else {
                    running = true;
                }
            }
            Show::Fading { start, to, .. } => {
                if start.elapsed().as_secs_f32() * 1000. >= PAN_FADE_MS {
                    self.show = if to > 0. { Show::Shown } else { Show::Hidden };
                    if to == 0. {
                        self.over = Over::None;
                        self.picker = None;
                    }
                } else {
                    running = true;
                }
            }
            _ => {}
        }
        if let Some(p) = &self.picker {
            if let Some(h) = p.hiding {
                if h.elapsed().as_secs_f32() * 1000. >= PAN_FADE_MS {
                    self.picker = None;
                } else {
                    running = true;
                }
            } else if p.shown.elapsed().as_secs_f32() * 1000. < PAN_FADE_MS {
                running = true;
            }
        }
        if self.footer_from.1.elapsed().as_secs_f32() * 1000. < SELECTION_MS {
            running = true;
        }
        running
    }
}

/// A section's place in the list (`SectionInfo`).
#[derive(Clone, Copy, Debug)]
struct SectionInfo {
    section: usize,
    count: usize,
    top: f32,
    rows_top: f32,
    rows_bottom: f32,
}

/// `countDesiredHeight` at the list's width.
#[derive(Clone, Debug)]
struct ListLayout {
    columns: usize,
    single: (f32, f32),
    rows_left: f32,
    sections: Vec<SectionInfo>,
    height: f32,
}

impl ListLayout {
    fn new(width: f32, counts: &[usize], min_height: f32) -> Self {
        let full = lg(MARGIN_LEFT) + width + lg(MARGIN_RIGHT);
        let inner = full - lg(PADDING_LEFT) - lg(PADDING_RIGHT);
        let columns = ((inner / lg(DESIRED_SIZE)).floor() as usize).max(1);
        let single_w = (inner / columns as f32).floor();
        let rows_left =
            lg(PADDING_LEFT) + ((inner - columns as f32 * single_w) / 2.).floor() - lg(MARGIN_LEFT);
        let single = (single_w, single_w - 2. * lg(VERTICAL_SIZE_SUB));
        let search = lg(SEARCH_MARGIN[1]) + lg(SEARCH_HEIGHT) + lg(SEARCH_MARGIN[3]);
        let mut sections = Vec::new();
        let mut top = 0.;
        for (i, count) in counts.iter().enumerate() {
            let rows = count.div_ceil(columns);
            let rows_top = top + if i == 0 { search } else { lg(HEADER) };
            let rows_bottom = rows_top + rows as f32 * single.1;
            sections.push(SectionInfo {
                section: i,
                count: *count,
                top,
                rows_top,
                rows_bottom,
            });
            top = rows_bottom;
        }
        let last = sections.last().map_or(0., |s| s.rows_bottom);
        Self {
            columns,
            single,
            rows_left,
            sections,
            height: (last + lg(PADDING_BOTTOM)).max(min_height),
        }
    }

    fn emoji_rect(&self, section: usize, index: usize) -> Bounds<Pixels> {
        let info = self.sections[section];
        let x = self.rows_left + (index % self.columns) as f32 * self.single.0;
        let y = info.rows_top + (index / self.columns) as f32 * self.single.1;
        Bounds::new(
            point(gpx(x), gpx(y)),
            gpui::size(gpx(self.single.0), gpx(self.single.1)),
        )
    }

    fn hit(&self, x: f32, y: f32) -> Over {
        for info in &self.sections {
            if y >= info.rows_top && y < info.rows_bottom {
                let column = ((x - self.rows_left) / self.single.0).floor();
                if column < 0. || column >= self.columns as f32 {
                    return Over::None;
                }
                let row = ((y - info.rows_top) / self.single.1).floor() as usize;
                let index = row * self.columns + column as usize;
                return if index < info.count {
                    Over::Emoji {
                        section: info.section,
                        index,
                    }
                } else {
                    Over::None
                };
            }
        }
        Over::None
    }

    /// `sectionInfoByOffset`.
    fn section_at(&self, y: f32) -> usize {
        self.sections
            .iter()
            .find(|s| y < s.rows_bottom)
            .map_or(self.sections.len().saturating_sub(1), |s| s.section)
    }

    /// The "color all" button of the people section's header.
    fn color_all_rect(&self) -> Option<Bounds<Pixels>> {
        let info = self.sections.get(1)?;
        let right = self.rows_left + self.columns as f32 * self.single.0;
        let size = lg(COLOR_ALL_SIZE);
        Some(Bounds::new(
            point(
                gpx(right - lg(COLOR_ALL_SKIP) - size),
                gpx(info.top + lg(COLOR_ALL_TOP)),
            ),
            gpui::size(gpx(size), gpx(size)),
        ))
    }
}

fn contains(b: &Bounds<Pixels>, x: f32, y: f32) -> bool {
    x >= f32::from(b.origin.x)
        && y >= f32::from(b.origin.y)
        && x < f32::from(b.origin.x + b.size.width)
        && y < f32::from(b.origin.y + b.size.height)
}

impl MainView {
    fn emoji_counts(&self) -> Vec<usize> {
        match &self.emoji_panel.results {
            Some(results) => vec![results.len()],
            None => self.emoji_panel.lists.iter().map(Vec::len).collect(),
        }
    }

    fn emoji_at(&self, section: usize, index: usize) -> Option<u16> {
        match &self.emoji_panel.results {
            Some(results) => results.get(index).copied(),
            None => self.emoji_panel.lists.get(section)?.get(index).copied(),
        }
    }

    /// The list's width inside the selector of `width`.
    fn emoji_list_width(width: f32) -> f32 {
        width - lg(PAN_RADIUS) - lg(SCROLL_WIDTH)
    }

    /// The mouse entered or left the field's emoji button (`otherEnter`,
    /// `otherLeave`).
    pub(super) fn emoji_button_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.emoji_panel.section || self.selected.is_none() {
            return;
        }
        if hovered {
            self.emoji_panel.show_animated();
            self.ensure_emoji_search(cx);
        } else if !self.emoji_panel.prevent_auto_hide() {
            if matches!(self.emoji_panel.show, Show::Fading { .. }) {
                self.emoji_panel.hide_animated();
            } else {
                self.schedule_emoji_hide(Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    fn schedule_emoji_hide(&mut self, after: Duration, cx: &mut Context<Self>) {
        self.emoji_panel.hide_at = Some(Instant::now() + after);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn emoji_panel_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.emoji_panel.show_animated();
        } else if !self.emoji_panel.prevent_auto_hide() {
            if matches!(
                self.emoji_panel.show,
                Show::Unrolling(_) | Show::Fading { .. }
            ) {
                self.emoji_panel.hide_animated();
            } else {
                self.schedule_emoji_hide(HIDE_TIMEOUT, cx);
            }
        }
        cx.notify();
    }

    /// A click on the emoji button: shows the panel, or docks it as the third
    /// column where there is room (`toggleTabbedSelectorMode`), or undocks it.
    pub(super) fn emoji_button_clicked(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.emoji_panel.section {
            self.emoji_panel.section = false;
            self.emoji_panel.show = Show::Hidden;
        } else if self.emoji_panel.show == Show::Hidden {
            self.emoji_panel.show_animated();
            self.ensure_emoji_search(cx);
        } else if self.can_show_emoji_section(window) {
            self.emoji_panel.section = true;
            self.emoji_panel.show = Show::Shown;
            self.emoji_panel.hide_at = None;
            self.emoji_panel.refresh();
        } else {
            self.emoji_panel.hide_animated();
        }
        cx.notify();
    }

    /// `canShowThirdSection`: the chat keeps `columnMinimalWidthMain` beside it.
    pub(super) fn can_show_emoji_section(&self, window: &Window) -> bool {
        if self.one_column() {
            return false;
        }
        let body = f32::from(window.viewport_size().width) - self.filters_width();
        body - self.dialogs_width(window) - lg(SECTION_WIDTH) >= lg(super::COLUMN_MIN_MAIN)
    }

    /// The docked selector's width (0 when not docked).
    pub(super) fn emoji_section_width(&self, window: &Window) -> f32 {
        if self.emoji_panel.section
            && self.selected.is_some()
            && self.can_show_emoji_section(window)
        {
            lg(SECTION_WIDTH)
        } else {
            0.
        }
    }

    fn ensure_emoji_search(&mut self, cx: &mut Context<Self>) {
        if self.emoji_panel.search.is_some() {
            return;
        }
        let palette = self.palette();
        let field = cx.new(|cx| {
            let mut field = TextInput::new("Search", palette, cx);
            field.set_single_line(true);
            field
        });
        cx.observe(&field, |this, field, cx| {
            let query = field.read(cx).text();
            if query != this.emoji_panel.query {
                this.emoji_panel.query = query.clone();
                this.emoji_panel.results =
                    (!query.trim().is_empty()).then(|| crate::emoji_search::search(&query));
                this.emoji_panel.over = Over::None;
                this.emoji_panel.scroll.set_offset(point(gpx(0.), gpx(0.)));
            }
            cx.notify();
        })
        .detach();
        self.emoji_panel.search = Some(field);
    }

    /// `selectEmoji`: into the field at the caret; counted as recent.
    fn choose_emoji(&mut self, emoji: u16, window: &mut Window, cx: &mut Context<Self>) {
        let Some(one) = tdr_emoji::get(emoji) else {
            return;
        };
        self.emoji_panel.recent.increment(emoji);
        self.emoji_panel.save_recent();
        let text = one.text();
        let input = self.input.clone();
        input.update(cx, |i, cx| i.insert_text(&text, cx));
        window.focus(&input.focus_handle(cx));
    }

    fn emoji_mouse_move(
        &mut self,
        position: Point<Pixels>,
        layout: &ListLayout,
        cx: &mut Context<Self>,
    ) {
        let origin = self.emoji_panel.origin.get();
        let (x, y) = (
            f32::from(position.x - origin.x),
            f32::from(position.y - origin.y),
        );
        let over = if self.emoji_panel.results.is_none()
            && layout.color_all_rect().is_some_and(|r| contains(&r, x, y))
        {
            Over::ColorAll
        } else {
            layout.hit(x, y)
        };
        if over != self.emoji_panel.over {
            self.emoji_panel.over = over;
            cx.notify();
        }
    }

    fn emoji_mouse_down(&mut self, layout: &ListLayout, cx: &mut Context<Self>) {
        if self
            .emoji_panel
            .picker
            .as_ref()
            .is_some_and(|p| p.hiding.is_none())
        {
            // `checkPickerHide`: a press elsewhere closes the picker.
            if let Some(p) = &mut self.emoji_panel.picker {
                p.hiding = Some(Instant::now());
            }
            cx.notify();
            return;
        }
        let over = self.emoji_panel.over;
        self.emoji_panel.pressed = over;
        if let Over::Emoji { section, index } = over
            && let Some(emoji) = self.emoji_at(section, index)
            && tdr_emoji::get(emoji).is_some_and(|e| e.has_variants())
            && self.emoji_panel.results.is_none()
        {
            if !self.emoji_panel.recent.has_chosen_variant(emoji) {
                self.show_emoji_picker(over, layout);
            } else {
                let at = Instant::now();
                self.emoji_panel.press_at = Some(at);
                let layout = layout.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PICKER_DELAY).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.emoji_panel.press_at == Some(at) {
                            this.emoji_panel.press_at = None;
                            this.show_emoji_picker(over, &layout);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn emoji_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pressed = std::mem::take(&mut self.emoji_panel.pressed);
        self.emoji_panel.press_at = None;
        if self.emoji_panel.picker.is_some() {
            return;
        }
        let over = self.emoji_panel.over;
        if over != pressed {
            return;
        }
        match over {
            Over::Emoji { section, index } => {
                if let Some(emoji) = self.emoji_at(section, index) {
                    self.choose_emoji(emoji, window, cx);
                }
            }
            Over::ColorAll => {
                let layout = self.emoji_panel.layout.clone();
                if let Some(layout) = layout {
                    self.show_emoji_picker(Over::ColorAll, &layout);
                }
            }
            Over::None => {}
        }
        cx.notify();
    }

    /// `showPicker`: above the emoji (below it when there is no room), its x
    /// proportional to the emoji's column.
    fn show_emoji_picker(&mut self, target: Over, layout: &ListLayout) {
        let (variants, all, coef, y, height) = match target {
            Over::Emoji { section, index } => {
                let Some(emoji) = self.emoji_at(section, index).and_then(tdr_emoji::get) else {
                    return;
                };
                let variants = (0..=5).map(|i| emoji.variant(i)).collect();
                let coef = (index % layout.columns) as f32 / (layout.columns.max(2) - 1) as f32;
                let r = layout.emoji_rect(section, index);
                (
                    variants,
                    false,
                    coef,
                    f32::from(r.origin.y),
                    layout.single.1 - 2. * lg(PAN_RADIUS),
                )
            }
            Over::ColorAll => {
                let Some(hand) = tdr_emoji::by_text("👋").and_then(tdr_emoji::get) else {
                    return;
                };
                let r = layout.color_all_rect().unwrap();
                (
                    (0..=5).map(|i| hand.variant(i)).collect(),
                    true,
                    1.,
                    f32::from(r.origin.y),
                    f32::from(r.size.height) - 2. * lg(PAN_RADIUS),
                )
            }
            Over::None => return,
        };
        let (pw, ph) = picker_size(layout.single, all);
        let visible_top = -f32::from(self.emoji_panel.scroll.offset().y);
        let width = self.emoji_panel.list_width;
        let mut py = y - ph + lg(PAN_RADIUS);
        if py - visible_top < lg(HEADER) {
            py += ph + height;
        }
        let px = ((width - pw) * coef).round();
        self.emoji_panel.picker = Some(Picker {
            variants,
            all,
            at: (px, py),
            shown: Instant::now(),
            hiding: None,
            selected: None,
            target,
        });
    }

    /// `colorChosen`.
    fn emoji_color_chosen(&mut self, variant: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.emoji_panel.picker else {
            return;
        };
        let Some(&emoji) = picker.variants.get(variant) else {
            return;
        };
        picker.hiding = Some(Instant::now());
        let target = picker.target;
        match target {
            Over::ColorAll => {
                self.emoji_panel.recent.save_all_variants(emoji);
                self.emoji_panel.save_recent();
                self.emoji_panel.refresh();
            }
            Over::Emoji { section, index } => {
                self.emoji_panel.recent.save_variant(emoji);
                self.emoji_panel.save_recent();
                if section > 0
                    && let Some(slot) = self
                        .emoji_panel
                        .lists
                        .get_mut(section)
                        .and_then(|l| l.get_mut(index))
                {
                    *slot = emoji;
                }
                self.choose_emoji(emoji, window, cx);
            }
            Over::None => {}
        }
        cx.notify();
    }

    /// `fillRecentMenu`: for the recent emoji only.
    fn open_emoji_menu(&mut self, at: Point<Pixels>) {
        let Over::Emoji { section: 0, index } = self.emoji_panel.over else {
            return;
        };
        if self.emoji_panel.results.is_some() {
            return;
        }
        let Some(emoji) = self.emoji_at(0, index) else {
            return;
        };
        let text = tdr_emoji::get(emoji).map(|e| e.text()).unwrap_or_default();
        let entries = vec![
            Entry::action("Copy emoji", Icon::MenuCopy, MenuAction::Copy(text, None)),
            Entry::action(
                "Remove from Recent",
                Icon::MenuCancel,
                MenuAction::EmojiRemoveRecent(emoji),
            ),
            Entry::WideSeparator,
            Entry::attention(
                "Reset recents",
                Icon::MenuRestore,
                MenuAction::EmojiResetRecent,
            ),
        ];
        self.open_popup(PopupMenu::new(entries, true, at));
    }

    pub(super) fn emoji_remove_recent(&mut self, emoji: u16) {
        self.emoji_panel.recent.hide(emoji);
        self.emoji_panel.save_recent();
        self.emoji_panel.refresh();
    }

    pub(super) fn emoji_reset_recent(&mut self) {
        self.emoji_panel.recent.reset();
        self.emoji_panel.save_recent();
        self.emoji_panel.refresh();
    }

    /// Scrolls to a section (the footer's icons, `showSet`).
    fn emoji_show_section(&mut self, section: usize) {
        if let Some(layout) = &self.emoji_panel.layout
            && let Some(info) = layout.sections.get(section)
        {
            let top = if section == 0 { 0. } else { info.top };
            self.emoji_panel
                .scroll
                .set_offset(point(gpx(0.), gpx(-top)));
        }
    }

    /// The panel over the chat's bottom right corner, `None` when hidden.
    pub(super) fn render_emoji_panel(
        &mut self,
        chat_height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.emoji_panel.section {
            return None;
        }
        self.emoji_panel.menu_open = self.text_ui.menu.as_ref().is_some_and(|m| !m.menu.gone());
        if self.emoji_panel.step() {
            window.request_animation_frame();
        }
        if self.emoji_panel.show == Show::Hidden {
            return None;
        }
        // `updateContentHeight`: three quarters of the room above the field.
        let added = 2. * lg(PAN_MARGIN);
        let margins = lg(TABS_HEIGHT - 1.) + lg(FOOTER);
        let available = chat_height - margins;
        let wanted = (PAN_HEIGHT_RATIO * available).round() - added;
        let content = wanted.clamp(lg(PAN_MIN_HEIGHT), lg(PAN_MAX_HEIGHT));
        let (w, h) = (lg(PAN_WIDTH), margins + content);
        let selector = self.render_selector(w, h, true, window, cx);
        let p = self.palette();

        // `PanelAnimation` (Origin::BottomRight): the frame grows from the corner.
        let (fw, fh, opacity) = match self.emoji_panel.show {
            Show::Unrolling(start) => {
                let dt = (start.elapsed().as_secs_f32() * 1000. / PAN_SHOW_MS).min(1.);
                let fw = w * (0.5 + 0.5 * ease_out_circ((dt / 0.6).min(1.)));
                let fh = h * (0.3 + 0.7 * ease_out_circ((dt / 0.9).min(1.)));
                let opacity = if dt < 0.3 {
                    ease_out_circ(dt / 0.3)
                } else {
                    1.
                };
                (fw, fh, opacity)
            }
            _ => (w, h, self.emoji_panel.opacity()),
        };
        let frame = div()
            .id("emoji-panel")
            .absolute()
            .right(px(PAN_MARGIN))
            .bottom(px(PAN_MARGIN))
            .w(gpx(fw))
            .h(gpx(fh))
            .rounded(px(PAN_RADIUS))
            .bg(c(p.emoji_pan_bg))
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.25),
                offset: point(gpx(0.), px(1.)),
                blur_radius: px(5.),
                spread_radius: gpx(0.),
            }])
            .overflow_hidden()
            .opacity(opacity)
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.emoji_panel_hovered(*hovered, cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .w(gpx(w))
                    .h(gpx(h))
                    .child(selector),
            );
        Some(frame.into_any_element())
    }

    /// The docked selector (`TabbedSection`): the third column.
    pub(super) fn render_emoji_section(
        &mut self,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let width = self.emoji_section_width(window);
        if width <= 0. {
            return None;
        }
        self.ensure_emoji_search(cx);
        if self.emoji_panel.lists.is_empty() {
            self.emoji_panel.refresh();
        }
        if self.emoji_panel.step() {
            window.request_animation_frame();
        }
        let p = self.palette();
        let selector = self.render_selector(width, height, false, window, cx);
        Some(
            div()
                .flex_none()
                .w(gpx(width))
                .h_full()
                .bg(c(p.emoji_pan_bg))
                .border_l_1()
                .border_color(c(p.shadow_fg))
                .child(selector)
                .into_any_element(),
        )
    }

    /// `TabbedSelector`: tabs, the scrolled list and the footer.
    fn render_selector(
        &mut self,
        w: f32,
        h: f32,
        rounded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let tabs_h = lg(TABS_HEIGHT - 1.);
        let footer_h = lg(FOOTER);
        let scroll_h = (h - tabs_h - footer_h).max(0.);
        let list_w = Self::emoji_list_width(w);
        self.emoji_panel.list_width = list_w;
        let counts = self.emoji_counts();
        let layout = ListLayout::new(list_w, &counts, scroll_h);
        self.emoji_panel.layout = Some(layout.clone());

        // The footer's icon follows the scroll (`currentSet`).
        let visible_top = -f32::from(self.emoji_panel.scroll.offset().y);
        let current = if self.emoji_panel.results.is_some() {
            usize::MAX
        } else {
            layout.section_at(visible_top)
        };
        let single_w = ((w - 2. * lg(ICON_SKIP)) / 8.).floor();
        if current != self.emoji_panel.footer && current != usize::MAX {
            let from = self.footer_selection_x(single_w);
            self.emoji_panel.footer = current;
            self.emoji_panel.footer_from = (from, Instant::now());
        }

        let tabs = self.render_selector_tabs(w, window);
        let list = self.render_emoji_list(&layout, list_w, window, cx);
        let footer = self.render_emoji_footer(w, single_w, rounded, window, cx);
        let picker = self.render_emoji_picker(&layout, cx);
        let bar = self.render_scrollbar(super::Area::Emoji, cx);

        div()
            .relative()
            .w(gpx(w))
            .h(gpx(h))
            .bg(c(p.emoji_pan_bg))
            .child(tabs)
            .child(
                div()
                    .id("emoji-scroll-area")
                    .absolute()
                    .left(px(PAN_RADIUS))
                    .top(gpx(tabs_h))
                    .w(gpx(w - lg(PAN_RADIUS)))
                    .h(gpx(scroll_h))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.area_hovered(super::Area::Emoji, *hovered, cx);
                        if !*hovered && this.emoji_panel.over != Over::None {
                            this.emoji_panel.over = Over::None;
                            cx.notify();
                        }
                    }))
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.scrolled(super::Area::Emoji, cx);
                    }))
                    .child(
                        div()
                            .id("emoji-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.emoji_panel.scroll)
                            .child(list),
                    )
                    .children(picker)
                    .children(bar),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(gpx(tabs_h))
                    .w(gpx(w))
                    .h(gpx(lg(1.)))
                    .bg(c(p.shadow_fg)),
            )
            .child(footer)
            .into_any_element()
    }

    fn footer_selection_x(&self, single_w: f32) -> f32 {
        let (from, start) = self.emoji_panel.footer_from;
        let to = self.emoji_panel.footer as f32 * single_w;
        let t = (start.elapsed().as_secs_f32() * 1000. / SELECTION_MS).min(1.);
        from + (to - from) * ease_out_circ(t)
    }

    /// The tabs (`SettingsSlider` with `defaultTabsSlider`): only "Emoji" works yet.
    fn render_selector_tabs(&self, w: f32, window: &mut Window) -> AnyElement {
        let p = self.palette();
        let labels = ["Emoji", "Stickers", "GIFs"];
        let section_w = w / labels.len() as f32;
        let f = scale::font(13.);
        let mut row = div()
            .absolute()
            .left_0()
            .top_0()
            .w(gpx(w))
            .h(px(TABS_HEIGHT))
            .overflow_hidden();
        for (i, label) in labels.iter().enumerate() {
            let active = i == 0;
            let run = TextRun {
                len: label.len(),
                font: gpui::Font {
                    weight: FontWeight::SEMIBOLD,
                    ..font("Open Sans")
                },
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let label_w = f32::from(
                window
                    .text_system()
                    .shape_line(SharedString::from(*label), f.size, &[run], None)
                    .width,
            );
            let left = section_w * i as f32;
            row = row.child(
                div()
                    .absolute()
                    .left(gpx(left))
                    .top(px(TABS_LABEL_TOP))
                    .w(gpx(section_w))
                    .flex()
                    .justify_center()
                    .text_size(f.size)
                    .line_height(f.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(if active {
                        p.light_button_fg
                    } else {
                        p.window_sub_text_fg
                    }))
                    .child(*label),
            );
            if active {
                row = row.child(
                    div()
                        .absolute()
                        .left(gpx(left + ((section_w - label_w) / 2.).floor()))
                        .top(px(TABS_BAR_TOP))
                        .w(gpx(label_w.ceil()))
                        .h(px(6.))
                        .rounded(px(2.))
                        .bg(c(p.active_line_fg)),
                );
            }
        }
        row.into_any_element()
    }

    /// The search field and the emoji (`EmojiListWidget::paint`).
    fn render_emoji_list(
        &mut self,
        layout: &ListLayout,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let size_large = crate::emoji::size_large();
        let over = self.emoji_panel.over;
        let picker_target = self
            .emoji_panel
            .picker
            .as_ref()
            .filter(|p| p.hiding.is_none())
            .map(|p| p.target);
        let lists: Vec<Vec<u16>> = match &self.emoji_panel.results {
            Some(r) => vec![r.clone()],
            None => self.emoji_panel.lists.clone(),
        };
        let searching = self.emoji_panel.results.is_some();
        let origin = self.emoji_panel.origin.clone();
        let painted = layout.clone();
        let header_color = p.emoji_pan_header_fg;
        let over_bg = p.window_bg_over;
        let skin_color = if over == Over::ColorAll {
            p.small_close_icon_fg_over
        } else {
            p.small_close_icon_fg
        };
        let list = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                origin.set(bounds.origin);
                let clip = window.content_mask().bounds;
                let f = scale::font(13.);
                let area = (lg(AREA.0), lg(AREA.1));
                let esize = size_large as f32 / scale::get();
                for info in &painted.sections {
                    let top = bounds.origin.y + gpx(info.top);
                    if top > clip.bottom() || bounds.origin.y + gpx(info.rows_bottom) < clip.top() {
                        continue;
                    }
                    if info.section > 0 && !searching {
                        let title = SECTION_TITLES[info.section - 1];
                        let run = TextRun {
                            len: title.len(),
                            font: gpui::Font {
                                weight: FontWeight::SEMIBOLD,
                                ..font("Open Sans")
                            },
                            color: c(header_color).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            SharedString::from(title),
                            f.size,
                            &[run],
                            None,
                        );
                        let _ = shaped.paint(
                            point(
                                bounds.origin.x + gpx(lg(HEADER_LEFT) - lg(MARGIN_LEFT)),
                                top + px(HEADER_TOP),
                            ),
                            f.line_height(),
                            window,
                            cx,
                        );
                    }
                    let Some(list) = lists.get(info.section) else {
                        continue;
                    };
                    for (index, emoji) in list.iter().enumerate() {
                        let r = painted.emoji_rect(info.section, index);
                        let y = bounds.origin.y + r.origin.y;
                        if y > clip.bottom() || y + r.size.height < clip.top() {
                            continue;
                        }
                        let x = bounds.origin.x + r.origin.x;
                        let ax = x + gpx(((f32::from(r.size.width) - area.0) / 2.).floor());
                        let ay = y + gpx(((f32::from(r.size.height) - area.1) / 2.).floor());
                        let state = Over::Emoji {
                            section: info.section,
                            index,
                        };
                        if state == over || Some(state) == picker_target {
                            window.paint_quad(
                                gpui::fill(
                                    Bounds::new(
                                        point(ax, ay),
                                        gpui::size(gpx(area.0), gpx(area.1)),
                                    ),
                                    c(over_bg),
                                )
                                .corner_radii(Corners::all(px(PAN_RADIUS))),
                            );
                        }
                        let at = point(
                            ax + gpx(((area.0 - esize) / 2.).floor()),
                            ay + gpx(((area.1 - esize) / 2.).floor()),
                        );
                        crate::emoji::paint(*emoji, at, size_large, window);
                    }
                }
                let _ = skin_color;
            },
        )
        .w(gpx(width))
        .h(gpx(layout.height));

        let layout_move = layout.clone();
        let layout_down = layout.clone();
        let color_all = (!searching)
            .then(|| layout.color_all_rect())
            .flatten()
            .map(|r| {
                div()
                    .absolute()
                    .left(r.origin.x)
                    .top(r.origin.y)
                    .size(r.size.width)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(Icon::EmojiSkin, skin_color, window))
            });
        let search = self.emoji_panel.search.clone().map(|field| {
            let [ml, mt, mr, _] = SEARCH_MARGIN;
            div()
                .absolute()
                .left(gpx(lg(ml)))
                .top(px(mt))
                .w(gpx(width - lg(ml) - lg(mr)))
                .h(px(SEARCH_HEIGHT))
                .rounded(px(SEARCH_HEIGHT / 2.))
                .bg(c(p.window_bg_over))
                .flex()
                .items_center()
                .cursor_text()
                .on_mouse_down(MouseButton::Left, {
                    let field = field.clone();
                    move |_, window, cx| {
                        window.focus(&field.focus_handle(cx));
                        cx.stop_propagation();
                    }
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(33.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(Icon::EmojiSearch, p.emoji_icon_fg, window)),
                )
                .child(div().flex_1().min_w(px(0.)).child(field))
        });
        div()
            .id("emoji-list")
            .relative()
            .w(gpx(width))
            .h(gpx(layout.height))
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                this.emoji_mouse_move(e.position, &layout_move, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.emoji_mouse_down(&layout_down, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.emoji_mouse_up(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                    this.open_emoji_menu(e.position);
                    cx.notify();
                }),
            )
            .when(matches!(over, Over::Emoji { .. } | Over::ColorAll), |d| {
                d.cursor_pointer()
            })
            .child(list)
            .children(color_all)
            .children(search)
            .into_any_element()
    }

    /// `EmojiColorPicker`.
    fn render_emoji_picker(
        &mut self,
        layout: &ListLayout,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.palette();
        let picker = self.emoji_panel.picker.as_ref()?;
        let opacity = match picker.hiding {
            Some(h) => 1. - (h.elapsed().as_secs_f32() * 1000. / PAN_FADE_MS).min(1.),
            None => (picker.shown.elapsed().as_secs_f32() * 1000. / PAN_FADE_MS).min(1.),
        };
        let (pw, ph) = picker_size(layout.single, picker.all);
        let visible_top = -f32::from(self.emoji_panel.scroll.offset().y);
        let (x, y) = (picker.at.0, picker.at.1 - visible_top);
        let m = lg(PAN_MARGIN);
        let pad = lg(COLORS_PADDING);
        let top_skip = if picker.all { color_all_skip() } else { 0. };
        let area = (lg(AREA.0), lg(AREA.1));
        let esize = crate::emoji::size_large() as f32 / scale::get();
        let (sw, sh) = layout.single;
        let mut cells = Vec::new();
        for (i, emoji) in picker.variants.iter().enumerate() {
            let cx0 = m + pad + i as f32 * sw + if i > 0 { 2. * pad + lg(COLORS_SEP) } else { 0. };
            let cy0 = m + top_skip + pad;
            let selected = picker.selected == Some(i);
            let emoji = *emoji;
            cells.push(
                div()
                    .id(("emoji-variant", i))
                    .absolute()
                    .left(gpx(cx0))
                    .top(gpx(cy0))
                    .w(gpx(sw))
                    .h(gpx(sh))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let Some(p) = &mut this.emoji_panel.picker {
                            if *hovered {
                                p.selected = Some(i);
                            } else if p.selected == Some(i) {
                                p.selected = None;
                            }
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.emoji_color_chosen(i, window, cx);
                        }),
                    )
                    .when(selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(gpx(((sw - area.0) / 2.).floor()))
                                .top(gpx(((sh - area.1) / 2.).floor()))
                                .w(gpx(area.0))
                                .h(gpx(area.1))
                                .rounded(px(PAN_RADIUS))
                                .bg(c(p.window_bg_over)),
                        )
                    })
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let at = point(
                                    bounds.origin.x + gpx(((f32::from(bounds.size.width) - esize) / 2.).floor()),
                                    bounds.origin.y + gpx(((f32::from(bounds.size.height) - esize) / 2.).floor()),
                                );
                                crate::emoji::paint(emoji, at, crate::emoji::size_large(), window);
                            },
                        )
                        .size_full(),
                    ),
            );
        }
        let label = picker.all.then(|| {
            let f = scale::font(12.);
            div()
                .absolute()
                .left(gpx(m + lg(COLOR_ALL_PADDING[0])))
                .top(gpx(m + lg(COLOR_ALL_PADDING[1])))
                .w(gpx(pw
                    - 2. * m
                    - lg(COLOR_ALL_PADDING[0])
                    - lg(COLOR_ALL_PADDING[2])))
                .flex()
                .justify_center()
                .text_size(f.size)
                .line_height(f.line_height())
                .text_color(c(p.window_sub_text_fg))
                .child("Choose color for all emoji")
        });
        let inner_h = ph - 2. * m;
        Some(
            div()
                .id("emoji-picker")
                .absolute()
                .left(gpx(x))
                .top(gpx(y))
                .w(gpx(pw))
                .h(gpx(ph))
                .opacity(opacity)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .absolute()
                        .left(gpx(m))
                        .top(gpx(m))
                        .w(gpx(pw - 2. * m))
                        .h(gpx(inner_h))
                        .rounded(px(PAN_RADIUS))
                        .bg(c(p.emoji_pan_bg))
                        .shadow(vec![gpui::BoxShadow {
                            color: gpui::hsla(0., 0., 0., 0.25),
                            offset: point(gpx(0.), px(1.)),
                            blur_radius: px(5.),
                            spread_radius: gpx(0.),
                        }]),
                )
                .child(
                    div()
                        .absolute()
                        .left(gpx(m + 2. * pad + sw))
                        .top(gpx(m + top_skip + pad))
                        .w(gpx(lg(COLORS_SEP)))
                        .h(gpx(inner_h - 2. * pad - top_skip))
                        .bg(c(p.shadow_fg)),
                )
                .children(label)
                .children(cells)
                .into_any_element(),
        )
    }

    /// The footer (`StickersListFooter` without custom sets): the recent emoji and
    /// the sections, the selected one on a rounded background that slides.
    fn render_emoji_footer(
        &mut self,
        w: f32,
        single_w: f32,
        rounded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let icons = [
            Icon::EmojiRecent,
            Icon::EmojiSmile,
            Icon::EmojiNature,
            Icon::EmojiFood,
            Icon::EmojiActivities,
            Icon::EmojiTravel,
            Icon::MenuEmojiObjects,
            Icon::EmojiLove,
        ];
        let footer_h = lg(FOOTER);
        let left = lg(ICON_SKIP);
        let area_x = ((single_w - lg(ICON_AREA)) / 2.).floor();
        let area_y = ((footer_h - lg(ICON_AREA)) / 2.).floor();
        let searching = self.emoji_panel.results.is_some();
        let sel_x = self.footer_selection_x(single_w);
        let mut row = div()
            .absolute()
            .left_0()
            .bottom_0()
            .w(gpx(w))
            .h(gpx(footer_h))
            .bg(c(p.emoji_pan_categories))
            .when(rounded, |d| {
                d.rounded_bl(px(PAN_RADIUS)).rounded_br(px(PAN_RADIUS))
            });
        if !searching {
            row = row.child(
                div()
                    .absolute()
                    .left(gpx(left + sel_x + area_x))
                    .top(gpx(area_y))
                    .w(gpx(single_w - 2. * area_x))
                    .h(px(ICON_AREA))
                    .rounded(px(PAN_RADIUS))
                    .bg(c(p.window_bg_ripple)),
            );
        }
        for (i, glyph) in icons.into_iter().enumerate() {
            let active = !searching && i == self.emoji_panel.footer;
            row = row.child(
                div()
                    .id(("emoji-footer", i))
                    .absolute()
                    .left(gpx(left + i as f32 * single_w))
                    .top_0()
                    .w(gpx(single_w))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon(
                        glyph,
                        if active {
                            p.emoji_sub_icon_fg_active
                        } else {
                            p.emoji_icon_fg
                        },
                        window,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(field) = this.emoji_panel.search.clone()
                            && this.emoji_panel.results.is_some()
                        {
                            field.update(cx, |f, cx| f.set_text(String::new(), cx));
                        }
                        this.emoji_show_section(i);
                        cx.notify();
                    })),
            );
        }
        row.into_any_element()
    }
}

/// The picker's size with its shadow margins (`updateSize`).
fn picker_size(single: (f32, f32), all: bool) -> (f32, f32) {
    let m = lg(PAN_MARGIN);
    let width = m + single.0 * 6. + 4. * lg(COLORS_PADDING) + lg(COLORS_SEP) + m;
    let mut height = m + 2. * lg(COLORS_PADDING) + single.1 + m;
    if all {
        height += color_all_skip();
    }
    (width, height)
}

/// `topColorAllSkip`: the label's padding and one line of 12 px text.
fn color_all_skip() -> f32 {
    lg(COLOR_ALL_PADDING[1]) + scale::font(12.).height + lg(COLOR_ALL_PADDING[3])
}
