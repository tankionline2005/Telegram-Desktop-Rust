//! Chat folders (`dialogFilter`), ported from tdesktop's `Data::ChatFilter` and
//! `Ui::ComputeFilterIcon`.

use tdr_storage::PeerKey;
use tdr_tl::enums;

/// One entry of the folder list: "All chats" stands where the server says
/// (`dialogFilterDefault`), the others are folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterEntry {
    All,
    Folder(ChatFilter),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatFilter {
    pub id: i32,
    /// The title's text (custom emoji in it are shown as their text).
    pub title: String,
    pub emoticon: Option<String>,
    /// Shown first, in this order.
    pub pinned: Vec<PeerKey>,
    pub include: Vec<PeerKey>,
    pub exclude: Vec<PeerKey>,
    pub contacts: bool,
    pub non_contacts: bool,
    pub groups: bool,
    pub channels: bool,
    pub bots: bool,
    pub no_muted: bool,
    pub no_read: bool,
    pub no_archived: bool,
}

/// What a folder needs to know about a chat.
#[derive(Debug, Clone, Copy)]
pub struct ChatTraits {
    pub peer: PeerKey,
    pub kind: crate::describe::PeerKind,
    pub contact: bool,
    pub muted: bool,
    pub unread: bool,
    pub archived: bool,
}

impl Default for ChatTraits {
    fn default() -> Self {
        Self {
            peer: PeerKey::user(0),
            kind: Default::default(),
            contact: false,
            muted: false,
            unread: false,
            archived: false,
        }
    }
}

/// tdesktop's folder icons (`Ui::FilterIcon`), in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterIcon {
    Cat,
    Book,
    Money,
    Game,
    Light,
    Like,
    Note,
    Palette,
    Travel,
    Sport,
    Favorite,
    Study,
    Airplane,
    Private,
    Groups,
    All,
    Unread,
    Bots,
    Crown,
    Flower,
    Home,
    Love,
    Mask,
    Party,
    Trade,
    Work,
    Unmuted,
    Channels,
    Custom,
    Setup,
    /// The "Edit" button under the folders.
    Edit,
}

/// `kIcons`: the emoji that pick an icon (variation selectors ignored).
const EMOJI_ICONS: &[(&str, FilterIcon)] = &[
    ("\u{1F431}", FilterIcon::Cat),
    ("\u{1F4D5}", FilterIcon::Book),
    ("\u{1F4B0}", FilterIcon::Money),
    ("\u{1F3AE}", FilterIcon::Game),
    ("\u{1F4A1}", FilterIcon::Light),
    ("\u{1F44C}", FilterIcon::Like),
    ("\u{1F3B5}", FilterIcon::Note),
    ("\u{1F3A8}", FilterIcon::Palette),
    ("\u{2708}", FilterIcon::Travel),
    ("\u{26BD}", FilterIcon::Sport),
    ("\u{2B50}", FilterIcon::Favorite),
    ("\u{1F393}", FilterIcon::Study),
    ("\u{1F6EB}", FilterIcon::Airplane),
    ("\u{1F464}", FilterIcon::Private),
    ("\u{1F465}", FilterIcon::Groups),
    ("\u{1F4AC}", FilterIcon::All),
    ("\u{2705}", FilterIcon::Unread),
    ("\u{1F916}", FilterIcon::Bots),
    ("\u{1F451}", FilterIcon::Crown),
    ("\u{1F339}", FilterIcon::Flower),
    ("\u{1F3E0}", FilterIcon::Home),
    ("\u{2764}", FilterIcon::Love),
    ("\u{1F3AD}", FilterIcon::Mask),
    ("\u{1F378}", FilterIcon::Party),
    ("\u{1F4C8}", FilterIcon::Trade),
    ("\u{1F4BC}", FilterIcon::Work),
    ("\u{1F514}", FilterIcon::Unmuted),
    ("\u{1F4E2}", FilterIcon::Channels),
    ("\u{1F4C1}", FilterIcon::Custom),
    ("\u{1F4CB}", FilterIcon::Setup),
];

impl ChatFilter {
    pub fn from_tl(filter: &enums::DialogFilter) -> Option<Self> {
        let peers = |list: &[enums::InputPeer]| -> Vec<PeerKey> {
            list.iter().filter_map(input_peer_key).collect()
        };
        let title = |t: &enums::TextWithEntities| match t {
            enums::TextWithEntities::TextWithEntities(t) => t.text.clone(),
        };
        match filter {
            enums::DialogFilter::DialogFilter(f) => Some(Self {
                id: f.id,
                title: title(&f.title),
                emoticon: f.emoticon.clone(),
                pinned: peers(&f.pinned_peers),
                include: peers(&f.include_peers),
                exclude: peers(&f.exclude_peers),
                contacts: f.contacts,
                non_contacts: f.non_contacts,
                groups: f.groups,
                channels: f.broadcasts,
                bots: f.bots,
                no_muted: f.exclude_muted,
                no_read: f.exclude_read,
                no_archived: f.exclude_archived,
            }),
            // A shared folder: its chats only.
            enums::DialogFilter::Chatlist(f) => Some(Self {
                id: f.id,
                title: title(&f.title),
                emoticon: f.emoticon.clone(),
                pinned: peers(&f.pinned_peers),
                include: peers(&f.include_peers),
                ..Default::default()
            }),
            enums::DialogFilter::Default => None,
        }
    }

    /// `ChatFilter::contains`.
    pub fn contains(&self, chat: &ChatTraits) -> bool {
        use crate::describe::PeerKind as K;
        if self.exclude.contains(&chat.peer) {
            return false;
        }
        if self.include.contains(&chat.peer) || self.pinned.contains(&chat.peer) {
            return true;
        }
        let by_type = match chat.kind {
            K::Bot => self.bots,
            K::User if chat.contact => self.contacts,
            K::User => self.non_contacts,
            K::Group => self.groups,
            K::Channel => self.channels,
        };
        by_type
            && !(self.no_muted && chat.muted)
            && !(self.no_read && !chat.unread)
            && !(self.no_archived && chat.archived)
    }

    /// `Ui::ComputeFilterIcon`: by the emoji, else by what the folder holds.
    pub fn icon(&self) -> FilterIcon {
        if let Some(icon) = self.emoticon.as_deref().and_then(icon_by_emoji) {
            return icon;
        }
        let types = [
            self.contacts,
            self.non_contacts,
            self.groups,
            self.channels,
            self.bots,
        ];
        if !self.include.is_empty()
            || !self.pinned.is_empty()
            || !self.exclude.is_empty()
            || !types.contains(&true)
        {
            return FilterIcon::Custom;
        }
        let only = |c: bool, n: bool, g: bool, ch: bool, b: bool| types == [c, n, g, ch, b];
        if only(true, false, false, false, false)
            || only(false, true, false, false, false)
            || only(true, true, false, false, false)
        {
            FilterIcon::Private
        } else if only(false, false, true, false, false) {
            FilterIcon::Groups
        } else if only(false, false, false, true, false) {
            FilterIcon::Channels
        } else if only(false, false, false, false, true) {
            FilterIcon::Bots
        } else if self.no_read && !self.no_muted {
            FilterIcon::Unread
        } else if self.no_muted && !self.no_read {
            FilterIcon::Unmuted
        } else {
            FilterIcon::Custom
        }
    }
}

fn icon_by_emoji(emoji: &str) -> Option<FilterIcon> {
    let bare: String = emoji
        .chars()
        .filter(|c| *c != '\u{FE0F}' && *c != '\u{FE0E}')
        .collect();
    EMOJI_ICONS
        .iter()
        .find(|(e, _)| *e == bare)
        .map(|(_, icon)| *icon)
}

fn input_peer_key(peer: &enums::InputPeer) -> Option<PeerKey> {
    match peer {
        enums::InputPeer::User(u) => Some(PeerKey::user(u.user_id)),
        enums::InputPeer::Chat(c) => Some(PeerKey::chat(c.chat_id)),
        enums::InputPeer::Channel(c) => Some(PeerKey::channel(c.channel_id)),
        enums::InputPeer::UserFromMessage(u) => Some(PeerKey::user(u.user_id)),
        enums::InputPeer::ChannelFromMessage(c) => Some(PeerKey::channel(c.channel_id)),
        enums::InputPeer::InputPeerSelf | enums::InputPeer::Empty => None,
    }
}

/// The folder's unread badge (`FiltersMenu::prepareButton`): unread chats, counted
/// with the muted ones; grey when all of them are muted. `None`: no badge.
pub fn unread_badge<'a>(
    chats: impl Iterator<Item = &'a ChatTraits>,
    filter: Option<&ChatFilter>,
) -> Option<(u32, bool)> {
    let (mut count, mut muted) = (0u32, 0u32);
    for chat in chats {
        if !chat.unread || filter.is_some_and(|f| !f.contains(chat)) {
            continue;
        }
        // "All chats" is the main list: archived chats are not in it.
        if filter.is_none() && chat.archived {
            continue;
        }
        count += 1;
        if chat.muted {
            muted += 1;
        }
    }
    (count > 0).then_some((count, count == muted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::describe::PeerKind;

    fn chat(id: i64, kind: PeerKind) -> ChatTraits {
        ChatTraits {
            peer: PeerKey::user(id),
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn folders_match_like_tdesktop() {
        let groups = ChatFilter {
            groups: true,
            no_muted: true,
            exclude: vec![PeerKey::user(3)],
            include: vec![PeerKey::user(4)],
            ..Default::default()
        };
        assert!(groups.contains(&chat(1, PeerKind::Group)));
        assert!(!groups.contains(&chat(2, PeerKind::User)));
        assert!(!groups.contains(&ChatTraits {
            muted: true,
            ..chat(1, PeerKind::Group)
        }));
        // Excluded beats the type, included beats everything else.
        assert!(!groups.contains(&chat(3, PeerKind::Group)));
        assert!(groups.contains(&chat(4, PeerKind::User)));
        let contacts = ChatFilter {
            contacts: true,
            ..Default::default()
        };
        assert!(contacts.contains(&ChatTraits {
            contact: true,
            ..chat(1, PeerKind::User)
        }));
        assert!(!contacts.contains(&chat(1, PeerKind::User)));
        assert!(!contacts.contains(&chat(1, PeerKind::Bot)));
    }

    #[test]
    fn icons_like_tdesktop() {
        let f = |emoticon: Option<&str>| ChatFilter {
            emoticon: emoticon.map(str::to_string),
            groups: true,
            ..Default::default()
        };
        assert_eq!(f(None).icon(), FilterIcon::Groups);
        assert_eq!(f(Some("\u{1F431}")).icon(), FilterIcon::Cat);
        assert_eq!(f(Some("\u{2708}\u{FE0F}")).icon(), FilterIcon::Travel);
        let unread = ChatFilter {
            contacts: true,
            non_contacts: true,
            groups: true,
            channels: true,
            bots: true,
            no_read: true,
            ..Default::default()
        };
        assert_eq!(unread.icon(), FilterIcon::Unread);
        let custom = ChatFilter {
            include: vec![PeerKey::user(1)],
            ..Default::default()
        };
        assert_eq!(custom.icon(), FilterIcon::Custom);
    }

    #[test]
    fn badge_counts_unread_chats() {
        let chats = [
            ChatTraits {
                unread: true,
                muted: true,
                ..chat(1, PeerKind::Group)
            },
            ChatTraits {
                unread: true,
                ..chat(2, PeerKind::User)
            },
            chat(3, PeerKind::User),
        ];
        assert_eq!(unread_badge(chats.iter(), None), Some((2, false)));
        let groups = ChatFilter {
            groups: true,
            ..Default::default()
        };
        assert_eq!(unread_badge(chats.iter(), Some(&groups)), Some((1, true)));
    }
}
