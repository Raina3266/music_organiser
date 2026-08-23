//! Parse `.lrc` sidecars and encode them as ID3 synchronised-lyrics payloads.
//!
//! spotDL never writes a SYLT frame: `--lyrics synced` only embeds the plain
//! text in USLT, and `--generate-lrc` writes the timed lyrics to a separate
//! `.lrc` file. These helpers turn that file into the bytes of a SYLT frame.

use id3::{
    Content, Frame, Version,
    frame::{SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat, Unknown},
};

pub(crate) const LYRICS_DESCRIPTION: &str = "";
/// Detection below this confidence is ignored in favour of the configured
/// default; short or mixed-script lyrics are easy to guess wrong.
const MIN_DETECTION_LINES: usize = 2;
/// SYLT text encoding `$01`: UTF-16 with a byte-order mark, which ID3v2.3
/// allows and which keeps non-Latin lyrics intact.
const SYLT_ENCODING_UTF16: u8 = 0x01;
const UTF16_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16_TERMINATOR: [u8; 2] = [0x00, 0x00];

/// Build the `SynchronisedLyrics` value for a parsed `.lrc` file.
///
/// `language` is the ISO-639-2 code recorded in the frame; `.lrc` files carry
/// no language of their own.
pub(crate) fn synchronised_lyrics(
    content: Vec<(u32, String)>,
    language: &str,
) -> SynchronisedLyrics {
    SynchronisedLyrics {
        lang: language.to_owned(),
        timestamp_format: TimestampFormat::Ms,
        content_type: SynchronisedLyricsType::Lyrics,
        description: LYRICS_DESCRIPTION.to_owned(),
        content,
    }
}

/// Build the SYLT frame from hand-encoded bytes.
///
/// The `id3` crate's own SYLT encoder appends a stray NUL byte after the last
/// timestamp. That byte is not in the ID3v2.3 specification, and strict parsers
/// — mutagen among them, which is what spotDL and much of the tooling around it
/// use — discard the whole frame because of it. Writing the payload directly
/// keeps the frame readable everywhere; the crate still supplies the frame
/// header and size.
pub(crate) fn sylt_frame(lyrics: &SynchronisedLyrics, version: Version) -> Frame {
    Frame::with_content(
        "SYLT",
        Content::Unknown(Unknown {
            data: encode_sylt(lyrics),
            version,
        }),
    )
}

fn encode_sylt(lyrics: &SynchronisedLyrics) -> Vec<u8> {
    let mut data = vec![SYLT_ENCODING_UTF16];
    data.extend(lyrics.lang.bytes().chain(std::iter::repeat(b' ')).take(3));
    data.push(match lyrics.timestamp_format {
        TimestampFormat::Mpeg => 1,
        TimestampFormat::Ms => 2,
    });
    data.push(match lyrics.content_type {
        SynchronisedLyricsType::Other => 0,
        SynchronisedLyricsType::Lyrics => 1,
        SynchronisedLyricsType::Transcription => 2,
        SynchronisedLyricsType::PartName => 3,
        SynchronisedLyricsType::Event => 4,
        SynchronisedLyricsType::Chord => 5,
        SynchronisedLyricsType::Trivia => 6,
    });
    push_utf16_string(&mut data, &lyrics.description);
    for (milliseconds, text) in &lyrics.content {
        push_utf16_string(&mut data, text);
        data.extend_from_slice(&milliseconds.to_be_bytes());
    }
    data
}

/// Append a BOM-prefixed, NUL-terminated UTF-16LE string.
fn push_utf16_string(data: &mut Vec<u8>, text: &str) {
    data.extend_from_slice(&UTF16_BOM);
    for unit in text.encode_utf16() {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data.extend_from_slice(&UTF16_TERMINATOR);
}

/// Guess the ISO-639-2 language of a parsed `.lrc` file.
///
/// Spotify does not expose a track's language, so the lyrics themselves are the
/// only evidence available. `None` means the guess was unreliable — too little
/// text, or a script the detector could not place — and the caller should fall
/// back to its configured default rather than record something invented.
pub(crate) fn detect_language(content: &[(u32, String)]) -> Option<String> {
    let text = content
        .iter()
        .map(|(_, line)| line.as_str())
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

/// Parse LRC text into `(milliseconds, line)` pairs sorted by time.
///
/// Metadata tags such as `[ar:...]` and lines without a timestamp are ignored.
/// A line may carry several timestamps, which repeats its text at each one.
pub(crate) fn parse_lrc(contents: &str) -> Vec<(u32, String)> {
    let mut entries: Vec<(u32, String)> = Vec::new();

    for line in contents.lines() {
        let mut rest = line.trim_start_matches('\u{feff}').trim();
        let mut timestamps = Vec::new();

        while let Some(after_bracket) = rest.strip_prefix('[') {
            let Some(end) = after_bracket.find(']') else {
                break;
            };
            let Some(milliseconds) = parse_timestamp(&after_bracket[..end]) else {
                break;
            };
            timestamps.push(milliseconds);
            rest = after_bracket[end + 1..].trim_start();
        }

        if timestamps.is_empty() {
            continue;
        }
        let text = rest.trim_end().to_owned();
        for milliseconds in timestamps {
            entries.push((milliseconds, text.clone()));
        }
    }

    entries.sort_by_key(|(milliseconds, _)| *milliseconds);
    entries.dedup();
    entries
}

/// Parse `mm:ss`, `mm:ss.xx`, or `hh:mm:ss.xxx` into milliseconds.
fn parse_timestamp(value: &str) -> Option<u32> {
    let value = value.trim();
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (value, None),
    };

    let parts: Vec<&str> = whole.split(':').collect();
    if !matches!(parts.len(), 2 | 3) {
        return None;
    }

    let mut total_seconds = 0u64;
    for part in &parts {
        let part = part.trim();
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        total_seconds = total_seconds
            .checked_mul(60)?
            .checked_add(part.parse::<u64>().ok()?)?;
    }

    let mut milliseconds = total_seconds.checked_mul(1000)?;
    if let Some(fraction) = fraction {
        let fraction = fraction.trim();
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let digits: String = fraction.chars().take(3).collect();
        let scale = 10u64.pow(3 - digits.len() as u32);
        milliseconds = milliseconds.checked_add(digits.parse::<u64>().ok()? * scale)?;
    }

    u32::try_from(milliseconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_timestamp_forms_spotdl_writes() {
        assert_eq!(parse_timestamp("00:12.34"), Some(12_340));
        assert_eq!(parse_timestamp("01:02"), Some(62_000));
        assert_eq!(parse_timestamp("00:00.5"), Some(500));
        assert_eq!(parse_timestamp("00:01.234"), Some(1_234));
        assert_eq!(parse_timestamp("01:02:03.500"), Some(3_723_500));
    }

    #[test]
    fn rejects_metadata_tags_and_junk() {
        assert_eq!(parse_timestamp("ar:Some Artist"), None);
        assert_eq!(parse_timestamp("offset:+500"), None);
        assert_eq!(parse_timestamp("length:3:21"), None);
        assert_eq!(parse_timestamp("12"), None);
    }

    #[test]
    fn skips_metadata_and_keeps_timed_lines_in_order() {
        let lrc = "\u{feff}[ar: Some Artist]\n[ti: Some Song]\n[00:12.00] second line\n[00:01.50]first line\nno timestamp at all\n";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                (1_500, "first line".to_owned()),
                (12_000, "second line".to_owned()),
            ]
        );
    }

    #[test]
    fn repeats_text_for_every_timestamp_on_a_line() {
        let lrc = "[00:10.00][00:40.00]chorus\n[00:20.00]\n";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                (10_000, "chorus".to_owned()),
                (20_000, String::new()),
                (40_000, "chorus".to_owned()),
            ]
        );
    }

    #[test]
    fn a_metadata_only_file_produces_nothing() {
        assert!(parse_lrc("[ar: Artist]\n[al: Album]\n\n").is_empty());
    }

    #[test]
    fn detects_the_language_of_the_lyrics() {
        let english = parse_lrc(
            "[00:01.00]We are the champions my friends\n[00:05.00]And we will keep on fighting till the end\n[00:09.00]We are the champions of the world\n",
        );
        assert_eq!(detect_language(&english).as_deref(), Some("eng"));

        let french = parse_lrc(
            "[00:01.00]Je te promets le trone d or et la lumiere\n[00:05.00]Je te promets la liberte et je te donne\n[00:09.00]Toute ma vie je te promets tout mon amour\n",
        );
        assert_eq!(detect_language(&french).as_deref(), Some("fra"));
    }

    #[test]
    fn refuses_to_guess_from_too_little_text() {
        assert_eq!(detect_language(&[]), None);
        assert_eq!(detect_language(&[(0, "oh".to_owned())]), None);
        // Blank interlude markers are not evidence of anything.
        assert_eq!(
            detect_language(&[(0, String::new()), (1_000, "  ".to_owned())]),
            None
        );
    }

    #[test]
    fn maps_macrolanguages_onto_iso_639_2() {
        assert_eq!(iso_639_2("cmn"), "zho");
        assert_eq!(iso_639_2("pes"), "fas");
        assert_eq!(iso_639_2("jpn"), "jpn");
        assert_eq!(iso_639_2("deu"), "deu");
    }

    #[test]
    fn encodes_a_spec_shaped_sylt_payload_without_a_trailing_nul() {
        let data = encode_sylt(&synchronised_lyrics(vec![(1_000, "hi".to_owned())], "eng"));

        let expected = [
            &[0x01][..],
            b"eng",
            &[0x02, 0x01],
            // empty description: BOM then terminator
            &[0xFF, 0xFE, 0x00, 0x00],
            // "hi" as BOM + UTF-16LE + terminator, then the big-endian timestamp
            &[0xFF, 0xFE, b'h', 0x00, b'i', 0x00, 0x00, 0x00],
            &1_000u32.to_be_bytes(),
        ]
        .concat();
        assert_eq!(data, expected);
        assert_ne!(
            data.last(),
            Some(&0u8),
            "a trailing NUL makes strict parsers drop the frame"
        );
    }
}
