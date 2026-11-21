//! The country list of the login's phone step (`Ui::CountrySelectBox`): a search
//! field over the countries with their calling codes; a click, or Enter on the
//! selected row, picks one.

use gpui::{
    AnyElement, AppContext, Context, Entity, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, StatefulInteractiveElement, Styled,
    Window, div,
};
use tdr_data::countries::{self, Country};

use super::{MainView, TITLE_HEIGHT, c, text_width};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, lg, px};
use crate::text_input::{Submit, TextInput};

/// `boxWidth`, `boxMaxListHeight`, the title (`boxTitlePosition`, `boxTitleHeight`).
const BOX_WIDTH: f32 = 320.;
const MAX_LIST_HEIGHT: f32 = 492.;
const TITLE_POSITION: (f32, f32) = (24., 13.);
const TITLE_HEIGHT_BOX: f32 = 48.;
/// The buttons row (`defaultBox.buttonPadding` and height).
const BUTTONS_HEIGHT: f32 = 54.;
/// `defaultMultiSelect`: padding (8, 6, 8, 6), the 32 px field after the icon.
const SELECT_HEIGHT: f32 = 44.;
const SELECT_ICON: (f32, f32) = (18., 15.);
const SELECT_FIELD: (f32, f32) = (46., 13.);
/// `countryRowHeight`, `countryRowPadding` (22, 9, 8, 0), `countriesSkip`.
const ROW_HEIGHT: f32 = 36.;
const ROW_LEFT: f32 = 22.;
const ROW_TOP: f32 = 9.;
const ROW_CODE_SKIP: f32 = 8.;
const LIST_SKIP: f32 = 12.;
/// `noContactsHeight`, `boxScroll.width`.
const EMPTY_HEIGHT: f32 = 100.;
const SCROLL_WIDTH: f32 = 10.;
const FONT: f32 = 13.;

pub(super) struct CountryBox {
    list: Vec<Country>,
    search: Entity<TextInput>,
    query: String,
    /// The shown entries (all, or those matching the search).
    filtered: Vec<usize>,
    /// The row under the mouse or picked with the arrows (`_selected`).
    selected: Option<usize>,
    scroll: gpui::ScrollHandle,
    focus_pending: bool,
}

impl MainView {
    /// `CountryInput::mousePressEvent`: the list over the login.
    pub(super) fn open_country_box(&mut self, cx: &mut Context<Self>) {
        let Some((intro, _)) = &self.intro else {
            return;
        };
        let list = intro.read(cx).countries().to_vec();
        let palette = self.palette();
        let search = cx.new(|cx| {
            let mut field = TextInput::new("Search", palette, cx);
            field.set_single_line(true);
            field
        });
        cx.observe(&search, |this: &mut Self, _, cx| {
            this.country_query_changed(cx)
        })
        .detach();
        cx.subscribe(&search, |this: &mut Self, _, _: &Submit, cx| {
            this.choose_country_row(None, cx);
        })
        .detach();
        self.country_box = Some(CountryBox {
            filtered: (0..list.len()).collect(),
            list,
            search,
            query: String::new(),
            selected: None,
            scroll: gpui::ScrollHandle::new(),
            focus_pending: true,
        });
        self.layers.show_layer(super::layers::LayerBox::Countries);
        cx.notify();
    }

    /// `Inner::updateFilter`: the rows whose name parts start with the words.
    fn country_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(b) = &mut self.country_box else {
            return;
        };
        let query = b.search.read(cx).text();
        let words = countries::search_words(&query).join(" ");
        if words == b.query {
            return;
        }
        b.query = words;
        b.filtered = countries::search(&b.list, &query);
        b.selected = (!b.filtered.is_empty()).then_some(0);
        b.scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        cx.notify();
    }

    /// `Inner::chooseCountry`: the box closes; a row picked goes to the login.
    fn choose_country_row(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        let Some(b) = &self.country_box else {
            return;
        };
        let country = row
            .or(b.selected)
            .and_then(|r| b.filtered.get(r))
            .map(|&i| b.list[i].clone());
        self.layers.hide_layer();
        if let (Some(country), Some((intro, _))) = (country, &self.intro) {
            intro.update(cx, |v, cx| v.country_picked(&country, cx));
        }
        cx.notify();
    }

    /// `selectSkip`: the arrows move the selection, the list follows it.
    fn country_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(b) = &mut self.country_box else {
            return;
        };
        let page = ((lg(MAX_LIST_HEIGHT) - lg(SELECT_HEIGHT)) / lg(ROW_HEIGHT)).floor() as i64;
        let step: i64 = match e.keystroke.key.as_str() {
            "down" => 1,
            "up" => -1,
            "pagedown" => page,
            "pageup" => -page,
            _ => return,
        };
        let count = b.filtered.len() as i64;
        let index = b.selected.map_or(-1, |s| s as i64) + step;
        b.selected = if index <= 0 {
            (count > 0).then_some(0)
        } else if index >= count {
            None
        } else {
            Some(index as usize)
        };
        if let Some(s) = b.selected {
            // The first child is the skip above the rows.
            b.scroll.scroll_to_item(s + 1);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_country_box(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let Some(b) = &mut self.country_box else {
            return div().into_any_element();
        };
        if std::mem::take(&mut b.focus_pending) {
            window.focus(&b.search.focus_handle(cx));
        }
        let width = lg(BOX_WIDTH);
        let font = scale::font(FONT);
        let title_font = scale::font(16.);
        // The list's height: its rows, at most `boxMaxListHeight` with the field,
        // and within the window.
        let window_h = f32::from(window.viewport_size().height) - lg(TITLE_HEIGHT);
        let rows_h = if b.filtered.is_empty() {
            lg(EMPTY_HEIGHT)
        } else {
            lg(LIST_SKIP) + b.filtered.len() as f32 * lg(ROW_HEIGHT)
        };
        let room =
            window_h - lg(TITLE_HEIGHT_BOX) - lg(SELECT_HEIGHT) - lg(BUTTONS_HEIGHT) - 2. * lg(20.);
        let list_h = rows_h
            .min(lg(MAX_LIST_HEIGHT) - lg(SELECT_HEIGHT))
            .min(room.max(lg(ROW_HEIGHT)));

        let mut rows = div()
            .id("country-rows")
            .h(gpui::px(list_h))
            .overflow_y_scroll()
            .track_scroll(&b.scroll)
            .flex()
            .flex_col();
        if b.filtered.is_empty() {
            rows = rows.child(
                div()
                    .h(px(EMPTY_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(font.size)
                    .text_color(c(p.window_sub_text_fg))
                    .child("Country not found"),
            );
        } else {
            rows = rows.child(div().flex_none().h(px(LIST_SKIP)));
            for (row, &i) in b.filtered.iter().enumerate() {
                let country = &b.list[i];
                let selected = b.selected == Some(row);
                let code = format!("+{}", country.code);
                let code_w = text_width(window, &code, FONT, FontWeight::NORMAL);
                let name_max = width - lg(ROW_LEFT) - lg(ROW_CODE_SKIP) - code_w - lg(SCROLL_WIDTH);
                let name_w =
                    text_width(window, &country.name, FONT, FontWeight::SEMIBOLD).min(name_max);
                rows = rows.child(
                    div()
                        .id(("country-row", row))
                        .relative()
                        .flex_none()
                        .h(px(ROW_HEIGHT))
                        .bg(c(if selected {
                            p.window_bg_over
                        } else {
                            p.window_bg
                        }))
                        .cursor_pointer()
                        .child(
                            div()
                                .absolute()
                                .left(px(ROW_LEFT))
                                .top(px(ROW_TOP))
                                .w(gpui::px(name_w.ceil() + 1.))
                                .text_size(font.size)
                                .line_height(font.line_height())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(p.box_text_fg))
                                .child(one_line(country.name.clone())),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(gpui::px(lg(ROW_LEFT) + name_w + lg(ROW_CODE_SKIP)))
                                .top(px(ROW_TOP))
                                .text_size(font.size)
                                .line_height(font.line_height())
                                .text_color(c(if selected {
                                    p.window_sub_text_fg_over
                                } else {
                                    p.window_sub_text_fg
                                }))
                                .child(code),
                        )
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if let Some(b) = &mut this.country_box {
                                if *hovered {
                                    b.selected = Some(row);
                                } else if b.selected == Some(row) {
                                    b.selected = None;
                                }
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_country_row(Some(row), cx);
                        })),
                );
            }
        }
        let search = b.search.clone();
        let select = div()
            .flex_none()
            .relative()
            .h(px(SELECT_HEIGHT))
            .bg(c(p.box_search_bg))
            .cursor(gpui::CursorStyle::IBeam)
            .child(
                div()
                    .absolute()
                    .left(px(SELECT_ICON.0))
                    .top(px(SELECT_ICON.1))
                    .child(icon(Icon::BoxSearch, p.menu_icon_fg, window)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(SELECT_FIELD.0))
                    .top(px(SELECT_FIELD.1))
                    .right(px(10.))
                    .child(search.clone()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| {
                    window.focus(&search.focus_handle(cx));
                }),
            );
        let body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(TITLE_HEIGHT_BOX))
                    .pl(px(TITLE_POSITION.0))
                    .pt(px(TITLE_POSITION.1))
                    .text_size(title_font.size)
                    .line_height(title_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.box_title_fg))
                    .child("Select Country"),
            )
            .child(select)
            .child(rows);
        let close = self.box_button(
            "countries-close",
            "Close",
            false,
            window,
            cx,
            |this, _, _| {
                this.layers.hide_layer();
            },
        );
        self.box_frame(BOX_WIDTH, body, vec![close])
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.country_key(e, cx)))
            .into_any_element()
    }
}
