//! One line of text, cut with "…" to the width it gets (chat names, last messages,
//! drafts).
//!
//! GPUI 0.2.2's own `text_ellipsis()` is unusable: `LineWrapper::truncate_line`
//! shortens the text runs in place, and the measure closure that owns them runs
//! several times with different widths. From the second pass on the runs no longer
//! match the text, and with multi-byte text (Cyrillic) a run ends inside a character:
//! `layout_line` slices the string there and panics, killing the app. (Its `nowrap`
//! text, on the other hand, keeps a stale size and never truncates.) So this element
//! shapes and cuts the line itself, once per paint, from a fresh run.

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, ShapedLine, SharedString, Style, Window, px, relative,
};

const ELLIPSIS: &str = "…";

pub struct OneLine {
    text: SharedString,
    centered: bool,
}

/// One line of `text` in the current text style, as wide as its parent, cut with "…".
pub fn one_line(text: impl Into<SharedString>) -> OneLine {
    OneLine {
        text: text.into(),
        centered: false,
    }
}

impl OneLine {
    /// Centered in its width (`style::al_top` labels).
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
}

impl IntoElement for OneLine {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct Prepaint {
    line: ShapedLine,
    line_height: Pixels,
}

impl Element for OneLine {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_height = window.text_style().line_height_in_pixels(window.rem_size());
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.min_size.width = px(0.).into();
        style.size.height = line_height.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height_in_pixels(window.rem_size());
        // Text from the outside world may hold line breaks: one line shows them as spaces.
        let text: SharedString = if self.text.contains(['\n', '\r']) {
            self.text.replace(['\n', '\r'], " ").into()
        } else {
            self.text.clone()
        };
        let shape = |window: &mut Window, text: SharedString| {
            let run = style.to_run(text.len());
            window
                .text_system()
                .shape_line(text, font_size, &[run], None)
        };
        let full = shape(window, text.clone());
        let width = bounds.size.width;
        if full.width <= width {
            return Prepaint {
                line: full,
                line_height,
            };
        }
        let ellipsis_width = shape(window, ELLIPSIS.into()).width;
        let room = (width - ellipsis_width).max(px(0.));
        let mut cut = full.closest_index_for_x(room).min(text.len());
        while cut > 0 && (!text.is_char_boundary(cut) || full.x_for_index(cut) > room) {
            cut -= 1;
        }
        let cut_text: SharedString = format!("{}{ELLIPSIS}", text[..cut].trim_end()).into();
        Prepaint {
            line: shape(window, cut_text),
            line_height,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut origin = bounds.origin;
        if self.centered {
            origin.x += ((bounds.size.width - prepaint.line.width) / 2.)
                .max(px(0.))
                .floor();
        }
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            let _ = prepaint
                .line
                .paint(origin, prepaint.line_height, window, cx);
        });
    }
}
