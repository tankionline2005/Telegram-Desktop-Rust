//! What the UI needs to know about cached messages and peers beyond their text and
//! name: media kind, service action, "edited", peer kind, color, members, status,
//! userpic. Read from the TL objects kept in the cache rows (`raw`), so nothing has to
//! be stored twice. The UI turns these into strings (tdesktop's `lng_in_dlg_*`,
//! `lng_action_*`).

use tdr_storage::{MessageRow, PeerKey, PeerRow};
use tdr_tl::{Deserializable, enums};

/// A message's media, as the chat list names it (`HistoryItem::notificationText`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Media {
    Photo,
    Video,
    Gif,
    VoiceMessage,
    VideoMessage,
    /// Music: performer and title (or the file name).
    Audio(String),
    /// The sticker's emoji.
    Sticker(String),
    /// The file name.
    File(String),
    Contact,
    Location,
    LiveLocation,
    /// The venue title.
    Venue(String),
    /// The question.
    Poll(String),
    /// The emoji (🎲, 🎯, ...).
    Dice(String),
    Game(String),
    Invoice(String),
    Story,
    Giveaway,
    Checklist,
    /// Anything else (unsupported here).
    Other,
}

/// A service message (`messageService`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    ChatCreate {
        title: String,
    },
    ChannelCreate,
    EditTitle {
        title: String,
    },
    EditPhoto,
    DeletePhoto,
    AddUsers {
        users: Vec<i64>,
    },
    DeleteUser {
        user: i64,
    },
    JoinedByLink,
    JoinedByRequest,
    Migrated,
    PinMessage,
    HistoryClear,
    Call {
        video: bool,
        missed: bool,
        duration: Option<i32>,
    },
    GroupCall {
        ended: bool,
    },
    ScreenshotTaken,
    ContactSignUp,
    Other,
}

/// What a message replies to (`messageReplyHeader`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyTo {
    pub msg_id: Option<i32>,
    /// The chat of the replied message when it is another one.
    pub peer: Option<PeerKey>,
    /// The thread's root (a topic, a post's comments).
    pub top_id: Option<i32>,
    pub forum_topic: bool,
    /// The quoted part (`quote_text`).
    pub quote: Option<String>,
    /// A reply to a message of a chat we can't see (`reply_from`): its sender.
    pub from: Option<PeerKey>,
    pub from_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageInfo {
    pub media: Option<Media>,
    pub service: Option<Service>,
    /// Shown as "edited" next to the time (`edit_date` without `edit_hide`).
    pub edited: bool,
    /// When it was last edited (shown in its context menu).
    pub edit_date: Option<i32>,
    /// Messages of one album share it.
    pub grouped_id: Option<i64>,
    /// A channel post (views, signature).
    pub post: bool,
    /// How many saw the post (`views`).
    pub views: Option<i32>,
    /// The author's signature of a post (`post_author`).
    pub post_author: Option<String>,
    /// A post with a discussion group: its comment count (`replies` with
    /// `comments`).
    pub comments: Option<i32>,
    /// The last commenters (`recent_repliers`), for the comments button.
    pub recent_repliers: Vec<PeerKey>,
    /// The discussion group of a post's comments (`replies.channel_id`).
    pub comments_channel: Option<i64>,
    pub reply: Option<ReplyTo>,
    pub forward: Option<Forward>,
    /// Formatting and links of the text (`message.entities`).
    pub entities: Vec<Entity>,
}

/// A formatting or link entity of a message text (`MessageEntity`). Offsets are in
/// UTF-16 code units, as the API sends them; [`Entity::byte_range`] converts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub offset: i32,
    pub length: i32,
    pub kind: EntityKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    /// Inline code: copied on click.
    Code,
    /// A code block with its language (empty: none).
    Pre {
        language: String,
    },
    Blockquote {
        collapsed: bool,
    },
    /// The text is the link.
    Url,
    /// A link hidden behind other text.
    TextUrl(String),
    Email,
    /// `@username`.
    Mention,
    /// A mention of a user without a username.
    MentionName(i64),
    Hashtag,
    Cashtag,
    BotCommand,
    Phone,
    BankCard,
    /// A date (`messageEntityFormattedDate`): with no flags the text stays and is a
    /// link; with flags the client shows the date in that format instead.
    FormattedDate {
        date: i32,
        flags: DateFlags,
    },
}

/// How a formatted date is shown (`FormattedDateFlag`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DateFlags {
    pub relative: bool,
    pub short_time: bool,
    pub long_time: bool,
    pub short_date: bool,
    pub long_date: bool,
    pub day_of_week: bool,
}

impl DateFlags {
    pub fn any(self) -> bool {
        self != Self::default()
    }
}

impl Entity {
    /// The entity's byte range in `text`, or `None` when it falls outside it.
    pub fn byte_range(&self, text: &str) -> Option<std::ops::Range<usize>> {
        let start = utf16_to_byte(text, self.offset.max(0) as usize)?;
        let end = utf16_to_byte(text, (self.offset.max(0) + self.length.max(0)) as usize)?;
        (start < end).then_some(start..end)
    }
}

/// The byte offset of UTF-16 offset `units` (rounded up to a char boundary), `None`
/// past the end.
fn utf16_to_byte(text: &str, units: usize) -> Option<usize> {
    let mut counted = 0;
    for (i, ch) in text.char_indices() {
        if counted >= units {
            return Some(i);
        }
        counted += ch.len_utf16();
    }
    (counted >= units).then_some(text.len())
}

fn entities(list: &[enums::MessageEntity]) -> Vec<Entity> {
    use enums::MessageEntity as E;
    list.iter()
        .filter_map(|e| {
            let (offset, length, kind) = match e {
                E::Bold(e) => (e.offset, e.length, EntityKind::Bold),
                E::Italic(e) => (e.offset, e.length, EntityKind::Italic),
                E::Underline(e) => (e.offset, e.length, EntityKind::Underline),
                E::Strike(e) => (e.offset, e.length, EntityKind::Strike),
                E::Spoiler(e) => (e.offset, e.length, EntityKind::Spoiler),
                E::Code(e) => (e.offset, e.length, EntityKind::Code),
                E::Pre(e) => (
                    e.offset,
                    e.length,
                    EntityKind::Pre {
                        language: e.language.clone(),
                    },
                ),
                E::Blockquote(e) => (
                    e.offset,
                    e.length,
                    EntityKind::Blockquote {
                        collapsed: e.collapsed,
                    },
                ),
                E::Url(e) => (e.offset, e.length, EntityKind::Url),
                E::TextUrl(e) => (e.offset, e.length, EntityKind::TextUrl(e.url.clone())),
                E::Email(e) => (e.offset, e.length, EntityKind::Email),
                E::Mention(e) => (e.offset, e.length, EntityKind::Mention),
                E::MentionName(e) => (e.offset, e.length, EntityKind::MentionName(e.user_id)),
                E::Hashtag(e) => (e.offset, e.length, EntityKind::Hashtag),
                E::Cashtag(e) => (e.offset, e.length, EntityKind::Cashtag),
                E::BotCommand(e) => (e.offset, e.length, EntityKind::BotCommand),
                E::Phone(e) => (e.offset, e.length, EntityKind::Phone),
                E::BankCard(e) => (e.offset, e.length, EntityKind::BankCard),
                E::FormattedDate(e) => (
                    e.offset,
                    e.length,
                    EntityKind::FormattedDate {
                        date: e.date,
                        flags: DateFlags {
                            relative: e.relative,
                            short_time: e.short_time,
                            long_time: e.long_time,
                            short_date: e.short_date,
                            long_date: e.long_date,
                            day_of_week: e.day_of_week,
                        },
                    },
                ),
                _ => return None,
            };
            Some(Entity {
                offset,
                length,
                kind,
            })
        })
        .collect()
}

/// Where a forwarded message came from (`HistoryMessageForwarded`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forward {
    /// The original sender (`fwd_from.from_id`).
    pub from: Option<PeerKey>,
    /// A forward of a forward (`forwardOfForward`: `saved_from_id` or
    /// `saved_from_name`): who it was saved from, and whether it was outgoing there.
    pub forward_of_forward: bool,
    pub saved_from: Option<PeerKey>,
    pub saved_out: bool,
    /// A hidden original sender (`from_name`), a channel post's signature.
    pub from_name: Option<String>,
    pub post_author: Option<String>,
    /// Who it was saved from when hidden (`saved_from_name`).
    pub saved_from_name: Option<String>,
    /// The message this copy was saved from (`saved_from_peer`, `saved_from_msg_id`):
    /// the "go to original" button opens it.
    pub original: Option<(PeerKey, i32)>,
    /// When the original was sent (`fwd_from.date`).
    pub date: i32,
}

impl Forward {
    /// `Message::hasOutLayout` in Saved Messages: a forward is drawn as outgoing only
    /// when it came from `self_id` (or was saved from them, or was outgoing there).
    pub fn outgoing_in_saved(&self, self_id: PeerKey) -> bool {
        if self.forward_of_forward {
            self.saved_from == Some(self_id) || self.saved_out
        } else {
            self.from == Some(self_id)
        }
    }
}

fn parse<T: Deserializable>(raw: &Option<(i32, Vec<u8>)>) -> Option<T> {
    let (layer, bytes) = raw.as_ref()?;
    (*layer == tdr_tl::LAYER)
        .then(|| T::from_bytes(&tdr_tl::Bytes::copy_from_slice(bytes)).ok())
        .flatten()
}

/// The cached user object of a peer row.
pub fn user(row: &PeerRow) -> Option<enums::User> {
    parse(&row.raw)
}

pub fn message_info(row: &MessageRow) -> MessageInfo {
    match parse::<enums::Message>(&row.raw) {
        Some(enums::Message::Message(m)) => MessageInfo {
            media: m.media.as_ref().and_then(media),
            service: None,
            edited: m.edit_date.is_some() && !m.edit_hide,
            edit_date: m.edit_date.filter(|_| !m.edit_hide),
            grouped_id: m.grouped_id,
            post: m.post,
            views: m.views,
            post_author: m.post_author.clone(),
            comments: m.replies.as_ref().and_then(|r| {
                let enums::MessageReplies::MessageReplies(r) = r;
                r.comments.then_some(r.replies)
            }),
            recent_repliers: m
                .replies
                .as_ref()
                .map(|r| {
                    let enums::MessageReplies::MessageReplies(r) = r;
                    r.recent_repliers
                        .iter()
                        .flatten()
                        .map(crate::convert::peer_key)
                        .collect()
                })
                .unwrap_or_default(),
            comments_channel: m.replies.as_ref().and_then(|r| {
                let enums::MessageReplies::MessageReplies(r) = r;
                r.channel_id.filter(|_| r.comments)
            }),
            reply: m.reply_to.as_ref().and_then(reply_to),
            forward: m.fwd_from.as_ref().map(|f| {
                let enums::MessageFwdHeader::MessageFwdHeader(f) = f;
                Forward {
                    from: f.from_id.as_ref().map(crate::convert::peer_key),
                    forward_of_forward: f.saved_from_id.is_some() || f.saved_from_name.is_some(),
                    saved_from: f.saved_from_id.as_ref().map(crate::convert::peer_key),
                    saved_out: f.saved_out,
                    from_name: f.from_name.clone(),
                    post_author: f.post_author.clone(),
                    saved_from_name: f.saved_from_name.clone(),
                    original: f
                        .saved_from_peer
                        .as_ref()
                        .zip(f.saved_from_msg_id)
                        .map(|(p, id)| (crate::convert::peer_key(p), id)),
                    date: f.date,
                }
            }),
            entities: m.entities.as_deref().map(entities).unwrap_or_default(),
        },
        Some(enums::Message::Service(m)) => MessageInfo {
            service: Some(service(&m.action)),
            ..Default::default()
        },
        _ => MessageInfo::default(),
    }
}

fn reply_to(h: &enums::MessageReplyHeader) -> Option<ReplyTo> {
    match h {
        enums::MessageReplyHeader::MessageReplyHeader(h) => Some(ReplyTo {
            msg_id: h.reply_to_msg_id,
            peer: h.reply_to_peer_id.as_ref().map(crate::convert::peer_key),
            top_id: h.reply_to_top_id,
            forum_topic: h.forum_topic,
            quote: h.quote_text.clone().filter(|_| h.quote),
            from: h.reply_from.as_ref().and_then(|f| {
                let enums::MessageFwdHeader::MessageFwdHeader(f) = f;
                f.from_id.as_ref().map(crate::convert::peer_key)
            }),
            from_name: h.reply_from.as_ref().and_then(|f| {
                let enums::MessageFwdHeader::MessageFwdHeader(f) = f;
                f.from_name.clone()
            }),
        }),
        _ => None,
    }
}

fn media(m: &enums::MessageMedia) -> Option<Media> {
    use enums::MessageMedia as M;
    Some(match m {
        M::Empty | M::WebPage(_) => return None,
        M::Photo(p) => {
            // An expired self-destructing photo has none.
            p.photo.as_ref()?;
            Media::Photo
        }
        M::Document(d) => {
            let Some(enums::Document::Document(doc)) = &d.document else {
                return None;
            };
            document(doc, d.voice, d.round, d.video)
        }
        M::Geo(_) => Media::Location,
        M::GeoLive(_) => Media::LiveLocation,
        M::Venue(v) => Media::Venue(v.title.clone()),
        M::Contact(_) => Media::Contact,
        M::Poll(p) => {
            let enums::Poll::Poll(poll) = &p.poll;
            let enums::TextWithEntities::TextWithEntities(q) = &poll.question;
            Media::Poll(q.text.clone())
        }
        M::Dice(d) => Media::Dice(d.emoticon.clone()),
        M::Game(g) => {
            let enums::Game::Game(game) = &g.game;
            Media::Game(game.title.clone())
        }
        M::Invoice(i) => Media::Invoice(i.title.clone()),
        M::Story(_) => Media::Story,
        M::Giveaway(_) | M::GiveawayResults(_) => Media::Giveaway,
        M::ToDo(_) => Media::Checklist,
        _ => Media::Other,
    })
}

fn document(doc: &tdr_tl::types::Document, voice: bool, round: bool, video: bool) -> Media {
    use enums::DocumentAttribute as A;
    let mut file_name = None;
    let mut audio = None;
    let mut sticker = None;
    let mut animated = false;
    let mut is_video = video;
    let mut is_round = round;
    let mut is_voice = voice;
    for attribute in &doc.attributes {
        match attribute {
            A::Filename(f) => file_name = Some(f.file_name.clone()),
            A::Audio(a) => {
                is_voice |= a.voice;
                let title = match (&a.performer, &a.title) {
                    (Some(p), Some(t)) if !p.is_empty() && !t.is_empty() => format!("{p} – {t}"),
                    (_, Some(t)) if !t.is_empty() => t.clone(),
                    _ => String::new(),
                };
                audio = Some(title);
            }
            A::Sticker(s) => sticker = Some(s.alt.clone()),
            A::Animated => animated = true,
            A::Video(v) => {
                is_video = true;
                is_round |= v.round_message;
            }
            _ => {}
        }
    }
    if is_voice {
        Media::VoiceMessage
    } else if is_round {
        Media::VideoMessage
    } else if let Some(emoji) = sticker {
        Media::Sticker(emoji)
    } else if animated {
        Media::Gif
    } else if is_video {
        Media::Video
    } else if let Some(title) = audio {
        Media::Audio(if title.is_empty() {
            file_name.unwrap_or_default()
        } else {
            title
        })
    } else {
        Media::File(file_name.unwrap_or_default())
    }
}

fn service(action: &enums::MessageAction) -> Service {
    use enums::MessageAction as A;
    match action {
        A::ChatCreate(a) => Service::ChatCreate {
            title: a.title.clone(),
        },
        A::ChannelCreate(_) => Service::ChannelCreate,
        A::ChatEditTitle(a) => Service::EditTitle {
            title: a.title.clone(),
        },
        A::ChatEditPhoto(_) => Service::EditPhoto,
        A::ChatDeletePhoto => Service::DeletePhoto,
        A::ChatAddUser(a) => Service::AddUsers {
            users: a.users.clone(),
        },
        A::ChatDeleteUser(a) => Service::DeleteUser { user: a.user_id },
        A::ChatJoinedByLink(_) => Service::JoinedByLink,
        A::ChatJoinedByRequest => Service::JoinedByRequest,
        A::ChatMigrateTo(_) | A::ChannelMigrateFrom(_) => Service::Migrated,
        A::PinMessage => Service::PinMessage,
        A::HistoryClear => Service::HistoryClear,
        A::PhoneCall(a) => Service::Call {
            video: a.video,
            missed: matches!(
                a.reason,
                Some(enums::PhoneCallDiscardReason::Missed | enums::PhoneCallDiscardReason::Busy)
            ),
            duration: a.duration,
        },
        A::GroupCall(a) => Service::GroupCall {
            ended: a.duration.is_some(),
        },
        A::ScreenshotTaken => Service::ScreenshotTaken,
        A::ContactSignUp => Service::ContactSignUp,
        _ => Service::Other,
    }
}

/// What kind of chat a peer is (`ChatTypeIcon`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerKind {
    #[default]
    User,
    Bot,
    Group,
    Channel,
}

/// A user's last-seen state (`Data::LastseenStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lastseen {
    Online { until: i32 },
    Offline { was: i32 },
    Recently,
    WithinWeek,
    WithinMonth,
    LongAgo,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerInfo {
    pub kind: PeerKind,
    /// The color index the peer chose (`peerColor.color`, 0..=6 are the basic ones).
    pub color: Option<u8>,
    /// Members of a group or subscribers of a channel, when the object carries it.
    pub members: Option<i32>,
    pub lastseen: Option<Lastseen>,
    /// Small userpic: photo id and the DC that stores it.
    pub photo: Option<(i64, i32)>,
    pub verified: bool,
    /// An emoji status is set (drawn instead of the Premium star).
    pub emoji_status: bool,
    /// A group or channel we are not in (left, kicked, never joined): tdesktop
    /// keeps it out of the chat list (`amIn`).
    pub left: bool,
    /// A Telegram Premium user.
    pub premium: bool,
    pub first_name: Option<String>,
    /// A user in the contacts.
    pub contact: bool,
    /// A user's phone number (digits), when visible.
    pub phone: Option<String>,
    /// A basic group upgraded to a supergroup (`migrated_to`): tdesktop leaves it out
    /// of the chat list (`History::shouldBeInChatList`), the supergroup goes on.
    pub migrated: bool,
    /// A channel we may post to (`ChannelData::canPostMessages`: its creator or an
    /// admin with `post_messages`); groups and users are always writable here.
    pub can_post: bool,
    /// A channel taking direct messages (`broadcast_messages_allowed` with a linked
    /// monoforum): the "Direct Messages" button under its feed.
    pub direct_messages: bool,
    /// We made the group or channel (`amCreator`), or are its admin.
    pub creator: bool,
    pub admin: bool,
    /// `canDeleteMessages`: the creator, or an admin with `delete_messages`.
    pub delete_messages: bool,
    /// It has a username (`isPublic`).
    pub public: bool,
    /// Its content can't be forwarded or saved (`noforwards`).
    pub noforwards: bool,
    /// Joining it needs an admin's approval (`join_request`).
    pub join_request: bool,
    /// A group with topics (`forum`).
    pub forum: bool,
}

fn color(c: &Option<enums::PeerColor>) -> Option<u8> {
    match c {
        Some(enums::PeerColor::PeerColor(c)) => c.color.and_then(|v| u8::try_from(v).ok()),
        _ => None,
    }
}

pub fn peer_info(row: &PeerRow) -> PeerInfo {
    use tdr_storage::PeerKind as K;
    match row.key.kind {
        K::User => match parse::<enums::User>(&row.raw) {
            Some(enums::User::User(u)) => PeerInfo {
                kind: if u.bot { PeerKind::Bot } else { PeerKind::User },
                color: color(&u.color),
                members: None,
                lastseen: u.status.as_ref().map(|s| match s {
                    enums::UserStatus::Online(o) => Lastseen::Online { until: o.expires },
                    enums::UserStatus::Offline(o) => Lastseen::Offline { was: o.was_online },
                    enums::UserStatus::Recently(_) => Lastseen::Recently,
                    enums::UserStatus::LastWeek(_) => Lastseen::WithinWeek,
                    enums::UserStatus::LastMonth(_) => Lastseen::WithinMonth,
                    enums::UserStatus::Empty => Lastseen::LongAgo,
                }),
                photo: match &u.photo {
                    Some(enums::UserProfilePhoto::UserProfilePhoto(p)) => {
                        Some((p.photo_id, p.dc_id))
                    }
                    _ => None,
                },
                verified: u.verified,
                emoji_status: u
                    .emoji_status
                    .as_ref()
                    .is_some_and(|s| !matches!(s, enums::EmojiStatus::Empty)),
                premium: u.premium,
                first_name: u.first_name.clone(),
                contact: u.contact,
                phone: u.phone.clone(),
                left: false,
                migrated: false,
                can_post: true,
                direct_messages: false,
                creator: false,
                admin: false,
                delete_messages: false,
                public: u.username.is_some() || u.usernames.as_ref().is_some_and(|l| !l.is_empty()),
                noforwards: false,
                join_request: false,
                forum: false,
            },
            _ => PeerInfo::default(),
        },
        K::Chat | K::Channel => match parse::<enums::Chat>(&row.raw) {
            Some(enums::Chat::Chat(c)) => PeerInfo {
                kind: PeerKind::Group,
                left: c.left || c.deactivated,
                members: Some(c.participants_count),
                photo: chat_photo(&c.photo),
                migrated: c.migrated_to.is_some(),
                can_post: true,
                creator: c.creator,
                admin: c.creator || c.admin_rights.is_some(),
                delete_messages: c.creator
                    || c.admin_rights.as_ref().is_some_and(|r| {
                        let enums::ChatAdminRights::ChatAdminRights(r) = r;
                        r.delete_messages
                    }),
                noforwards: c.noforwards,
                ..Default::default()
            },
            Some(enums::Chat::Channel(c)) => PeerInfo {
                kind: if c.broadcast {
                    PeerKind::Channel
                } else {
                    PeerKind::Group
                },
                color: color(&c.color),
                left: c.left,
                can_post: !c.broadcast
                    || c.creator
                    || c.admin_rights.as_ref().is_some_and(|r| {
                        let enums::ChatAdminRights::ChatAdminRights(r) = r;
                        r.post_messages
                    }),
                direct_messages: c.broadcast
                    && c.broadcast_messages_allowed
                    && c.linked_monoforum_id.is_some(),
                creator: c.creator,
                admin: c.creator || c.admin_rights.is_some(),
                delete_messages: c.creator
                    || c.admin_rights.as_ref().is_some_and(|r| {
                        let enums::ChatAdminRights::ChatAdminRights(r) = r;
                        r.delete_messages
                    }),
                public: c.username.is_some() || c.usernames.as_ref().is_some_and(|u| !u.is_empty()),
                noforwards: c.noforwards,
                join_request: c.join_request,
                forum: c.forum,
                members: c.participants_count,
                photo: chat_photo(&c.photo),
                verified: c.verified,
                emoji_status: c
                    .emoji_status
                    .as_ref()
                    .is_some_and(|s| !matches!(s, enums::EmojiStatus::Empty)),
                ..Default::default()
            },
            // `chatForbidden`, `channelForbidden`: not ours any more.
            Some(_) => PeerInfo {
                kind: PeerKind::Group,
                left: true,
                ..Default::default()
            },
            None => PeerInfo {
                kind: PeerKind::Group,
                ..Default::default()
            },
        },
    }
}

/// What the full info of a group or channel says (`channelFull`), as far as the
/// window uses it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullInfo {
    pub members: Option<i32>,
    /// `stargiftsAvailable`: the gift button under a channel.
    pub stargifts: bool,
    /// `paid_messages_available`.
    pub paid_messages: bool,
    /// The discussion group of a channel (or the channel of a group).
    pub linked_chat: Option<i64>,
    pub pinned_msg_id: Option<i32>,
    pub online: Option<i32>,
    /// `canViewMembers`.
    pub can_view_participants: bool,
}

/// The cached full info (`Storage::peer_full`).
pub fn full_info(raw: &Option<(i32, Vec<u8>)>) -> Option<FullInfo> {
    match parse::<enums::ChatFull>(raw)? {
        enums::ChatFull::ChannelFull(c) => Some(FullInfo {
            members: c.participants_count,
            stargifts: c.stargifts_available,
            paid_messages: c.paid_messages_available,
            linked_chat: c.linked_chat_id,
            pinned_msg_id: c.pinned_msg_id,
            online: c.online_count,
            can_view_participants: c.can_view_participants,
        }),
        enums::ChatFull::ChatFull(c) => Some(FullInfo {
            pinned_msg_id: c.pinned_msg_id,
            ..Default::default()
        }),
        enums::ChatFull::CommunityFull(_) => Some(FullInfo::default()),
    }
}

/// The peer's username from its cached object (rows cached before usernames lists
/// were read keep only the plain `username` in their column).
pub fn username(row: &PeerRow) -> Option<String> {
    use tdr_storage::PeerKind as K;
    match row.key.kind {
        K::User => match parse::<enums::User>(&row.raw) {
            Some(enums::User::User(u)) => {
                crate::convert::primary_username(u.username.as_ref(), u.usernames.as_deref())
            }
            _ => None,
        },
        K::Channel => match parse::<enums::Chat>(&row.raw) {
            Some(enums::Chat::Channel(c)) => {
                crate::convert::primary_username(c.username.as_ref(), c.usernames.as_deref())
            }
            _ => None,
        },
        K::Chat => None,
    }
    .or_else(|| row.username.clone().filter(|u| !u.is_empty()))
}

fn chat_photo(p: &enums::ChatPhoto) -> Option<(i64, i32)> {
    match p {
        enums::ChatPhoto::ChatPhoto(p) => Some((p.photo_id, p.dc_id)),
        enums::ChatPhoto::Empty => None,
    }
}

/// `SpoilerLoginCode`'s pattern: the first code of digits and dashes in `text`,
/// `min_length` to 8 characters long, starting and ending with a digit, not inside a
/// word (`(?<![\w\-#])(\d[\d\-]{min-2,6}\d)(?!\w\-)`). Its byte range.
pub fn login_code(text: &str, min_length: usize) -> Option<std::ops::Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let inner_min = min_length.saturating_sub(2);
    for start in 0..chars.len() {
        let (_, first) = chars[start];
        if !first.is_ascii_digit() {
            continue;
        }
        if start > 0 {
            let prev = chars[start - 1].1;
            if word(prev) || prev == '-' || prev == '#' {
                continue;
            }
        }
        // Greedy: the longest run of digits and dashes first.
        let mut run = 0;
        while run < 6
            && start + 1 + run < chars.len()
            && (chars[start + 1 + run].1.is_ascii_digit() || chars[start + 1 + run].1 == '-')
        {
            run += 1;
        }
        for inner in (inner_min..=run).rev() {
            let last = start + 1 + inner;
            if last >= chars.len() || !chars[last].1.is_ascii_digit() {
                continue;
            }
            // `(?!\w\-)`: not followed by a word character and a dash.
            let after = (chars.get(last + 1), chars.get(last + 2));
            if let (Some((_, a)), Some((_, '-'))) = after
                && word(*a)
            {
                continue;
            }
            let end = chars.get(last + 1).map_or(text.len(), |(i, _)| *i);
            return Some(chars[start].0..end);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn saved_messages_layout_like_tdesktop() {
        let me = PeerKey::user(1);
        let other = PeerKey::user(2);
        let fwd = |from, fof, saved_from, saved_out| Forward {
            from,
            forward_of_forward: fof,
            saved_from,
            saved_out,
            ..Default::default()
        };
        // Forwarded from someone else: incoming; from yourself: outgoing.
        assert!(!fwd(Some(other), false, None, false).outgoing_in_saved(me));
        assert!(fwd(Some(me), false, None, false).outgoing_in_saved(me));
        // A forward of a forward: by who it was saved from, or its outgoing flag.
        assert!(fwd(Some(other), true, Some(me), false).outgoing_in_saved(me));
        assert!(fwd(Some(other), true, Some(other), true).outgoing_in_saved(me));
        assert!(!fwd(Some(me), true, Some(other), false).outgoing_in_saved(me));
    }

    use super::*;
    use tdr_storage::PeerKey;
    use tdr_tl::{Serializable, types};

    #[test]
    fn entity_offsets_are_utf16() {
        // "😀" is two UTF-16 units and four bytes; "ж" one unit and two bytes.
        let text = "😀 жbold";
        let e = |offset, length| Entity {
            offset,
            length,
            kind: EntityKind::Bold,
        };
        assert_eq!(e(4, 4).byte_range(text), Some(7..11));
        assert_eq!(e(0, 2).byte_range(text), Some(0..4));
        assert_eq!(e(3, 1).byte_range(text), Some(5..7));
        assert_eq!(e(4, 10).byte_range(text), None);
        assert_eq!(e(2, 0).byte_range(text), None);
    }

    fn row_with(message: enums::Message) -> MessageRow {
        MessageRow {
            peer: PeerKey::user(1),
            id: 1,
            date: 0,
            from: None,
            out: false,
            text: String::new(),
            raw: Some((tdr_tl::LAYER, message.to_bytes())),
        }
    }

    fn voice_document() -> enums::Document {
        types::Document {
            id: 1,
            access_hash: 0,
            file_reference: Default::default(),
            date: 0,
            mime_type: "audio/ogg".into(),
            size: 10,
            thumbs: None,
            video_thumbs: None,
            dc_id: 2,
            attributes: vec![
                types::DocumentAttributeAudio {
                    voice: true,
                    duration: 3,
                    title: None,
                    performer: None,
                    waveform: None,
                }
                .into(),
            ],
        }
        .into()
    }

    #[test]
    fn service_and_media_from_raw() {
        let service = types::MessageService {
            out: false,
            mentioned: false,
            media_unread: false,
            reactions_are_possible: false,
            silent: false,
            post: false,
            legacy: false,
            id: 5,
            from_id: None,
            peer_id: types::PeerChat { chat_id: 3 }.into(),
            saved_peer_id: None,
            reply_to: None,
            date: 0,
            action: types::MessageActionChatAddUser { users: vec![7, 8] }.into(),
            reactions: None,
            ttl_period: None,
        };
        let info = message_info(&row_with(service.into()));
        assert_eq!(info.service, Some(Service::AddUsers { users: vec![7, 8] }));

        let media = types::MessageMediaDocument {
            nopremium: false,
            spoiler: false,
            video: false,
            round: false,
            voice: true,
            document: Some(voice_document()),
            alt_documents: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        };
        assert_eq!(super::media(&media.into()), Some(Media::VoiceMessage));
        // Rows from another layer are not parsed.
        let mut stale = row_with(service_into_message());
        stale.raw.as_mut().unwrap().0 -= 1;
        assert_eq!(message_info(&stale), MessageInfo::default());
    }

    fn service_into_message() -> enums::Message {
        types::MessageService {
            out: false,
            mentioned: false,
            media_unread: false,
            reactions_are_possible: false,
            silent: false,
            post: false,
            legacy: false,
            id: 6,
            from_id: None,
            peer_id: types::PeerChat { chat_id: 3 }.into(),
            saved_peer_id: None,
            reply_to: None,
            date: 0,
            action: enums::MessageAction::PinMessage,
            reactions: None,
            ttl_period: None,
        }
        .into()
    }

    #[test]
    fn login_codes_like_tdesktop() {
        let code = |t: &str| login_code(t, 5).map(|r| t[r].to_string());
        assert_eq!(
            code("Login code: 12345. Do not give it"),
            Some("12345".into())
        );
        assert_eq!(code("Код: 52-118"), Some("52-118".into()));
        // Too short, inside words, after a hash.
        assert_eq!(code("Code 1234"), None);
        assert_eq!(code("id a12345"), None);
        assert_eq!(code("#12345"), None);
        // A longer run is cut at eight characters ending with a digit.
        assert_eq!(code("123456789012"), Some("12345678".into()));
        assert_eq!(code("7xeY_4JeM-k"), None);
    }
}
