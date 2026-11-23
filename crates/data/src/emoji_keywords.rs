//! Emoji keywords of languages (tdesktop's `EmojiKeywords` requests): which
//! languages the server has (`messages.getEmojiKeywordsLanguages`), a language's
//! keywords (`getEmojiKeywords`) and their changes since a version
//! (`getEmojiKeywordsDifference`). The state and queries are `tdr_emoji::keywords`.

use tdr_api::Client;
use tdr_tl::{enums, functions};

use crate::Result;

/// A keyword's change: added (true) or deleted emoticons.
#[derive(Clone, Debug)]
pub struct KeywordChange {
    pub added: bool,
    pub keyword: String,
    pub emoticons: Vec<String>,
}

/// A language's keywords since `from_version` (all of them from 0).
pub struct Difference {
    pub lang_code: String,
    pub version: i32,
    pub changes: Vec<KeywordChange>,
}

/// `refreshRemoteList`: the languages of `codes` the server has keywords for.
pub async fn languages(client: &Client, codes: Vec<String>) -> Result<Vec<String>> {
    let list: Vec<enums::EmojiLanguage> = client
        .invoke(&functions::messages::GetEmojiKeywordsLanguages { lang_codes: codes })
        .await?;
    Ok(list
        .into_iter()
        .map(|l| match l {
            enums::EmojiLanguage::EmojiLanguage(l) => l.lang_code,
        })
        .collect())
}

/// `LangPack::refresh`: the keywords of `lang_code` (since `version`, if any).
pub async fn difference(client: &Client, lang_code: &str, version: i32) -> Result<Difference> {
    let result: enums::EmojiKeywordsDifference = if version > 0 {
        client
            .invoke(&functions::messages::GetEmojiKeywordsDifference {
                lang_code: lang_code.to_string(),
                from_version: version,
            })
            .await?
    } else {
        client
            .invoke(&functions::messages::GetEmojiKeywords {
                lang_code: lang_code.to_string(),
            })
            .await?
    };
    let enums::EmojiKeywordsDifference::EmojiKeywordsDifference(d) = result;
    Ok(Difference {
        lang_code: d.lang_code,
        version: d.version,
        changes: d
            .keywords
            .into_iter()
            .map(|k| match k {
                enums::EmojiKeyword::EmojiKeyword(k) => KeywordChange {
                    added: true,
                    keyword: k.keyword,
                    emoticons: k.emoticons,
                },
                enums::EmojiKeyword::Deleted(k) => KeywordChange {
                    added: false,
                    keyword: k.keyword,
                    emoticons: k.emoticons,
                },
            })
            .collect(),
    })
}
