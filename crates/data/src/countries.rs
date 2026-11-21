//! Countries and phone numbers (tdesktop's `Countries::CountriesInstance`): the list
//! (built in until the server's arrives), the country of a calling code, the
//! longest valid code of typed digits, a number's groups and formatting (ported
//! from TDLib through tdesktop), and the country search of `CountrySelectBox`.

use crate::countries_fallback;

/// A country with one of its calling codes: a country with several codes (rare)
/// is several entries, in its order, as `CountrySelectBox` lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Country {
    pub iso2: String,
    pub name: String,
    /// "United States of America" for "USA" (`ProcessAlternativeName`), else empty.
    pub alternative_name: String,
    /// Calling code without "+".
    pub code: String,
    /// The first digits after the code that pick this country among those with the
    /// same code ("" matches any number).
    pub prefixes: Vec<String>,
    /// Number patterns ("XXX XXX XXXX").
    pub patterns: Vec<String>,
}

/// The built-in list (`FallbackList`).
pub fn fallback() -> Vec<Country> {
    countries_fallback::LIST
        .iter()
        .filter(|(.., hidden)| !hidden)
        .flat_map(|(name, iso2, alternative, codes, _)| {
            codes.iter().map(move |(code, prefixes, patterns)| Country {
                iso2: iso2.to_string(),
                name: name.to_string(),
                alternative_name: alternative.to_string(),
                code: code.to_string(),
                // A code without prefixes matches any number, as the server's list
                // is read (`convertMTP(..., true)`).
                prefixes: if prefixes.is_empty() {
                    vec![String::new()]
                } else {
                    prefixes.iter().map(|p| p.to_string()).collect()
                },
                patterns: patterns.iter().map(|p| p.to_string()).collect(),
            })
        })
        .collect()
}

/// `ProcessAlternativeName`.
pub fn alternative_name(name: &str) -> String {
    if name == "USA" {
        "United States of America".into()
    } else {
        String::new()
    }
}

/// The country of a calling code (`byCode`): a hash filled in list order, so of
/// countries sharing a code the last one wins ("7": Russian Federation, not
/// Kazakhstan; "1": USA).
pub fn by_code<'a>(list: &'a [Country], code: &str) -> Option<&'a Country> {
    list.iter().rev().find(|c| c.code == code)
}

/// The first entry of a country (`byISO2`).
pub fn by_iso2<'a>(list: &'a [Country], iso2: &str) -> Option<&'a Country> {
    list.iter().find(|c| c.iso2 == iso2)
}

/// `validPhoneCode`: the longest start of `digits` that is a calling code.
pub fn valid_phone_code(list: &[Country], digits: &str) -> String {
    let mut code = digits.to_string();
    while !code.is_empty() {
        if by_code(list, &code).is_some() {
            return code;
        }
        code.pop();
    }
    code
}

/// The calling code and the country's entry that fit a full number best: the
/// longest code plus prefix it starts with (`CountriesInstance::format`).
fn best<'a>(list: &'a [Country], phone: &str) -> Option<&'a Country> {
    let mut best = None;
    let mut best_length = 0;
    for country in list {
        if let Some(rest) = phone.strip_prefix(country.code.as_str()) {
            for prefix in &country.prefixes {
                let length = country.code.len() + prefix.len();
                if length > best_length && rest.starts_with(prefix.as_str()) {
                    best = Some(country);
                    best_length = length;
                }
            }
        }
    }
    best
}

/// `Countries::Groups`: the group sizes of a number being typed (digits only, the
/// code first), for spacing it as it is typed; empty when no pattern fits.
pub fn groups(list: &[Country], phone: &str) -> Vec<usize> {
    let Some(country) = best(list, phone) else {
        return Vec::new();
    };
    let code_size = country.code.len();
    let initial = vec![code_size];
    if country.patterns.is_empty() {
        return initial;
    }
    let digits: Vec<char> = phone.chars().collect();
    let mut best_groups = initial.clone();
    let mut best_size = 0;
    let mut best_matches: i32 = -1;
    for pattern in &country.patterns {
        let mut groups = initial.clone();
        let mut size = 0;
        let mut spaces = 0;
        let mut matched = 0;
        let mut not_best = false;
        for (i, c) in pattern.chars().enumerate() {
            if c.is_ascii_digit() {
                let n = i - spaces + code_size;
                if n < digits.len() && digits[n] == c {
                    matched += 1;
                } else {
                    not_best = true;
                }
            }
            if c.is_whitespace() {
                groups.push(std::mem::take(&mut size));
                spaces += 1;
            } else {
                size += 1;
            }
        }
        if matched > best_matches {
            best_matches = if not_best { -1 } else { matched };
            best_groups = groups;
            best_size = size;
        }
    }
    if best_size > 0 {
        best_groups.push(best_size);
    }
    best_groups
}

/// `CountriesInstance::format`: "+7 900 123 4567" from "79001234567".
pub fn format(list: &[Country], phone: &str) -> String {
    let phone: String = phone.chars().filter(char::is_ascii_digit).collect();
    if phone.is_empty() {
        return String::new();
    }
    let Some(country) = best(list, &phone) else {
        return format!("+{phone}");
    };
    let part = &phone[country.code.len()..];
    let mut formatted = part.to_string();
    let mut max_matched = 0;
    for pattern in &country.patterns {
        let pattern: Vec<char> = pattern.chars().collect();
        let mut result = String::new();
        let mut pos = 0;
        let mut failed = false;
        let mut matched = 0;
        for c in part.chars() {
            while pos < pattern.len() && pattern[pos] != 'X' && !pattern[pos].is_ascii_digit() {
                result.push(pattern[pos]);
                pos += 1;
            }
            if pos >= pattern.len() || pattern[pos] == 'X' {
                pos += 1;
                result.push(c);
            } else if c == pattern[pos] {
                matched += 1;
                pos += 1;
                result.push(c);
            } else {
                failed = true;
                break;
            }
        }
        if !failed && matched >= max_matched {
            max_matched = matched;
            formatted = result;
        }
    }
    format!("+{} {}", country.code, formatted)
        .trim_end()
        .to_string()
}

/// `TextUtilities::PrepareSearchWords`, simplified: lower-case words split at
/// anything but letters and digits.
pub fn search_words(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// `CountrySelectBox::Inner::updateFilter`: the entries every word of `query`
/// starts a part of the name of (name and alternative name split at spaces and
/// dashes). An empty query keeps all.
pub fn search(list: &[Country], query: &str) -> Vec<usize> {
    let words = search_words(query);
    if words.is_empty() {
        return (0..list.len()).collect();
    }
    list.iter()
        .enumerate()
        .filter(|(_, c)| {
            let full = format!("{} {}", c.name, c.alternative_name).to_lowercase();
            let names: Vec<&str> = full
                .split(|ch: char| ch.is_whitespace() || ch == '-')
                .filter(|p| !p.is_empty())
                .collect();
            words
                .iter()
                .all(|w| names.iter().any(|n| n.starts_with(w.as_str())))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_codes_pick_the_last_country() {
        let list = fallback();
        assert_eq!(by_code(&list, "7").unwrap().iso2, "RU");
        assert_eq!(by_code(&list, "1").unwrap().iso2, "US");
        assert_eq!(by_code(&list, "44").unwrap().iso2, "GB");
        assert!(by_code(&list, "999").is_none());
    }

    #[test]
    fn valid_codes() {
        let list = fallback();
        assert_eq!(valid_phone_code(&list, "79991"), "7");
        assert_eq!(valid_phone_code(&list, "3801"), "380");
        assert_eq!(valid_phone_code(&list, "999"), "");
    }

    #[test]
    fn numbers_group_by_their_country() {
        let list = fallback();
        // Russia (no prefix) and Kazakhstan (prefix 6) share +7.
        assert_eq!(groups(&list, "7900"), [1, 3, 3, 4]);
        assert_eq!(groups(&list, "7600"), [1, 3, 3, 2, 2]);
        assert_eq!(format(&list, "79001234567"), "+7 900 123 4567");
        assert_eq!(format(&list, "76001234567"), "+7 600 123 45 67");
        assert_eq!(format(&list, "15550100"), "+1 555 010 0");
        assert_eq!(format(&list, "999"), "+999");
    }

    #[test]
    fn search_by_word_starts() {
        let list = fallback();
        let names = |q: &str| -> Vec<String> {
            search(&list, q)
                .iter()
                .map(|&i| list[i].iso2.clone())
                .collect()
        };
        assert!(names("rus").contains(&"RU".to_string()));
        assert!(names("united states").contains(&"US".to_string()));
        assert!(names("fed").contains(&"RU".to_string()));
        assert!(names("zzz").is_empty());
        assert_eq!(search(&list, "").len(), list.len());
    }
}
