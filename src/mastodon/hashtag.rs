//! Local hashtag name contract from Mastodon 4.6.5 Tag/FeaturedTag.
use std::sync::LazyLock;

/// Ruby's POSIX word class is `Letter/Mark/Number/Connector_Punctuation`, not
/// Rust regex's broader Unicode word class (which also includes join controls).
pub(crate) fn valid_name(name: &str) -> bool {
    static NAME: LazyLock<regex::Regex> = LazyLock::new(|| {
        let word = r"[\p{L}\p{M}\p{N}\p{Pc}]";
        let separator = "_\u{00b7}\u{30fb}\u{200c}";
        regex::Regex::new(&format!(
            r"\A(?:{word}[\p{{L}}\p{{M}}\p{{N}}\p{{Pc}}{separator}]*[\p{{Alphabetic}}{separator}][\p{{L}}\p{{M}}\p{{N}}\p{{Pc}}{separator}]*{word}|{word}*\p{{Alphabetic}}{word}*)\z"
        )).expect("pinned hashtag name expression")
    });
    NAME.is_match(name)
}

pub(crate) fn display_name(name: &str) -> String {
    name.chars()
        .filter(|c| {
            c.is_alphanumeric()
                || matches!(
                    c,
                    '_' | '\u{00b7}' | '\u{30fb}' | '\u{200c}' | '\u{0e47}'..='\u{0e4e}'
                )
        })
        .collect()
}

pub(crate) fn featured_name(name: &str) -> &str {
    // Ruby String#strip strips ASCII whitespace and NUL, not Unicode whitespace.
    let name = name.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    name.strip_prefix('#').unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::repository::normalize_hashtag;

    #[test]
    fn pinned_tag_names_and_normalization() {
        for name in [
            "a",
            "Café",
            "ＦＯＯ",
            "a\u{301}",
            "1_2",
            "a·b",
            "a\u{200c}b",
            "ภาษาไทย",
            "東京",
        ] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", "123", "_", "#rust", "foo bar", "·ab", "ab·", "💙"] {
            assert!(!valid_name(name), "{name}");
        }
        assert_eq!(normalize_hashtag("ＣＡＦÉ"), "cafe");
        assert_eq!(featured_name(" \t#Café\n"), "Café");
        assert_eq!(featured_name("##rust"), "#rust");
        assert_eq!(featured_name("\u{a0}rust\u{a0}"), "\u{a0}rust\u{a0}");
    }
}
