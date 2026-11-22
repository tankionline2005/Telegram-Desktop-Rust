//! Telegram objects → cache rows.

use tdr_storage::{MessageRow, PeerKey, PeerRow};
use tdr_tl::{Serializable, enums, types};

pub(crate) fn raw(object: &impl Serializable) -> Option<(i32, Vec<u8>)> {
    Some((tdr_tl::LAYER, object.to_bytes()))
}

pub fn peer_key(peer: &enums::Peer) -> PeerKey {
    match peer {
        enums::Peer::User(p) => PeerKey::user(p.user_id),
        enums::Peer::Chat(p) => PeerKey::chat(p.chat_id),
        enums::Peer::Channel(p) => PeerKey::channel(p.channel_id),
    }
}

/// `peerNotifySettings.mute_until` (`None`: the default of the chat type applies).
pub fn mute_until(settings: &enums::PeerNotifySettings) -> Option<i32> {
    let enums::PeerNotifySettings::PeerNotifySettings(s) = settings;
    s.mute_until
}

pub fn cloud_draft(draft: &enums::DraftMessage) -> crate::drafts::CloudDraft {
    use crate::drafts::CloudDraft;
    match draft {
        enums::DraftMessage::Empty(d) => CloudDraft::Empty {
            date: d.date.unwrap_or(0),
        },
        enums::DraftMessage::DraftMessage(d) => CloudDraft::Text {
            text: d.message.clone(),
            date: d.date,
        },
    }
}

pub fn user_row(user: &enums::User) -> Option<PeerRow> {
    let enums::User::User(u) = user else {
        return None;
    };
    let mut name = u.first_name.clone().unwrap_or_default();
    if let Some(last) = u.last_name.as_deref().filter(|l| !l.is_empty()) {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(last);
    }
    if name.is_empty() && u.deleted {
        name = "Deleted Account".into();
    }
    Some(PeerRow {
        key: PeerKey::user(u.id),
        access_hash: u.access_hash,
        is_min: u.min,
        name,
        username: primary_username(u.username.as_ref(), u.usernames.as_deref()),
        is_megagroup: false,
        raw: raw(user),
    })
}

/// The username shown and linked (`UsernamesInfo::username`): with a list of
/// usernames (collectible ones, several of a bot or a channel) the first active one,
/// else the plain `username`.
pub fn primary_username(
    username: Option<&String>,
    usernames: Option<&[enums::Username]>,
) -> Option<String> {
    match usernames {
        Some(list) if !list.is_empty() => list.iter().find_map(|u| {
            let enums::Username::Username(u) = u;
            u.active.then(|| u.username.clone())
        }),
        _ => username.cloned(),
    }
    .filter(|u| !u.is_empty())
}

pub fn chat_row(chat: &enums::Chat) -> Option<PeerRow> {
    let row =
        |key, access_hash, is_min, name: &str, username: Option<String>, is_megagroup| PeerRow {
            key,
            access_hash,
            is_min,
            name: name.to_string(),
            username,
            is_megagroup,
            raw: raw(chat),
        };
    Some(match chat {
        enums::Chat::Chat(c) => row(PeerKey::chat(c.id), None, false, &c.title, None, false),
        enums::Chat::Forbidden(c) => row(PeerKey::chat(c.id), None, false, &c.title, None, false),
        enums::Chat::Channel(c) => row(
            PeerKey::channel(c.id),
            c.access_hash,
            c.min,
            &c.title,
            primary_username(c.username.as_ref(), c.usernames.as_deref()),
            c.megagroup,
        ),
        enums::Chat::ChannelForbidden(c) => row(
            PeerKey::channel(c.id),
            Some(c.access_hash),
            false,
            &c.title,
            None,
            c.megagroup,
        ),
        // Communities (layer 233) are not shown yet.
        enums::Chat::Empty(_) | enums::Chat::Community(_) | enums::Chat::CommunityForbidden(_) => {
            return None;
        }
    })
}

/// Text to show for a message in lists: its text, or a marker for media and service
/// messages (proper rendering comes with the UI).
/// An incoming message that mentions us, not read yet (`mentioned` and
/// `media_unread`).
pub fn unread_mention(row: &MessageRow) -> bool {
    use tdr_tl::Deserializable;
    let Some((layer, bytes)) = &row.raw else {
        return false;
    };
    if row.out || *layer != tdr_tl::LAYER {
        return false;
    }
    matches!(
        enums::Message::from_bytes(&tdr_tl::Bytes::copy_from_slice(bytes)),
        Ok(enums::Message::Message(m)) if m.mentioned && m.media_unread
    )
}

/// The "Telegram" account as `Session::serviceNotification` makes it when it is not
/// loaded yet: "Telegram", phone 42777, verified, seen recently.
pub fn service_user(id: i64) -> enums::User {
    enums::User::User(Box::new(types::User {
        self_: false,
        contact: false,
        mutual_contact: false,
        deleted: false,
        bot: false,
        bot_chat_history: false,
        bot_nochats: false,
        verified: true,
        restricted: false,
        min: false,
        bot_inline_geo: false,
        support: false,
        scam: false,
        apply_min_photo: false,
        fake: false,
        bot_attach_menu: false,
        premium: false,
        attach_menu_enabled: false,
        bot_can_edit: false,
        close_friend: false,
        stories_hidden: false,
        stories_unavailable: false,
        contact_require_premium: false,
        bot_business: false,
        bot_has_main_app: false,
        bot_forum_view: false,
        bot_forum_can_manage_topics: false,
        bot_can_manage_bots: false,
        bot_guestchat: false,
        bot_guard: false,
        bot_forum_sidebar: false,
        id,
        access_hash: None,
        first_name: Some("Telegram".into()),
        last_name: None,
        username: None,
        phone: Some("42777".into()),
        photo: Some(enums::UserProfilePhoto::Empty),
        status: Some(enums::UserStatus::Recently(types::UserStatusRecently {
            by_me: false,
        })),
        bot_info_version: None,
        restriction_reason: None,
        bot_inline_placeholder: None,
        lang_code: None,
        emoji_status: None,
        usernames: None,
        stories_max_id: None,
        color: None,
        profile_color: None,
        bot_active_users: None,
        bot_verification_icon: None,
        send_paid_messages_stars: None,
        linked_community_id: None,
    }))
}

/// A service notification as the message `insertCheckedServiceNotification` adds:
/// from and to "Telegram", with its entities and media, unread.
pub fn service_message(
    peer_id: i64,
    id: i32,
    date: i32,
    n: &types::UpdateServiceNotification,
) -> Option<MessageRow> {
    let peer = enums::Peer::User(types::PeerUser { user_id: peer_id });
    let message = enums::Message::Message(Box::new(types::Message {
        out: false,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: n.invert_media,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id,
        from_id: Some(peer.clone()),
        from_boosts_applied: None,
        from_rank: None,
        peer_id: peer,
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: None,
        date,
        message: n.message.clone(),
        media: Some(n.media.clone()),
        reply_markup: None,
        entities: Some(n.entities.clone()),
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    }));
    message_row(&message)
}

pub fn message_row(message: &enums::Message) -> Option<MessageRow> {
    match message {
        enums::Message::Message(m) => {
            let text = m.message.clone();
            Some(MessageRow {
                peer: peer_key(&m.peer_id),
                id: m.id,
                date: m.date,
                from: m.from_id.as_ref().map(peer_key),
                out: m.out,
                text,
                raw: raw(message),
            })
        }
        enums::Message::Service(m) => Some(MessageRow {
            peer: peer_key(&m.peer_id),
            id: m.id,
            date: m.date,
            from: m.from_id.as_ref().map(peer_key),
            out: m.out,
            text: String::new(),
            raw: raw(message),
        }),
        enums::Message::Empty(_) => None,
    }
}

pub fn short_message_row(m: &types::UpdateShortMessage, self_id: i64) -> MessageRow {
    MessageRow {
        peer: PeerKey::user(m.user_id),
        id: m.id,
        date: m.date,
        from: Some(PeerKey::user(if m.out { self_id } else { m.user_id })),
        out: m.out,
        text: m.message.clone(),
        raw: None,
    }
}

pub fn short_chat_message_row(m: &types::UpdateShortChatMessage) -> MessageRow {
    MessageRow {
        peer: PeerKey::chat(m.chat_id),
        id: m.id,
        date: m.date,
        from: Some(PeerKey::user(m.from_id)),
        out: m.out,
        text: m.message.clone(),
        raw: None,
    }
}

/// `InputPeer` for a cached peer (needs its access hash for users and channels).
pub fn peer_to_tl(key: PeerKey) -> enums::Peer {
    use tdr_storage::PeerKind;
    match key.kind {
        PeerKind::User => types::PeerUser { user_id: key.id }.into(),
        PeerKind::Chat => types::PeerChat { chat_id: key.id }.into(),
        PeerKind::Channel => types::PeerChannel { channel_id: key.id }.into(),
    }
}

pub fn input_peer(key: PeerKey, access_hash: Option<i64>) -> Option<enums::InputPeer> {
    use tdr_storage::PeerKind;
    Some(match key.kind {
        PeerKind::User => types::InputPeerUser {
            user_id: key.id,
            access_hash: access_hash?,
        }
        .into(),
        PeerKind::Chat => types::InputPeerChat { chat_id: key.id }.into(),
        PeerKind::Channel => types::InputPeerChannel {
            channel_id: key.id,
            access_hash: access_hash?,
        }
        .into(),
    })
}
