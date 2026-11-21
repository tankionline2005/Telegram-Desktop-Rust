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
    /// Byte ranges hidden under spoiler particles, in this color.
    spoilers: Vec<std::ops::Range<usize>>,
    spoiler_color: Option<tdr_theme::Color>,
}

/// One line of `text` in the current text style, as wide as its parent, cut with "…".
pub fn one_line(text: impl Into<SharedString>) -> OneLine {
    OneLine {
        text: text.into(),
        centered: false,
        spoilers: Vec::new(),
        spoiler_color: None,
    }
}

impl OneLine {
    /// Centered in its width (`style::al_top` labels).
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Hides these byte ranges under spoiler particles of `color` (a chat list's
    /// last message, `Ui::Text::String` with `SpoilerLoginCode`).
    pub fn spoilers(
        mut self,
        ranges: Vec<std::ops::Range<usize>>,
        color: tdr_theme::Color,
    ) -> Self {
        self.spoilers = ranges;
        self.spoiler_color = Some(color);
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
    /// The spoilers' x ranges in the shaped line.
    spoilers: Vec<(Pixels, Pixels)>,
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
        // Spoiler text is shaped transparent: the particles take its place.
        let spoilers: Vec<std::ops::Range<usize>> = self
            .spoilers
            .iter()
            .filter(|r| r.start < r.end && r.end <= text.len())
            .filter(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
            .cloned()
            .collect();
        let shape = |window: &mut Window, text: SharedString| {
            let mut runs = Vec::new();
            let mut at = 0;
            for r in spoilers.iter().filter(|r| r.end <= text.len()) {
                if r.start > at {
                    runs.push(style.to_run(r.start - at));
                }
                let mut hidden = style.to_run(r.end - r.start.max(at));
                hidden.color.a = 0.;
                runs.push(hidden);
                at = r.end;
            }
            if text.len() > at {
                runs.push(style.to_run(text.len() - at));
            }
            if runs.is_empty() {
                runs.push(style.to_run(text.len()));
            }
            window
                .text_system()
                .shape_line(text, font_size, &runs, None)
        };
        let ranges = |line: &ShapedLine, len: usize| -> Vec<(Pixels, Pixels)> {
            spoilers
                .iter()
                .filter(|r| r.start < len)
                .map(|r| (line.x_for_index(r.start), line.x_for_index(r.end.min(len))))
                .collect()
        };
        let full = shape(window, text.clone());
        let width = bounds.size.width;
        if full.width <= width {
            let spoilers = ranges(&full, text.len());
            return Prepaint {
                line: full,
                line_height,
                spoilers,
            };
        }
        let ellipsis_width = shape(window, ELLIPSIS.into()).width;
        let room = (width - ellipsis_width).max(px(0.));
        let mut cut = full.closest_index_for_x(room).min(text.len());
        while cut > 0 && (!text.is_char_boundary(cut) || full.x_for_index(cut) > room) {
            cut -= 1;
        }
        let kept = text[..cut].trim_end().len();
        let cut_text: SharedString = format!("{}{ELLIPSIS}", &text[..kept]).into();
        let line = shape(window, cut_text);
        let spoilers = ranges(&line, kept);
        Prepaint {
            line,
            line_height,
            spoilers,
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
            if let Some(color) = self.spoiler_color
                && !prepaint.spoilers.is_empty()
            {
                let frame = crate::spoiler::current_frame();
                for (x0, x1) in &prepaint.spoilers {
                    let rect = Bounds::new(
                        gpui::point(origin.x + *x0, origin.y),
                        gpui::size(*x1 - *x0, prepaint.line_height),
                    );
                    crate::spoiler::fill(rect, origin, color, frame, window);
                }
                window.request_animation_frame();
            }
        });
    }
}
