//! Deciding whether a search result is the release that was asked for.
//!
//! A wrong copyright is worse than none, so every source uses the same rule:
//! a candidate counts only when both its artist and its release name match.
//! The comparison is loose enough to survive the differences between
//! catalogues — punctuation, case, and edition suffixes — and no looser.

/// Whether a candidate name means the same release as the expected one.
///
/// Punctuation and case are ignored, and one name may extend the other so that
/// `Random Access Memories` matches `Random Access Memories (Deluxe Edition)`,
/// and `Daft Punk` matches a catalogue's disambiguated `Daft Punk (2)`.
/// Anything less similar is treated as a different release.
pub fn matches_name(candidate: Option<&str>, expected: &str) -> bool {
    let (Some(candidate), expected) = (candidate.map(normalize), normalize(expected)) else {
        return false;
    };
    if candidate.is_empty() || expected.is_empty() {
        return false;
    }
    candidate == expected
        || candidate.starts_with(&format!("{expected} "))
        || expected.starts_with(&format!("{candidate} "))
}

pub fn normalize(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        // Apostrophes vanish rather than splitting a word, so that "Pepper's"
        // still matches a store listing spelled "Peppers".
        if matches!(character, '\'' | '\u{2019}' | '\u{02bc}') {
            continue;
        }
        if character.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            pending_space = false;
            normalized.extend(character.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    normalized
}

/// The four-digit year at the start of a catalogue date, if there is one.
///
/// The sources spell dates differently — `2001`, `2001-03-12`, and the odd
/// empty string — and only the year ever reaches the copyright line.
pub fn year_of(date: Option<&str>) -> Option<String> {
    let date = date?.trim();
    let year: String = date.chars().take_while(char::is_ascii_digit).collect();
    (year.len() == 4).then_some(year)
}

/// A copyright line built from its parts, or `None` when the owner is unknown.
///
/// The year is optional because catalogues do lose it, but the owner is not:
/// `\u{2117} 2001` on its own says nothing worth writing to a tag.
pub fn copyright_line(symbol: char, year: Option<&str>, owner: &str) -> Option<String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }
    Some(match year {
        Some(year) => format!("{symbol} {year} {owner}"),
        None => format!("{symbol} {owner}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_punctuation_and_spacing() {
        assert_eq!(normalize("  Sgt. Pepper's  Lonely  "), "sgt peppers lonely");
        assert_eq!(normalize("Sgt Peppers Lonely"), "sgt peppers lonely");
        assert_eq!(normalize("Don\u{2019}t Stop"), "dont stop");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("()"), "");
    }

    #[test]
    fn matches_editions_and_disambiguated_artists() {
        assert!(matches_name(
            Some("Random Access Memories (Deluxe Edition)"),
            "Random Access Memories"
        ));
        // Discogs disambiguates artists that share a name.
        assert!(matches_name(Some("Daft Punk (2)"), "Daft Punk"));
        assert!(!matches_name(Some("Discovery"), "Homework"));
        assert!(!matches_name(None, "Discovery"));
        // A name that normalizes to nothing cannot be matched safely.
        assert!(!matches_name(Some("( )"), "( )"));
    }

    #[test]
    fn takes_the_year_off_the_front_of_a_date() {
        assert_eq!(year_of(Some("2001-03-12")).as_deref(), Some("2001"));
        assert_eq!(year_of(Some("2001")).as_deref(), Some("2001"));
        assert_eq!(year_of(Some("")), None);
        assert_eq!(year_of(Some("March 2001")), None);
        assert_eq!(year_of(None), None);
    }

    #[test]
    fn builds_a_line_only_when_an_owner_is_known() {
        assert_eq!(
            copyright_line('\u{2117}', Some("2001"), "Daft Life Limited").as_deref(),
            Some("\u{2117} 2001 Daft Life Limited")
        );
        assert_eq!(
            copyright_line('\u{a9}', None, "Daft Life Limited").as_deref(),
            Some("\u{a9} Daft Life Limited")
        );
        assert_eq!(copyright_line('\u{2117}', Some("2001"), "  "), None);
    }
}
