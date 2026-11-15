//! Telegram objects → cache rows.

use tdr_storage::{MessageRow, PeerKey, PeerRow};
use tdr_tl::{Serializable, enums, types};

fn raw(object: &impl Serializable) -> Option<(i32, Vec<u8>)> {
    Some((tdr_tl::LAYER, object.to_bytes()))
}

pub fn peer_key(peer: &enums::Peer) -> PeerKey {
    match peer {
        enums::Peer::User(p) => PeerKey::user(p.user_id),
        enums::Peer::Chat(p) => PeerKey::chat(p.chat_id),
        enums::Peer::Channel(p) => PeerKey::channel(p.channel_id),
    }
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
        username: u.username.clone(),
        is_megagroup: false,
        raw: raw(user),
    })
}

pub fn chat_row(chat: &enums::Chat) -> Option<PeerRow> {
    let row =
        |key, access_hash, is_min, name: &str, username: Option<&String>, is_megagroup| PeerRow {
            key,
            access_hash,
            is_min,
            name: name.to_string(),
            username: username.cloned(),
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
            c.username.as_ref(),
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
pub fn message_row(message: &enums::Message) -> Option<MessageRow> {
    match message {
        enums::Message::Message(m) => {
            let text = if m.message.is_empty() && m.media.is_some() {
                "[media]".to_string()
            } else {
                m.message.clone()
            };
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
            text: "[service message]".into(),
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
