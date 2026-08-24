//! Deciding whether a search result is the release that was asked for.
//!
//! A wrong copyright is worse than none, so every source uses the same rule:
//! a candidate counts only when both its artist and its release name match.
//! The comparison has to survive the differences between catalogues — case,
//! punctuation, accents, `&` against `and`, and the edition suffix one
//! catalogue prints and another does not — without ever conceding that two
//! genuinely different releases are the same one.
//!
//! The difference is not decoration. `Earth, Wind & Fire` and
//! `Earth, Wind & Fire Vol. 1` differ by a suffix; so do `I Am` and
//! `I Am Sasha Fierce`. Treating a trailing word as an edition marker, which
//! is what a plain prefix test does, quietly hands the second release's
//! copyright to the first. So a suffix is ignorable only when it says so:
//! bracketed, or spelled out of a small vocabulary of edition words.

/// How well a candidate name matches, best first.
///
/// The order matters: a search returns several candidates and the closest one
/// should win, so that a plain `Discovery` is preferred over a
/// `Discovery (Live)` that happens to be listed first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Confidence {
    /// The same name once case, punctuation, and accents are set aside.
    Exact,
    /// The same release once an edition suffix is set aside — a
    /// `(Deluxe Edition)`, a `[Platinum Edition]`, a Discogs `(2)`.
    Edition,
}

/// Whether a piece of corroborating evidence agrees, is missing, or conflicts.
///
/// Three states rather than a numeric distance, because the distances are not
/// comparable: being two tracks out and being two years out say different
/// things, and inventing a scale to weigh them against each other would be
/// making up precision that is not there. Missing evidence sits between
/// agreement and conflict, so a candidate that can be checked and agrees beats
/// one that cannot be checked, which in turn beats one that disagrees.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Agreement {
    Agrees,
    Unknown,
    Differs,
}

/// How well a candidate release matches everything known about the wanted one.
///
/// Ordered best first, field by field: the release name decides, then the
/// artist, then the corroborating evidence. Track count comes before year
/// because it is the sharper instrument — `Discovery` has fourteen tracks and
/// `Discovery (Deluxe Edition)` has twenty, while both were released the same
/// year.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Score {
    pub name: Confidence,
    pub artist: Confidence,
    pub tracks: Agreement,
    pub year: Agreement,
}

impl Score {
    /// Score a candidate whose name and artist have already been matched.
    pub fn new(
        name: Confidence,
        artist: Confidence,
        candidate_tracks: Option<u32>,
        wanted_tracks: Option<u32>,
        candidate_year: Option<&str>,
        wanted_year: Option<&str>,
    ) -> Self {
        Self {
            name,
            artist,
            tracks: agreement(candidate_tracks, wanted_tracks, |a, b| a == b),
            year: agreement(year_of(candidate_year), year_of(wanted_year), years_agree),
        }
    }
}

fn agreement<T>(
    candidate: Option<T>,
    wanted: Option<T>,
    same: impl Fn(&T, &T) -> bool,
) -> Agreement {
    match (candidate, wanted) {
        (Some(candidate), Some(wanted)) if same(&candidate, &wanted) => Agreement::Agrees,
        (Some(_), Some(_)) => Agreement::Differs,
        _ => Agreement::Unknown,
    }
}

/// Whether two release years are close enough to be the same release.
///
/// A year of slack, because a release crosses new year between storefronts and
/// catalogues disagree about which side it landed on.
fn years_agree(candidate: &String, wanted: &String) -> bool {
    match (candidate.parse::<i32>(), wanted.parse::<i32>()) {
        (Ok(candidate), Ok(wanted)) => (candidate - wanted).abs() <= 1,
        _ => candidate == wanted,
    }
}

/// Words that mark an edition of a release rather than a different release.
///
/// `feat` is here because a featured-artist credit annotates a release without
/// changing it. Deliberately absent: `vol`, `volume`, `part`, `pt`, `disc`,
/// and the like — those distinguish `Vol. 1` from `Vol. 2`, and dropping them
/// would merge two different records.
const EDITION_WORDS: &[&str] = &[
    "deluxe",
    "edition",
    "expanded",
    "remaster",
    "remastered",
    "anniversary",
    "special",
    "bonus",
    "explicit",
    "clean",
    "reissue",
    "platinum",
    "gold",
    "standard",
    "original",
    "version",
    "feat",
    "featuring",
    "ft",
];

/// Words that mark a genuinely different recording, whatever else surrounds
/// them.
///
/// A `(Deluxe Version)` is the same music; an `(Acoustic Version)` is not, and
/// carries its own copyright line. These veto a suffix that would otherwise
/// look ignorable.
const DIFFERENT_RECORDING_WORDS: &[&str] = &[
    "live",
    "remix",
    "remixes",
    "acoustic",
    "instrumental",
    "demo",
    "karaoke",
    "cover",
    "sped",
    "slowed",
    "reverb",
    "radio",
    "mix",
];

/// How well a candidate name matches the expected one, or `None` when they are
/// different releases.
pub fn confidence(candidate: Option<&str>, expected: &str) -> Option<Confidence> {
    let candidate = candidate?;
    let (candidate_full, expected_full) = (normalize(candidate), normalize(expected));
    if candidate_full.is_empty() || expected_full.is_empty() {
        return None;
    }
    if candidate_full == expected_full {
        return Some(Confidence::Exact);
    }
    (base(candidate) == base(expected)).then_some(Confidence::Edition)
}

/// Whether a candidate name means the same release as the expected one.
pub fn matches_name(candidate: Option<&str>, expected: &str) -> bool {
    confidence(candidate, expected).is_some()
}

/// The name with any ignorable edition suffix removed.
///
/// Empty when nothing survives, which never matches anything: a release whose
/// whole name is an edition word cannot be told apart from another.
fn base(value: &str) -> String {
    let without_groups = strip_ignorable_groups(value);
    let normalized = normalize(&without_groups);
    let trimmed = strip_trailing_edition_words(&normalized);
    strip_leading_article(&trimmed)
}

/// Remove bracketed groups that only qualify the release.
///
/// `Random Access Memories (Deluxe Edition)` loses its suffix;
/// `Daft Punk (2)`, the way Discogs disambiguates two artists of one name,
/// loses its number. `Discovery (Live)` keeps everything, because that is a
/// different recording, and a group carrying real title text —
/// `The Lion King: The Gift` — is never dropped either.
fn strip_ignorable_groups(value: &str) -> String {
    let mut kept = String::with_capacity(value.len());
    let mut group = String::new();
    let mut depth = 0usize;

    for character in value.chars() {
        match character {
            '(' | '[' => {
                depth += 1;
                if depth == 1 {
                    group.clear();
                    continue;
                }
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if !is_ignorable_group(&group) {
                        kept.push(' ');
                        kept.push_str(&group);
                    }
                    continue;
                }
            }
            _ => {}
        }
        if depth == 0 {
            kept.push(character);
        } else {
            group.push(character);
        }
    }
    // An unclosed bracket means the name was never structured; keep the text.
    if depth > 0 {
        kept.push(' ');
        kept.push_str(&group);
    }
    kept
}

/// Whether a bracketed group can be dropped without changing which release is
/// meant.
fn is_ignorable_group(group: &str) -> bool {
    let normalized = normalize(group);
    if normalized.is_empty() {
        return true;
    }
    let words: Vec<&str> = normalized.split(' ').collect();
    if words.iter().any(|word| is(word, DIFFERENT_RECORDING_WORDS)) {
        return false;
    }
    // A bare number is Discogs disambiguating two artists who share a name.
    if words
        .iter()
        .all(|word| word.chars().all(|c| c.is_numeric()))
    {
        return true;
    }
    words.iter().any(|word| is(word, EDITION_WORDS))
}

/// Remove a trailing run of edition words that were never bracketed.
///
/// Catalogues do print them bare: Spotify lists `B'Day Deluxe Edition`, not
/// `B'Day (Deluxe Edition)`. The run must reach the end of the name and must
/// leave something behind.
fn strip_trailing_edition_words(normalized: &str) -> String {
    let words: Vec<&str> = normalized.split(' ').filter(|w| !w.is_empty()).collect();
    let mut end = words.len();
    while end > 0 && is(words[end - 1], EDITION_WORDS) {
        end -= 1;
    }
    if end == 0 || end == words.len() {
        return normalized.to_owned();
    }
    if words[..end]
        .iter()
        .any(|word| is(word, DIFFERENT_RECORDING_WORDS))
    {
        return normalized.to_owned();
    }
    words[..end].join(" ")
}

/// Drop a leading `the`, which catalogues disagree about constantly.
fn strip_leading_article(normalized: &str) -> String {
    match normalized.strip_prefix("the ") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => normalized.to_owned(),
    }
}

fn is(word: &str, vocabulary: &[&str]) -> bool {
    vocabulary.contains(&word)
}

/// Reduce a name to the letters and digits that carry its identity.
///
/// Case, punctuation, and spacing go. Accents are folded, so a tag written
/// `Beyonce` still finds Spotify's `Beyoncé`. `&` becomes `and`, because one
/// catalogue prints `Earth, Wind & Fire` and the next prints it spelled out,
/// and dropping the symbol entirely would make those two disagree.
pub fn normalize(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        // Apostrophes vanish rather than splitting a word, so that "Pepper's"
        // still matches a store listing spelled "Peppers".
        if matches!(character, '\'' | '\u{2019}' | '\u{02bc}') {
            continue;
        }
        if character == '&' {
            push_word(&mut normalized, &mut pending_space, "and");
            continue;
        }
        if character.is_alphanumeric() {
            let folded = fold(character);
            push_word(&mut normalized, &mut pending_space, &folded);
        } else {
            pending_space = true;
        }
    }
    normalized
}

fn push_word(normalized: &mut String, pending_space: &mut bool, text: &str) {
    if *pending_space && !normalized.is_empty() {
        normalized.push(' ');
    }
    *pending_space = false;
    normalized.push_str(text);
}

/// One character, lowercased and stripped of its accent.
///
/// Only the Latin letters a catalogue actually differs on are folded. Anything
/// outside that — Hangul, kana, Han — is left exactly as it is, since those
/// scripts have no accent variants to reconcile and folding them would be
/// meaningless at best.
fn fold(character: char) -> String {
    let lowered: String = character.to_lowercase().collect();
    let mut folded = String::with_capacity(lowered.len());
    for character in lowered.chars() {
        folded.push_str(match character {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
            'ð' | 'ď' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'į' | 'ı' => "i",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'œ' => "oe",
            'ŕ' | 'ř' => "r",
            'ś' | 'š' | 'ş' | 'ŝ' => "s",
            'ß' => "ss",
            'ţ' | 'ť' | 'ŧ' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'þ' => "th",
            other => {
                folded.push(other);
                continue;
            }
        });
    }
    folded
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

    fn rank(candidate: &str, expected: &str) -> Option<Confidence> {
        confidence(Some(candidate), expected)
    }

    #[test]
    fn normalizes_case_punctuation_and_spacing() {
        assert_eq!(normalize("  Sgt. Pepper's  Lonely  "), "sgt peppers lonely");
        assert_eq!(normalize("Sgt Peppers Lonely"), "sgt peppers lonely");
        assert_eq!(normalize("Don\u{2019}t Stop"), "dont stop");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("()"), "");
    }

    /// Real catalogue spellings: Spotify lists `Earth, Wind & Fire`, other
    /// catalogues spell the ampersand out, and dropping it entirely would make
    /// the two disagree.
    #[test]
    fn reconciles_ampersands_and_accents() {
        assert_eq!(
            rank("Earth, Wind & Fire", "Earth, Wind and Fire"),
            Some(Confidence::Exact)
        );
        assert_eq!(rank("Beyonc\u{e9}", "Beyonce"), Some(Confidence::Exact));
        assert_eq!(rank("Sigur R\u{f3}s", "Sigur Ros"), Some(Confidence::Exact));
        assert_eq!(rank("Bj\u{f6}rk", "Bjork"), Some(Confidence::Exact));
        assert_eq!(rank("JA\u{178}-Z", "JAY-Z"), Some(Confidence::Exact));
        // Scripts with no accent variants are left alone and still match.
        assert_eq!(rank("DAWN 던", "DAWN 던"), Some(Confidence::Exact));
    }

    /// The bug this replaced: a plain prefix test treats any trailing word as
    /// an edition marker, so a different release quietly supplies its
    /// copyright. Every pair here is two genuinely different records.
    #[test]
    fn refuses_a_longer_name_that_is_a_different_release() {
        assert_eq!(
            rank("Earth, Wind & Fire Vol. 1", "Earth, Wind & Fire"),
            None
        );
        assert_eq!(rank("I Am Sasha Fierce", "I Am"), None);
        assert_eq!(rank("Discovery Zone", "Discovery"), None);
        assert_eq!(
            rank(
                "The Best Of Earth, Wind & Fire Vol. 1",
                "Earth, Wind & Fire"
            ),
            None
        );
        // Volumes and parts are different records, bracketed or not.
        assert_eq!(rank("Greatest Hits (Vol. 2)", "Greatest Hits"), None);
        assert_eq!(rank("NCT RESONANCE, Pt. 2", "NCT RESONANCE"), None);
    }

    /// Edition suffixes still have to work, in every spelling a catalogue uses.
    #[test]
    fn accepts_an_edition_suffix_however_it_is_spelled() {
        for candidate in [
            "Random Access Memories (Deluxe Edition)",
            "Random Access Memories [Deluxe Edition]",
            "Random Access Memories Deluxe Edition",
            "Random Access Memories (Remastered)",
            "Random Access Memories (10th Anniversary Edition)",
        ] {
            assert_eq!(
                rank(candidate, "Random Access Memories"),
                Some(Confidence::Edition),
                "{candidate}"
            );
        }
        // Discogs disambiguates artists sharing a name with a bare number.
        assert_eq!(
            rank("Daft Punk (2)", "Daft Punk"),
            Some(Confidence::Edition)
        );
        // A featured credit annotates a release without changing it.
        assert_eq!(
            rank(
                "Feels (feat. Pharrell Williams, Katy Perry & Big Sean)",
                "Feels"
            ),
            Some(Confidence::Edition)
        );
    }

    /// A different recording carries its own copyright, so a suffix naming one
    /// is never ignorable — not even next to an edition word.
    #[test]
    fn refuses_a_suffix_that_names_a_different_recording() {
        assert_eq!(rank("Discovery (Live)", "Discovery"), None);
        assert_eq!(rank("Discovery (Acoustic Version)", "Discovery"), None);
        assert_eq!(rank("Anxiety (Sped Up)", "Anxiety"), None);
        assert_eq!(rank("Feels (Radio Edit)", "Feels"), None);
        assert_eq!(rank("Discovery (Remix)", "Discovery"), None);
    }

    /// An exact match must beat an edition match, so a search returning both
    /// takes the plain release rather than whichever came first.
    #[test]
    fn an_exact_name_outranks_an_edition() {
        assert!(Confidence::Exact < Confidence::Edition);
        assert_eq!(rank("Discovery", "Discovery"), Some(Confidence::Exact));
        assert_eq!(
            rank("Discovery (Deluxe Edition)", "Discovery"),
            Some(Confidence::Edition)
        );
    }

    #[test]
    fn ignores_a_leading_article_the_catalogues_disagree_about() {
        assert_eq!(rank("The Beatles", "Beatles"), Some(Confidence::Edition));
        // Something must survive: a release actually called "The" is not a
        // match for everything.
        assert_eq!(rank("The", "Discovery"), None);
    }

    /// Title text inside brackets is part of the name, not a qualifier.
    #[test]
    fn keeps_a_bracketed_group_that_carries_real_title_text() {
        assert_eq!(
            rank(
                "The Lion King: The Gift [Deluxe Edition]",
                "The Lion King: The Gift"
            ),
            Some(Confidence::Edition)
        );
        assert_eq!(
            rank("JENNIE Special Single [You & Me]", "JENNIE Special Single"),
            None
        );
    }

    #[test]
    fn a_name_that_normalizes_to_nothing_never_matches() {
        assert_eq!(rank("( )", "( )"), None);
        assert_eq!(confidence(None, "Discovery"), None);
        // Nor does one that is nothing but an edition word.
        assert_eq!(rank("Deluxe Edition", "Discovery"), None);
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

    #[test]
    fn evidence_ranks_agreement_above_ignorance_above_conflict() {
        assert!(Agreement::Agrees < Agreement::Unknown);
        assert!(Agreement::Unknown < Agreement::Differs);
    }

    #[test]
    fn the_release_name_outweighs_the_corroborating_evidence() {
        let exact_but_unverified =
            Score::new(Confidence::Exact, Confidence::Exact, None, None, None, None);
        let edition_with_everything_agreeing = Score::new(
            Confidence::Edition,
            Confidence::Exact,
            Some(14),
            Some(14),
            Some("2001"),
            Some("2001"),
        );
        // A name that matches exactly wins even when a deluxe edition's track
        // count and year both agree: the name identifies the release.
        assert!(exact_but_unverified < edition_with_everything_agreeing);
    }

    #[test]
    fn the_track_count_decides_between_two_identical_names() {
        let agrees = Score::new(
            Confidence::Exact,
            Confidence::Exact,
            Some(14),
            Some(14),
            None,
            None,
        );
        let differs = Score::new(
            Confidence::Exact,
            Confidence::Exact,
            Some(20),
            Some(14),
            None,
            None,
        );
        assert!(agrees < differs);
    }

    /// Catalogues put a release either side of new year, so a year of slack
    /// counts as agreement and anything more does not.
    #[test]
    fn a_year_of_slack_still_counts_as_the_same_release() {
        let near = Score::new(
            Confidence::Exact,
            Confidence::Exact,
            None,
            None,
            Some("2002-01-04"),
            Some("2001"),
        );
        assert_eq!(near.year, Agreement::Agrees);
        let far = Score::new(
            Confidence::Exact,
            Confidence::Exact,
            None,
            None,
            Some("2021"),
            Some("2001"),
        );
        assert_eq!(far.year, Agreement::Differs);
    }
}
