//! The update state machine (no I/O), following https://core.telegram.org/api/updates
//! and tdesktop's `api/api_updates.cpp`.
//!
//! Every update that changes the message box carries a sequence number: `pts` for the
//! common box (private chats and basic groups), a per-channel `pts`, `qts` for secret
//! chats and bot events, `seq` for update containers. An update is applied only if it
//! continues the local sequence exactly; a gap is waited out for a moment (the missing
//! update may still arrive out of order) and then recovered with `getDifference`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tdr_tl::{enums, types};

/// How long to wait for a missing update before asking for the difference
/// (tdesktop: `PtsWaiter::kWaitForSkippedTimeout`).
pub const GAP_TIMEOUT: Duration = Duration::from_millis(1000);

/// Position in the common update sequence. Persisted together with the data it
/// describes, so a restart continues exactly where it stopped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub pts: i32,
    pub qts: i32,
    pub date: i32,
    pub seq: i32,
}

impl From<&types::updates::State> for State {
    fn from(s: &types::updates::State) -> Self {
        Self {
            pts: s.pts,
            qts: s.qts,
            date: s.date,
            seq: s.seq,
        }
    }
}

/// Something to apply to the local data, in order.
#[derive(Debug, Clone)]
pub enum Item {
    Users(Vec<enums::User>),
    Chats(Vec<enums::Chat>),
    /// One update, already checked against its sequence.
    Update(enums::Update),
    /// Messages from a difference (new since the last known state).
    NewMessages(Vec<enums::Message>),
    /// `updateShortMessage`: a private message in compact form.
    ShortMessage(Box<types::UpdateShortMessage>),
    /// `updateShortChatMessage`: a basic-group message in compact form.
    ShortChatMessage(Box<types::UpdateShortChatMessage>),
    /// Too much was missed (`updates.differenceTooLong`): local chat data is stale and
    /// should be reloaded (e.g. the chat list).
    ResetCommon,
    /// Too much was missed in a channel: replace its local history with `messages`.
    ResetChannel {
        channel_id: i64,
        dialog: enums::Dialog,
        messages: Vec<enums::Message>,
    },
}

/// A request the driver must make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    GetDifference,
    GetChannelDifference { channel_id: i64, pts: i32 },
}

#[derive(Debug, Default)]
pub struct Output {
    pub items: Vec<Item>,
    pub actions: Vec<Action>,
}

impl Output {
    fn merge(&mut self, other: Output) {
        self.items.extend(other.items);
        for a in other.actions {
            if !self.actions.contains(&a) {
                self.actions.push(a);
            }
        }
    }
}

/// Which sequence an update belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequence {
    Common {
        pts: i32,
        count: i32,
    },
    Channel {
        channel_id: i64,
        pts: i32,
        count: i32,
    },
    Qts {
        qts: i32,
    },
    ChannelTooLong {
        channel_id: i64,
    },
    /// Not part of any sequence (typing, statuses, ...): applied as is.
    None,
}

fn peer_channel(peer: Option<&enums::Peer>) -> Option<i64> {
    match peer {
        Some(enums::Peer::Channel(c)) => Some(c.channel_id),
        _ => None,
    }
}

/// The channel a message belongs to, if any.
pub fn message_channel(m: &enums::Message) -> Option<i64> {
    match m {
        enums::Message::Message(m) => peer_channel(Some(&m.peer_id)),
        enums::Message::Service(m) => peer_channel(Some(&m.peer_id)),
        enums::Message::Empty(m) => peer_channel(m.peer_id.as_ref()),
    }
}

/// Names of the `Update` constructors classified below; a test compares this list with
/// the schema so that a new sequenced update in an upstream layer is not missed.
pub const SEQUENCED_UPDATES: &[&str] = &[
    "updateNewMessage",
    "updateDeleteMessages",
    "updateReadHistoryInbox",
    "updateReadHistoryOutbox",
    "updateWebPage",
    "updateReadMessagesContents",
    "updateEditMessage",
    "updateFolderPeers",
    "updatePinnedMessages",
    "updateMessageWidgetData",
    "updateNewChannelMessage",
    "updateEditChannelMessage",
    "updateDeleteChannelMessages",
    "updateChannelWebPage",
    "updatePinnedChannelMessages",
    "updateChannelTooLong",
    "updateReadChannelInbox",
    "updateNewEncryptedMessage",
    "updateMessagePollVote",
    "updateChatParticipant",
    "updateChannelParticipant",
    "updateBotStopped",
    "updateBotChatInviteRequester",
    "updateBotChatBoost",
    "updateBotMessageReaction",
    "updateBotMessageReactions",
    "updateBotBusinessConnect",
    "updateBotNewBusinessMessage",
    "updateBotEditBusinessMessage",
    "updateBotDeleteBusinessMessage",
    "updateBotPurchasedPaidMedia",
    "updateManagedBot",
    "updateBotGuestChatQuery",
    "updateBotStarsSubscription",
];

pub fn classify(u: &enums::Update) -> Sequence {
    use enums::Update as U;
    let common = |pts, count| Sequence::Common { pts, count };
    let channel = |channel_id, pts, count| Sequence::Channel {
        channel_id,
        pts,
        count,
    };
    let qts = |qts| Sequence::Qts { qts };
    match u {
        U::NewMessage(x) => common(x.pts, x.pts_count),
        U::DeleteMessages(x) => common(x.pts, x.pts_count),
        U::ReadHistoryInbox(x) => common(x.pts, x.pts_count),
        U::ReadHistoryOutbox(x) => common(x.pts, x.pts_count),
        U::WebPage(x) => common(x.pts, x.pts_count),
        U::ReadMessagesContents(x) => common(x.pts, x.pts_count),
        U::EditMessage(x) => common(x.pts, x.pts_count),
        U::FolderPeers(x) => common(x.pts, x.pts_count),
        U::PinnedMessages(x) => common(x.pts, x.pts_count),
        U::MessageWidgetData(x) => common(x.pts, x.pts_count),

        U::NewChannelMessage(x) => match message_channel(&x.message) {
            Some(id) => channel(id, x.pts, x.pts_count),
            None => Sequence::None,
        },
        U::EditChannelMessage(x) => match message_channel(&x.message) {
            Some(id) => channel(id, x.pts, x.pts_count),
            None => Sequence::None,
        },
        U::DeleteChannelMessages(x) => channel(x.channel_id, x.pts, x.pts_count),
        U::ChannelWebPage(x) => channel(x.channel_id, x.pts, x.pts_count),
        U::PinnedChannelMessages(x) => channel(x.channel_id, x.pts, x.pts_count),
        U::ChannelTooLong(x) => Sequence::ChannelTooLong {
            channel_id: x.channel_id,
        },
        // Carries the channel's pts for information only: no pts_count, no gap check.
        U::ReadChannelInbox(_) => Sequence::None,

        U::NewEncryptedMessage(x) => qts(x.qts),
        U::MessagePollVote(x) => qts(x.qts),
        U::ChatParticipant(x) => qts(x.qts),
        U::ChannelParticipant(x) => qts(x.qts),
        U::BotStopped(x) => qts(x.qts),
        U::BotChatInviteRequester(x) => qts(x.qts),
        U::BotChatBoost(x) => qts(x.qts),
        U::BotMessageReaction(x) => qts(x.qts),
        U::BotMessageReactions(x) => qts(x.qts),
        U::BotBusinessConnect(x) => qts(x.qts),
        U::BotNewBusinessMessage(x) => qts(x.qts),
        U::BotEditBusinessMessage(x) => qts(x.qts),
        U::BotDeleteBusinessMessage(x) => qts(x.qts),
        U::BotPurchasedPaidMedia(x) => qts(x.qts),
        U::ManagedBot(x) => qts(x.qts),
        U::BotGuestChatQuery(x) => qts(x.qts),
        U::BotStarsSubscription(x) => qts(x.qts),
        _ => Sequence::None,
    }
}

/// An update waiting for the updates before it.
#[derive(Debug)]
struct Pending {
    pts: i32,
    count: i32,
    item: Item,
}

#[derive(Debug, Default)]
struct Channel {
    pts: i32,
    pending: Vec<Pending>,
    deadline: Option<Instant>,
    /// A getChannelDifference is in flight.
    requesting: bool,
}

/// What the result of [`Engine::feed`] etc. changed in the persisted state.
#[derive(Debug, Default)]
pub struct Engine {
    state: State,
    /// No state yet (or a difference in flight): sequenced updates are held back.
    requesting: bool,
    pending: Vec<Pending>,
    pending_qts: Vec<Pending>,
    deadline: Option<Instant>,
    seq_deadline: Option<Instant>,
    channels: HashMap<i64, Channel>,
    /// Channels whose pts changed since the last [`Engine::take_dirty_channels`].
    dirty_channels: HashSet<i64>,
}

impl Engine {
    /// `state`: the saved position, or `None` before the first `updates.getState`.
    pub fn new(state: Option<State>, channels: impl IntoIterator<Item = (i64, i32)>) -> Self {
        Self {
            state: state.unwrap_or_default(),
            requesting: state.is_none(),
            channels: channels
                .into_iter()
                .map(|(id, pts)| {
                    (
                        id,
                        Channel {
                            pts,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn has_state(&self) -> bool {
        self.state.pts != 0
    }

    pub fn channel_pts(&self, channel_id: i64) -> Option<i32> {
        self.channels
            .get(&channel_id)
            .map(|c| c.pts)
            .filter(|p| *p > 0)
    }

    /// Records a channel's pts learnt elsewhere (e.g. `dialog.pts` from the chat list).
    pub fn set_channel_pts(&mut self, channel_id: i64, pts: i32) {
        let c = self.channels.entry(channel_id).or_default();
        if pts > c.pts {
            c.pts = pts;
            self.dirty_channels.insert(channel_id);
        }
    }

    /// `(channel_id, pts)` changed since the last call, for saving.
    pub fn take_dirty_channels(&mut self) -> Vec<(i64, i32)> {
        let ids: Vec<i64> = self.dirty_channels.drain().collect();
        ids.into_iter()
            .filter_map(|id| self.channels.get(&id).map(|c| (id, c.pts)))
            .collect()
    }

    /// Sets the state from `updates.getState`.
    pub fn set_state(&mut self, state: State) -> Output {
        self.state = state;
        self.requesting = false;
        self.drain_common()
    }

    /// The earliest moment [`Engine::poll`] has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        [self.deadline, self.seq_deadline]
            .into_iter()
            .chain(self.channels.values().map(|c| c.deadline))
            .flatten()
            .min()
    }

    /// Call when a deadline passes: unresolved gaps turn into difference requests.
    pub fn poll(&mut self, now: Instant) -> Output {
        let mut out = Output::default();
        let common_due = self.deadline.is_some_and(|d| d <= now);
        let seq_due = self.seq_deadline.is_some_and(|d| d <= now);
        if (common_due || seq_due) && !self.requesting {
            self.start_difference(&mut out);
        }
        for (&id, c) in &mut self.channels {
            if c.deadline.is_some_and(|d| d <= now) && !c.requesting {
                c.deadline = None;
                c.requesting = true;
                out.actions.push(Action::GetChannelDifference {
                    channel_id: id,
                    pts: c.pts,
                });
            }
        }
        out
    }

    fn start_difference(&mut self, out: &mut Output) {
        self.requesting = true;
        self.deadline = None;
        self.seq_deadline = None;
        out.actions.push(Action::GetDifference);
    }

    /// The connection was re-established or the server started a new session: updates
    /// may have been lost in between.
    pub fn connection_restored(&mut self) -> Output {
        let mut out = Output::default();
        if self.has_state() && !self.requesting {
            self.start_difference(&mut out);
        }
        out
    }

    /// Feeds an `Updates` object received from the server (pushed or as an RPC result).
    pub fn feed(&mut self, updates: enums::Updates, now: Instant) -> Output {
        let mut out = Output::default();
        match updates {
            enums::Updates::TooLong => {
                if !self.requesting {
                    self.start_difference(&mut out);
                }
            }
            enums::Updates::UpdateShortMessage(m) => {
                let (pts, count) = (m.pts, m.pts_count);
                self.state.date = self.state.date.max(m.date);
                out.merge(self.feed_common(pts, count, Item::ShortMessage(m), now));
            }
            enums::Updates::UpdateShortChatMessage(m) => {
                let (pts, count) = (m.pts, m.pts_count);
                self.state.date = self.state.date.max(m.date);
                out.merge(self.feed_common(pts, count, Item::ShortChatMessage(m), now));
            }
            enums::Updates::UpdateShort(s) => {
                out.merge(self.feed_update(s.update, now));
                self.state.date = self.state.date.max(s.date);
            }
            enums::Updates::UpdateShortSentMessage(s) => {
                // The answer to our own sendMessage: only advances pts.
                out.merge(self.feed_pts_only(s.pts, s.pts_count, now));
            }
            enums::Updates::Combined(c) => {
                out.merge(self.feed_container(
                    c.seq_start,
                    c.seq,
                    c.date,
                    c.users,
                    c.chats,
                    c.updates,
                    now,
                ));
            }
            enums::Updates::Updates(c) => {
                out.merge(
                    self.feed_container(c.seq, c.seq, c.date, c.users, c.chats, c.updates, now),
                );
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn feed_container(
        &mut self,
        seq_start: i32,
        seq: i32,
        date: i32,
        users: Vec<enums::User>,
        chats: Vec<enums::Chat>,
        updates: Vec<enums::Update>,
        now: Instant,
    ) -> Output {
        let mut out = Output::default();
        if seq != 0 && self.has_state() && !self.requesting {
            if seq <= self.state.seq {
                return out; // Already applied.
            }
            if seq_start > self.state.seq + 1 {
                // A container is missing: wait a moment, then get the difference. The
                // sequenced updates inside are still checked one by one below.
                self.seq_deadline.get_or_insert(now + GAP_TIMEOUT);
            } else {
                self.state.seq = seq;
                self.seq_deadline = None;
            }
        }
        if !users.is_empty() {
            out.items.push(Item::Users(users));
        }
        if !chats.is_empty() {
            out.items.push(Item::Chats(chats));
        }
        for u in updates {
            out.merge(self.feed_update(u, now));
        }
        if date > self.state.date {
            self.state.date = date;
        }
        out
    }

    fn feed_update(&mut self, u: enums::Update, now: Instant) -> Output {
        match classify(&u) {
            Sequence::Common { pts, count } => self.feed_common(pts, count, Item::Update(u), now),
            Sequence::Channel {
                channel_id,
                pts,
                count,
            } => self.feed_channel(channel_id, pts, count, Item::Update(u), now),
            Sequence::Qts { qts } => self.feed_qts(qts, Item::Update(u), now),
            Sequence::ChannelTooLong { channel_id } => self.channel_too_long(channel_id),
            Sequence::None => Output {
                items: vec![Item::Update(u)],
                actions: vec![],
            },
        }
    }

    fn feed_pts_only(&mut self, pts: i32, count: i32, now: Instant) -> Output {
        // An empty marker keeps the ordering logic uniform.
        self.feed_common(pts, count, Item::Users(vec![]), now)
    }

    fn feed_common(&mut self, pts: i32, count: i32, item: Item, now: Instant) -> Output {
        self.pending.push(Pending { pts, count, item });
        if self.requesting {
            return Output::default();
        }
        let mut out = self.drain_common();
        if !self.pending.is_empty() {
            self.deadline.get_or_insert(now + GAP_TIMEOUT);
        } else {
            self.deadline = None;
        }
        out.items
            .retain(|i| !matches!(i, Item::Users(u) if u.is_empty()));
        out
    }

    /// Applies pending common updates that continue the local pts; drops duplicates.
    fn drain_common(&mut self) -> Output {
        let mut out = Output::default();
        let applied = drain(&mut self.pending, &mut self.state.pts, &mut out.items);
        if applied && self.pending.is_empty() {
            self.deadline = None;
        }
        let applied_qts = drain(&mut self.pending_qts, &mut self.state.qts, &mut out.items);
        let _ = applied_qts;
        out.items
            .retain(|i| !matches!(i, Item::Users(u) if u.is_empty()));
        out
    }

    fn feed_qts(&mut self, qts: i32, item: Item, now: Instant) -> Output {
        self.pending_qts.push(Pending {
            pts: qts,
            count: 1,
            item,
        });
        if self.requesting {
            return Output::default();
        }
        let mut out = Output::default();
        drain(&mut self.pending_qts, &mut self.state.qts, &mut out.items);
        if !self.pending_qts.is_empty() {
            self.deadline.get_or_insert(now + GAP_TIMEOUT);
        }
        out
    }

    fn feed_channel(
        &mut self,
        channel_id: i64,
        pts: i32,
        count: i32,
        item: Item,
        now: Instant,
    ) -> Output {
        let mut out = Output::default();
        let c = self.channels.entry(channel_id).or_default();
        if c.pts == 0 {
            // A channel we know nothing about yet: take its position from this update.
            c.pts = pts;
            self.dirty_channels.insert(channel_id);
            out.items.push(item);
            return out;
        }
        c.pending.push(Pending { pts, count, item });
        if c.requesting {
            return out;
        }
        let before = c.pts;
        drain(&mut c.pending, &mut c.pts, &mut out.items);
        if c.pts != before {
            self.dirty_channels.insert(channel_id);
        }
        if c.pending.is_empty() {
            c.deadline = None;
        } else {
            c.deadline.get_or_insert(now + GAP_TIMEOUT);
        }
        out
    }

    fn channel_too_long(&mut self, channel_id: i64) -> Output {
        let mut out = Output::default();
        let c = self.channels.entry(channel_id).or_default();
        if !c.requesting && c.pts > 0 {
            c.requesting = true;
            c.deadline = None;
            out.actions.push(Action::GetChannelDifference {
                channel_id,
                pts: c.pts,
            });
        }
        out
    }

    /// Applies the result of `updates.getDifference`.
    pub fn apply_difference(&mut self, diff: enums::updates::Difference, now: Instant) -> Output {
        let mut out = Output::default();
        match diff {
            enums::updates::Difference::Empty(e) => {
                self.state.date = e.date;
                self.state.seq = e.seq;
                self.finish_difference(&mut out, now);
            }
            enums::updates::Difference::Difference(d) => {
                self.apply_difference_body(
                    d.users,
                    d.chats,
                    d.new_messages,
                    d.other_updates,
                    &mut out,
                    now,
                );
                self.state = State::from(match &d.state {
                    enums::updates::State::State(s) => &**s,
                });
                self.finish_difference(&mut out, now);
            }
            enums::updates::Difference::Slice(d) => {
                self.apply_difference_body(
                    d.users,
                    d.chats,
                    d.new_messages,
                    d.other_updates,
                    &mut out,
                    now,
                );
                self.state = State::from(match &d.intermediate_state {
                    enums::updates::State::State(s) => &**s,
                });
                out.actions.push(Action::GetDifference);
            }
            enums::updates::Difference::TooLong(t) => {
                self.state.pts = t.pts;
                out.items.push(Item::ResetCommon);
                self.finish_difference(&mut out, now);
            }
        }
        out
    }

    fn apply_difference_body(
        &mut self,
        users: Vec<enums::User>,
        chats: Vec<enums::Chat>,
        messages: Vec<enums::Message>,
        other: Vec<enums::Update>,
        out: &mut Output,
        now: Instant,
    ) {
        if !users.is_empty() {
            out.items.push(Item::Users(users));
        }
        if !chats.is_empty() {
            out.items.push(Item::Chats(chats));
        }
        if !messages.is_empty() {
            out.items.push(Item::NewMessages(messages));
        }
        for u in other {
            match classify(&u) {
                // Common and qts updates in a difference are already in order.
                Sequence::Common { .. } | Sequence::Qts { .. } | Sequence::None => {
                    out.items.push(Item::Update(u));
                }
                Sequence::Channel {
                    channel_id,
                    pts,
                    count,
                } => {
                    out.merge(self.feed_channel(channel_id, pts, count, Item::Update(u), now));
                }
                Sequence::ChannelTooLong { channel_id } => {
                    out.merge(self.channel_too_long(channel_id))
                }
            }
        }
    }

    fn finish_difference(&mut self, out: &mut Output, now: Instant) {
        self.requesting = false;
        self.seq_deadline = None;
        out.merge(self.drain_common());
        if !self.pending.is_empty() || !self.pending_qts.is_empty() {
            self.deadline.get_or_insert(now + GAP_TIMEOUT);
        }
    }

    /// Applies the result of `updates.getChannelDifference`.
    pub fn apply_channel_difference(
        &mut self,
        channel_id: i64,
        diff: enums::updates::ChannelDifference,
        now: Instant,
    ) -> Output {
        let mut out = Output::default();
        let finished = match diff {
            enums::updates::ChannelDifference::Empty(e) => {
                self.set_channel_pts_exact(channel_id, e.pts);
                true
            }
            enums::updates::ChannelDifference::ChannelDifference(d) => {
                if !d.users.is_empty() {
                    out.items.push(Item::Users(d.users));
                }
                if !d.chats.is_empty() {
                    out.items.push(Item::Chats(d.chats));
                }
                if !d.new_messages.is_empty() {
                    out.items.push(Item::NewMessages(d.new_messages));
                }
                out.items
                    .extend(d.other_updates.into_iter().map(Item::Update));
                self.set_channel_pts_exact(channel_id, d.pts);
                if !d.r#final {
                    out.actions.push(Action::GetChannelDifference {
                        channel_id,
                        pts: d.pts,
                    });
                }
                d.r#final
            }
            enums::updates::ChannelDifference::TooLong(d) => {
                let pts = match &d.dialog {
                    enums::Dialog::Dialog(x) => x.pts,
                    _ => None,
                };
                if let Some(pts) = pts {
                    self.set_channel_pts_exact(channel_id, pts);
                }
                out.items.push(Item::Users(d.users));
                out.items.push(Item::Chats(d.chats));
                out.items.push(Item::ResetChannel {
                    channel_id,
                    dialog: d.dialog,
                    messages: d.messages,
                });
                true
            }
        };
        if finished {
            let c = self.channels.entry(channel_id).or_default();
            c.requesting = false;
            drain(&mut c.pending, &mut c.pts, &mut out.items);
            if !c.pending.is_empty() {
                c.deadline.get_or_insert(now + GAP_TIMEOUT);
            }
        }
        out.items
            .retain(|i| !matches!(i, Item::Users(u) if u.is_empty()));
        out.items
            .retain(|i| !matches!(i, Item::Chats(c) if c.is_empty()));
        out
    }

    fn set_channel_pts_exact(&mut self, channel_id: i64, pts: i32) {
        self.channels.entry(channel_id).or_default().pts = pts;
        self.dirty_channels.insert(channel_id);
    }

    /// A channel difference failed for good (e.g. we left the channel): stop tracking it.
    pub fn forget_channel(&mut self, channel_id: i64) {
        self.channels.remove(&channel_id);
        self.dirty_channels.remove(&channel_id);
    }

    /// A difference request failed: allow a new attempt on the next gap or reconnect.
    pub fn difference_failed(&mut self, now: Instant) {
        self.requesting = false;
        self.deadline = Some(now + GAP_TIMEOUT);
    }
}

/// Applies pending entries that continue `*pts` (in arrival order) and drops the ones
/// already applied. An entry continues the sequence when `local + count == pts`; it is a
/// duplicate when `local + count > pts` and waits for a gap to fill when `<`.
/// Returns whether anything was applied.
fn drain(pending: &mut Vec<Pending>, pts: &mut i32, items: &mut Vec<Item>) -> bool {
    let mut applied = false;
    loop {
        pending.retain(|p| p.pts - p.count >= *pts);
        let Some(i) = pending.iter().position(|p| p.pts - p.count == *pts) else {
            break;
        };
        let p = pending.remove(i);
        *pts = p.pts;
        items.push(p.item);
        applied = true;
    }
    applied
}

#[cfg(test)]
mod tests;
