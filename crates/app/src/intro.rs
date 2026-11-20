//! The login screens (tdesktop's `Intro::Widget`): the QR code step, the phone
//! number, the code and the cloud password. Shown in the window when there is no
//! account yet and for "Add Account".

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, RenderImage, Styled, Window, canvas, div,
    img, prelude::*,
};
use tdr_data::login::{Country, LoginCommand, LoginEvent};
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, px};

fn c(color: Color) -> gpui::Rgba {
    gpui::rgba(color.rgba())
}

// intro.style
const STEP_WIDTH: f32 = 300.; // introNextButton.width
const INTRO_HEIGHT: f32 = 406.;
const STEP_TOP_MIN: f32 = 76.;
const NEXT_TOP: f32 = 266.;
const TITLE_TOP: f32 = 1.;
const DESCRIPTION_TOP: f32 = 34.;
const FIELD_TOP: f32 = 96.;
const PHONE_TOP: f32 = 6.;
const LINK_TOP: f32 = 24.;
const ERROR_TOP: f32 = 235.;
const PASSWORD_TOP: f32 = 74.;
const PASSWORD_HINT_TOP: f32 = 151.;
const QR_LOGIN_LINK_TOP: f32 = 368.;
const QR_TOP: f32 = -18.;
const QR_MAX: f32 = 180.;
const QR_SKIP: f32 = 12.;
const QR_RADIUS: f32 = 8.;
const QR_TITLE_TOP: f32 = 196.;
const QR_STEPS_TOP: f32 = 232.;
const QR_STEPS_WIDTH: f32 = 292.;
const QR_STEP_LEFT: f32 = 20.;
const QR_STEP_TOP: f32 = 8.;
const QR_SKIP_TOP: f32 = 360.;
const QR_ERROR_TOP: f32 = 336.;
const QR_CENTER: f32 = 44.;
const BUTTON_RADIUS: f32 = 3.; // st::buttonRadius
const NEXT_HEIGHT: f32 = 42.;
const NEXT_RADIUS: f32 = 6.;
const BACK_SIZE: f32 = 56.;
// introCountry / introPhone / introCountryCode / introPassword
const FIELD_HEIGHT: f32 = 61.;
const FIELD_TEXT_TOP: f32 = 27.;
const FIELD_FONT: f32 = 16.;
const CODE_FIELD_WIDTH: f32 = 64.;
const PHONE_FIELD_WIDTH: f32 = 225.;
// introCodeDigit*
const DIGIT_HEIGHT: f32 = 50.;
const DIGIT_SKIP: f32 = 10.;
const DIGIT_BORDER: f32 = 4.;
const DIGIT_FONT: f32 = 20.;
const BOX_RADIUS: f32 = 3.;
/// `defaultInputField`: the placeholder floats up 20 px, scaled to 0.9.
const PLACEHOLDER_SHIFT: f32 = -20.;
const CARET_BLINK: Duration = Duration::from_millis(530);

/// The login screen asks the window to go back, or is done.
pub enum IntroEvent {
    Back,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Qr,
    Phone,
    Code,
    Password,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldId {
    CountryCode,
    Phone,
    Password,
}

/// A one-line field of the login steps (`Ui::InputField` with `introCountry`
/// styles): an underline, a floating placeholder, 16 px text.
struct Field {
    focus: FocusHandle,
    text: String,
    placeholder: &'static str,
    masked: bool,
    digits: bool,
    max: usize,
    error: bool,
}

impl Field {
    fn new(
        placeholder: &'static str,
        masked: bool,
        digits: bool,
        max: usize,
        cx: &mut Context<IntroView>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            placeholder,
            masked,
            digits,
            max,
            error: false,
        }
    }

    fn shown(&self) -> String {
        if self.masked {
            "\u{2022}".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }
}

pub struct IntroView {
    palette: &'static Palette,
    commands: Option<mpsc::UnboundedSender<LoginCommand>>,
    step: Step,
    can_back: bool,
    qr: Option<(Arc<RenderImage>, f32)>,
    qr_url: Option<String>,
    qr_shown: Instant,
    radial: Instant,
    countries: Vec<Country>,
    country_name: String,
    fields: [Field; 3],
    code_digits: Vec<Option<u8>>,
    code_current: usize,
    code_focus: FocusHandle,
    code_error: bool,
    phone: String,
    via_app: bool,
    can_resend: bool,
    hint: String,
    error: Option<String>,
    busy: bool,
    hovered: Option<&'static str>,
    blink: Instant,
    /// The step changed without a window at hand: its field takes the focus when
    /// drawn.
    focus_pending: bool,
}

impl EventEmitter<IntroEvent> for IntroView {}

impl IntroView {
    /// `login`: the login thread's channels (`None`: a demo, nothing happens).
    pub fn new(
        palette: &'static Palette,
        can_back: bool,
        login: Option<(
            mpsc::UnboundedSender<LoginCommand>,
            mpsc::UnboundedReceiver<LoginEvent>,
        )>,
        cx: &mut Context<Self>,
    ) -> Self {
        let commands = match login {
            Some((commands, mut events)) => {
                cx.spawn(async move |this, cx| {
                    while let Some(event) = events.recv().await {
                        if this.update(cx, |v, cx| v.handle(event, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                Some(commands)
            }
            None => None,
        };
        // The caret blinks.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let fields = [
            Field::new("", false, true, 4, cx),
            Field::new("", false, true, 15, cx),
            Field::new("Your cloud password", true, false, 256, cx),
        ];
        let mut this = Self {
            palette,
            commands,
            step: Step::Qr,
            can_back,
            qr: None,
            qr_url: None,
            qr_shown: Instant::now(),
            radial: Instant::now(),
            countries: Vec::new(),
            country_name: String::new(),
            fields,
            code_digits: vec![None; 5],
            code_current: 0,
            code_focus: cx.focus_handle(),
            code_error: false,
            phone: String::new(),
            via_app: true,
            can_resend: false,
            hint: String::new(),
            error: None,
            busy: false,
            hovered: None,
            blink: Instant::now(),
            focus_pending: false,
        };
        if this.commands.is_none() {
            // The demo: a QR code that leads nowhere, a few countries.
            this.qr_url = Some("tg://login?token=demo".into());
            let country = |iso2: &str, name: &str, code: &str, pattern: &str| Country {
                iso2: iso2.into(),
                name: name.into(),
                code: code.into(),
                patterns: vec![pattern.into()],
            };
            this.handle(
                LoginEvent::Countries {
                    list: vec![
                        country("RU", "Russian Federation", "7", "XXX XXX XXXX"),
                        country("US", "USA", "1", "XXX XXX XXXX"),
                        country("GB", "United Kingdom", "44", "XXXX XXXXXX"),
                    ],
                    nearest: "RU".into(),
                },
                cx,
            );
        }
        this
    }

    pub fn set_palette(&mut self, palette: &'static Palette, cx: &mut Context<Self>) {
        self.palette = palette;
        self.qr = None;
        cx.notify();
    }

    fn send(&mut self, command: LoginCommand, cx: &mut Context<Self>) {
        if let Some(c) = &self.commands {
            let _ = c.send(command);
            return;
        }
        // The demo walks through the steps without a server.
        let event = match command {
            LoginCommand::Phone(phone) => LoginEvent::CodeSent {
                phone,
                via_app: true,
                length: 5,
                can_resend: true,
            },
            LoginCommand::Code(_) => LoginEvent::PasswordNeeded {
                hint: "the usual one".into(),
            },
            LoginCommand::Password(_) => {
                LoginEvent::Error("You have entered a wrong password.".into())
            }
            _ => return,
        };
        self.handle(event, cx);
    }

    /// The number grouped by the country's pattern as it is typed
    /// (`PhonePartInput`): "900 123 4567".
    fn phone_shown(&self) -> String {
        let code: String = self.fields[FieldId::CountryCode as usize]
            .text
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let digits = &self.fields[FieldId::Phone as usize].text;
        let pattern = self
            .countries
            .iter()
            .find(|c| c.code == code)
            .and_then(|c| c.patterns.first());
        let Some(pattern) = pattern else {
            return digits.clone();
        };
        let mut out = String::new();
        let mut chars = digits.chars().peekable();
        for p in pattern.chars() {
            if chars.peek().is_none() {
                break;
            }
            if p == ' ' {
                out.push(' ');
            } else if let Some(c) = chars.next() {
                out.push(c);
            }
        }
        out.extend(chars);
        out
    }

    fn handle(&mut self, event: LoginEvent, cx: &mut Context<Self>) {
        self.busy = false;
        match event {
            LoginEvent::Countries { list, nearest } => {
                self.countries = list;
                if self.fields[FieldId::CountryCode as usize].text.is_empty()
                    && let Some(country) = self.countries.iter().find(|c| c.iso2 == nearest)
                {
                    self.fields[FieldId::CountryCode as usize].text = format!("+{}", country.code);
                }
                self.update_country();
            }
            LoginEvent::QrCode(url) => {
                self.qr_url = Some(url);
                self.qr = None;
                self.qr_shown = Instant::now();
                self.error = None;
            }
            LoginEvent::CodeSent {
                phone,
                via_app,
                length,
                can_resend,
            } => {
                self.phone = phone;
                self.via_app = via_app;
                self.can_resend = can_resend;
                self.code_digits = vec![None; length];
                self.code_current = 0;
                self.code_error = false;
                self.error = None;
                self.step = Step::Code;
                self.focus_pending = true;
            }
            LoginEvent::PasswordNeeded { hint } => {
                self.hint = hint;
                self.error = None;
                self.step = Step::Password;
                self.focus_pending = true;
            }
            LoginEvent::Error(text) => {
                match self.step {
                    Step::Phone => self.fields[FieldId::Phone as usize].error = true,
                    Step::Code => {
                        self.code_error = true;
                        self.code_digits.iter_mut().for_each(|d| *d = None);
                        self.code_current = 0;
                    }
                    Step::Password => self.fields[FieldId::Password as usize].error = true,
                    Step::Qr => {}
                }
                self.error = Some(text);
            }
            LoginEvent::Done => cx.emit(IntroEvent::Done),
        }
        cx.notify();
    }

    /// The country of the typed code (`CountryInput::onChooseCode`).
    fn update_country(&mut self) {
        let code: String = self.fields[FieldId::CountryCode as usize]
            .text
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        self.country_name = if code.is_empty() {
            "Country".into()
        } else {
            self.countries
                .iter()
                .find(|c| c.code == code)
                .map_or_else(|| "Invalid country code".into(), |c| c.name.clone())
        };
    }

    fn go(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        match (self.step, step) {
            (Step::Qr, Step::Phone) => self.send(LoginCommand::StopQr, cx),
            (_, Step::Qr) => self.send(LoginCommand::Qr, cx),
            _ => {}
        }
        self.step = step;
        match step {
            Step::Phone => window.focus(&self.fields[FieldId::Phone as usize].focus),
            Step::Code => window.focus(&self.code_focus),
            Step::Password => window.focus(&self.fields[FieldId::Password as usize].focus),
            Step::Qr => {}
        }
        cx.notify();
    }

    /// `Widget::backRequested`: the previous step, else back to the accounts.
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Code | Step::Password => self.go(Step::Phone, window, cx),
            Step::Phone => self.go(Step::Qr, window, cx),
            Step::Qr if self.can_back => cx.emit(IntroEvent::Back),
            Step::Qr => {}
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.step {
            Step::Qr => {}
            Step::Phone => {
                let digits: String = format!(
                    "{}{}",
                    self.fields[FieldId::CountryCode as usize].text,
                    self.fields[FieldId::Phone as usize].text
                )
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
                // `AllowPhoneAttempt`: more than one digit.
                if digits.len() < 2 {
                    self.fields[FieldId::Phone as usize].error = true;
                    self.error = Some("Invalid phone number. Please try again.".into());
                    window.focus(&self.fields[FieldId::Phone as usize].focus);
                } else {
                    self.busy = true;
                    self.send(LoginCommand::Phone(format!("+{digits}")), cx);
                }
            }
            Step::Code => {
                let code: String = self
                    .code_digits
                    .iter()
                    .flatten()
                    .map(|d| char::from(b'0' + d))
                    .collect();
                if code.len() == self.code_digits.len() {
                    self.busy = true;
                    self.send(LoginCommand::Code(code), cx);
                }
            }
            Step::Password => {
                let text = self.fields[FieldId::Password as usize].text.clone();
                if text.is_empty() {
                    self.fields[FieldId::Password as usize].error = true;
                } else {
                    self.busy = true;
                    self.send(LoginCommand::Password(text), cx);
                }
            }
        }
        cx.notify();
    }

    fn field_key(
        &mut self,
        id: FieldId,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &e.keystroke;
        let field = &mut self.fields[id as usize];
        field.error = false;
        self.blink = Instant::now();
        match ks.key.as_str() {
            "backspace" => {
                if ks.modifiers.control {
                    field.text.clear();
                } else {
                    field.text.pop();
                }
                // `PhonePartInput::frontBackspaceEvent`: erasing on into the code.
                if id == FieldId::Phone && field.text.is_empty() && ks.modifiers.control {
                    window.focus(&self.fields[FieldId::CountryCode as usize].focus);
                }
            }
            "enter" => {
                self.submit(window, cx);
                return;
            }
            "tab" => {
                let next = match id {
                    FieldId::CountryCode => FieldId::Phone,
                    FieldId::Phone => FieldId::CountryCode,
                    FieldId::Password => FieldId::Password,
                };
                window.focus(&self.fields[next as usize].focus);
            }
            "v" if ks.modifiers.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.insert(id, &text, window);
                }
            }
            // Typed text comes through the input handler (`replace_text_in_range`).
            _ => return,
        }
        if id == FieldId::CountryCode {
            self.update_country();
        }
        self.error = None;
        cx.stop_propagation();
        cx.notify();
    }

    fn insert(&mut self, id: FieldId, text: &str, window: &mut Window) {
        let field = &mut self.fields[id as usize];
        for ch in text.chars() {
            if field.text.chars().count() >= field.max {
                break;
            }
            if field.digits {
                if ch.is_ascii_digit()
                    || (id == FieldId::CountryCode && ch == '+' && field.text.is_empty())
                {
                    field.text.push(ch);
                }
            } else if !ch.is_control() {
                field.text.push(ch);
            }
        }
        if id == FieldId::CountryCode {
            // The code always starts with "+"; a known code moves on to the number.
            let digits: String = field.text.chars().filter(char::is_ascii_digit).collect();
            field.text = format!("+{digits}");
            if self.countries.iter().any(|c| c.code == digits)
                && !self
                    .countries
                    .iter()
                    .any(|c| c.code.len() > digits.len() && c.code.starts_with(&digits))
            {
                window.focus(&self.fields[FieldId::Phone as usize].focus);
            }
        }
    }

    fn code_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &e.keystroke;
        let count = self.code_digits.len();
        self.code_error = false;
        self.error = None;
        match ks.key.as_str() {
            "right" | "down" | "space" => self.code_current = (self.code_current + 1) % count,
            "left" | "up" => self.code_current = (self.code_current + count - 1) % count,
            "home" => self.code_current = 0,
            "end" => self.code_current = count - 1,
            "delete" => self.code_digits[self.code_current] = None,
            "backspace" => {
                let was = self.code_digits[self.code_current];
                self.code_digits[self.code_current] = None;
                self.code_current = self.code_current.saturating_sub(1);
                if was.is_none() {
                    self.code_digits[self.code_current] = None;
                }
            }
            "enter" => self.submit(window, cx),
            "v" if ks.modifiers.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    let digits: Vec<u8> = text
                        .chars()
                        .filter_map(|c| c.to_digit(10))
                        .map(|d| d as u8)
                        .take(count)
                        .collect();
                    for (i, d) in digits.iter().enumerate() {
                        self.code_digits[i] = Some(*d);
                    }
                    self.code_current = digits.len().min(count - 1);
                    if digits.len() == count {
                        self.submit(window, cx);
                    }
                }
            }
            // Digits come through the input handler.
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A typed digit in the code boxes (`CodeInput::processDigit`).
    fn code_digit(&mut self, d: u8, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.code_digits.len();
        self.code_error = false;
        self.error = None;
        self.code_digits[self.code_current] = Some(d);
        if self.code_current + 1 < count {
            self.code_current += 1;
        } else if self.code_digits.iter().all(Option::is_some) {
            // `CodeInput::codeCollected`: the last digit sends the code.
            self.submit(window, cx);
        }
        cx.notify();
    }

    /// Text typed into whatever is focused (the input handler).
    fn typed(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_focus.is_focused(window) {
            for d in text.chars().filter_map(|c| c.to_digit(10)) {
                self.code_digit(d as u8, window, cx);
            }
            return;
        }
        let focused = [FieldId::CountryCode, FieldId::Phone, FieldId::Password]
            .into_iter()
            .find(|id| self.fields[*id as usize].focus.is_focused(window));
        if let Some(id) = focused {
            self.fields[id as usize].error = false;
            self.blink = Instant::now();
            self.insert(id, text, window);
            if id == FieldId::CountryCode {
                self.update_country();
            }
            self.error = None;
            cx.notify();
        }
    }

    /// Registers the input handler while `focus` is focused (painting).
    fn input_target(focus: &FocusHandle, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let focus = focus.clone();
        let entity = cx.entity();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                window.handle_input(
                    &focus,
                    gpui::ElementInputHandler::new(bounds, entity.clone()),
                    cx,
                );
            },
        )
    }

    // ----- Painting -----

    fn qr_image(&mut self, window: &Window) -> Option<(Arc<RenderImage>, f32)> {
        if self.qr.is_none()
            && let Some(url) = &self.qr_url
        {
            self.qr = render_qr(url, window.scale_factor());
        }
        self.qr.clone()
    }

    fn link(
        &self,
        id: &'static str,
        text: &str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let p = self.palette;
        let over = self.hovered == Some(id);
        div()
            .id(id)
            .cursor_pointer()
            .text_color(c(p.window_active_text_fg))
            .when(over, |d| d.underline())
            .child(text.to_string())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered = if *hovered { Some(id) } else { None };
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
    }

    fn render_field(
        &self,
        id: FieldId,
        width: f32,
        center: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let field = &self.fields[id as usize];
        let focused = field.focus.is_focused(window);
        let shown = if id == FieldId::Phone {
            self.phone_shown()
        } else {
            field.shown()
        };
        let font = scale::font(FIELD_FONT);
        let line = if field.error {
            (p.active_line_fg_error, 2.)
        } else if focused {
            (p.active_line_fg, 2.)
        } else {
            (p.input_border_fg, 1.)
        };
        let side = if id == FieldId::CountryCode { 3. } else { 12. };
        let side = if id == FieldId::Password { 3. } else { side };
        // The floating placeholder (`InputField::paintEvent`).
        let placeholder = (!field.placeholder.is_empty()).then(|| {
            let up = focused || !field.text.is_empty();
            let p_font = scale::font(if up { 14. * 0.9 } else { 14. });
            div()
                .absolute()
                .left(px(side))
                .top(px(FIELD_TEXT_TOP + if up { PLACEHOLDER_SHIFT } else { 0. }))
                .text_size(p_font.size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(if focused {
                    p.placeholder_fg_active
                } else {
                    p.placeholder_fg
                }))
                .child(field.placeholder)
        });
        let text_w = super_text_width(window, &shown, FIELD_FONT, FontWeight::NORMAL);
        let caret_on = focused
            && (self.blink.elapsed().as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2);
        let text_left = if center {
            ((width - text_w) / 2.).max(0.)
        } else {
            side
        };
        let focus = field.focus.clone();
        div()
            .relative()
            .w(px(width))
            .h(px(FIELD_HEIGHT))
            .track_focus(&field.focus)
            .cursor(gpui::CursorStyle::IBeam)
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                this.field_key(id, e, window, cx)
            }))
            .child(Self::input_target(&field.focus, cx).absolute().size_full())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| {
                    window.focus(&focus);
                    cx.notify();
                }),
            )
            .children(placeholder)
            .child(
                div()
                    .absolute()
                    .left(px(text_left))
                    .top(px(FIELD_TEXT_TOP))
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .text_color(c(p.window_fg))
                    .child(shown),
            )
            .when(caret_on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(text_left + text_w))
                        .top(px(FIELD_TEXT_TOP))
                        .w(gpui::px(1. / window.scale_factor()))
                        .h(font.line_height())
                        .bg(c(p.window_fg)),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(line.1))
                    .bg(c(line.0)),
            )
    }

    fn render_next(&self, text: &str, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let p = self.palette;
        let over = self.hovered == Some("intro-next");
        div()
            .id("intro-next")
            .w(px(STEP_WIDTH))
            .h(px(NEXT_HEIGHT))
            .rounded(px(NEXT_RADIUS))
            .bg(c(if over {
                p.active_button_bg_over
            } else {
                p.active_button_bg
            }))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(p.active_button_fg))
            .child(text.to_string())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = if *hovered { Some("intro-next") } else { None };
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
    }

    fn render_qr_step(
        &mut self,
        width: f32,
        top: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = self.palette;
        let qr = self.qr_image(window);
        let size = QR_MAX + 2. * QR_SKIP;
        let shown = (self.qr_shown.elapsed().as_secs_f32() / 0.2).min(1.);
        let radial_start = self.radial;
        let has_qr = qr.is_some();
        let qr_side = qr.as_ref().map_or(QR_MAX, |(_, s)| *s);
        let mut out = Vec::new();
        let mut code = div()
            .absolute()
            .left(px((width - size) / 2.))
            .top(px(top + QR_TOP))
            .size(px(size))
            .child(
                div()
                    .absolute()
                    .left(px((size - qr_side) / 2. - QR_SKIP))
                    .top(px((size - qr_side) / 2. - QR_SKIP))
                    .size(px(qr_side + 2. * QR_SKIP))
                    .rounded(px(QR_RADIUS))
                    .bg(gpui::white()),
            );
        if let Some((image, side)) = qr {
            code = code.child(
                img(image)
                    .absolute()
                    .left(px((size - side) / 2.))
                    .top(px((size - side) / 2.))
                    .size(px(side))
                    .opacity(shown),
            );
        }
        // The Telegram logo in the middle: a blue circle with the plane.
        let logo = div()
            .absolute()
            .left(px((size - QR_CENTER) / 2.))
            .top(px((size - QR_CENTER) / 2.))
            .size(px(QR_CENTER))
            .rounded_full()
            .bg(gpui::rgb(0x40a7e3))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(Icon::IntroQrPlane, p.active_button_fg, window));
        code = code.child(logo);
        if !has_qr {
            // Waiting for the first code: the radial around the logo.
            code = code.child(
                div()
                    .absolute()
                    .left(px((size - QR_CENTER) / 2. - 3.))
                    .top(px((size - QR_CENTER) / 2. - 3.))
                    .size(px(QR_CENTER + 6.))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let ms = radial_start.elapsed().as_secs_f32() * 1000.;
                                let s = f32::from(bounds.size.width);
                                crate::ui::paint_radial(
                                    bounds,
                                    s,
                                    scale::lg(3.),
                                    ms,
                                    Color(0x40a7e3ff),
                                    window,
                                );
                                window.request_animation_frame();
                            },
                        )
                        .size_full(),
                    ),
            );
        }
        if shown < 1. {
            window.request_animation_frame();
        }
        out.push(code.into_any_element());
        out.push(
            div()
                .absolute()
                .left_0()
                .w(gpui::px(width))
                .top(px(top + QR_TITLE_TOP))
                .flex()
                .justify_center()
                .text_size(scale::font(20.).size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(p.intro_title_fg))
                .child("Scan From Mobile Telegram")
                .into_any_element(),
        );
        let steps = [
            "Open Telegram on your phone",
            "Go to Settings > Devices > Link Desktop Device",
            "Scan this image to Log In",
        ];
        let mut list = div()
            .absolute()
            .left(px((width - QR_STEPS_WIDTH) / 2.))
            .top(px(top + QR_STEPS_TOP))
            .w(px(QR_STEPS_WIDTH))
            .flex()
            .flex_col();
        for (i, text) in steps.iter().enumerate() {
            list = list.child(
                div()
                    .relative()
                    .pl(px(QR_STEP_LEFT))
                    .pt(px(QR_STEP_TOP))
                    .text_color(c(p.window_fg))
                    .child(
                        div()
                            .absolute()
                            .left(px(QR_STEP_LEFT - 18.))
                            .top(px(QR_STEP_TOP))
                            .w(px(15.))
                            .flex()
                            .justify_end()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{}.", i + 1)),
                    )
                    .child(*text),
            );
        }
        out.push(list.into_any_element());
        out.push(
            div()
                .absolute()
                .left_0()
                .w(gpui::px(width))
                .top(px(top + QR_SKIP_TOP))
                .flex()
                .justify_center()
                .child(self.link(
                    "intro-to-phone",
                    "Log in using phone number",
                    cx,
                    |this, window, cx| this.go(Step::Phone, window, cx),
                ))
                .into_any_element(),
        );
        out
    }
}

/// tdesktop's `Qr::Generate` (lib_qr): modules with rounded ends that join their
/// neighbours, round finder squares, the middle left empty for the logo.
fn render_qr(url: &str, scale_factor: f32) -> Option<(Arc<RenderImage>, f32)> {
    use resvg::tiny_skia::{
        Color as SkColor, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform,
    };
    let code =
        qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::Q).ok()?;
    let size = code.width() as i32;
    let colors = code.to_colors();
    let dark = |row: i32, col: i32| {
        row >= 0
            && col >= 0
            && row < size
            && col < size
            && colors[(row * size + col) as usize] == qrcode::Color::Dark
    };
    // `ReplaceElements`: the middle third is left out (Quartile).
    let elements = {
        let e = size / 3;
        let shift = (size - e) % 2;
        e + shift
    };
    let from = (size - elements) / 2;
    let till = size - from;
    let value = |row: i32, col: i32| {
        dark(row, col) && (row < from || row >= till || col < from || col >= till)
    };
    // `TelegramQr`: pixel = 180 / size, at least 1, in device pixels.
    let logical_pixel = ((QR_MAX / size as f32).floor()).max(1.);
    let pixel = (logical_pixel * scale_factor).round().max(1.);
    let side = size as f32 * pixel;
    let mut pixmap = Pixmap::new(side as u32, side as u32)?;
    pixmap.fill(SkColor::WHITE);
    let mut fg = Paint::default();
    fg.set_color(SkColor::BLACK);
    fg.anti_alias = true;
    let mut bg = Paint::default();
    bg.set_color(SkColor::WHITE);
    bg.anti_alias = true;
    let rect = |pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, paint: &Paint| {
        if let Some(r) = Rect::from_xywh(x, y, w, h) {
            pixmap.fill_rect(r, paint, Transform::identity(), None);
        }
    };
    let rounded = |pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, paint: &Paint| {
        let k = 0.552_284_8 * r;
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        pb.close();
        if let Some(path) = pb.finish() {
            pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
        }
    };
    let half = (pixel / 2.).floor();
    let skip = pixel - half;
    let black_full = |row: i32, col: i32| {
        (value(row - 1, col) && value(row + 1, col)) || (value(row, col - 1) && value(row, col + 1))
    };
    let white_corner = |row: i32, col: i32, dx: i32, dy: i32| {
        !value(row + dy, col) || !value(row, col + dx) || !value(row + dy, col + dx)
    };
    let white_full = |row: i32, col: i32| {
        white_corner(row, col, -1, -1)
            && white_corner(row, col, 1, -1)
            && white_corner(row, col, 1, 1)
            && white_corner(row, col, -1, 1)
    };
    for row in 0..size {
        for col in 0..size {
            if (row < 7 && (col < 7 || col >= size - 7))
                || (col < 7 && (row < 7 || row >= size - 7))
            {
                continue;
            }
            let (x, y) = (col as f32 * pixel, row as f32 * pixel);
            if value(row, col) {
                if black_full(row, col) {
                    rect(&mut pixmap, x, y, pixel, pixel, &fg);
                } else {
                    rect(&mut pixmap, x, y, pixel, pixel, &bg);
                    rounded(&mut pixmap, x, y, pixel, pixel, pixel / 2., &fg);
                    if value(row - 1, col) {
                        rect(&mut pixmap, x, y, pixel, half, &fg);
                    } else if value(row + 1, col) {
                        rect(&mut pixmap, x, y + skip, pixel, half, &fg);
                    }
                    if value(row, col - 1) {
                        rect(&mut pixmap, x, y, half, pixel, &fg);
                    } else if value(row, col + 1) {
                        rect(&mut pixmap, x + skip, y, half, pixel, &fg);
                    }
                }
            } else if white_full(row, col) {
                rect(&mut pixmap, x, y, pixel, pixel, &bg);
            } else {
                rect(&mut pixmap, x, y, pixel, pixel, &fg);
                rounded(&mut pixmap, x, y, pixel, pixel, pixel / 2., &bg);
                let wc = |dx, dy| white_corner(row, col, dx, dy);
                if wc(-1, -1) && wc(1, -1) {
                    rect(&mut pixmap, x, y, pixel, half, &bg);
                } else if wc(-1, 1) && wc(1, 1) {
                    rect(&mut pixmap, x, y + skip, pixel, half, &bg);
                }
                if wc(-1, -1) && wc(-1, 1) {
                    rect(&mut pixmap, x, y, half, pixel, &bg);
                } else if wc(1, -1) && wc(1, 1) {
                    rect(&mut pixmap, x + skip, y, half, pixel, &bg);
                }
                if wc(-1, -1) {
                    rect(&mut pixmap, x, y, half, half, &bg);
                }
                if wc(1, -1) {
                    rect(&mut pixmap, x + skip, y, half, half, &bg);
                }
                if wc(1, 1) {
                    rect(&mut pixmap, x + skip, y + skip, half, half, &bg);
                }
                if wc(-1, 1) {
                    rect(&mut pixmap, x, y + skip, half, half, &bg);
                }
            }
        }
    }
    let large = |pixmap: &mut Pixmap, x: f32, y: f32| {
        rounded(pixmap, x, y, pixel * 7., pixel * 7., pixel * 2., &fg);
        rounded(
            pixmap,
            x + pixel,
            y + pixel,
            pixel * 5.,
            pixel * 5.,
            pixel * 1.5,
            &bg,
        );
        rounded(
            pixmap,
            x + pixel * 2.,
            y + pixel * 2.,
            pixel * 3.,
            pixel * 3.,
            pixel,
            &fg,
        );
    };
    large(&mut pixmap, 0., 0.);
    large(&mut pixmap, (size - 7) as f32 * pixel, 0.);
    large(&mut pixmap, 0., (size - 7) as f32 * pixel);
    // GPUI wants BGRA.
    let mut out = image::RgbaImage::new(pixmap.width(), pixmap.height());
    for (px, src) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        *px = image::Rgba([c.blue(), c.green(), c.red(), c.alpha()]);
    }
    let frame = image::Frame::new(out);
    Some((
        Arc::new(RenderImage::new(smallvec::smallvec![frame])),
        side / scale_factor,
    ))
}

fn super_text_width(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    crate::ui::text_width(window, text, size, weight)
}

impl Focusable for IntroView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.code_focus.clone()
    }
}

impl Render for IntroView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        if std::mem::take(&mut self.focus_pending) {
            match self.step {
                Step::Code => window.focus(&self.code_focus),
                Step::Password => window.focus(&self.fields[FieldId::Password as usize].focus),
                Step::Phone => window.focus(&self.fields[FieldId::Phone as usize].focus),
                Step::Qr => {}
            }
        }
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height) - scale::lg(crate::ui::TITLE_HEIGHT);
        // `Step::contentTop`, `contentLeft`.
        let top = ((height - scale::lg(INTRO_HEIGHT)) / 2.).max(scale::lg(STEP_TOP_MIN));
        let left = (width - STEP_WIDTH) / 2.;
        let mut root = div()
            .id("intro")
            .relative()
            .size_full()
            .bg(c(p.intro_bg))
            .text_color(c(p.window_fg));
        // The back arrow (`introBackButton`).
        let has_back = match self.step {
            Step::Qr => self.can_back,
            _ => true,
        };
        if has_back {
            let over = self.hovered == Some("intro-back");
            root = root.child(
                div()
                    .id("intro-back")
                    .absolute()
                    .left_0()
                    .top_0()
                    .size(px(BACK_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(40.))
                            .rounded_full()
                            .when(over, |d| d.bg(c(p.window_bg_over)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(
                                Icon::BoxBack,
                                if over {
                                    p.menu_icon_fg_over
                                } else {
                                    p.menu_icon_fg
                                },
                                window,
                            )),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.hovered = if *hovered { Some("intro-back") } else { None };
                        cx.notify();
                    }))
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            );
        }
        // "Settings" (`defaultBoxButton`, upper case): settings come later.
        {
            let over = self.hovered == Some("intro-settings");
            root = root.child(
                div()
                    .id("intro-settings")
                    .absolute()
                    .right(px(10.))
                    .top(px(10.))
                    .h(px(36.))
                    .px(px(12.))
                    .rounded(px(BUTTON_RADIUS))
                    .when(over, |d| d.bg(c(p.light_button_bg_over)))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.light_button_fg))
                    .child("SETTINGS")
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.hovered = if *hovered {
                            Some("intro-settings")
                        } else {
                            None
                        };
                        cx.notify();
                    })),
            );
        }
        let title = |text: String| {
            div()
                .absolute()
                .left(px(left + BUTTON_RADIUS))
                .top(px(top + TITLE_TOP))
                .text_size(scale::font(17.).size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(p.intro_title_fg))
                .child(text)
        };
        let description = |text: &str| {
            div()
                .absolute()
                .left(px(left + BUTTON_RADIUS))
                .top(px(top + DESCRIPTION_TOP))
                .w(px(STEP_WIDTH))
                .line_height(px(20.))
                .text_color(c(p.intro_description_fg))
                .child(text.to_string())
        };
        let error_at = |top_y: f32, centered: bool, text: &str| {
            let d = div()
                .absolute()
                .top(px(top_y))
                .line_height(px(20.))
                .text_color(c(p.intro_description_fg));
            if centered {
                d.left_0()
                    .w(gpui::px(width))
                    .flex()
                    .justify_center()
                    .child(text.to_string())
            } else {
                d.left(px(left + BUTTON_RADIUS)).child(text.to_string())
            }
        };
        let next_at = |el: gpui::Stateful<Div>| {
            div()
                .absolute()
                .left(px(left))
                .top(px(top + NEXT_TOP))
                .child(el)
        };
        match self.step {
            Step::Qr => {
                for el in self.render_qr_step(width, top, window, cx) {
                    root = root.child(el);
                }
                if let Some(e) = &self.error {
                    root = root.child(error_at(top + QR_ERROR_TOP, true, e));
                }
            }
            Step::Phone => {
                root = root
                    .child(title("Your Phone Number".into()))
                    .child(description(
                        "Please confirm your country code\nand enter your phone number.",
                    ));
                // The country (`CountryInput`): its name and the drop-down arrow.
                let country = div()
                    .absolute()
                    .left(px(left))
                    .top(px(top + FIELD_TOP))
                    .w(px(STEP_WIDTH))
                    .h(px(FIELD_HEIGHT))
                    .child(
                        div()
                            .absolute()
                            .left(px(3.))
                            .top(px(FIELD_TEXT_TOP))
                            .right(px(30.))
                            .text_size(scale::font(FIELD_FONT).size)
                            .child(one_line(self.country_name.clone())),
                    )
                    .child(div().absolute().right(px(8.)).top(px(37.)).child(icon(
                        Icon::IntroCountryDropdown,
                        p.menu_icon_fg,
                        window,
                    )))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(1.))
                            .bg(c(p.input_border_fg)),
                    );
                let phone_top = top + FIELD_TOP + FIELD_HEIGHT + PHONE_TOP;
                root = root
                    .child(country)
                    .child(div().absolute().left(px(left)).top(px(phone_top)).child(
                        self.render_field(FieldId::CountryCode, CODE_FIELD_WIDTH, true, window, cx),
                    ))
                    .child(
                        div()
                            .absolute()
                            .left(px(left + STEP_WIDTH - PHONE_FIELD_WIDTH))
                            .top(px(phone_top))
                            .child(self.render_field(
                                FieldId::Phone,
                                PHONE_FIELD_WIDTH,
                                false,
                                window,
                                cx,
                            )),
                    )
                    .child(next_at(self.render_next("Next", cx)))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .w(gpui::px(width))
                            .top(px(top + QR_LOGIN_LINK_TOP))
                            .flex()
                            .justify_center()
                            .child(self.link(
                                "intro-to-qr",
                                "Quick log in using QR code",
                                cx,
                                |this, window, cx| this.go(Step::Qr, window, cx),
                            )),
                    );
                if let Some(e) = &self.error {
                    root = root.child(error_at(top + ERROR_TOP, true, e));
                }
            }
            Step::Code => {
                let phone = tdr_data::login::format_phone(&self.phone, &self.countries);
                root = root.child(title(phone)).child(description(if self.via_app {
                    "A code was sent via Telegram to your other\ndevices, if you have any connected."
                } else {
                    "We've sent an activation code to your phone.\nPlease enter it below."
                }));
                let focused = self.code_focus.is_focused(window);
                let count = self.code_digits.len();
                let digit_w = DIGIT_HEIGHT * 0.8;
                let mut boxes = div()
                    .id("intro-code")
                    .absolute()
                    .left(px(left - 1.))
                    .top(px(top + FIELD_TOP + PHONE_TOP * 3.))
                    .flex()
                    .gap(px(DIGIT_SKIP))
                    .pl(px(DIGIT_SKIP))
                    .track_focus(&self.code_focus)
                    .child(
                        Self::input_target(&self.code_focus, cx)
                            .absolute()
                            .size_full(),
                    )
                    .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        this.code_key(e, window, cx)
                    }));
                for i in 0..count {
                    let border = if self.code_error {
                        p.active_line_fg_error
                    } else if focused && i == self.code_current {
                        p.window_active_text_fg
                    } else {
                        p.window_bg_ripple
                    };
                    let digit = self.code_digits[i]
                        .map(|d| d.to_string())
                        .unwrap_or_default();
                    boxes = boxes.child(
                        div()
                            .id(("code-digit", i))
                            .w(px(digit_w))
                            .h(px(DIGIT_HEIGHT))
                            .rounded(px(BOX_RADIUS))
                            .bg(c(p.window_bg_over))
                            .border(px(DIGIT_BORDER / 2.))
                            .border_color(c(border))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(scale::font(DIGIT_FONT).size)
                            .text_color(c(p.window_fg))
                            .child(digit)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.code_current = i;
                                window.focus(&this.code_focus);
                                cx.notify();
                            })),
                    );
                }
                root = root.child(boxes);
                let link_top = top + FIELD_TOP + PHONE_TOP * 3. + DIGIT_HEIGHT + LINK_TOP;
                if self.via_app && self.can_resend {
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(left + BUTTON_RADIUS))
                            .top(px(link_top))
                            .child(self.link(
                                "intro-sms",
                                "Send code via SMS",
                                cx,
                                |this, _, cx| {
                                    this.send(LoginCommand::ResendCode, cx);
                                    cx.notify();
                                },
                            )),
                    );
                }
                root = root.child(next_at(self.render_next("Next", cx)));
                if let Some(e) = &self.error {
                    root = root.child(error_at(top + ERROR_TOP, false, e));
                }
            }
            Step::Password => {
                root = root
                    .child(title("Cloud password check".into()))
                    .child(description("Please enter your cloud password."))
                    .child(
                        div()
                            .absolute()
                            .left(px(left))
                            .top(px(top + PASSWORD_TOP))
                            .child(self.render_field(
                                FieldId::Password,
                                STEP_WIDTH,
                                false,
                                window,
                                cx,
                            )),
                    );
                if !self.hint.is_empty() {
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(left + BUTTON_RADIUS))
                            .top(px(top + PASSWORD_HINT_TOP))
                            .text_color(c(p.window_fg))
                            .child(format!("Hint: {}", self.hint)),
                    );
                }
                // "Forgot password?": recovery comes later.
                root = root
                    .child(
                        div()
                            .absolute()
                            .left(px(left + BUTTON_RADIUS))
                            .top(px(top + PASSWORD_HINT_TOP + 20. + LINK_TOP))
                            .child(self.link(
                                "intro-recover",
                                "Forgot password?",
                                cx,
                                |_, _, _| {},
                            )),
                    )
                    .child(next_at(self.render_next("Submit", cx)));
                if let Some(e) = &self.error {
                    root = root.child(error_at(top + ERROR_TOP, false, e));
                }
            }
        }
        root
    }
}

impl gpui::EntityInputHandler for IntroView {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _actual: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        Some(String::new())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(gpui::UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.typed(text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        _text: &str,
        _selected: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Compositions (IME) show once committed.
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
