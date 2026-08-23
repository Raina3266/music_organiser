//! Rewrite the ID3 tag spotDL leaves behind.
//!
//! Each downloaded file gets one tag update that: drops the unwanted `POPM`
//! rating and `TSSE` encoder-settings frames, sets the
//! `TCOP` copyright message fetched from the iTunes Search API, records the
//! `TLAN` language, and turns the `.lrc` sidecar into a `SYLT` frame while
//! removing the untimed `USLT` one. The result is read back from disk before
//! the `.lrc` file is deleted.
//!
//! The `TSRC` ISRC frame is deliberately left as spotDL wrote it: iTunes does
//! not publish ISRCs, and spotDL already fills the frame from its own metadata.

use std::{
    fs,
    path::{Path, PathBuf},
};

use id3::{
    ErrorKind, Frame, Tag, TagLike, Version,
    frame::{Content, SynchronisedLyrics},
};

use crate::files::{FileError, sibling_lyrics_file, write_tag_safely};
use crate::lyrics::{detect_language, parse_lrc, sylt_frame, synchronised_lyrics};

/// Frames are written as ID3v2.3, matching the rest of this project.
const TAG_VERSION: Version = Version::Id3v23;
/// Frames removed from every download.
///
/// `POPM` is the popularimeter spotDL fills from Spotify's popularity score —
/// the frame taggers display as a rating. `TSSE` is the encoder-settings string
/// FFmpeg leaves behind, which describes the transcode rather than the music.
const STRIPPED_FRAMES: &[&str] = &["POPM", "TSSE"];
/// The copyright message frame.
const COPYRIGHT_FRAME: &str = "TCOP";
/// The language frame, an ISO-639-2 code.
const LANGUAGE_FRAME: &str = "TLAN";

#[derive(Debug, Default, Eq, PartialEq)]
pub struct MetadataReport {
    /// Audio files whose tag was rewritten.
    pub files_updated: usize,
    /// Unwanted frames removed; see `STRIPPED_FRAMES`.
    pub frames_stripped: usize,
    /// Files that received a `TCOP` copyright message.
    pub copyrights_written: usize,
    /// Files whose copyright lookup failed outright. The rest of the tag is
    /// still rewritten, so this is a warning rather than a failure.
    pub copyright_lookups_failed: usize,
    /// Files whose `TLAN` language was detected from their lyrics rather than
    /// taken from the configured default.
    pub languages_detected: usize,
    /// `.lrc` sidecars embedded as `SYLT` and then deleted.
    pub lyrics_embedded: usize,
    /// Lyric lines written across all embedded files.
    pub lines_embedded: usize,
    /// `USLT` frames removed after their synced replacement was verified.
    pub uslt_frames_removed: usize,
    /// Files that could not be finished, and why. Their `.lrc` sidecar is kept.
    pub failures: Vec<FileError>,
}

impl MetadataReport {
    pub(crate) fn absorb(&mut self, other: MetadataReport) {
        self.files_updated += other.files_updated;
        self.frames_stripped += other.frames_stripped;
        self.copyrights_written += other.copyrights_written;
        self.copyright_lookups_failed += other.copyright_lookups_failed;
        self.languages_detected += other.languages_detected;
        self.lyrics_embedded += other.lyrics_embedded;
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

/// Apply the download-time metadata rules to one audio file.
///
/// `copyright` is the message to store, or `None` when no album matched, in
/// which case the frame is left alone. `default_language` is the ISO-639-2
/// code to record when the lyrics are not enough to detect one. Any `.lrc`
/// file sitting next to `audio` is embedded and then deleted, but only after
/// the whole tag has been read back and checked.
pub fn finalize(audio: &Path, copyright: Option<&str>, default_language: &str) -> MetadataReport {
    let mut report = MetadataReport::default();

    let mut tag = match Tag::read_from_path(audio) {
        Ok(tag) => tag,
        Err(error) if matches!(error.kind, ErrorKind::NoTag) => Tag::new(),
        Err(error) => {
            report.fail(
                audio.to_path_buf(),
                format!("cannot read the ID3 tag: {error}"),
            );
            return report;
        }
    };

    let sidecar = sibling_lyrics_file(audio);
    let mut lyrics = None;
    let mut language = default_language.to_owned();
    let mut detected = false;
    if let Some(sidecar) = &sidecar {
        match fs::read(sidecar) {
            Ok(bytes) => {
                let lines = parse_lrc(&String::from_utf8_lossy(&bytes));
                if lines.is_empty() {
                    report.fail(
                        sidecar.clone(),
                        "the .lrc file contains no timestamped line, so no synchronised lyrics could be built",
                    );
                    return report;
                }
                if let Some(guess) = detect_language(&lines) {
                    language = guess;
                    detected = true;
                }
                lyrics = Some(synchronised_lyrics(lines, &language));
            }
            Err(error) => {
                report.fail(
                    sidecar.clone(),
                    format!("cannot read the .lrc file: {error}"),
                );
                return report;
            }
        }
    }

    let frames_stripped = STRIPPED_FRAMES
        .iter()
        .map(|frame_id| tag.remove(frame_id).len())
        .sum::<usize>();
    let uslt_removed = tag.lyrics().count();
    set_text(&mut tag, COPYRIGHT_FRAME, copyright);
    set_text(&mut tag, LANGUAGE_FRAME, Some(language.as_str()));
    if let Some(lyrics) = &lyrics {
        tag.remove_all_synchronised_lyrics();
        tag.remove_all_lyrics();
        tag.add_frame(sylt_frame(lyrics, TAG_VERSION));
    }

    if let Err(error) = write_tag_safely(audio, &tag, TAG_VERSION) {
        report.fail(
            audio.to_path_buf(),
            format!("cannot write the ID3 tag: {error}"),
        );
        return report;
    }

    if let Err(reason) = verify(audio, copyright, &language, lyrics.as_ref()) {
        report.fail(audio.to_path_buf(), reason);
        return report;
    }

    if let (Some(sidecar), Some(lyrics)) = (&sidecar, &lyrics) {
        if let Err(error) = fs::remove_file(sidecar) {
            report.fail(
                sidecar.clone(),
                format!(
                    "the SYLT frame was verified but the .lrc file could not be deleted: {error}"
                ),
            );
            return report;
        }
        report.lyrics_embedded = 1;
        report.lines_embedded = lyrics.content.len();
        report.uslt_frames_removed = uslt_removed;
    }

    report.files_updated = 1;
    report.frames_stripped = frames_stripped;
    report.copyrights_written = usize::from(copyright.is_some());
    report.languages_detected = usize::from(detected);
    report
}

/// The album artist and album name recorded in a file's tag.
///
/// This is what the iTunes lookup searches with, so it comes from the tag
/// spotDL just wrote rather than from the input pair. `TPE2` is preferred over
/// `TPE1` because a compilation's tracks share an album artist but not a track
/// artist.
pub fn album_of(audio: &Path) -> Option<(String, String)> {
    let tag = Tag::read_from_path(audio).ok()?;
    let artist = tag.album_artist().or_else(|| tag.artist())?.trim();
    let album = tag.album()?.trim();
    (!artist.is_empty() && !album.is_empty()).then(|| (artist.to_owned(), album.to_owned()))
}

/// Replace a text frame, leaving it untouched when there is no value to store.
fn set_text(tag: &mut Tag, frame_id: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    tag.remove(frame_id);
    tag.add_frame(Frame::with_content(
        frame_id,
        Content::Text(value.to_owned()),
    ));
}

/// Read `audio` back from disk and confirm every rule actually took effect.
fn verify(
    audio: &Path,
    copyright: Option<&str>,
    language: &str,
    lyrics: Option<&SynchronisedLyrics>,
) -> Result<(), String> {
    let tag = Tag::read_from_path(audio)
        .map_err(|error| format!("cannot re-read the file to verify its tag: {error}"))?;

    if let Some(frame_id) = STRIPPED_FRAMES
        .iter()
        .find(|frame_id| tag.get(frame_id).is_some())
    {
        return Err(format!("the {frame_id} frame is still present"));
    }

    for (frame_id, expected, label) in [
        (COPYRIGHT_FRAME, copyright, "copyright message"),
        (LANGUAGE_FRAME, Some(language), "language"),
    ] {
        let Some(expected) = expected else {
            continue;
        };
        let stored = tag.get(frame_id).and_then(|frame| frame.content().text());
        if stored != Some(expected) {
            return Err(format!("the {frame_id} {label} was not stored as expected"));
        }
    }

    let Some(expected) = lyrics else {
        return Ok(());
    };
    let stored: Vec<&SynchronisedLyrics> = tag.synchronised_lyrics().collect();
    let [stored] = stored[..] else {
        return Err(format!(
            "the file holds {} SYLT frame(s) after writing instead of exactly one",
            stored.len()
        ));
    };
    if stored.timestamp_format != expected.timestamp_format
        || stored.content_type != expected.content_type
        || stored.content != expected.content
    {
        return Err("the SYLT frame read back does not match the .lrc file".to_owned());
    }
    if tag.lyrics().next().is_some() {
        return Err("the unsynchronised USLT frame is still present".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use id3::frame::{Popularimeter, SynchronisedLyricsType, TimestampFormat};
    use std::env;

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

    /// A file tagged the way spotDL leaves it: a rating, untimed lyrics, and an
    /// empty copyright message.
    fn write_spotdl_style_audio(path: &Path) {
        fs::write(path, b"").unwrap();
        let mut tag = Tag::new();
        tag.set_title("Song");
        tag.set_album("Discovery");
        tag.set_album_artist("Daft Punk");
        // spotDL fills the ISRC itself; this program must not disturb it.
        tag.add_frame(Frame::with_content(
            "TSRC",
            Content::Text("GBAYE0601498".to_owned()),
        ));
        tag.add_frame(Popularimeter {
            user: "spotdl@spotdl".to_owned(),
            rating: 0,
            counter: 0,
        });
        // FFmpeg's encoder-settings string, describing the transcode.
        tag.add_frame(Frame::with_content(
            "TSSE",
            Content::Text("Lavf58.76.100".to_owned()),
        ));
        tag.add_frame(id3::frame::Lyrics {
            lang: "eng".to_owned(),
            description: String::new(),
            text: "plain unsynced text".to_owned(),
        });
        tag.add_frame(Frame::with_content(
            COPYRIGHT_FRAME,
            Content::Text(String::new()),
        ));
        tag.write_to_path(path, TAG_VERSION).unwrap();
    }

    #[test]
    fn strips_the_unwanted_frames_fills_the_copyright_and_embeds_the_lyrics() {
        let directory = TempDir::new("finalize");
        let audio = directory.0.join("Artist - Song [abc123].mp3");
        let sidecar = directory.0.join("Artist - Song [abc123].lrc");
        write_spotdl_style_audio(&audio);
        fs::write(
            &sidecar,
            "[ar: Artist]\n[00:01.00]first\n[00:02.50]second\n",
        )
        .unwrap();

        let report = finalize(&audio, Some("\u{2117} 2001 Daft Life Limited"), "eng");
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.files_updated, 1);
        assert_eq!(report.frames_stripped, 2);
        assert_eq!(report.copyrights_written, 1);
        assert_eq!(report.lyrics_embedded, 1);
        assert_eq!(report.lines_embedded, 2);
        assert_eq!(report.uslt_frames_removed, 1);
        assert!(!sidecar.exists());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert!(tag.get("POPM").is_none());
        assert!(tag.get("TSSE").is_none());
        assert_eq!(
            tag.get(COPYRIGHT_FRAME).and_then(|f| f.content().text()),
            Some("\u{2117} 2001 Daft Life Limited")
        );
        // spotDL's own ISRC survives untouched.
        assert_eq!(
            tag.get("TSRC").and_then(|f| f.content().text()),
            Some("GBAYE0601498")
        );
        // Two short lyric lines are not enough to detect from, so the default
        // is recorded and the SYLT frame agrees with it.
        assert_eq!(report.languages_detected, 0);
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("eng")
        );
        assert_eq!(tag.lyrics().count(), 0);
        let stored = tag.synchronised_lyrics().next().unwrap();
        assert_eq!(stored.timestamp_format, TimestampFormat::Ms);
        assert_eq!(stored.content_type, SynchronisedLyricsType::Lyrics);
        assert_eq!(
            stored.content,
            vec![(1_000, "first".to_owned()), (2_500, "second".to_owned())]
        );
        // The rest of the tag survives the rewrite.
        assert_eq!(tag.title(), Some("Song"));
    }

    #[test]
    fn reads_the_album_key_the_itunes_lookup_searches_with() {
        let directory = TempDir::new("albumkey");
        let audio = directory.0.join("Keyed.mp3");
        write_spotdl_style_audio(&audio);
        assert_eq!(
            album_of(&audio),
            Some(("Daft Punk".to_owned(), "Discovery".to_owned()))
        );

        let untagged = directory.0.join("Untagged.mp3");
        fs::write(&untagged, b"").unwrap();
        assert_eq!(album_of(&untagged), None);
    }

    #[test]
    fn a_track_without_a_sidecar_still_loses_the_stripped_frames() {
        let directory = TempDir::new("norating");
        let audio = directory.0.join("Solo.mp3");
        write_spotdl_style_audio(&audio);

        let report = finalize(&audio, None, "eng");
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.frames_stripped, 2);
        assert_eq!(report.lyrics_embedded, 0);
        assert_eq!(report.copyrights_written, 0);

        let tag = Tag::read_from_path(&audio).unwrap();
        assert!(tag.get("POPM").is_none());
        assert!(tag.get("TSSE").is_none());
        // Without synced lyrics to replace it, the untimed USLT frame stays.
        assert_eq!(tag.lyrics().count(), 1);
    }

    #[test]
    fn an_untimed_sidecar_is_kept_and_reported() {
        let directory = TempDir::new("untimed");
        let audio = directory.0.join("Untimed.mp3");
        let sidecar = directory.0.join("Untimed.lrc");
        write_spotdl_style_audio(&audio);
        fs::write(&sidecar, "[ar: Artist]\njust prose\n").unwrap();

        let report = finalize(&audio, Some("\u{2117} 2001 Daft Life Limited"), "eng");
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.files_updated, 0);
        assert!(sidecar.exists());
        // Nothing was written, so the unwanted frames are still there to retry.
        let tag = Tag::read_from_path(&audio).unwrap();
        assert!(tag.get("POPM").is_some());
        assert!(tag.get("TSSE").is_some());
    }

    #[test]
    fn a_detected_language_beats_the_default_in_both_frames() {
        let directory = TempDir::new("language");
        let audio = directory.0.join("French.mp3");
        let sidecar = directory.0.join("French.lrc");
        fs::write(&audio, b"").unwrap();
        fs::write(
            &sidecar,
            "[00:01.00]Je te promets le trone d or et la lumiere\n[00:05.00]Je te promets la liberte et je te donne\n[00:09.00]Toute ma vie je te promets tout mon amour\n",
        )
        .unwrap();

        let report = finalize(&audio, None, "eng");
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.languages_detected, 1);

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("fra")
        );
        assert_eq!(tag.synchronised_lyrics().next().unwrap().lang, "fra");
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

        let report = finalize(&audio, None, "jpn");
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.lyrics_embedded, 1);

        let tag = Tag::read_from_path(&audio).unwrap();
        let stored = tag.synchronised_lyrics().next().unwrap();
        assert_eq!(
            stored.content,
            vec![
                (3_000, "\u{4f60}\u{597d}".to_owned()),
                (6_000, "caf\u{e9} \u{1f3b5}".to_owned()),
            ]
        );
        // Too little text to detect from, so the configured default is used
        // for both the SYLT language field and the TLAN frame.
        assert_eq!(stored.lang, "jpn");
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("jpn")
        );
    }
}
