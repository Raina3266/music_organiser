//! Move spotDL's `.lrc` sidecars into ID3v2.3 synchronised-lyrics (SYLT) frames.
//!
//! spotDL never writes a SYLT frame: `--lyrics synced` only embeds the plain
//! text in USLT, and `--generate-lrc` writes the timed lyrics to a separate
//! `.lrc` file. This module parses those sidecars, stores them as a real SYLT
//! frame with millisecond timestamps, drops the now-redundant USLT frame, and
//! deletes the sidecar only once the frame has been read back from disk.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use id3::{
    Content, ErrorKind, Frame, Tag, TagLike, Version,
    frame::{SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat, Unknown},
};

use crate::files::{FileError, lyrics_files_recursively, sibling_music_file, write_tag_safely};

/// ISO-639-2 language recorded in the SYLT frame. `.lrc` files carry no
/// language of their own, and the frame requires three letters.
const LYRICS_LANGUAGE: &str = "eng";
const LYRICS_DESCRIPTION: &str = "";
/// Frames are written as ID3v2.3, matching the rest of this project.
const TAG_VERSION: Version = Version::Id3v23;
/// SYLT text encoding `$01`: UTF-16 with a byte-order mark, which ID3v2.3
/// allows and which keeps non-Latin lyrics intact.
const SYLT_ENCODING_UTF16: u8 = 0x01;
const UTF16_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16_TERMINATOR: [u8; 2] = [0x00, 0x00];

#[derive(Debug, Default, Eq, PartialEq)]
pub struct LyricsReport {
    /// `.lrc` files considered during this pass.
    pub files_scanned: usize,
    /// Sidecars whose lyrics were verified in a SYLT frame and then deleted.
    pub files_embedded: usize,
    /// Lyric lines written across all embedded files.
    pub lines_embedded: usize,
    /// USLT frames removed after their synced replacement was verified.
    pub uslt_frames_removed: usize,
    /// Sidecars kept on disk because post-processing failed, and why.
    pub failures: Vec<FileError>,
}

impl LyricsReport {
    pub(crate) fn absorb(&mut self, other: LyricsReport) {
        self.files_scanned += other.files_scanned;
        self.files_embedded += other.files_embedded;
        self.lines_embedded += other.lines_embedded;
        self.uslt_frames_removed += other.uslt_frames_removed;
        self.failures.extend(other.failures);
    }

    fn fail(&mut self, path: PathBuf, message: impl Into<String>) {
        self.failures.push(FileError {
            path,
            message: message.into(),
        });
    }
}

/// Embed every `.lrc` file below `root` into its audio file, then delete it.
///
/// Sidecars listed in `skip` are left untouched, which keeps a failure from
/// being re-reported against every later download. A sidecar is deleted only
/// after its SYLT frame has been read back from the audio file, so a failure
/// never destroys the only copy of the lyrics.
pub fn embed_synced_lyrics(root: &Path, skip: &HashSet<PathBuf>) -> Result<LyricsReport, String> {
    let files = lyrics_files_recursively(root)
        .map_err(|error| format!("cannot scan {} for .lrc files: {error}", root.display()))?;

    let mut report = LyricsReport::default();
    for path in files {
        if skip.contains(&path) {
            continue;
        }
        report.files_scanned += 1;
        report.absorb(embed_one(&path));
    }
    Ok(report)
}

fn embed_one(path: &Path) -> LyricsReport {
    let mut report = LyricsReport::default();

    let contents = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            report.fail(
                path.to_path_buf(),
                format!("cannot read the .lrc file: {error}"),
            );
            return report;
        }
    };

    let lines = parse_lrc(&contents);
    if lines.is_empty() {
        report.fail(
            path.to_path_buf(),
            "the .lrc file contains no timestamped line, so no synchronised lyrics could be built",
        );
        return report;
    }

    let Some(audio) = sibling_music_file(path) else {
        report.fail(
            path.to_path_buf(),
            "no audio file sits next to this .lrc file",
        );
        return report;
    };

    let mut tag = match Tag::read_from_path(&audio) {
        Ok(tag) => tag,
        Err(error) if matches!(error.kind, ErrorKind::NoTag) => Tag::new(),
        Err(error) => {
            report.fail(
                path.to_path_buf(),
                format!("cannot read the ID3 tag of {}: {error}", audio.display()),
            );
            return report;
        }
    };

    let lyrics = SynchronisedLyrics {
        lang: LYRICS_LANGUAGE.to_owned(),
        timestamp_format: TimestampFormat::Ms,
        content_type: SynchronisedLyricsType::Lyrics,
        description: LYRICS_DESCRIPTION.to_owned(),
        content: lines,
    };
    let embedded_lines = lyrics.content.len();
    let uslt_removed = tag.lyrics().count();

    tag.remove_all_synchronised_lyrics();
    tag.remove_all_lyrics();
    tag.add_frame(sylt_frame(&lyrics));

    if let Err(error) = write_tag_safely(&audio, &tag, TAG_VERSION) {
        report.fail(
            path.to_path_buf(),
            format!(
                "cannot write the SYLT frame to {}: {error}",
                audio.display()
            ),
        );
        return report;
    }

    if let Err(reason) = verify_embedded(&audio, &lyrics) {
        report.fail(path.to_path_buf(), reason);
        return report;
    }

    if let Err(error) = fs::remove_file(path) {
        report.fail(
            path.to_path_buf(),
            format!("the SYLT frame was verified but the .lrc file could not be deleted: {error}"),
        );
        return report;
    }

    report.files_embedded = 1;
    report.lines_embedded = embedded_lines;
    report.uslt_frames_removed = uslt_removed;
    report
}

/// Build the SYLT frame from hand-encoded bytes.
///
/// The `id3` crate's own SYLT encoder appends a stray NUL byte after the last
/// timestamp. That byte is not in the ID3v2.3 specification, and strict parsers
/// — mutagen among them, which is what spotDL and much of the tooling around it
/// use — discard the whole frame because of it. Writing the payload directly
/// keeps the frame readable everywhere; the crate still supplies the frame
/// header and size.
fn sylt_frame(lyrics: &SynchronisedLyrics) -> Frame {
    Frame::with_content(
        "SYLT",
        Content::Unknown(Unknown {
            data: encode_sylt(lyrics),
            version: TAG_VERSION,
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

/// Read `audio` back from disk and confirm it now carries exactly `expected`.
fn verify_embedded(audio: &Path, expected: &SynchronisedLyrics) -> Result<(), String> {
    let tag = Tag::read_from_path(audio)
        .map_err(|error| format!("cannot re-read {} to verify SYLT: {error}", audio.display()))?;

    let stored: Vec<&SynchronisedLyrics> = tag.synchronised_lyrics().collect();
    let [stored] = stored[..] else {
        return Err(format!(
            "{} holds {} SYLT frame(s) after writing instead of exactly one",
            audio.display(),
            stored.len()
        ));
    };
    if stored.timestamp_format != expected.timestamp_format
        || stored.content_type != expected.content_type
        || stored.content != expected.content
    {
        return Err(format!(
            "the SYLT frame read back from {} does not match the .lrc file",
            audio.display()
        ));
    }
    if tag.lyrics().next().is_some() {
        return Err(format!(
            "the unsynchronised USLT frame is still present in {}",
            audio.display()
        ));
    }
    Ok(())
}

/// Parse LRC text into `(milliseconds, line)` pairs sorted by time.
///
/// Metadata tags such as `[ar:...]` and lines without a timestamp are ignored.
/// A line may carry several timestamps, which repeats its text at each one.
fn parse_lrc(contents: &str) -> Vec<(u32, String)> {
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
    use std::env;

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
    fn encodes_a_spec_shaped_sylt_payload_without_a_trailing_nul() {
        let data = encode_sylt(&SynchronisedLyrics {
            lang: "eng".to_owned(),
            timestamp_format: TimestampFormat::Ms,
            content_type: SynchronisedLyricsType::Lyrics,
            description: String::new(),
            content: vec![(1_000, "hi".to_owned())],
        });

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

    #[test]
    fn non_latin_lyrics_survive_the_round_trip() {
        let directory = TempDir::new("utf16");
        let audio = directory.0.join("Track.mp3");
        let sidecar = directory.0.join("Track.lrc");
        fs::write(&audio, b"").unwrap();
        fs::write(
            &sidecar,
            "[00:03.00]\u{4f60}\u{597d}\n[00:06.00]caf\u{e9} \u{1f3b5}\n",
        )
        .unwrap();

        let report = embed_synced_lyrics(&directory.0, &HashSet::new()).unwrap();
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.files_embedded, 1);

        let tag = Tag::read_from_path(&audio).unwrap();
        let stored = tag.synchronised_lyrics().next().unwrap();
        assert_eq!(
            stored.content,
            vec![
                (3_000, "\u{4f60}\u{597d}".to_owned()),
                (6_000, "caf\u{e9} \u{1f3b5}".to_owned()),
            ]
        );
        assert_eq!(stored.lang, "eng");
    }

    #[test]
    fn a_metadata_only_file_produces_nothing() {
        assert!(parse_lrc("[ar: Artist]\n[al: Album]\n\n").is_empty());
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                env::temp_dir().join(format!("music-tag-transfer-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_audio_with_uslt(path: &Path) {
        fs::write(path, b"").unwrap();
        let mut tag = Tag::new();
        tag.add_frame(id3::frame::Lyrics {
            lang: "eng".to_owned(),
            description: String::new(),
            text: "plain unsynced text".to_owned(),
        });
        tag.write_to_path(path, TAG_VERSION).unwrap();
    }

    #[test]
    fn embeds_verifies_and_removes_the_sidecar_and_uslt() {
        let directory = TempDir::new("embed");
        let audio = directory.0.join("Artist - Song [id].mp3");
        let sidecar = directory.0.join("Artist - Song [id].lrc");
        write_audio_with_uslt(&audio);
        fs::write(
            &sidecar,
            "[ar: Artist]\n[00:01.00]first\n[00:02.50]second\n",
        )
        .unwrap();

        let report = embed_synced_lyrics(&directory.0, &HashSet::new()).unwrap();
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.files_scanned, 1);
        assert_eq!(report.files_embedded, 1);
        assert_eq!(report.lines_embedded, 2);
        assert_eq!(report.uslt_frames_removed, 1);
        assert!(!sidecar.exists());

        let tag = Tag::read_from_path(&audio).unwrap();
        let stored = tag.synchronised_lyrics().next().unwrap();
        assert_eq!(stored.timestamp_format, TimestampFormat::Ms);
        assert_eq!(stored.content_type, SynchronisedLyricsType::Lyrics);
        assert_eq!(
            stored.content,
            vec![(1_000, "first".to_owned()), (2_500, "second".to_owned())]
        );
        assert_eq!(tag.lyrics().count(), 0);
    }

    #[test]
    fn keeps_the_sidecar_when_post_processing_fails() {
        let directory = TempDir::new("retain");
        let orphan = directory.0.join("No Audio.lrc");
        let untimed = directory.0.join("Untimed.lrc");
        fs::write(&orphan, "[00:01.00]lyrics without audio\n").unwrap();
        fs::write(&untimed, "[ar: Artist]\njust prose\n").unwrap();

        let report = embed_synced_lyrics(&directory.0, &HashSet::new()).unwrap();
        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.files_embedded, 0);
        assert_eq!(report.failures.len(), 2);
        assert!(orphan.exists());
        assert!(untimed.exists());

        let skip = report
            .failures
            .iter()
            .map(|failure| failure.path.clone())
            .collect::<HashSet<_>>();
        let second = embed_synced_lyrics(&directory.0, &skip).unwrap();
        assert_eq!(second.files_scanned, 0);
        assert!(second.failures.is_empty());
    }
}
