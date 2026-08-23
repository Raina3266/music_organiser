//! Read the `.lrc` sidecars spotDL writes.
//!
//! `--generate-lrc` leaves the timed lyrics in a `.lrc` file beside the audio.
//! That file's text is what goes into the ordinary `USLT` lyrics frame, so
//! these helpers only have to read it and guess its language; the timestamps
//! travel with the text rather than being encoded into a `SYLT` frame.

pub(crate) const LYRICS_DESCRIPTION: &str = "";
/// Detection below this confidence is ignored in favour of the configured
/// default; short or mixed-script lyrics are easy to guess wrong.
const MIN_DETECTION_LINES: usize = 2;

/// Guess the ISO-639-2 language of a `.lrc` file's lyric lines.
///
/// Spotify does not expose a track's language, so the lyrics themselves are the
/// only evidence available. The lines are the text alone: timestamps and any
/// `[ar:...]` metadata carry no language and would only mislead the detector.
/// `None` means the guess was unreliable — too little text, or a script the
/// detector could not place — and the caller should fall back to its configured
/// default rather than record something invented.
pub(crate) fn detect_language(lines: &[&str]) -> Option<String> {
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
    Some(iso_639_2(info.lang().code()).to_owned())
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
        assert_eq!(detect_language(&english).as_deref(), Some("eng"));

        let french = lyric_lines(
            "[00:01.00]Je te promets le trone d or et la lumiere\n[00:05.00]Je te promets la liberte et je te donne\n[00:09.00]Toute ma vie je te promets tout mon amour\n",
        );
        assert_eq!(detect_language(&french).as_deref(), Some("fra"));
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
