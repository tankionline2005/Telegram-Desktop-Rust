//! Reactions under a message (tdesktop's `HistoryView::Reactions::InlineList`):
//! a row of pills, each with its reaction's icon and the count or the userpics
//! of who chose it, wrapped to the bubble's width; the message's time takes a
//! skip block after the last pill (`updateSkipBlock`), as after a text. A click
//! chooses the reaction or takes ours back (`toggleReaction`).
//!
//! Changes animate: a new pill grows from a circle (`bubbleProgress`), one
//! taken away shrinks back and fades, a count slides to its new value; our own
//! new reaction flies there from the strip and plays its effect around it
//! (`Ui::ReactionFlyAnimation`). The rows are laid out at the final widths
//! (pills going away keep their place until they are gone), so a message's
//! height never jumps in the middle.

use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    AnyElement, Bounds, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div,
    point, prelude::FluentBuilder,
};
use tdr_data::describe::ReactionKey;
use tdr_data::tdr_storage::PeerKey;

use super::{MainView, c, text_width};
use crate::bridge::{Author, Command, MessageItem, MessageReactions, ReactionButton};
use crate::scale::{self, lg, px};

/// `reactionInlinePadding`, `reactionInlineSize`, `reactionInlineImage`,
/// `reactionInlineSkip`, `reactionInlineEmptySkip`, `reactionInlineBetween`,
/// `reactionInlineInBubbleLeft`.
const PADDING: [f32; 4] = [5., 2., 7., 2.];
const SIZE: f32 = 18.;
const IMAGE: f32 = 32.;
const SKIP: f32 = 3.;
const EMPTY_SKIP: f32 = 2.;
const BETWEEN: f32 = 4.;
const IN_BUBBLE_LEFT: f32 = -3.;
/// `reactionInlineUserpicsPadding` and `reactionInlineUserpics`.
const USERPICS_PADDING: [f32; 4] = [1., 1., 1., 1.];
const USERPIC: f32 = 20.;
const USERPIC_SHIFT: f32 = 7.;
const USERPIC_STROKE: f32 = 1.;
/// `kInNonChosenOpacity`, `kOutNonChosenOpacity`.
const IN_NON_CHOSEN: f32 = 0.12;
const OUT_NON_CHOSEN: f32 = 0.18;
/// `semiboldFont`.
const COUNT_FONT: f32 = 13.;
/// `reactionInlineTagSkip`, `reactionInlineTagLeftRadius`,
/// `reactionInlineTagRightRadius`, `reactionInlineTagArrow`,
/// `reactionInlineTagDot`, `reactionInlineTagDotSkip`, `reactionInlineTagFont`,
/// `reactionInlineTagNamePosition`.
const TAG_SKIP: f32 = 6.;
const TAG_LEFT_RADIUS: f32 = 6.;
const TAG_RIGHT_RADIUS: f32 = 3.;
const TAG_ARROW: f32 = 5.;
const TAG_DOT: f32 = 5.;
const TAG_DOT_SKIP: f32 = 2.;
const TAG_FONT: f32 = 12.;
const TAG_NAME_POSITION: (f32, f32) = (26., 2.);
/// `mediaInBubbleSkip`: between the text and the reactions.
pub(super) const MEDIA_IN_BUBBLE_SKIP: f32 = 5.;
/// A pill appearing, going away, a count changing (ours: tdesktop changes
/// them at once unless we chose the reaction).
const APPEAR_MS: f32 = 200.;
const GONE_MS: f32 = 200.;
const COUNT_MS: f32 = 200.;
/// `kFlyDuration`, `reactionFlyUp`.
const FLY_MS: f32 = 300.;
const FLY_UP: f32 = 50.;

/// The server's reactions as the window needs them.
#[derive(Default)]
pub(super) struct ReactionsUi {
    /// An emoji reaction's icon: its document and whether it is the select
    /// animation (`fromSelectAnimation`: drawn at half size).
    icons: HashMap<String, (i64, bool)>,
    /// The server's list, in its order (the selector).
    pub(super) available: Vec<tdr_data::reactions::Available>,
    /// Our top reactions (`messages.getTopReactions`): the strip's first ones.
    pub(super) top: Vec<ReactionKey>,
    /// `messages.getRecentReactions`: after the top ones.
    pub(super) recent: Vec<ReactionKey>,
    /// Saved Messages: the tags offered to everyone and ours (with names).
    pub(super) tags: Vec<ReactionKey>,
    pub(super) my_tags: Vec<tdr_data::reactions::SavedTag>,
    /// The account has Premium (set each frame).
    pub(super) premium: bool,
    asked: bool,
    /// The full panel's search field and its text.
    pub(super) search: Option<gpui::Entity<crate::text_input::TextInput>>,
    pub(super) query: String,
}

/// `kReactionsUniqMax`'s default (`reactions_uniq_max`): how many different
/// reactions a message may have when the chat does not say.
const UNIQUE_LIMIT: i32 = 11;

/// What a message's selector offers (`PossibleItemReactionsRef`).
#[derive(Clone, Debug, Default)]
pub(super) struct Possible {
    pub(super) keys: Vec<ReactionKey>,
    /// Custom emoji too: the full panel has the search and the sets
    /// (`customAllowed`).
    pub(super) custom_allowed: bool,
    /// Saved Messages tags (`reactionsAreTags`).
    pub(super) tags: bool,
}

fn push_new(keys: &mut Vec<ReactionKey>, key: &ReactionKey) {
    if !keys.contains(key) {
        keys.push(key.clone());
    }
}

type PillKey = (PeerKey, i32, ReactionKey);
/// A pill taken away: its old index, itself, since when.
type GonePill = (usize, ReactionButton, Instant);

/// The reactions' animations under messages.
#[derive(Default)]
pub(super) struct ReactionFx {
    appear: HashMap<PillKey, Instant>,
    /// The count it had and since when it changes.
    counts: HashMap<PillKey, (i32, Instant)>,
    /// Pills taken away, at their old place, until they are gone.
    gone: HashMap<(PeerKey, i32), Vec<GonePill>>,
    flies: HashMap<PillKey, Fly>,
}

/// Our new reaction flying in (`ReactionFlyAnimation`): from where it was
/// chosen (none: it starts in place), then its center animation and the
/// effect around it.
#[derive(Clone, Debug)]
pub(super) struct Fly {
    from: Option<Bounds<Pixels>>,
    start: Instant,
    /// What flies: the strip's frame, or the emoji itself.
    icon: Option<Clip>,
    emoji: Option<u16>,
    center: Option<Clip>,
    effect: Option<Clip>,
}

impl Fly {
    fn flying(&self) -> Option<f32> {
        self.from?;
        let t = self.start.elapsed().as_secs_f32() * 1000. / FLY_MS;
        (t < 1.).then_some(t)
    }

    fn done(&self) -> bool {
        // Frames not rendered yet are waited for a while.
        let waiting = self.start.elapsed().as_secs() < 3;
        let busy = |clip: Option<Clip>| {
            clip.is_some_and(|c| c.playing() || (waiting && c.frame().is_none()))
        };
        self.flying().is_none()
            && self.start.elapsed().as_secs_f32() * 1000. > FLY_MS
            && !busy(self.center)
            && !busy(self.effect)
    }
}

/// How one pill is drawn now.
#[derive(Clone, Debug)]
pub(super) struct PillFx {
    /// `bubbleProgress`: 0 a circle (and transparent), 1 the whole pill.
    bubble: f32,
    /// The count it slides from and how far.
    count_from: Option<(i32, f32)>,
    fly: Option<Fly>,
}

impl Default for PillFx {
    fn default() -> Self {
        Self {
            bubble: 1.,
            count_from: None,
            fly: None,
        }
    }
}

/// The pills of a message's reactions laid out at a width
/// (`InlineList::countCurrentSize`).
pub(super) struct ReactionsLayout {
    /// Each pill's place (logical, from the text's left) and size.
    pills: Vec<(f32, f32, f32, f32)>,
    pub(super) height: f32,
}

fn progress(at: Instant, ms: f32) -> f32 {
    (at.elapsed().as_secs_f32() * 1000. / ms).clamp(0., 1.)
}

impl MainView {
    /// Asks for the server's reactions once.
    pub(super) fn ask_reactions(&mut self) {
        if !self.reactions_ui.asked {
            self.reactions_ui.asked = true;
            let _ = self.commands.send(Command::LoadReactions);
        }
    }

    /// The server's reactions came: their icons are custom emoji documents.
    pub(super) fn reactions_came(
        &mut self,
        list: Vec<tdr_data::reactions::Available>,
        top: Vec<ReactionKey>,
        recent: Vec<ReactionKey>,
        (tags, my_tags): (Vec<ReactionKey>, Vec<tdr_data::reactions::SavedTag>),
    ) {
        self.reactions_ui.top = top;
        self.reactions_ui.recent = recent;
        self.reactions_ui.tags = tags;
        self.reactions_ui.my_tags = my_tags;
        self.reactions_ui.icons.clear();
        for r in &list {
            for doc in r.documents() {
                crate::custom_emoji::remember(doc);
            }
            let icon = match (&r.center, &r.select) {
                (Some(center), _) => Some((center.id, false)),
                (None, Some(select)) => Some((select.id, true)),
                _ => None,
            };
            if let Some(icon) = icon {
                self.reactions_ui.icons.insert(r.emoticon.clone(), icon);
            }
        }
        self.reactions_ui.available = list;
    }

    /// A chat's messages changed: reactions that came, went or changed count
    /// start their animations.
    pub(super) fn diff_reactions(
        &mut self,
        peer: PeerKey,
        old: &[MessageItem],
        new: &[MessageItem],
    ) {
        let now = Instant::now();
        let fx = &mut self.reaction_fx;
        let empty: &[ReactionButton] = &[];
        for n in new {
            let Some(o) = old.iter().find(|o| o.id == n.id) else {
                continue;
            };
            let ob = o.reactions.as_ref().map_or(empty, |r| r.buttons.as_slice());
            let nb = n.reactions.as_ref().map_or(empty, |r| r.buttons.as_slice());
            if ob == nb {
                continue;
            }
            for (i, b) in ob.iter().enumerate() {
                if !nb.iter().any(|x| x.key == b.key) {
                    let gone = fx.gone.entry((peer, n.id)).or_default();
                    if !gone.iter().any(|(_, g, _)| g.key == b.key) {
                        gone.push((i, b.clone(), now));
                    }
                }
            }
            for b in nb {
                let key = (peer, n.id, b.key.clone());
                match ob.iter().find(|x| x.key == b.key) {
                    None => {
                        if let Some(gone) = fx.gone.get_mut(&(peer, n.id)) {
                            gone.retain(|(_, g, _)| g.key != b.key);
                        }
                        fx.appear.insert(key, now);
                    }
                    Some(x) if x.count != b.count && x.userpics.is_none() => {
                        fx.counts.insert(key, (x.count, now));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Our reaction `key` to a message is being added: it flies in from
    /// `from` (the strip's cell) or plays in place.
    pub(super) fn start_fly(
        &mut self,
        peer: PeerKey,
        msg_id: i32,
        key: ReactionKey,
        from: Option<Bounds<Pixels>>,
    ) {
        let s = scale::get();
        let start = Instant::now();
        let effects_at = if from.is_some() {
            start + std::time::Duration::from_millis(FLY_MS as u64)
        } else {
            start
        };
        let available = match &key {
            ReactionKey::Emoji(e) => self
                .reactions_ui
                .available
                .iter()
                .find(|r| &r.emoticon == e),
            _ => None,
        };
        let image = (lg(IMAGE) * s).round() as u32;
        let clip = |doc: Option<&tdr_data::emoji_sets::EmojiDocument>, side: u32| {
            doc.map(|d| Clip {
                id: d.id,
                side,
                start: Some(effects_at),
            })
        };
        let center = available.and_then(|r| {
            clip(r.center.as_ref(), image).or_else(|| clip(r.select.as_ref(), image))
        });
        let effect = available.and_then(|r| clip(r.around.as_ref(), image * 2));
        let icon = available
            .and_then(|r| r.select.as_ref())
            .map(|d| Clip::new(d.id, (lg(STRIP_IMAGE) * HOVER_SCALE * s).round() as u32));
        let emoji = match &key {
            ReactionKey::Emoji(e) => tdr_emoji::find(e).map(|(e, _)| e),
            _ => None,
        };
        self.reaction_fx.flies.insert(
            (peer, msg_id, key),
            Fly {
                from,
                start,
                icon,
                emoji,
                center,
                effect,
            },
        );
    }

    /// Ends the animations that are over (a message whose pill went away is
    /// measured again); true while some still go.
    pub(super) fn tick_reaction_fx(&mut self) -> bool {
        let fx = &mut self.reaction_fx;
        fx.appear.retain(|_, at| progress(*at, APPEAR_MS) < 1.);
        fx.counts.retain(|_, (_, at)| progress(*at, COUNT_MS) < 1.);
        fx.flies.retain(|_, f| !f.done());
        let mut remeasure = Vec::new();
        fx.gone.retain(|(peer, id), list| {
            list.retain(|(_, _, at)| progress(*at, GONE_MS) < 1.);
            if list.is_empty() {
                remeasure.push((*peer, *id));
            }
            !list.is_empty()
        });
        // The center and effect animations ask for their own frames.
        let busy = !(fx.appear.is_empty() && fx.counts.is_empty() && fx.gone.is_empty())
            || fx.flies.values().any(|f| f.flying().is_some());
        for (peer, id) in remeasure {
            if self.selected == Some(peer)
                && let Some(ix) = self.messages.iter().position(|m| m.id == id)
            {
                self.messages_state.splice(ix..ix + 1, 1);
            }
        }
        busy
    }

    /// The pills a message shows now (those going away included, at their old
    /// places) and how each is drawn.
    pub(super) fn shown_reactions(
        &self,
        peer: Option<PeerKey>,
        msg_id: i32,
        r: Option<&MessageReactions>,
    ) -> Option<(MessageReactions, Vec<PillFx>)> {
        let Some(peer) = peer else {
            return r.map(|r| (r.clone(), vec![PillFx::default(); r.buttons.len()]));
        };
        let fx = &self.reaction_fx;
        let mut buttons: Vec<ReactionButton> = r.map(|r| r.buttons.clone()).unwrap_or_default();
        let mut pills: Vec<PillFx> = buttons
            .iter()
            .map(|b| {
                let key = (peer, msg_id, b.key.clone());
                let fly = fx.flies.get(&key).cloned();
                let appearing = fx.appear.get(&key);
                let bubble = match (&fly, appearing) {
                    // `bubbleProgress` follows the flight.
                    (Some(f), Some(_)) => f.flying().unwrap_or(1.),
                    (_, Some(at)) => progress(*at, APPEAR_MS),
                    _ => 1.,
                };
                PillFx {
                    bubble,
                    count_from: fx
                        .counts
                        .get(&key)
                        .map(|(old, at)| (*old, progress(*at, COUNT_MS))),
                    fly,
                }
            })
            .collect();
        if let Some(gone) = fx.gone.get(&(peer, msg_id)) {
            for (i, b, at) in gone {
                let at_index = (*i).min(buttons.len());
                buttons.insert(at_index, b.clone());
                pills.insert(
                    at_index,
                    PillFx {
                        bubble: 1. - progress(*at, GONE_MS),
                        ..Default::default()
                    },
                );
            }
        }
        if buttons.is_empty() {
            return None;
        }
        let tags = r.is_some_and(|r| r.tags);
        if tags {
            // `myTagTitle`.
            for b in &mut buttons {
                b.title = self
                    .reactions_ui
                    .my_tags
                    .iter()
                    .find(|t| t.key == b.key)
                    .and_then(|t| t.title.clone());
            }
        }
        Some((MessageReactions { buttons, tags }, pills))
    }

    /// `LookupPossibleReactions`: Saved Messages tags (ours, then the default
    /// ones); only the reactions a message already has when it has as many
    /// as the chat allows; else the top, recent and server's ones the chat
    /// allows, custom emoji only where all are allowed.
    pub(super) fn possible_reactions(&self, peer: PeerKey, msg_id: i32) -> Possible {
        use tdr_data::describe::AllowedKind;
        let ui = &self.reactions_ui;
        let Some(item) = self.messages.iter().find(|m| m.id == msg_id) else {
            return Possible::default();
        };
        if item.service.is_some() {
            return Possible::default();
        }
        let chat = self.chat_item(peer);
        let saved = chat.is_some_and(|c| c.is_saved);
        let existing: Vec<ReactionKey> = item
            .reactions
            .as_ref()
            .map(|r| r.buttons.iter().map(|b| b.key.clone()).collect())
            .unwrap_or_default();
        let tags = item
            .reactions
            .as_ref()
            .map_or(saved, |r| r.tags || (r.buttons.is_empty() && saved));
        let mut keys = Vec::new();
        if tags {
            for key in ui.my_tags.iter().map(|t| &t.key).chain(&ui.tags) {
                push_new(&mut keys, key);
            }
            return Possible {
                keys,
                custom_allowed: true,
                tags: true,
            };
        }
        let full: Vec<ReactionKey> = ui
            .available
            .iter()
            .filter(|r| !r.inactive)
            .map(|r| ReactionKey::Emoji(r.emoticon.clone()))
            .collect();
        let all: Vec<&ReactionKey> = ui.top.iter().chain(&ui.recent).chain(&full).collect();
        let allowed = chat.and_then(|c| c.reactions.clone()).unwrap_or_default();
        let limit = allowed.max.filter(|m| *m > 0).unwrap_or(UNIQUE_LIMIT) as usize;
        if existing.len() >= limit {
            for key in all.iter().copied().filter(|k| existing.contains(k)) {
                push_new(&mut keys, key);
            }
            for key in existing.iter().filter(|k| **k != ReactionKey::Paid) {
                push_new(&mut keys, key);
            }
            return Possible {
                keys,
                ..Default::default()
            };
        }
        for key in all {
            let ok = match (&allowed.kind, key) {
                (_, ReactionKey::Paid) => false,
                (AllowedKind::Some(list), key) => list.contains(key),
                (AllowedKind::Default, ReactionKey::Custom(_)) => false,
                _ => true,
            };
            if ok {
                push_new(&mut keys, key);
            }
        }
        if let AllowedKind::Some(list) = &allowed.kind {
            for key in list.iter().filter(|k| **k != ReactionKey::Paid) {
                push_new(&mut keys, key);
            }
        }
        Possible {
            keys,
            custom_allowed: allowed.kind == AllowedKind::All,
            tags: false,
        }
    }

    /// `Window::ShowReactPremiumError`: tags need Premium, so do custom
    /// emoji reactions out of channels (a toast in place of the Premium box).
    pub(super) fn react_premium_error(
        &mut self,
        msg_id: i32,
        key: &ReactionKey,
        tags: bool,
    ) -> bool {
        if self.reactions_ui.premium {
            return false;
        }
        if tags {
            self.show_long_toast(
                "Tags for Messages. Organize your Saved Messages with tags for quicker access.",
                4000.,
            );
            return true;
        }
        let item = self.messages.iter().find(|m| m.id == msg_id);
        let chosen = item
            .and_then(|m| m.reactions.as_ref())
            .is_some_and(|r| r.buttons.iter().any(|b| b.key == *key && b.chosen));
        let broadcast = self
            .selected
            .and_then(|p| self.chat_item(p))
            .is_some_and(|c| c.kind == tdr_data::describe::PeerKind::Channel);
        if chosen || broadcast || !matches!(key, ReactionKey::Custom(_)) {
            return false;
        }
        self.show_long_toast(
            "Infinite Reactions. React with thousands of emoji \u{2014} with multiple reactions per message.",
            4000.,
        );
        true
    }

    /// The width of one pill (`InlineList::countOptimalSize`).
    fn pill_width(b: &ReactionButton, tags: bool, window: &Window) -> f32 {
        let [pl, _, pr, _] = PADDING;
        if tags {
            let text = b.title.as_deref().map_or(0., |t| {
                text_width(window, t, TAG_FONT, FontWeight::NORMAL).ceil()
            });
            return lg(pl)
                + lg(SIZE)
                + lg(TAG_SKIP)
                + lg(pr)
                + text
                + if text > 0. { lg(SKIP) } else { 0. };
        }
        match &b.userpics {
            Some(pics) => {
                let n = pics.len().max(1) as f32;
                lg(pl)
                    + lg(SIZE)
                    + lg(USERPICS_PADDING[0])
                    + lg(USERPICS_PADDING[2])
                    + lg(USERPIC)
                    + (n - 1.) * (lg(USERPIC) - lg(USERPIC_SHIFT))
            }
            None if b.count == 0 => lg(pl) + lg(SIZE) + lg(pr) - lg(EMPTY_SKIP),
            None => {
                let text = crate::present::count_short(b.count);
                lg(pl)
                    + lg(SIZE)
                    + lg(SKIP)
                    + lg(pr)
                    + text_width(window, &text, COUNT_FONT, FontWeight::SEMIBOLD).ceil()
            }
        }
    }

    /// The widest the reactions get on one row with the skip block
    /// (`InlineList::maxWidth`), from the text's left.
    pub(super) fn reactions_max_width(r: &MessageReactions, skip_w: f32, window: &Window) -> f32 {
        let mut x = lg(IN_BUBBLE_LEFT);
        for b in &r.buttons {
            x += Self::pill_width(b, r.tags, window) + lg(BETWEEN);
        }
        x - lg(BETWEEN) + skip_w
    }

    /// `InlineList::countCurrentSize`: rows of pills at `width`, the skip block
    /// after the last one (or below it).
    pub(super) fn layout_reactions(
        r: &MessageReactions,
        width: f32,
        (skip_w, skip_h): (f32, f32),
        window: &Window,
    ) -> ReactionsLayout {
        let left = lg(IN_BUBBLE_LEFT);
        let h = lg(PADDING[1]) + lg(SIZE) + lg(PADDING[3]);
        let (mut x, mut y) = (left, 0.);
        let mut pills = Vec::with_capacity(r.buttons.len());
        for b in &r.buttons {
            let w = Self::pill_width(b, r.tags, window);
            if x > left && x + w > left + width {
                x = left;
                y += h + lg(BETWEEN);
            }
            pills.push((x, y, w, h));
            x += w + lg(BETWEEN);
        }
        let Some(&(lx, _, lw, _)) = pills.last() else {
            return ReactionsLayout { pills, height: 0. };
        };
        let add = if lx + lw + skip_w > width { skip_h } else { 0. };
        ReactionsLayout {
            pills,
            height: y + h + add,
        }
    }

    /// The pills of a message (`InlineList::paint` in a bubble).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_reactions(
        &self,
        peer: Option<PeerKey>,
        msg_id: i32,
        r: &MessageReactions,
        fx: &[PillFx],
        layout: &ReactionsLayout,
        out: bool,
        live: bool,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let bubble_bg = c(if out { p.msg_out_bg } else { p.msg_in_bg });
        let file_bg = c(if out {
            p.msg_file_out_bg
        } else {
            p.msg_file_in_bg
        });
        let mut row = div().relative().w_full().h(gpui::px(layout.height));
        let mut effects: Vec<AnyElement> = Vec::new();
        for (k, ((b, &(x, y, w, h)), pfx)) in r
            .buttons
            .iter()
            .zip(&layout.pills)
            .zip(fx.iter().chain(std::iter::repeat(&PillFx::default())))
            .enumerate()
        {
            let chosen = b.chosen;
            let bubble = pfx.bubble;
            let ready = bubble >= 1.;
            // `paint`: a pill not chosen is faint; growing ones fade in.
            let opacity = if !chosen {
                bubble * if out { OUT_NON_CHOSEN } else { IN_NON_CHOSEN }
            } else if !ready {
                bubble
            } else {
                1.
            };
            let mut bg = file_bg;
            bg.a *= opacity;
            // `bubbleSkip`: a growing pill is a circle widening to its size.
            let fill_w = h + (w - h) * bubble;
            // What the pill shows over the bubble (the userpics' rings).
            let shown = blend(bubble_bg, file_bg, opacity / bubble.max(0.01));
            let text_fg = if chosen {
                c(if out {
                    p.history_file_out_icon_fg
                } else {
                    p.history_file_in_icon_fg
                })
            } else {
                c(if out {
                    p.msg_out_service_fg
                } else {
                    p.msg_in_service_fg
                })
            };
            let id = SharedString::from(format!(
                "reaction-{}-{}-{msg_id}-{k}",
                peer.map_or(0, |p| p.id),
                live as u8
            ));
            let mut ripple_color = text_fg;
            ripple_color.a = 25. / 255.;
            // The fly draws the icon while it plays.
            let flying = pfx.fly.as_ref().is_some_and(|f| !f.done());
            let icon = (!flying).then(|| self.reaction_icon(&b.key, text_fg));
            let count = b.userpics.is_none().then_some(b.count).filter(|n| *n > 0);
            let pics = b
                .userpics
                .as_ref()
                .map(|pics| self.render_userpics_row(pics, shown));
            let key = b.key.clone();
            let tags = r.tags;
            // A tag's name after its icon.
            let title = b.title.clone().filter(|_| tags).map(|t| {
                div()
                    .absolute()
                    .left(gpui::px(lg(TAG_NAME_POSITION.0)))
                    .top(gpui::px(lg(TAG_NAME_POSITION.1)))
                    .text_size(scale::font(TAG_FONT).size)
                    .line_height(scale::font(TAG_FONT).line_height())
                    .text_color(text_fg)
                    .whitespace_nowrap()
                    .child(t)
            });
            let tag_shape = tags.then(|| Self::render_tag_shape(file_bg, opacity, fill_w, h));
            let ripple_id = id.clone();
            // The pill's place on screen (the ripple starts at the press).
            let pill_bounds = std::rc::Rc::new(std::cell::Cell::new(None));
            let bounds_out = pill_bounds.clone();
            let count_el = count.map(|n| {
                let left = lg(PADDING[0]) + lg(SIZE) + lg(SKIP);
                let text = |value: i32, dy: f32, alpha: f32| {
                    div()
                        .absolute()
                        .left(gpui::px(left))
                        .top(gpui::px(dy))
                        .h(gpui::px(h))
                        .flex()
                        .items_center()
                        .opacity(alpha)
                        .text_size(scale::font(COUNT_FONT).size)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_fg)
                        .child(crate::present::count_short(value))
                };
                match pfx.count_from {
                    // The old value goes up, the new one comes from below.
                    Some((old, t)) if t < 1. => {
                        let t = crate::anim::sine_in_out(t);
                        let shift = h / 2.;
                        let up = old < n;
                        let sign = if up { 1. } else { -1. };
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .size_full()
                            .child(text(old, -sign * shift * t, 1. - t))
                            .child(text(n, sign * shift * (1. - t), t))
                            .into_any_element()
                    }
                    _ => text(n, 0., 1.).into_any_element(),
                }
            });
            if let Some(fly) = pfx.fly.clone().filter(|_| flying) {
                effects.push(self.render_fly(fly, (x, y)));
            }
            row = row.child(
                div()
                    .id(id.clone())
                    .absolute()
                    .left(gpui::px(x))
                    .top(gpui::px(y))
                    .w(gpui::px(w))
                    .h(gpui::px(h))
                    .when(live && ready, |d| d.cursor_pointer())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .w(gpui::px(fill_w))
                            .h(gpui::px(h))
                            .when(!tags, |d| d.rounded_full().bg(bg))
                            .when(tags, |d| d.rounded(gpui::px(lg(TAG_LEFT_RADIUS))))
                            .overflow_hidden()
                            .children(tag_shape)
                            .children(self.ripple_layer_rgba(&id, w, h, ripple_color, false))
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds_out.set(Some(bounds)),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .left_0()
                                .top_0()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .w(gpui::px(w))
                                    .h(gpui::px(h))
                                    .opacity(if chosen { 1. } else { bubble })
                                    .children(icon)
                                    .children(title)
                                    .children(count_el.filter(|_| !tags))
                                    .children(pics.map(|pics| {
                                        div()
                                            .absolute()
                                            .left(gpui::px(
                                                lg(PADDING[0]) + lg(SIZE) + lg(USERPICS_PADDING[0]),
                                            ))
                                            .top(gpui::px(lg(USERPICS_PADDING[1])))
                                            .child(pics)
                                    })),
                            ),
                    )
                    .when(live && ready, |d| {
                        let (down, up, hover, click) =
                            (view.clone(), view.clone(), view.clone(), view.clone());
                        let was_chosen = chosen;
                        d.on_mouse_down(MouseButton::Left, move |e, _, cx| {
                            cx.stop_propagation();
                            // The press point inside the pill (it starts at the
                            // middle when the place is not known).
                            let at = pill_bounds
                                .get()
                                .map(|b: Bounds<Pixels>| {
                                    (
                                        f32::from(e.position.x - b.origin.x),
                                        f32::from(e.position.y - b.origin.y),
                                    )
                                })
                                .unwrap_or((w / 2., h / 2.));
                            let id = ripple_id.clone();
                            let _ = down.update(cx, |this, cx| {
                                this.press_button(id, at);
                                cx.notify();
                            });
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            let _ = up.update(cx, |this, cx| {
                                this.release_buttons();
                                cx.notify();
                            });
                        })
                        .on_hover(move |hovered, _, cx| {
                            if !*hovered {
                                let _ = hover.update(cx, |this, cx| {
                                    this.release_buttons();
                                    cx.notify();
                                });
                            }
                        })
                        .on_click(move |_, _, cx| {
                            let key = key.clone();
                            let _ = click.update(cx, |this, cx| {
                                if let Some(peer) = peer {
                                    // Tags filter the chat in tdesktop (not here
                                    // yet); custom emoji may need Premium.
                                    if tags || this.react_premium_error(msg_id, &key, false) {
                                        cx.notify();
                                        return;
                                    }
                                    // Adding ours plays its effect in place.
                                    if !was_chosen {
                                        this.start_fly(peer, msg_id, key.clone(), None);
                                    }
                                    let _ = this.commands.send(Command::ToggleReaction {
                                        peer,
                                        msg_id,
                                        key,
                                    });
                                }
                                cx.notify();
                            });
                        })
                    }),
            );
        }
        row.children(effects).into_any_element()
    }

    /// `InlineList::paintSingleBg` for a tag `fill_w` x `h`: its left end,
    /// a plain middle, its right end with the arrow and the dot.
    fn render_tag_shape(color: gpui::Rgba, opacity: f32, fill_w: f32, h: f32) -> AnyElement {
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let s = scale::get();
                let base_w = lg(PADDING[0]) + lg(SIZE) + lg(TAG_SKIP) + lg(PADDING[2]);
                let left = lg(TAG_LEFT_RADIUS);
                let rgba = (((color.r * 255.).round() as u32) << 24)
                    | (((color.g * 255.).round() as u32) << 16)
                    | (((color.b * 255.).round() as u32) << 8)
                    | ((color.a * 255.).round() as u32);
                let (left_img, right_img) = crate::icons::tag_shape_images(
                    rgba,
                    ((base_w * s).round() as u32, (h * s).round() as u32),
                    (left * s).round() as u32,
                    (
                        lg(TAG_LEFT_RADIUS) * s,
                        lg(TAG_RIGHT_RADIUS) * s,
                        lg(TAG_ARROW) * s,
                    ),
                    (lg(TAG_DOT) * s, lg(TAG_DOT_SKIP) * s),
                );
                let o = bounds.origin;
                let right_w = base_w - left;
                window.with_element_opacity(Some(opacity), |window| {
                    let use_left = fill_w.min(left);
                    window.with_content_mask(
                        Some(gpui::ContentMask {
                            bounds: Bounds::new(o, gpui::size(gpui::px(use_left), gpui::px(h))),
                        }),
                        |window| {
                            let _ = window.paint_image(
                                Bounds::new(o, gpui::size(gpui::px(left), gpui::px(h))),
                                gpui::Corners::default(),
                                left_img,
                                0,
                                false,
                            );
                        },
                    );
                    let middle = fill_w - left - right_w;
                    if middle > 0. {
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                point(o.x + gpui::px(left), o.y),
                                gpui::size(gpui::px(middle), gpui::px(h)),
                            ),
                            color,
                        ));
                    }
                    let use_right = fill_w - left;
                    if use_right > 0. {
                        let clip = Bounds::new(
                            point(o.x + gpui::px(fill_w - use_right), o.y),
                            gpui::size(gpui::px(use_right), gpui::px(h)),
                        );
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds: clip }),
                            |window| {
                                let _ = window.paint_image(
                                    Bounds::new(
                                        point(o.x + gpui::px(fill_w - right_w), o.y),
                                        gpui::size(gpui::px(right_w), gpui::px(h)),
                                    ),
                                    gpui::Corners::default(),
                                    right_img,
                                    0,
                                    false,
                                );
                            },
                        );
                    }
                });
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .w(gpui::px(fill_w))
        .h(gpui::px(h))
        .into_any_element()
    }

    /// `ReactionFlyAnimation::paintGetArea` over a pill at (`x`, `y`): the
    /// chosen icon flying in on a parabola and fading while the center
    /// animation fades in at the pill's image, then the center animation and
    /// the effect around it (twice the image) on top of everything.
    fn render_fly(&self, fly: Fly, (x, y): (f32, f32)) -> AnyElement {
        let image = lg(IMAGE);
        let skip = (lg(SIZE) - image) / 2.;
        let (ix, iy) = (x + lg(PADDING[0]) + skip, y + lg(PADDING[1]) + skip);
        gpui::deferred(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let target = bounds;
                    let size = f32::from(target.size.width);
                    match (fly.flying(), fly.from) {
                        (Some(t), Some(from)) => {
                            crate::repaint::after_ms(8);
                            // The flying icon ends at the middle half of the image.
                            let l = size / 4.;
                            let r = size / 2. - l;
                            let small = Bounds::new(
                                point(target.origin.x + gpui::px(l), target.origin.y + gpui::px(l)),
                                gpui::size(gpui::px(size - l - r), gpui::px(size - l - r)),
                            );
                            let lerp = |a: f32, b: f32| a + (b - a) * t;
                            let fy = f32::from(from.origin.y);
                            let ty = f32::from(small.origin.y);
                            let rect = Bounds::new(
                                point(
                                    gpui::px(lerp(
                                        f32::from(from.origin.x),
                                        f32::from(small.origin.x),
                                    )),
                                    gpui::px(parabolic(fy, ty, lg(FLY_UP), t)),
                                ),
                                gpui::size(
                                    gpui::px(lerp(
                                        f32::from(from.size.width),
                                        f32::from(small.size.width),
                                    )),
                                    gpui::px(lerp(
                                        f32::from(from.size.height),
                                        f32::from(small.size.height),
                                    )),
                                ),
                            );
                            let wide = Bounds::new(
                                point(rect.origin.x - gpui::px(l), rect.origin.y - gpui::px(l)),
                                gpui::size(
                                    rect.size.width + gpui::px(l + r),
                                    rect.size.height + gpui::px(l + r),
                                ),
                            );
                            window.with_element_opacity(Some(1. - t), |window| {
                                match (fly.icon, fly.emoji) {
                                    (Some(icon), _) => {
                                        crate::custom_emoji::paint_frame(
                                            icon.id, icon.side, 0, rect, window,
                                        );
                                    }
                                    (None, Some(emoji)) => {
                                        let side = crate::emoji::size_large();
                                        crate::emoji::paint(emoji, rect.origin, side, window);
                                    }
                                    _ => {}
                                }
                            });
                            if let Some(center) = fly.center {
                                window.with_element_opacity(Some(t), |window| {
                                    crate::custom_emoji::paint_frame(
                                        center.id,
                                        center.side,
                                        0,
                                        wide,
                                        window,
                                    );
                                });
                            }
                        }
                        _ => {
                            if let Some(center) = fly.center {
                                center.paint(target, window);
                            }
                            if let Some(effect) = fly.effect
                                && effect.playing()
                            {
                                let half = target.size.width / 2.;
                                let wide = Bounds::new(
                                    point(target.origin.x - half, target.origin.y - half),
                                    gpui::size(target.size.width * 2., target.size.height * 2.),
                                );
                                effect.paint(wide, window);
                            }
                        }
                    }
                },
            )
            .absolute()
            .left(gpui::px(ix))
            .top(gpui::px(iy))
            .size(gpui::px(image)),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// The reaction's icon in its pill: an emoji reaction's first frame at
    /// `reactionInlineImage` centered on the `reactionInlineSize` square, a
    /// custom emoji at the custom emoji size in the text's color.
    fn reaction_icon(&self, key: &ReactionKey, text_fg: gpui::Rgba) -> AnyElement {
        let key = key.clone();
        let icon = match &key {
            ReactionKey::Emoji(e) => self.reactions_ui.icons.get(e).copied(),
            _ => None,
        };
        let emoji = match &key {
            ReactionKey::Emoji(e) => tdr_emoji::find(e).map(|(e, _)| e),
            _ => None,
        };
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let s = scale::get();
                let inner = point(
                    bounds.origin.x + gpui::px(lg(PADDING[0])),
                    bounds.origin.y + gpui::px(lg(PADDING[1])),
                );
                match (&key, icon) {
                    (ReactionKey::Custom(id), _) => {
                        let side = crate::custom_emoji::size_normal();
                        let skip =
                            ((lg(crate::emoji::EMOJI_SIZE) * s - side as f32) / 2. / s).floor();
                        let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                        crate::custom_emoji::paint(*id, at, side, text_fg.into(), window);
                    }
                    (_, Some((id, from_select))) => {
                        // `resolveImageFor`: the select animation's frame is drawn
                        // at half size in the middle.
                        let full = lg(IMAGE);
                        let size = if from_select { full / 2. } else { full };
                        let side = (size * s).round() as u32;
                        let skip = (lg(SIZE) - size) / 2.;
                        let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                        crate::custom_emoji::paint_still(id, at, side, window);
                    }
                    (ReactionKey::Emoji(_), None) => {
                        // Until the server's list comes: the emoji itself.
                        if let Some(emoji) = emoji {
                            let side = crate::emoji::size_normal();
                            let size = side as f32 / s;
                            let skip = ((lg(SIZE) - size) / 2.).floor();
                            let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                            crate::emoji::paint(emoji, at, side, window);
                        }
                    }
                    (ReactionKey::Paid, _) => {}
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .into_any_element()
    }

    /// `GenerateUserpicsInRow`: the first on top, each ringed (transparent in
    /// tdesktop: here in the color the pill shows there).
    fn render_userpics_row(&self, authors: &[Author], ring: gpui::Rgba) -> AnyElement {
        let size = lg(USERPIC);
        let step = size - lg(USERPIC_SHIFT);
        let stroke = lg(USERPIC_STROKE);
        let mut row = div().relative().h(gpui::px(size)).w(gpui::px(
            size + authors.len().saturating_sub(1) as f32 * step,
        ));
        for (i, a) in authors.iter().enumerate().rev() {
            let left = i as f32 * step;
            row = row.child(
                div()
                    .absolute()
                    .top(gpui::px(-stroke / 2.))
                    .left(gpui::px(left - stroke / 2.))
                    .size(gpui::px(size + stroke))
                    .rounded_full()
                    .bg(ring)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(USERPIC))
                            .rounded_full()
                            .overflow_hidden()
                            .child(self.userpic(
                                USERPIC,
                                a.key,
                                a.photo,
                                &a.initials,
                                a.color,
                                false,
                            )),
                    ),
            );
        }
        row.into_any_element()
    }
}

/// `ComputeFlyParabola`: from `from` to `to` over `t`, `top` above the higher
/// of them at its peak.
fn parabolic(from: f32, to: f32, top: f32, t: f32) -> f32 {
    let y1 = to - from;
    let y0 = y1.min(0.) - top;
    let (a, b) = if y1.abs() < 0.001 {
        (-4. * y0, 4. * y0)
    } else {
        let ratio = y0 / y1;
        let root = (ratio * (ratio - 1.)).max(0.).sqrt();
        let t0 = if y1 > 0. { ratio + root } else { ratio - root };
        let a = y1 / (1. - 2. * t0);
        (a, y1 - a)
    };
    a * t * t + b * t + from
}

/// `color` over `under` at `opacity`.
fn blend(under: gpui::Rgba, color: gpui::Rgba, opacity: f32) -> gpui::Rgba {
    let a = color.a * opacity.clamp(0., 1.);
    gpui::Rgba {
        r: under.r * (1. - a) + color.r * a,
        g: under.g * (1. - a) + color.g * a,
        b: under.b * (1. - a) + color.b * a,
        a: 1.,
    }
}

/// `reactStripExtend`, `reactStripHeight`, `reactStripSize`, `reactStripImage`,
/// `reactStripSkip`, `reactStripMinWidth`, `reactStripBubbleRight`,
/// `reactionCornerShadow`, `kHoverScale`, `kHoverScaleDuration`,
/// `kMinNonTransparentColumns`.
pub(super) const STRIP_EXTEND: [f32; 4] = [21., 49., 39., 0.];
pub(super) const STRIP_HEIGHT: f32 = 40.;
pub(super) const STRIP_SIZE: f32 = 32.;
pub(super) const STRIP_IMAGE: f32 = 26.;
pub(super) const STRIP_SKIP: f32 = 7.;
const STRIP_MIN_WIDTH: f32 = 60.;
const STRIP_BUBBLE_RIGHT: f32 = 20.;
pub(super) const STRIP_SHADOW: [f32; 4] = [4., 8., 4., 8.];
pub(super) const HOVER_SCALE: f32 = 1.24;
const HOVER_SCALE_MS: f32 = 200.;
const MIN_COLUMNS: usize = 7;
/// `defaultEmojiPan.aboutPadding` and `defaultEmojiPanAbout`'s font.
const ABOUT_PADDING: [f32; 4] = [12., 3., 12., 2.];
const ABOUT_FONT: f32 = 11.;

/// The about text over a strip, in lines of words (true: the link).
pub(super) struct StripAbout {
    lines: Vec<Vec<(String, bool)>>,
    pub(super) height: f32,
}
/// `kExpandDuration`, `kScaleDuration`, `emojiPanMinHeight`,
/// `emojiPanMaxHeight`, `emojiPanRadius`, the list's `header`, `headerLeft`
/// less its margin and `headerTop`.
pub(super) const EXPAND_MS: f32 = 300.;
pub(super) const SCALE_MS: f32 = 120.;

/// A one-shot animation of a document (`AnimatedIcon::animate`): its frames
/// on its own clock, from the first; the first frame before and after.
#[derive(Clone, Copy, Debug)]
pub(super) struct Clip {
    pub(super) id: i64,
    pub(super) side: u32,
    pub(super) start: Option<std::time::Instant>,
}

impl Clip {
    pub(super) fn new(id: i64, side: u32) -> Self {
        Self {
            id,
            side,
            start: None,
        }
    }

    /// The frame now and whether it still plays (`None`: not ready yet).
    pub(super) fn frame(&self) -> Option<(usize, bool)> {
        let durations = crate::custom_emoji::timeline(self.id, self.side)?;
        let Some(start) = self.start else {
            return Some((0, false));
        };
        let mut left = start.elapsed().as_millis();
        for (k, d) in durations.iter().enumerate() {
            let d = u128::from(*d);
            if left < d {
                // The next frame is due then.
                crate::repaint::after_ms((d - left).max(1) as u64);
                return Some((k, true));
            }
            left -= d;
        }
        Some((0, false))
    }

    pub(super) fn playing(&self) -> bool {
        self.start.is_some() && self.frame().is_some_and(|(_, playing)| playing)
    }

    /// Draws the frame now into `bounds`; false while not ready.
    pub(super) fn paint(&self, bounds: gpui::Bounds<gpui::Pixels>, window: &mut Window) -> bool {
        match self.frame() {
            Some((k, _)) => crate::custom_emoji::paint_frame(self.id, self.side, k, bounds, window),
            None => {
                // Asks for it and draws its silhouette meanwhile.
                crate::custom_emoji::paint_frame(self.id, self.side, 0, bounds, window)
            }
        }
    }
}

/// A cell of the strip: its reaction's appear and select animations
/// (`ReactionIcons`).
#[derive(Clone, Debug)]
pub(super) struct StripCell {
    appear: Option<Clip>,
    select: Option<Clip>,
    /// `appearAnimated`, `selectAnimated`.
    appeared: bool,
    select_played: bool,
}

/// The reaction strip over a message's menu (`Reactions::Selector` collapsed,
/// `AttachSelectorToMenu`).
pub(super) struct StripState {
    pub(super) peer: PeerKey,
    pub(super) msg_id: i32,
    /// The reactions it offers, in order (`PossibleItemReactions::recent`).
    pub(super) keys: Vec<ReactionKey>,
    /// The cells shown: reactions, then the expand button when they do not all
    /// fit (`Strip::AddedButton::Expand`).
    pub(super) columns: usize,
    pub(super) expand: bool,
    pub(super) cells: Vec<StripCell>,
    /// The cell under the mouse and since when (its scale animates).
    pub(super) hovered: Option<(usize, std::time::Instant)>,
    pub(super) unhovered: Option<(usize, std::time::Instant)>,
    /// Where the cells are on screen (the fly animation starts there).
    pub(super) cell_bounds: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
    /// Custom emoji allowed (the expand button is always there) and Saved
    /// Messages tags (the about text over the strip).
    pub(super) custom_allowed: bool,
    pub(super) tags: bool,
    /// The expand button was pressed: the strip unrolls into the full panel
    /// (`Selector::expand`).
    pub(super) expanded: Option<std::time::Instant>,
    pub(super) panel_scroll: gpui::ScrollHandle,
}

impl StripCell {
    /// A cell shown at once (no appear animation to play).
    pub(super) fn new(appear: Option<Clip>, select: Option<Clip>) -> Self {
        Self {
            appear,
            select,
            appeared: true,
            select_played: false,
        }
    }
}

impl StripState {
    /// A column of all the reactions (the corner button's).
    pub(super) fn column(peer: PeerKey, msg_id: i32, possible: Possible) -> Self {
        let mut strip = Self::new(peer, msg_id, possible);
        strip.columns = strip.keys.len();
        strip
    }

    pub(super) fn new(peer: PeerKey, msg_id: i32, possible: Possible) -> Self {
        Self {
            peer,
            msg_id,
            keys: possible.keys,
            custom_allowed: possible.custom_allowed,
            tags: possible.tags,
            columns: 0,
            expand: false,
            cells: Vec::new(),
            hovered: None,
            unhovered: None,
            cell_bounds: Default::default(),
            expanded: None,
            panel_scroll: gpui::ScrollHandle::new(),
        }
    }

    /// `Selector::countWidth`: as many cells as fit over the menu (`menu_width`
    /// is the menu's own); returns the width the menu gets (the strip's less
    /// its extends).
    pub(super) fn fit(&mut self, menu_width: f32) -> f32 {
        let [el, _, er, _] = STRIP_EXTEND.map(lg);
        let added = el + er;
        let (size, skip) = (lg(STRIP_SIZE), self.skip_x());
        let desired = menu_width + added;
        let max_width = lg(crate::popup_menu::WIDTH_MAX) + added;
        let desired_columns = (((desired - 2. * skip) / size).ceil() as usize).max(MIN_COLUMNS);
        let possible = desired_columns.min(((max_width - 2. * skip) / size).floor() as usize);
        let max = self.keys.len() + self.custom_allowed as usize;
        self.columns = possible.min(max).max(1);
        self.expand = self.columns < max || self.custom_allowed;
        (2. * skip + self.columns as f32 * size).max(desired) - added
    }

    /// `countSkipLeft`.
    pub(super) fn skip_x(&self) -> f32 {
        let n = (self.keys.len() + self.custom_allowed as usize) as f32;
        ((lg(STRIP_MIN_WIDTH) - n * lg(STRIP_SIZE)) / 2.).max(lg(STRIP_SKIP))
    }

    /// The scale of cell `k` now (`selectedScale`, `anim::sineInOut`).
    pub(super) fn scale(&self, k: usize) -> f32 {
        let progress = |at: std::time::Instant| {
            crate::anim::sine_in_out((at.elapsed().as_secs_f32() * 1000. / HOVER_SCALE_MS).min(1.))
        };
        match (self.hovered, self.unhovered) {
            (Some((h, at)), _) if h == k => 1. + (HOVER_SCALE - 1.) * progress(at),
            (_, Some((u, at))) if u == k => HOVER_SCALE - (HOVER_SCALE - 1.) * progress(at),
            _ => 1.,
        }
    }

    /// A cell whose select animation is over plays it again on the next
    /// entering.
    pub(super) fn rearm(&mut self) {
        let hovered = self.hovered.map(|(h, _)| h);
        for (k, cell) in self.cells.iter_mut().enumerate() {
            if cell.select_played && !cell.select.is_some_and(|s| s.playing()) && hovered != Some(k)
            {
                cell.select_played = false;
            }
        }
    }

    /// Where cell `k`'s image is on screen now (the fly starts there).
    pub(super) fn image_bounds(&self, k: usize) -> Option<gpui::Bounds<gpui::Pixels>> {
        let b = *self.cell_bounds.borrow().get(k)?;
        let image = gpui::px(lg(STRIP_IMAGE) * self.scale(k));
        Some(gpui::Bounds::new(
            point(
                b.origin.x + (b.size.width - image) / 2.,
                b.origin.y + (b.size.height - image) / 2.,
            ),
            gpui::size(image, image),
        ))
    }

    /// A cell grows or shrinks (the animations' frames come by themselves).
    pub(super) fn scaling(&self) -> bool {
        let busy = |t: Option<(usize, std::time::Instant)>| {
            t.is_some_and(|(_, at)| at.elapsed().as_secs_f32() * 1000. < HOVER_SCALE_MS)
        };
        busy(self.hovered) || busy(self.unhovered)
    }

    pub(super) fn animating(&self) -> bool {
        self.expanded
            .is_some_and(|at| at.elapsed().as_secs_f32() * 1000. < EXPAND_MS + SCALE_MS)
            || self.scaling()
            || self.cells.iter().any(|c| {
                c.appear.is_some_and(|a| a.playing()) || c.select.is_some_and(|s| s.playing())
            })
    }

    /// `Strip::setSelected`: the select animation plays once on entering a
    /// cell (not while its appear or select animation plays).
    pub(super) fn set_hovered(&mut self, k: usize, over: bool) {
        let now = std::time::Instant::now();
        if over {
            if let Some((h, _)) = self.hovered
                && h != k
            {
                self.unhovered = Some((h, now));
            }
            self.hovered = Some((k, now));
            if let Some(cell) = self.cells.get_mut(k) {
                let busy = cell.select_played
                    || !cell.appeared
                    || cell.select.is_some_and(|s| s.playing())
                    || cell.appear.is_some_and(|a| a.playing());
                if !busy && let Some(select) = &mut cell.select {
                    select.start = Some(now);
                    cell.select_played = true;
                }
            }
        } else if self.hovered.is_some_and(|(h, _)| h == k) {
            self.hovered = None;
            self.unhovered = Some((k, now));
        }
    }
}

impl ReactionsUi {
    /// An emoji reaction's appear and select animations.
    pub(super) fn strip_clips(&self, emoticon: &str, side: u32) -> (Option<Clip>, Option<Clip>) {
        let Some(r) = self.available.iter().find(|r| r.emoticon == emoticon) else {
            return (None, None);
        };
        (
            r.appear.as_ref().map(|d| Clip::new(d.id, side)),
            r.select.as_ref().map(|d| Clip::new(d.id, side)),
        )
    }
}

impl MainView {
    /// The cells' animations, once the strip's columns are known.
    pub(super) fn prepare_strip_cells(&self, strip: &mut StripState, image: f32, appeared: bool) {
        if strip.cells.len() >= strip.columns {
            return;
        }
        let side = (lg(image) * HOVER_SCALE * scale::get()).round() as u32;
        strip.cells = (0..strip.columns)
            .map(|k| {
                let (appear, select) = match strip.keys.get(k) {
                    Some(ReactionKey::Emoji(e)) => self.reactions_ui.strip_clips(e, side),
                    _ => (None, None),
                };
                StripCell {
                    appear,
                    select,
                    appeared,
                    select_played: false,
                }
            })
            .collect();
    }

    /// `Selector::updateShowState`: the cells revealed by the strip's growing
    /// width start their appear animations.
    pub(super) fn reveal_strip(strip: &mut StripState, progress: f32) {
        let size = lg(STRIP_SIZE);
        let skip = strip.skip_x();
        let shown = Self::strip_appeared_width(strip, progress);
        let now = std::time::Instant::now();
        for (k, cell) in strip.cells.iter_mut().enumerate() {
            let left = skip + k as f32 * size;
            if !cell.appeared && left + size / 2. <= shown {
                cell.appeared = true;
                if let Some(appear) = &mut cell.appear {
                    appear.start = Some(now);
                }
            }
        }
        strip.rearm();
    }

    /// `countAppearedWidth`.
    fn strip_appeared_width(strip: &StripState, progress: f32) -> f32 {
        let size = lg(STRIP_SIZE);
        let skip = strip.skip_x();
        let full = 2. * skip + strip.columns as f32 * size;
        let from = 2. * skip + size;
        from + (full - from) * progress.clamp(0., 1.)
    }

    /// The strip over the menu's box at (`mx`, `my`) `mw` wide
    /// (`Selector::paintAppearing` / `paintCollapsed`): a rounded bar with a
    /// soft shadow and a tail towards the menu, growing to the right with the
    /// menu, a cell for each reaction (its icon grows under the mouse), the
    /// expand arrow last.
    pub(super) fn render_strip(
        &self,
        strip: &StripState,
        (mx, my, mw, mh): (f32, f32, f32, f32),
        (progress, opacity): (f32, f32),
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        use crate::icons::{Icon, icon, icon_width};
        if let Some(at) = strip.expanded {
            return self.render_strip_panel(strip, at, (mx, my, mw, mh), opacity, window, view);
        }
        let p = self.palette();
        let [_, et, er, _] = STRIP_EXTEND.map(lg);
        let size = lg(STRIP_SIZE);
        let skip = strip.skip_x();
        let inner_w = 2. * skip + strip.columns as f32 * size;
        let strip_h = lg(STRIP_HEIGHT);
        // Saved Messages: the about text over the cells, in the same round.
        let about = self.strip_about(strip, inner_w, window);
        let about_h = about.as_ref().map_or(0., |a| a.height);
        let inner_h = strip_h + about_h;
        // Right-aligned with the menu's extended right edge, `et` above its box.
        let right = mx + mw + er;
        let (x, y) = (right - inner_w, my - et - about_h);
        let shown_w = Self::strip_appeared_width(strip, progress);
        let radius = strip_h / 2.;
        let bubble_w = icon_width(Icon::ReactionsBubble, window);
        let bubble_right = lg(STRIP_BUBBLE_RIGHT).min((shown_w - bubble_w) / 2.);
        let tail_x = shown_w - bubble_right - bubble_w;
        let mut cells = div()
            .absolute()
            .left_0()
            .top(gpui::px(about_h))
            .w(gpui::px(shown_w))
            .h(gpui::px(strip_h))
            .overflow_hidden();
        for k in 0..strip.columns {
            let expand = strip.expand && k + 1 == strip.columns;
            let key = (!expand).then(|| strip.keys[k].clone());
            let scale = strip.scale(k);
            let cell = strip.cells.get(k).cloned();
            let bounds_out = strip.cell_bounds.clone();
            let image = key.clone().map(|key| {
                canvas(
                    move |bounds, _, _| {
                        let mut all = bounds_out.borrow_mut();
                        if all.len() <= k {
                            all.resize(k + 1, gpui::Bounds::default());
                        }
                        all[k] = bounds;
                    },
                    move |bounds, _, window, _| {
                        paint_reaction_icon(
                            &key,
                            cell.as_ref(),
                            bounds,
                            lg(STRIP_IMAGE) * scale,
                            window,
                        );
                    },
                )
                .size_full()
            });
            let (hover, click) = (view.clone(), view.clone());
            cells = cells.child(
                div()
                    .id(("strip-cell", k))
                    .absolute()
                    .left(gpui::px(skip + k as f32 * size))
                    .top(gpui::px((strip_h - size) / 2.))
                    .size(gpui::px(size))
                    .cursor_pointer()
                    .children(image)
                    .when(expand, |d| {
                        d.flex().items_center().justify_center().child(
                            div()
                                .relative()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(Icon::ReactionsRoundBig, p.window_bg_ripple, window))
                                .child(div().absolute().child(icon(
                                    Icon::ReactionsExpandPanel,
                                    p.window_sub_text_fg,
                                    window,
                                ))),
                        )
                    })
                    .on_hover(move |over, _, cx| {
                        let over = *over;
                        let _ = hover.update(cx, |this, cx| {
                            if let Some(strip) = this.menu_strip_mut() {
                                strip.set_hovered(k, over);
                            }
                            cx.notify();
                        });
                    })
                    .on_click(move |_, window, cx| {
                        let key = key.clone();
                        let _ = click.update(cx, |this, cx| {
                            if expand {
                                this.expand_strip(window, cx);
                            } else {
                                this.strip_chosen(key, k);
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        div()
            .id("reaction-strip")
            .absolute()
            .left(gpui::px(x))
            .top(gpui::px(y))
            .w(gpui::px(shown_w))
            .h(gpui::px(inner_h))
            .opacity(opacity)
            .occlude()
            // The tail under the bar, its shadow first.
            .child(
                div()
                    .absolute()
                    .left(gpui::px(tail_x))
                    .top(gpui::px(inner_h))
                    .child(icon(
                        Icon::ReactionsBubbleShadow,
                        p.window_shadow_fg,
                        window,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(gpui::px(tail_x))
                    .top(gpui::px(inner_h))
                    .child(icon(Icon::ReactionsBubble, p.emoji_pan_bg, window)),
            )
            // `RoundAreaWithShadow` in `shadowFg`.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .rounded(gpui::px(radius))
                    .bg(c(p.emoji_pan_bg))
                    .shadow(vec![gpui::BoxShadow {
                        color: c(p.shadow_fg).into(),
                        offset: point(gpui::px(0.), gpui::px(lg(1.))),
                        blur_radius: gpui::px(lg(STRIP_SHADOW[0])),
                        spread_radius: gpui::px(0.),
                    }]),
            )
            .child(cells)
            .children(
                about.map(|a| {
                    Self::render_strip_about(a, shown_w, p, window, view).into_any_element()
                }),
            )
            .into_any_element()
    }

    /// `ItemReactionsAbout` laid out at the strip's width (`aboutPadding`,
    /// 11 px words wrapped and centered).
    pub(super) fn strip_about(
        &self,
        strip: &StripState,
        width: f32,
        window: &Window,
    ) -> Option<StripAbout> {
        if !strip.tags {
            return None;
        }
        let words: Vec<(String, bool)> = if self.reactions_ui.premium {
            "Tag this message with an emoji for quick search."
                .split(' ')
                .map(|w| (w.to_string(), false))
                .collect()
        } else {
            "Organize your Saved Messages with tags."
                .split(' ')
                .map(|w| (w.to_string(), false))
                .chain([("Learn".to_string(), true), ("More...".to_string(), true)])
                .collect()
        };
        let available = width - lg(ABOUT_PADDING[0]) - lg(ABOUT_PADDING[2]);
        let space = text_width(window, " ", ABOUT_FONT, FontWeight::NORMAL);
        let mut lines: Vec<Vec<(String, bool)>> = vec![Vec::new()];
        let mut x = 0.;
        for (word, link) in words {
            let w = text_width(window, &word, ABOUT_FONT, FontWeight::NORMAL);
            let line = lines.last_mut().unwrap();
            if !line.is_empty() && x + space + w > available {
                lines.push(vec![(word, link)]);
                x = w;
            } else {
                x += if line.is_empty() { w } else { space + w };
                line.push((word, link));
            }
        }
        let line_h = scale::font(ABOUT_FONT).height;
        Some(StripAbout {
            height: lg(ABOUT_PADDING[1]) + lines.len() as f32 * line_h + lg(ABOUT_PADDING[3]),
            lines,
        })
    }

    /// The about text: grey, its link in the link color (Premium's page is
    /// not here: a toast says what it is about).
    pub(super) fn render_strip_about(
        about: StripAbout,
        width: f32,
        p: &tdr_theme::Palette,
        _window: &Window,
        view: &WeakEntity<Self>,
    ) -> impl IntoElement {
        let line_h = scale::font(ABOUT_FONT).line_height();
        let mut column = div()
            .absolute()
            .left_0()
            .top(gpui::px(lg(ABOUT_PADDING[1])))
            .w(gpui::px(width))
            .flex()
            .flex_col()
            .items_center()
            .text_size(scale::font(ABOUT_FONT).size)
            .line_height(line_h);
        for (n, line) in about.lines.into_iter().enumerate() {
            let mut row = div().flex().whitespace_nowrap();
            let mut text = String::new();
            let mut link = String::new();
            for (word, is_link) in line {
                let target = if is_link { &mut link } else { &mut text };
                if !target.is_empty() {
                    target.push(' ');
                }
                target.push_str(&word);
            }
            if !text.is_empty() {
                let space = if link.is_empty() { "" } else { " " };
                row = row.child(
                    div()
                        .text_color(c(p.window_sub_text_fg))
                        .child(format!("{text}{space}")),
                );
            }
            if !link.is_empty() {
                let view = view.clone();
                row = row.child(
                    div()
                        .id(("strip-about-link", n))
                        .cursor_pointer()
                        .text_color(c(p.window_active_text_fg))
                        .child(link)
                        .on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.react_premium_error(0, &ReactionKey::Paid, true);
                                this.hide_menu();
                                cx.notify();
                            });
                        }),
                );
            }
            column = column.child(row);
        }
        column
    }
}

/// `Strip::paintOne`: a reaction's icon `image` wide in the middle of
/// `bounds`: the appear animation while it plays, else the select one (its
/// first frame at rest); the emoji itself until the server's list comes.
pub(super) fn paint_reaction_icon(
    key: &ReactionKey,
    cell: Option<&StripCell>,
    bounds: gpui::Bounds<gpui::Pixels>,
    image: f32,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let target = gpui::Bounds::new(
        point(
            bounds.origin.x + gpui::px((w - image) / 2.),
            bounds.origin.y + gpui::px((h - image) / 2.),
        ),
        gpui::size(gpui::px(image), gpui::px(image)),
    );
    let clip = cell.and_then(|c| c.appear.filter(|a| a.playing()).or(c.select).or(c.appear));
    match (key, clip) {
        (ReactionKey::Custom(id), _) => {
            let side = (image * scale::get()).round() as u32;
            crate::custom_emoji::paint(*id, target.origin, side, gpui::Hsla::default(), window);
        }
        (_, Some(clip)) => {
            clip.paint(target, window);
        }
        (ReactionKey::Emoji(e), None) => {
            if let Some((emoji, _)) = tdr_emoji::find(e) {
                let s = scale::get();
                let side = crate::emoji::size_large();
                let size = side as f32 / s;
                let at = point(
                    bounds.origin.x + gpui::px(((w - size) / 2.).floor()),
                    bounds.origin.y + gpui::px(((h - size) / 2.).floor()),
                );
                crate::emoji::paint(emoji, at, side, window);
            }
        }
        (ReactionKey::Paid, None) => {}
    }
}
