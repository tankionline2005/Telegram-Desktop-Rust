//! Custom emoji and their sets (tdesktop's `Data::CustomEmojiManager`,
//! `Data::Stickers`): documents by id (`messages.getCustomEmojiDocuments`, 100 at a
//! time), the files, the installed and featured emoji sets, one set's contents,
//! installing and removing, and emoji statuses.
//!
//! Documents and sets are kept as TL blobs tagged with the layer (the file cache's
//! keys `emoji-doc:<id>`, `emoji-set:<id>`), so a schema update never breaks them.

use tdr_api::Client;
use tdr_tl::{Deserializable, Serializable, enums, functions, types};

use crate::{Error, Result};

/// `kMaxPerRequest`.
pub const DOCUMENTS_PER_REQUEST: usize = 100;

/// How an emoji is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmojiFormat {
    /// `application/x-tgsticker`: gzipped Lottie.
    Tgs,
    /// `video/webm`: VP9, often with alpha.
    Webm,
    /// `image/webp`: a still image.
    Webp,
}

/// A custom emoji document.
#[derive(Clone, Debug)]
pub struct EmojiDocument {
    pub id: i64,
    pub access_hash: i64,
    pub file_reference: Vec<u8>,
    pub dc_id: i32,
    pub size: i64,
    pub format: EmojiFormat,
    /// The emoji it stands for (`alt`).
    pub alt: String,
    /// Its set: id and access hash.
    pub set: Option<(i64, i64)>,
    /// Drawn in the text's color (`text_color`).
    pub text_color: bool,
    /// Usable without Premium (`free`).
    pub free: bool,
    /// A sticker (not an emoji): the stickers stage uses it.
    pub sticker: bool,
    /// The vector thumbnail (`photoPathSize`, Telegram's compressed SVG path in
    /// a 512 square), drawn while the file loads.
    pub path: Option<Vec<u8>>,
}

/// The document of a TL object, if it is a custom emoji or a sticker.
pub fn emoji_document(doc: &enums::Document) -> Option<EmojiDocument> {
    let enums::Document::Document(d) = doc else {
        return None;
    };
    let format = match d.mime_type.as_str() {
        "application/x-tgsticker" => EmojiFormat::Tgs,
        "video/webm" => EmojiFormat::Webm,
        "image/webp" => EmojiFormat::Webp,
        _ => return None,
    };
    let mut out = EmojiDocument {
        id: d.id,
        access_hash: d.access_hash,
        file_reference: d.file_reference.to_vec(),
        dc_id: d.dc_id,
        size: d.size,
        format,
        alt: String::new(),
        set: None,
        text_color: false,
        free: false,
        sticker: false,
        path: d.thumbs.iter().flatten().find_map(|t| match t {
            enums::PhotoSize::PhotoPathSize(p) => Some(p.bytes.to_vec()),
            _ => None,
        }),
    };
    let set_of = |s: &enums::InputStickerSet| match s {
        enums::InputStickerSet::ID(id) => Some((id.id, id.access_hash)),
        _ => None,
    };
    for a in &d.attributes {
        match a {
            enums::DocumentAttribute::CustomEmoji(e) => {
                out.alt = e.alt.clone();
                out.set = set_of(&e.stickerset);
                out.text_color = e.text_color;
                out.free = e.free;
            }
            enums::DocumentAttribute::Sticker(s) => {
                out.alt = s.alt.clone();
                out.set = set_of(&s.stickerset);
                out.sticker = true;
            }
            _ => {}
        }
    }
    Some(out)
}

/// A TL object as stored: the layer, then its bytes.
pub fn encode<T: Serializable>(value: &T) -> Vec<u8> {
    let mut blob = tdr_tl::LAYER.to_le_bytes().to_vec();
    blob.extend_from_slice(&value.to_bytes());
    blob
}

/// A stored TL object, if it was stored by this layer.
pub fn decode<T: Deserializable>(blob: &[u8]) -> Option<T> {
    let layer = i32::from_le_bytes(blob.get(..4)?.try_into().ok()?);
    if layer != tdr_tl::LAYER {
        return None;
    }
    T::from_bytes(&tdr_tl::Bytes::copy_from_slice(&blob[4..])).ok()
}

/// `messages.getCustomEmojiDocuments`.
pub async fn custom_emoji_documents(client: &Client, ids: &[i64]) -> Result<Vec<enums::Document>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(DOCUMENTS_PER_REQUEST) {
        let docs = client
            .invoke(&functions::messages::GetCustomEmojiDocuments {
                document_id: chunk.to_vec(),
            })
            .await?;
        out.extend(docs);
    }
    Ok(out)
}

/// The whole file of a document, from the DC that stores it.
pub async fn download_document(client: &Client, doc: &EmojiDocument) -> Result<Vec<u8>> {
    const PART: i32 = 512 * 1024;
    let location: enums::InputFileLocation = types::InputDocumentFileLocation {
        id: doc.id,
        access_hash: doc.access_hash,
        file_reference: tdr_tl::Bytes::copy_from_slice(&doc.file_reference),
        thumb_size: String::new(),
    }
    .into();
    let mut data = Vec::with_capacity(doc.size.max(0) as usize);
    loop {
        let request = functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: location.clone(),
            offset: data.len() as i64,
            limit: PART,
        };
        let enums::upload::File::File(part) = client.invoke_in(doc.dc_id, &request).await? else {
            return Err(Error::Unsupported("CDN file"));
        };
        let done = part.bytes.len() < PART as usize;
        data.extend_from_slice(&part.bytes);
        if done {
            return Ok(data);
        }
    }
}

/// A set's header (`stickerSet`).
#[derive(Clone, Debug, PartialEq)]
pub struct SetInfo {
    pub id: i64,
    pub access_hash: i64,
    pub title: String,
    pub short_name: String,
    pub count: i32,
    pub hash: i32,
    pub emojis: bool,
    pub archived: bool,
    pub official: bool,
    pub installed: bool,
    pub text_color: bool,
    pub creator: bool,
    /// The document shown as its icon (`thumb_document_id`), else the first one.
    pub thumb_document_id: Option<i64>,
}

pub fn set_info(set: &enums::StickerSet) -> SetInfo {
    let enums::StickerSet::StickerSet(s) = set;
    SetInfo {
        id: s.id,
        access_hash: s.access_hash,
        title: s.title.clone(),
        short_name: s.short_name.clone(),
        count: s.count,
        hash: s.hash,
        emojis: s.emojis,
        archived: s.archived,
        official: s.official,
        installed: s.installed_date.is_some(),
        text_color: s.text_color,
        creator: s.creator,
        thumb_document_id: s.thumb_document_id,
    }
}

/// A set with its documents (`messages.stickerSet`).
#[derive(Clone, Debug)]
pub struct FullSet {
    pub info: SetInfo,
    pub documents: Vec<enums::Document>,
    /// Emoji -> documents (the packs), for search.
    pub packs: Vec<(String, Vec<i64>)>,
    /// Document -> keywords.
    pub keywords: Vec<(i64, Vec<String>)>,
}

pub fn full_set(set: &enums::messages::StickerSet) -> Option<FullSet> {
    let enums::messages::StickerSet::StickerSet(s) = set else {
        return None;
    };
    Some(FullSet {
        info: set_info(&s.set),
        documents: s.documents.clone(),
        packs: s
            .packs
            .iter()
            .map(|p| {
                let enums::StickerPack::StickerPack(p) = p;
                (p.emoticon.clone(), p.documents.clone())
            })
            .collect(),
        keywords: s
            .keywords
            .iter()
            .map(|k| {
                let enums::StickerKeyword::StickerKeyword(k) = k;
                (k.document_id, k.keyword.clone())
            })
            .collect(),
    })
}

/// A set by id or by its link name (`t.me/addemoji/<name>`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SetRef {
    Id(i64, i64),
    ShortName(String),
}

impl SetRef {
    fn input(&self) -> enums::InputStickerSet {
        match self {
            SetRef::Id(id, access_hash) => enums::InputStickerSet::ID(types::InputStickerSetID {
                id: *id,
                access_hash: *access_hash,
            }),
            SetRef::ShortName(name) => types::InputStickerSetShortName {
                short_name: name.clone(),
            }
            .into(),
        }
    }
}

/// `messages.getStickerSet` (the raw answer, to keep it).
pub async fn sticker_set(client: &Client, set: &SetRef) -> Result<enums::messages::StickerSet> {
    Ok(client
        .invoke(&functions::messages::GetStickerSet {
            stickerset: set.input(),
            hash: 0,
        })
        .await?)
}

/// `messages.getEmojiStickers`: the installed emoji sets, in the user's order.
pub async fn installed_emoji_sets(client: &Client) -> Result<Vec<SetInfo>> {
    let answer = client
        .invoke(&functions::messages::GetEmojiStickers { hash: 0 })
        .await?;
    Ok(match answer {
        enums::messages::AllStickers::AllStickers(all) => all.sets.iter().map(set_info).collect(),
        enums::messages::AllStickers::NotModified => Vec::new(),
    })
}

/// A featured set: its header and the documents shown for it.
#[derive(Clone, Debug)]
pub struct FeaturedSet {
    pub info: SetInfo,
    pub documents: Vec<enums::Document>,
}

/// `messages.getFeaturedEmojiStickers`.
pub async fn featured_emoji_sets(client: &Client) -> Result<Vec<FeaturedSet>> {
    let answer = client
        .invoke(&functions::messages::GetFeaturedEmojiStickers { hash: 0 })
        .await?;
    let enums::messages::FeaturedStickers::FeaturedStickers(f) = answer else {
        return Ok(Vec::new());
    };
    Ok(f.sets
        .iter()
        .map(|covered| match covered {
            enums::StickerSetCovered::StickerSetCovered(c) => FeaturedSet {
                info: set_info(&c.set),
                documents: vec![c.cover.clone()],
            },
            enums::StickerSetCovered::StickerSetMultiCovered(c) => FeaturedSet {
                info: set_info(&c.set),
                documents: c.covers.clone(),
            },
            enums::StickerSetCovered::StickerSetFullCovered(c) => FeaturedSet {
                info: set_info(&c.set),
                documents: c.documents.clone(),
            },
            enums::StickerSetCovered::StickerSetNoCovered(c) => FeaturedSet {
                info: set_info(&c.set),
                documents: Vec::new(),
            },
        })
        .collect())
}

/// `messages.installStickerSet`.
pub async fn install_set(client: &Client, set: &SetRef) -> Result<()> {
    client
        .invoke(&functions::messages::InstallStickerSet {
            stickerset: set.input(),
            archived: false,
        })
        .await?;
    Ok(())
}

/// `messages.uninstallStickerSet`.
pub async fn uninstall_set(client: &Client, set: &SetRef) -> Result<()> {
    client
        .invoke(&functions::messages::UninstallStickerSet {
            stickerset: set.input(),
        })
        .await?;
    Ok(())
}

/// The document of an emoji status (its custom emoji) and when it ends.
pub fn status_document(status: &enums::EmojiStatus) -> Option<(i64, Option<i32>)> {
    match status {
        enums::EmojiStatus::EmojiStatus(s) => Some((s.document_id, s.until)),
        enums::EmojiStatus::Collectible(c) => Some((c.document_id, c.until)),
        _ => None,
    }
}

/// `account.getDefaultEmojiStatuses` and `account.getRecentEmojiStatuses`: the
/// statuses offered first in the status panel (documents).
pub async fn status_suggestions(client: &Client) -> Result<(Vec<i64>, Vec<i64>)> {
    let list = |answer: enums::account::EmojiStatuses| -> Vec<i64> {
        match answer {
            enums::account::EmojiStatuses::EmojiStatuses(s) => s
                .statuses
                .iter()
                .filter_map(status_document)
                .map(|(id, _)| id)
                .collect(),
            enums::account::EmojiStatuses::NotModified => Vec::new(),
        }
    };
    let defaults = client
        .invoke(&functions::account::GetDefaultEmojiStatuses { hash: 0 })
        .await?;
    let recent = client
        .invoke(&functions::account::GetRecentEmojiStatuses { hash: 0 })
        .await?;
    Ok((list(defaults), list(recent)))
}

/// `account.updateEmojiStatus`: a custom emoji (until a time) or none.
pub async fn set_emoji_status(
    client: &Client,
    document: Option<i64>,
    until: Option<i32>,
) -> Result<()> {
    let status = match document {
        Some(document_id) => {
            enums::EmojiStatus::EmojiStatus(Box::new(types::EmojiStatus { document_id, until }))
        }
        None => enums::EmojiStatus::Empty,
    };
    client
        .invoke(&functions::account::UpdateEmojiStatus {
            emoji_status: status,
        })
        .await?;
    Ok(())
}

/// A stored document blob (from [`encode`]) as an emoji document.
pub fn decode_document(blob: &[u8]) -> Option<EmojiDocument> {
    decode::<enums::Document>(blob).and_then(|d| emoji_document(&d))
}
