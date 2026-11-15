//! Message drafts: the local draft (what the input field holds) and the cloud draft
//! (synced through the server), ported from tdesktop (`data/data_drafts.cpp`,
//! `History::{createCloudDraft, createLocalDraftFromCloud, skipCloudDraftUpdate,
//! startSavingCloudDraft, finishSavingCloudDraft}`, `ApiWrap::saveDraftsToCloud`).
//!
//! A pure state machine: callers do the network and storage work.
//!
//! One deliberate change, [`Drafts::keep_local_on_empty_cloud`]: in tdesktop an empty
//! cloud draft (`draftMessageEmpty`) from another device wipes the local draft and the
//! open input field. Mobile clients send one when a chat is merely opened before
//! their draft sync arrived, so text typed on the desktop vanishes. With the flag on,
//! an empty cloud draft never touches the local one; instead, when an outgoing message
//! from another device matches the local draft (it was sent from there), the draft is
//! cleared.

use std::collections::HashMap;

use tdr_storage::PeerKey;

/// tdesktop's `kSkipCloudDraftsFor`: after saving, ignore cloud drafts older than the
/// save time plus this (they are echoes of earlier states).
const SKIP_CLOUD_DRAFTS_FOR: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// Unix time; 0 when unknown.
    pub date: i32,
}

impl Draft {
    /// tdesktop's `DraftIsNull` for text drafts.
    fn is_null(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// A draft from the server (`updateDraftMessage`, `dialog.draft`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudDraft {
    /// `draftMessageEmpty`.
    Empty { date: i32 },
    /// `draftMessage`.
    Text { text: String, date: i32 },
}

#[derive(Debug, Default, Clone)]
struct PeerDrafts {
    local: Option<Draft>,
    cloud: Option<Draft>,
    /// Cloud saves in flight.
    saving: u32,
    /// Cloud drafts older than this are ignored.
    accept_after: i32,
    /// The local draft changed since the last cloud save.
    dirty: bool,
}

/// The local draft of a chat changed (the input field should show it, when that chat is
/// open): `None` means cleared.
pub type Change = (PeerKey, Option<String>);

#[derive(Debug, Default)]
pub struct Drafts {
    peers: HashMap<PeerKey, PeerDrafts>,
    /// See the module docs.
    pub keep_local_on_empty_cloud: bool,
}

impl Drafts {
    pub fn new(keep_local_on_empty_cloud: bool) -> Self {
        Self {
            peers: HashMap::new(),
            keep_local_on_empty_cloud,
        }
    }

    /// Restores a local draft from the cache (on start).
    pub fn restore_local(&mut self, peer: PeerKey, text: String, date: i32) {
        let p = self.peers.entry(peer).or_default();
        p.local = Some(Draft { text, date });
        // The cloud might not have it (the app quit before saving): save it again.
        p.dirty = true;
    }

    pub fn local(&self, peer: PeerKey) -> Option<&Draft> {
        self.peers
            .get(&peer)
            .and_then(|p| p.local.as_ref())
            .filter(|d| !d.is_null())
    }

    /// The input field of `peer` holds `text` (the user typed, or left the chat).
    /// Returns whether anything changed (then it should be stored and later saved to
    /// the cloud).
    pub fn set_local(&mut self, peer: PeerKey, text: &str, now: i32) -> bool {
        let p = self.peers.entry(peer).or_default();
        let same = match &p.local {
            Some(d) => d.text == text,
            None => text.trim().is_empty(),
        };
        if same {
            return false;
        }
        p.local = (!text.trim().is_empty()).then(|| Draft {
            text: text.to_string(),
            date: now,
        });
        p.dirty = true;
        true
    }

    /// Chats whose local draft should be saved to the cloud.
    pub fn pending_cloud_saves(&self) -> Vec<PeerKey> {
        self.peers
            .iter()
            .filter(|(_, p)| p.dirty)
            .map(|(k, _)| *k)
            .collect()
    }

    /// `ApiWrap::saveDraftsToCloud` for one chat: the cloud draft becomes the local one
    /// (dated now) and a save starts. Returns the text to send (`""` clears the cloud
    /// draft). Pair with [`Drafts::finish_save`].
    pub fn start_save(&mut self, peer: PeerKey, now: i32) -> String {
        let p = self.peers.entry(peer).or_default();
        p.dirty = false;
        p.saving += 1;
        p.cloud = p.local.as_ref().filter(|d| !d.is_null()).map(|d| Draft {
            text: d.text.clone(),
            date: now,
        });
        p.cloud.as_ref().map(|d| d.text.clone()).unwrap_or_default()
    }

    /// `History::finishSavingCloudDraft`: the save request ended (done or failed) at
    /// server time `saved_at`.
    pub fn finish_save(&mut self, peer: PeerKey, saved_at: i32) {
        let p = self.peers.entry(peer).or_default();
        p.saving = p.saving.saturating_sub(1);
        p.accept_after = p.accept_after.max(saved_at + SKIP_CLOUD_DRAFTS_FOR);
    }

    /// A cloud draft arrived. `from_dialog`: it came with the chat list, where tdesktop
    /// applies only non-empty drafts (`History::applyDialog`).
    pub fn cloud_update(
        &mut self,
        peer: PeerKey,
        draft: CloudDraft,
        from_dialog: bool,
    ) -> Option<Change> {
        let keep_local = self.keep_local_on_empty_cloud;
        let p = self.peers.entry(peer).or_default();
        let date = match &draft {
            CloudDraft::Empty { date } | CloudDraft::Text { date, .. } => *date,
        };
        if from_dialog && matches!(draft, CloudDraft::Empty { .. }) {
            return None;
        }
        // `skipCloudDraftUpdate`.
        if p.saving > 0 || date < p.accept_after {
            return None;
        }
        match draft {
            CloudDraft::Text { text, date } => {
                // `ApplyPeerCloudDraft`, then `createLocalDraftFromCloud`.
                let cloud = Draft { text, date };
                p.cloud = Some(cloud.clone());
                if cloud.is_null() || cloud.date == 0 {
                    return None;
                }
                let replace = match &p.local {
                    None => true,
                    Some(local) => local.is_null() || local.date == 0 || cloud.date >= local.date,
                };
                if !replace || p.local.as_ref().is_some_and(|l| l.text == cloud.text) {
                    if replace {
                        p.local = Some(cloud);
                    }
                    return None;
                }
                p.local = Some(cloud.clone());
                p.dirty = false;
                Some((peer, Some(cloud.text)))
            }
            CloudDraft::Empty { .. } => {
                // `ClearPeerCloudDraft`: the cloud draft goes away, and with it tdesktop
                // clears the local draft and the field.
                if keep_local {
                    return None;
                }
                p.cloud = None;
                let had = p.local.take().is_some_and(|d| !d.is_null());
                p.dirty = false;
                had.then_some((peer, None))
            }
        }
    }

    /// We sent a message from the field (with `clear_draft`): both drafts are gone.
    pub fn message_sent(&mut self, peer: PeerKey) {
        let p = self.peers.entry(peer).or_default();
        p.local = None;
        p.cloud = None;
        p.dirty = false;
    }

    /// An outgoing message arrived that was sent elsewhere (another device). With
    /// [`Drafts::keep_local_on_empty_cloud`], the empty cloud draft that came with it
    /// was ignored: clear the local draft here if it is that very text.
    pub fn outgoing_message(&mut self, peer: PeerKey, text: &str) -> Option<Change> {
        if !self.keep_local_on_empty_cloud {
            return None;
        }
        let p = self.peers.get_mut(&peer)?;
        let local = p.local.as_ref()?;
        if local.text.trim() != text.trim() {
            return None;
        }
        p.local = None;
        p.cloud = None;
        p.dirty = false;
        Some((peer, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: PeerKey = PeerKey {
        kind: tdr_storage::PeerKind::User,
        id: 42,
    };

    fn text(t: &str, date: i32) -> CloudDraft {
        CloudDraft::Text {
            text: t.into(),
            date,
        }
    }

    /// Typed on the desktop, left the chat: saved locally and to the cloud.
    fn typed_and_left(d: &mut Drafts) {
        assert!(d.set_local(CHAT, "hello from desktop", 1000));
        assert_eq!(d.pending_cloud_saves(), vec![CHAT]);
        assert_eq!(d.start_save(CHAT, 1001), "hello from desktop");
        d.finish_save(CHAT, 1001);
    }

    #[test]
    fn tdesktop_wipes_the_draft_on_an_empty_cloud_draft() {
        // The bug, reproduced with tdesktop's behaviour: a phone that had not received
        // the draft yet "syncs" its empty field, and the desktop draft vanishes.
        let mut d = Drafts::new(false);
        typed_and_left(&mut d);
        let change = d.cloud_update(CHAT, CloudDraft::Empty { date: 1100 }, false);
        assert_eq!(change, Some((CHAT, None)));
        assert_eq!(d.local(CHAT), None);
    }

    #[test]
    fn fix_keeps_the_draft_on_an_empty_cloud_draft() {
        let mut d = Drafts::new(true);
        typed_and_left(&mut d);
        assert_eq!(
            d.cloud_update(CHAT, CloudDraft::Empty { date: 1100 }, false),
            None
        );
        assert_eq!(d.local(CHAT).unwrap().text, "hello from desktop");
        // Sent from the phone: the matching outgoing message clears it.
        assert_eq!(
            d.outgoing_message(CHAT, "hello from desktop"),
            Some((CHAT, None))
        );
        assert_eq!(d.local(CHAT), None);
    }

    #[test]
    fn fix_keeps_the_draft_when_another_text_is_sent_elsewhere() {
        let mut d = Drafts::new(true);
        typed_and_left(&mut d);
        assert_eq!(d.outgoing_message(CHAT, "something else"), None);
        assert!(d.local(CHAT).is_some());
    }

    #[test]
    fn newer_cloud_text_replaces_the_local_draft() {
        for keep in [false, true] {
            let mut d = Drafts::new(keep);
            typed_and_left(&mut d);
            let change = d.cloud_update(CHAT, text("edited on phone", 1200), false);
            assert_eq!(change, Some((CHAT, Some("edited on phone".into()))));
            assert_eq!(d.local(CHAT).unwrap().text, "edited on phone");
        }
    }

    #[test]
    fn older_cloud_text_does_not_replace_a_newer_local_draft() {
        let mut d = Drafts::new(true);
        assert!(d.set_local(CHAT, "newer", 2000));
        assert_eq!(d.cloud_update(CHAT, text("older", 1500), false), None);
        assert_eq!(d.local(CHAT).unwrap().text, "newer");
    }

    #[test]
    fn cloud_drafts_are_skipped_while_saving_and_right_after() {
        let mut d = Drafts::new(false);
        d.set_local(CHAT, "mine", 1000);
        d.start_save(CHAT, 1001);
        // In flight: ignored.
        assert_eq!(d.cloud_update(CHAT, text("echo", 1001), false), None);
        d.finish_save(CHAT, 1001);
        // Within kSkipCloudDraftsFor of the save: ignored.
        assert_eq!(
            d.cloud_update(CHAT, CloudDraft::Empty { date: 1002 }, false),
            None
        );
        assert_eq!(d.local(CHAT).unwrap().text, "mine");
        // Later ones apply.
        assert_eq!(
            d.cloud_update(CHAT, CloudDraft::Empty { date: 1003 }, false),
            Some((CHAT, None))
        );
    }

    #[test]
    fn empty_drafts_in_the_chat_list_are_ignored() {
        let mut d = Drafts::new(false);
        d.set_local(CHAT, "kept", 1000);
        assert_eq!(
            d.cloud_update(CHAT, CloudDraft::Empty { date: 5000 }, true),
            None
        );
        assert!(d.local(CHAT).is_some());
        // A non-empty one from the chat list applies.
        let mut d = Drafts::new(false);
        assert_eq!(
            d.cloud_update(CHAT, text("from list", 900), true),
            Some((CHAT, Some("from list".into())))
        );
    }

    #[test]
    fn sending_clears_and_unchanged_text_is_not_a_change() {
        let mut d = Drafts::new(true);
        d.set_local(CHAT, "x", 1);
        assert!(!d.set_local(CHAT, "x", 2));
        d.message_sent(CHAT);
        assert_eq!(d.local(CHAT), None);
        assert!(d.pending_cloud_saves().is_empty());
        // Clearing an empty field is not a change either.
        assert!(!d.set_local(CHAT, "  ", 3));
    }
}
