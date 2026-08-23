//! Read the `.lrc` sidecars spotDL writes.
//!
//! `--generate-lrc` leaves the timed lyrics in a `.lrc` file beside the audio.
//! That file's text is what goes into the ordinary `USLT` lyrics frame, so
//! these helpers only have to read it and guess its language; the timestamps
//! travel with the text rather than being encoded into a `SYLT` frame.

use whatlang::Lang;

pub(crate) const LYRICS_DESCRIPTION: &str = "";
/// Detection below this confidence is ignored in favour of the configured
/// default; short or mixed-script lyrics are easy to guess wrong.
const MIN_DETECTION_LINES: usize = 2;

/// A language in both forms an ID3v2.3 tag needs.
///
/// `TLAN` carries the readable name, which is what a tagger shows. The
/// three-byte language field inside a `USLT` frame cannot: the frame format
/// fixes its width, so it keeps the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// ISO-639-2 code, such as `eng`.
    pub code: String,
    /// English name, such as `English`.
    pub name: String,
}

/// Read a language from an English name or an ISO-639-2/639-3 code.
///
/// Both are accepted because the tag now shows names while the older option
/// values, and the codes inside the frames themselves, are three letters.
pub fn parse_language(value: &str) -> Option<Language> {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    if let Some(lang) = Lang::from_code(value.as_str()) {
        return Some(language_of(lang));
    }
    // The collective codes this program writes for the two macrolanguages, and
    // the bibliographic spellings of the same two, are not detector codes.
    match value.as_str() {
        "zho" | "chi" => return Some(language_of(Lang::Cmn)),
        "fas" | "per" => return Some(language_of(Lang::Pes)),
        _ => {}
    }
    Lang::all()
        .iter()
        .copied()
        .find(|lang| {
            language_of(*lang).name.to_lowercase() == value
                || lang.eng_name().to_lowercase() == value
        })
        .map(language_of)
}

/// Both forms of one detected language.
fn language_of(lang: Lang) -> Language {
    Language {
        code: iso_639_2(lang.code()).to_owned(),
        name: english_name(lang).to_owned(),
    }
}

/// The name to store in `TLAN`.
///
/// The detector names the two macrolanguages by their dominant variety, which
/// is more precise than a music tag wants; the collective name matches the
/// collective code `iso_639_2` already returns for them.
fn english_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Cmn => "Chinese",
        Lang::Pes => "Persian",
        other => other.eng_name(),
    }
}

/// Guess the language of a `.lrc` file's lyric lines.
///
/// Spotify does not expose a track's language, so the lyrics themselves are the
/// only evidence available. The lines are the text alone: timestamps and any
/// `[ar:...]` metadata carry no language and would only mislead the detector.
/// `None` means the guess was unreliable — too little text, or a script the
/// detector could not place — and the caller should fall back to its configured
/// default rather than record something invented.
pub(crate) fn detect_language(lines: &[&str]) -> Option<Language> {
    let text = lines
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if text.len() < MIN_DETECTION_LINES {
        return None;
    }

    let info = whatlang::detect(&text.join("\n"))?;
    if !info.is_reliable() {
        return None;
    }
    Some(language_of(info.lang()))
}

/// Translate the detector's ISO-639-3 code into one ID3v2.3 accepts.
///
/// ID3v2.3 asks for ISO-639-2, and the terminological (`639-2/T`) codes are
/// identical to ISO-639-3 for every individual language the detector reports.
/// Only the macrolanguages need mapping onto their collective code.
fn iso_639_2(code: &str) -> &str {
    match code {
        // Mandarin -> Chinese
        "cmn" => "zho",
        // Western Persian -> Persian
        "pes" => "fas",
        other => other,
    }
}

/// The lyric text of a `.lrc` file, without timestamps or `[ar:...]` metadata.
///
/// Only the language detector needs the text on its own; the frame itself keeps
/// the file verbatim, timestamps included. A file whose lines carry no
/// timestamp is not a special case — stripping finds nothing to remove.
pub(crate) fn lyric_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(|line| strip_leading_brackets(line).trim())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Drop the leading `[...]` groups an LRC line begins with.
///
/// That covers both the `[mm:ss.xx]` timestamps a line may carry several of and
/// the `[ar:...]`-style metadata header, which leaves nothing behind.
fn strip_leading_brackets(line: &str) -> &str {
    let mut rest = line.trim_start_matches('\u{feff}').trim();
    while let Some(after_bracket) = rest.strip_prefix('[') {
        let Some(end) = after_bracket.find(']') else {
            break;
        };
        rest = after_bracket[end + 1..].trim_start();
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_timestamps_and_metadata_from_the_lyric_lines() {
        let lrc = "\u{feff}[ar: Some Artist]\n[ti: Some Song]\n[00:01.50]first line\n[00:12.00] second line\n\n";
        assert_eq!(lyric_lines(lrc), vec!["first line", "second line"]);
    }

    #[test]
    fn keeps_a_line_carrying_several_timestamps_once() {
        assert_eq!(
            lyric_lines("[00:10.00][00:40.00]chorus\n[00:20.00]\n"),
            vec!["chorus"]
        );
    }

    #[test]
    fn an_untimed_file_is_read_as_plain_lines() {
        assert_eq!(
            lyric_lines("first line\n\nsecond line\n"),
            vec!["first line", "second line"]
        );
    }

    #[test]
    fn a_metadata_only_file_produces_nothing() {
        assert!(lyric_lines("[ar: Artist]\n[al: Album]\n\n").is_empty());
    }

    #[test]
    fn detects_the_language_of_the_lyrics() {
        let english = lyric_lines(
            "[00:01.00]We are the champions my friends\n[00:05.00]And we will keep on fighting till the end\n[00:09.00]We are the champions of the world\n",
        );
        let detected = detect_language(&english).unwrap();
        assert_eq!(detected.name, "English");
        assert_eq!(detected.code, "eng");

        let french = lyric_lines(
            "[00:01.00]Je te promets le trone d or et la lumiere\n[00:05.00]Je te promets la liberte et je te donne\n[00:09.00]Toute ma vie je te promets tout mon amour\n",
        );
        let detected = detect_language(&french).unwrap();
        assert_eq!(detected.name, "French");
        assert_eq!(detected.code, "fra");
    }

    #[test]
    fn reads_a_language_from_a_name_or_a_code() {
        for value in ["English", "english", "ENG", "eng"] {
            let language = parse_language(value).unwrap();
            assert_eq!(language.name, "English");
            assert_eq!(language.code, "eng");
        }

        assert_eq!(parse_language("Korean").unwrap().code, "kor");
        assert_eq!(parse_language("kor").unwrap().name, "Korean");
        assert_eq!(parse_language("Spanish").unwrap().code, "spa");
        assert_eq!(parse_language(" Japanese ").unwrap().code, "jpn");

        assert!(parse_language("").is_none());
        assert!(parse_language("Klingon").is_none());
        assert!(parse_language("e1g").is_none());
    }

    #[test]
    fn the_two_macrolanguages_keep_their_collective_name_and_code() {
        for value in ["Chinese", "zho", "chi", "cmn", "Mandarin"] {
            let language = parse_language(value).unwrap();
            assert_eq!(language.name, "Chinese");
            assert_eq!(language.code, "zho");
        }
        for value in ["Persian", "fas", "per", "pes"] {
            let language = parse_language(value).unwrap();
            assert_eq!(language.name, "Persian");
            assert_eq!(language.code, "fas");
        }
    }

    #[test]
    fn refuses_to_guess_from_too_little_text() {
        assert_eq!(detect_language(&[]), None);
        assert_eq!(detect_language(&["oh"]), None);
        // Blank interlude markers are not evidence of anything.
        assert_eq!(detect_language(&["", "  "]), None);
    }

    #[test]
    fn maps_macrolanguages_onto_iso_639_2() {
        assert_eq!(iso_639_2("cmn"), "zho");
        assert_eq!(iso_639_2("pes"), "fas");
        assert_eq!(iso_639_2("jpn"), "jpn");
        assert_eq!(iso_639_2("deu"), "deu");
    }
}
