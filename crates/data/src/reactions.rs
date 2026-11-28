//! Message reactions (tdesktop's `Data::Reactions`): the reactions the server
//! offers (`messages.getAvailableReactions`), the top and recent ones (the
//! order of the selector), and sending ours (`messages.sendReaction`).

use tdr_api::Client;
use tdr_tl::{enums, functions};

use crate::Result;
use crate::describe::ReactionKey;
use crate::emoji_sets::{EmojiDocument, emoji_document};

/// An emoji reaction of the server's list (`Data::Reaction`) with the
/// animations it is drawn with.
#[derive(Debug, Clone)]
pub struct Available {
    pub emoticon: String,
    pub title: String,
    pub premium: bool,
    /// Not offered for new reactions any more (`inactive`).
    pub inactive: bool,
    /// The icon in message reactions (its first frame) and the selector
    /// (`centerIcon`; older reactions have only `selectAnimation`).
    pub center: Option<EmojiDocument>,
    pub select: Option<EmojiDocument>,
    pub appear: Option<EmojiDocument>,
    pub activate: Option<EmojiDocument>,
    pub effect: Option<EmojiDocument>,
    pub around: Option<EmojiDocument>,
}

impl Available {
    /// Every document it has (to be downloaded like custom emoji).
    pub fn documents(&self) -> impl Iterator<Item = &EmojiDocument> {
        [
            &self.center,
            &self.select,
            &self.appear,
            &self.activate,
            &self.effect,
            &self.around,
        ]
        .into_iter()
        .flatten()
    }
}

/// `messages.getAvailableReactions`, with all their documents as sent (to be
/// downloaded like custom emoji).
pub async fn available(client: &Client) -> Result<(Vec<Available>, Vec<enums::Document>)> {
    let answer = client
        .invoke(&functions::messages::GetAvailableReactions { hash: 0 })
        .await?;
    let enums::messages::AvailableReactions::AvailableReactions(list) = answer else {
        return Ok((Vec::new(), Vec::new()));
    };
    let raw: Vec<enums::Document> = list
        .reactions
        .iter()
        .flat_map(|r| {
            let enums::AvailableReaction::AvailableReaction(r) = r;
            [
                Some(&r.select_animation),
                Some(&r.appear_animation),
                Some(&r.activate_animation),
                Some(&r.effect_animation),
                r.center_icon.as_ref(),
                r.around_animation.as_ref(),
            ]
            .into_iter()
            .flatten()
            .cloned()
        })
        .collect();
    let list = list
        .reactions
        .iter()
        .map(|r| {
            let enums::AvailableReaction::AvailableReaction(r) = r;
            Available {
                emoticon: r.reaction.clone(),
                title: r.title.clone(),
                premium: r.premium,
                inactive: r.inactive,
                center: r.center_icon.as_ref().and_then(emoji_document),
                select: emoji_document(&r.select_animation),
                appear: emoji_document(&r.appear_animation),
                activate: emoji_document(&r.activate_animation),
                effect: emoji_document(&r.effect_animation),
                around: r.around_animation.as_ref().and_then(emoji_document),
            }
        })
        .collect();
    Ok((list, raw))
}

fn keys(answer: enums::messages::Reactions) -> Vec<ReactionKey> {
    match answer {
        enums::messages::Reactions::Reactions(r) => {
            r.reactions.iter().filter_map(ReactionKey::of).collect()
        }
        enums::messages::Reactions::NotModified => Vec::new(),
    }
}

/// `messages.getTopReactions` (`kTopReactionsLimit`).
pub async fn top(client: &Client) -> Result<Vec<ReactionKey>> {
    Ok(keys(
        client
            .invoke(&functions::messages::GetTopReactions { limit: 14, hash: 0 })
            .await?,
    ))
}

/// `messages.getRecentReactions` (`kRecentReactionsLimit`).
pub async fn recent(client: &Client) -> Result<Vec<ReactionKey>> {
    Ok(keys(
        client
            .invoke(&functions::messages::GetRecentReactions { limit: 40, hash: 0 })
            .await?,
    ))
}

/// `messages.sendReaction` with all our reactions to the message (none: ours
/// are taken back).
pub async fn send(
    client: &Client,
    peer: enums::InputPeer,
    msg_id: i32,
    chosen: &[ReactionKey],
    add_to_recent: bool,
) -> Result<enums::Updates> {
    Ok(client
        .invoke(&functions::messages::SendReaction {
            big: false,
            add_to_recent,
            peer,
            msg_id,
            reaction: Some(chosen.iter().map(ReactionKey::tl).collect()),
        })
        .await?)
}

/// One of our Saved Messages tags (`savedReactionTag`): its name, if given,
/// and how many messages have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTag {
    pub key: ReactionKey,
    pub title: Option<String>,
    pub count: i32,
}

/// `messages.getSavedReactionTags`: the tags we used, most used first.
pub async fn saved_tags(client: &Client) -> Result<Vec<SavedTag>> {
    let answer = client
        .invoke(&functions::messages::GetSavedReactionTags {
            peer: None,
            hash: 0,
        })
        .await?;
    Ok(match answer {
        enums::messages::SavedReactionTags::SavedReactionTags(t) => t
            .tags
            .iter()
            .filter_map(|t| {
                let enums::SavedReactionTag::SavedReactionTag(t) = t;
                Some(SavedTag {
                    key: ReactionKey::of(&t.reaction)?,
                    title: t.title.clone().filter(|t| !t.is_empty()),
                    count: t.count,
                })
            })
            .collect(),
        enums::messages::SavedReactionTags::NotModified => Vec::new(),
    })
}

/// `messages.getDefaultTagReactions`: the tags offered to everyone.
pub async fn default_tags(client: &Client) -> Result<Vec<ReactionKey>> {
    Ok(keys(
        client
            .invoke(&functions::messages::GetDefaultTagReactions { hash: 0 })
            .await?,
    ))
}
