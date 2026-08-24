//! Rewrite the ID3 tag spotDL leaves behind.
//!
//! Each downloaded file gets one tag update that: drops the unwanted `POPM`
//! rating and `TSSE` encoder-settings frames, sets the
//! `TCOP` copyright message fetched from the iTunes Search API, records the
//! `TLAN` language, and pastes the `.lrc` sidecar into the ordinary `USLT`
//! lyrics frame while removing any `SYLT` synchronised-lyrics frame. The result
//! is read back from disk before the `.lrc` file is deleted.
//!
//! The lyrics keep their `[mm:ss.xx]` timestamps: `USLT` is the frame players
//! actually read, and the ones that understand timed lyrics parse them out of
//! its text.
//!
//! The `TSRC` ISRC frame is deliberately left as spotDL wrote it: iTunes does
//! not publish ISRCs, and spotDL already fills the frame from its own metadata.

use std::{
    fs,
    path::{Path, PathBuf},
};

use id3::{
    ErrorKind, Frame, Tag, TagLike, Version,
    frame::{Content, Lyrics},
};

use crate::AlbumEvidence;
use crate::files::{FileError, sibling_lyrics_file, write_tag_safely};
use crate::lyrics::{LYRICS_DESCRIPTION, Language, detect_language, lyric_lines};

/// Frames are written as ID3v2.3, matching the rest of this project.
pub(crate) const TAG_VERSION: Version = Version::Id3v23;
/// Frames removed from every download.
///
/// `POPM` is the popularimeter spotDL fills from Spotify's popularity score —
/// the frame taggers display as a rating. `TSSE` is the encoder-settings string
/// FFmpeg leaves behind, which describes the transcode rather than the music.
///
/// `TYER` is the release year. spotDL writes it beside the full `TDRC`
/// recording date it also writes, so the year is the same information a second
/// time with the day and month thrown away, and taggers show the pair as two
/// competing date fields. `TDRC` is kept because it is the complete one.
const STRIPPED_FRAMES: &[&str] = &["POPM", "TSSE", "TYER"];
/// The copyright message frame.
pub(crate) const COPYRIGHT_FRAME: &str = "TCOP";
/// The ISRC frame, which spotDL fills only when it used the official API.
const ISRC_FRAME: &str = "TSRC";
/// The language frame. ID3v2.3 specifies an ISO-639-2 code here, but the
/// readable name is what a tagger shows, so that is what is written.
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
    /// Files whose `TLAN` language a catalogue supplied.
    pub languages_looked_up: usize,
    /// Files whose `TLAN` language was detected from their lyrics rather than
    /// taken from the configured default.
    pub languages_detected: usize,
    /// `.lrc` sidecars pasted into `USLT` and then deleted.
    pub lyrics_embedded: usize,
    /// Lyric lines written across all embedded files.
    pub lines_embedded: usize,
    /// `SYLT` frames removed, spotDL's own included.
    pub sylt_frames_removed: usize,
    /// Files renamed out of spotDL's `[{track-id}]` naming once their tag was
    /// written. Renaming finishes a download rather than a tag, so `finalize`
    /// never sets this.
    pub files_renamed: usize,
    /// Renames that replaced an earlier download of the same track.
    pub files_replaced: usize,
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
        self.languages_looked_up += other.languages_looked_up;
        self.lyrics_embedded += other.lyrics_embedded;
        self.lines_embedded += other.lines_embedded;
        self.sylt_frames_removed += other.sylt_frames_removed;
        self.files_renamed += other.files_renamed;
        self.files_replaced += other.files_replaced;
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
/// which case the frame is left alone.
///
/// `known_language` is a language a catalogue has already settled, which is
/// taken as authoritative: it describes the song rather than guessing at its
/// text. `None` means nobody knew, and the lyrics are read for a guess;
/// `default_language` is recorded when even that comes to nothing.
///
/// Any `.lrc` file sitting next to `audio` is pasted into the `USLT` lyrics
/// frame and then deleted, but only after the whole tag has been read back and
/// checked.
pub fn finalize(
    audio: &Path,
    copyright: Option<&str>,
    known_language: Option<&Language>,
    default_language: &Language,
) -> MetadataReport {
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
    let mut language = known_language
        .cloned()
        .unwrap_or_else(|| default_language.clone());
    let mut detected = false;
    if let Some(sidecar) = &sidecar {
        match fs::read(sidecar) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).trim().to_owned();
                if text.is_empty() {
                    report.fail(sidecar.clone(), "the .lrc file is empty");
                    return report;
                }
                // Only guess when nothing authoritative was supplied: a
                // catalogue that names the language knows better than a
                // detector reading two lines of a chorus.
                if known_language.is_none()
                    && let Some(guess) = detect_language(&lyric_lines(&text))
                {
                    language = guess;
                    detected = true;
                }
                lyrics = Some(text);
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
    // spotDL writes a SYLT frame of its own whenever its lyrics arrive in LRC
    // format, so this removal is not only about undoing earlier runs.
    let sylt_removed = tag.synchronised_lyrics().count();
    tag.remove_all_synchronised_lyrics();
    set_text(&mut tag, COPYRIGHT_FRAME, copyright);
    set_text(&mut tag, LANGUAGE_FRAME, Some(language.name.as_str()));
    if let Some(lyrics) = &lyrics {
        tag.remove_all_lyrics();
        tag.add_frame(Lyrics {
            lang: language.code.clone(),
            description: LYRICS_DESCRIPTION.to_owned(),
            text: lyrics.clone(),
        });
    }

    if let Err(error) = write_tag_safely(audio, &tag, TAG_VERSION) {
        report.fail(
            audio.to_path_buf(),
            format!("cannot write the ID3 tag: {error}"),
        );
        return report;
    }

    if let Err(reason) = verify(audio, copyright, &language.name, lyrics.as_deref()) {
        report.fail(audio.to_path_buf(), reason);
        return report;
    }

    if let (Some(sidecar), Some(lyrics)) = (&sidecar, &lyrics) {
        if let Err(error) = fs::remove_file(sidecar) {
            report.fail(
                sidecar.clone(),
                format!(
                    "the USLT frame was verified but the .lrc file could not be deleted: {error}"
                ),
            );
            return report;
        }
        report.lyrics_embedded = 1;
        report.lines_embedded = lyric_lines(lyrics).len();
    }
    report.sylt_frames_removed = sylt_removed;

    report.files_updated = 1;
    report.frames_stripped = frames_stripped;
    report.copyrights_written = usize::from(copyright.is_some());
    report.languages_detected = usize::from(detected);
    report.languages_looked_up = usize::from(known_language.is_some());
    report
}

/// The album artist and album name recorded in a file's tag.
///
/// This is what the iTunes lookup searches with, so it comes from the tag
/// spotDL just wrote rather than from the input pair. `TPE2` is preferred over
/// `TPE1` because a compilation's tracks share an album artist but not a track
/// artist.
pub fn album_of(audio: &Path) -> Option<(String, String)> {
    album_key(&Tag::read_from_path(audio).ok()?)
}

/// Everything a file's tag knows about the release it belongs to.
///
/// Read from disk after spotDL has written the tag, so a download that used
/// the official API has an ISRC here to look the recording up by exactly.
pub fn evidence_of(audio: &Path) -> Option<AlbumEvidence> {
    album_evidence(&Tag::read_from_path(audio).ok()?)
}

/// The same key read from a tag already in hand.
pub(crate) fn album_key(tag: &Tag) -> Option<(String, String)> {
    let artist = tag.album_artist().or_else(|| tag.artist())?.trim();
    let album = tag.album()?.trim();
    (!artist.is_empty() && !album.is_empty()).then(|| (artist.to_owned(), album.to_owned()))
}

/// Everything in a tag that helps identify which release a file belongs to.
///
/// `None` when there is not even an artist and album to search with. The rest
/// is best-effort: a missing ISRC, year, or track count weakens the match
/// rather than preventing it.
pub(crate) fn album_evidence(tag: &Tag) -> Option<AlbumEvidence> {
    let (artist, album) = album_key(tag)?;
    Some(AlbumEvidence {
        artist,
        album,
        isrc: text(tag, ISRC_FRAME).filter(|isrc| is_an_isrc(isrc)),
        year: recorded_year(tag),
        // The total in "5/12", not the track's own number.
        total_tracks: tag.total_tracks(),
        track_title: tag
            .title()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
    })
}

fn text(tag: &Tag, frame_id: &str) -> Option<String> {
    let value = tag.get(frame_id)?.content().text()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The recording year, preferring the complete `TDRC` date.
fn recorded_year(tag: &Tag) -> Option<String> {
    if let Some(recorded) = tag.date_recorded() {
        return Some(recorded.year.to_string());
    }
    // A tag written before TYER was dropped, or by something else entirely.
    tag.year().map(|year| year.to_string())
}

/// Whether a value looks like an ISRC: two letters, three alphanumerics, and
/// seven digits, conventionally written without separators.
///
/// Checked because a wrong ISRC is worse than none — it would search
/// confidently for the wrong recording — and because taggers do leave
/// placeholder text in the frame.
fn is_an_isrc(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect();
    compact.len() == 12
        && compact[..2].chars().all(|c| c.is_ascii_alphabetic())
        && compact[2..5].chars().all(|c| c.is_ascii_alphanumeric())
        && compact[5..].chars().all(|c| c.is_ascii_digit())
}

/// Replace a text frame, leaving it untouched when there is no value to store.
pub(crate) fn set_text(tag: &mut Tag, frame_id: &str, value: Option<&str>) {
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
    lyrics: Option<&str>,
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

    if tag.synchronised_lyrics().next().is_some() {
        return Err("a SYLT synchronised-lyrics frame is still present".to_owned());
    }

    let Some(expected) = lyrics else {
        return Ok(());
    };
    let stored: Vec<&Lyrics> = tag.lyrics().collect();
    let [stored] = stored[..] else {
        return Err(format!(
            "the file holds {} USLT frame(s) after writing instead of exactly one",
            stored.len()
        ));
    };
    if stored.text != expected {
        return Err("the USLT frame read back does not match the .lrc file".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse_language;
    use id3::frame::{Popularimeter, SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat};

    fn english() -> Language {
        parse_language("English").unwrap()
    }
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
        // spotDL writes the release date twice: the whole date in TDRC, and
        // the year again in TYER.
        tag.add_frame(Frame::with_content(
            "TDRC",
            Content::Text("2001-03-12".to_owned()),
        ));
        tag.add_frame(Frame::with_content(
            "TYER",
            Content::Text("2001".to_owned()),
        ));
        tag.add_frame(id3::frame::Lyrics {
            lang: "eng".to_owned(),
            description: String::new(),
            text: "plain unsynced text".to_owned(),
        });
        // spotDL writes a SYLT frame of its own whenever its lyrics arrive in
        // LRC format. It is the frame most players ignore, so it goes.
        tag.add_frame(SynchronisedLyrics {
            lang: "eng".to_owned(),
            timestamp_format: TimestampFormat::Ms,
            content_type: SynchronisedLyricsType::Lyrics,
            description: String::new(),
            content: vec![(1_000, "first".to_owned())],
        });
        tag.add_frame(Frame::with_content(
            COPYRIGHT_FRAME,
            Content::Text(String::new()),
        ));
        tag.write_to_path(path, TAG_VERSION).unwrap();
    }

    #[test]
    fn strips_the_unwanted_frames_fills_the_copyright_and_pastes_the_lyrics() {
        let directory = TempDir::new("finalize");
        let audio = directory.0.join("Artist - Song [abc123].mp3");
        let sidecar = directory.0.join("Artist - Song [abc123].lrc");
        write_spotdl_style_audio(&audio);
        fs::write(
            &sidecar,
            "[ar: Artist]\n[00:01.00]first\n[00:02.50]second\n",
        )
        .unwrap();

        let report = finalize(
            &audio,
            Some("\u{2117} 2001 Daft Life Limited"),
            None,
            &english(),
        );
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.files_updated, 1);
        assert_eq!(report.frames_stripped, 3);
        assert_eq!(report.copyrights_written, 1);
        assert_eq!(report.lyrics_embedded, 1);
        assert_eq!(report.lines_embedded, 2);
        assert_eq!(report.sylt_frames_removed, 1);
        assert!(!sidecar.exists());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert!(tag.get("POPM").is_none());
        assert!(tag.get("TSSE").is_none());
        // The redundant year goes; the whole date it duplicated stays.
        assert!(tag.get("TYER").is_none());
        assert_eq!(
            tag.get("TDRC").and_then(|frame| frame.content().text()),
            Some("2001-03-12")
        );
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
        // is recorded and the lyrics frame agrees with it.
        assert_eq!(report.languages_detected, 0);
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("English")
        );
        // No synchronised frame is left, and the .lrc file went into the
        // ordinary lyrics frame with its timestamps intact.
        assert_eq!(tag.synchronised_lyrics().count(), 0);
        let stored = tag.lyrics().next().unwrap();
        assert_eq!(stored.lang, "eng");
        assert_eq!(
            stored.text,
            "[ar: Artist]\n[00:01.00]first\n[00:02.50]second"
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

        let report = finalize(&audio, None, None, &english());
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.frames_stripped, 3);
        assert_eq!(report.lyrics_embedded, 0);
        assert_eq!(report.copyrights_written, 0);

        let tag = Tag::read_from_path(&audio).unwrap();
        assert!(tag.get("POPM").is_none());
        assert!(tag.get("TSSE").is_none());
        // The redundant year goes; the whole date it duplicated stays.
        assert!(tag.get("TYER").is_none());
        assert_eq!(
            tag.get("TDRC").and_then(|frame| frame.content().text()),
            Some("2001-03-12")
        );
        // Without a .lrc file to paste, whatever spotDL wrote into the lyrics
        // frame stays; only its synchronised frame is removed.
        assert_eq!(tag.lyrics().count(), 1);
        assert_eq!(tag.synchronised_lyrics().count(), 0);
        assert_eq!(report.sylt_frames_removed, 1);
    }

    #[test]
    fn an_untimed_sidecar_is_pasted_like_any_other() {
        let directory = TempDir::new("untimed");
        let audio = directory.0.join("Untimed.mp3");
        let sidecar = directory.0.join("Untimed.lrc");
        write_spotdl_style_audio(&audio);
        fs::write(&sidecar, "[ar: Artist]\njust prose\n").unwrap();

        let report = finalize(
            &audio,
            Some("\u{2117} 2001 Daft Life Limited"),
            None,
            &english(),
        );
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.lyrics_embedded, 1);
        assert!(!sidecar.exists());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.lyrics().next().unwrap().text,
            "[ar: Artist]\njust prose"
        );
    }

    #[test]
    fn an_empty_sidecar_is_kept_and_reported() {
        let directory = TempDir::new("emptylrc");
        let audio = directory.0.join("Empty.mp3");
        let sidecar = directory.0.join("Empty.lrc");
        write_spotdl_style_audio(&audio);
        fs::write(&sidecar, "  \n\n").unwrap();

        let report = finalize(&audio, None, None, &english());
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

        let report = finalize(&audio, None, None, &english());
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.languages_detected, 1);

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("French")
        );
        assert_eq!(tag.lyrics().next().unwrap().lang, "fra");
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

        let report = finalize(&audio, None, None, &parse_language("Japanese").unwrap());
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.lyrics_embedded, 1);

        let tag = Tag::read_from_path(&audio).unwrap();
        let stored = tag.lyrics().next().unwrap();
        assert_eq!(
            stored.text,
            "[00:03.00]\u{4f60}\u{597d}\n[00:06.00]caf\u{e9} \u{1f3b5}"
        );
        // Too little text to detect from, so the configured default is used
        // for both the lyrics frame's language field and the TLAN frame.
        assert_eq!(stored.lang, "jpn");
        assert_eq!(
            tag.get(LANGUAGE_FRAME).and_then(|f| f.content().text()),
            Some("Japanese")
        );
    }

    #[test]
    fn accepts_only_something_shaped_like_an_isrc() {
        // Two letters, three alphanumerics, seven digits.
        assert!(is_an_isrc("GBUM71029604"));
        assert!(is_an_isrc("US-RC1-72-00023"), "separators are conventional");
        assert!(!is_an_isrc(""));
        assert!(!is_an_isrc("unknown"));
        // A wrong ISRC is worse than none: it searches confidently for the
        // wrong recording, so anything malformed is refused.
        assert!(!is_an_isrc("GBUM7102960"), "too short");
        assert!(!is_an_isrc("GBUM710296045"), "too long");
        assert!(!is_an_isrc("1BUM71029604"), "country must be letters");
        assert!(!is_an_isrc("GBUM7102960X"), "the tail must be digits");
    }

    #[test]
    fn reads_every_scrap_of_evidence_the_tag_carries() {
        let mut tag = Tag::with_version(Version::Id3v23);
        tag.set_album_artist("Daft Punk");
        tag.set_album("Discovery");
        tag.set_title("One More Time");
        tag.set_total_tracks(14);
        tag.set_track(1);
        tag.add_frame(Frame::text("TSRC", "GBUM71029604"));
        tag.set_date_recorded(id3::Timestamp {
            year: 2001,
            month: Some(3),
            day: Some(12),
            hour: None,
            minute: None,
            second: None,
        });

        let evidence = album_evidence(&tag).unwrap();

        assert_eq!(evidence.artist, "Daft Punk");
        assert_eq!(evidence.album, "Discovery");
        assert_eq!(evidence.isrc.as_deref(), Some("GBUM71029604"));
        assert_eq!(evidence.year.as_deref(), Some("2001"));
        // The total from "1/14", not the track's own number.
        assert_eq!(evidence.total_tracks, Some(14));
        assert_eq!(evidence.track_title.as_deref(), Some("One More Time"));
    }

    /// Most files carry far less than that, and must still be searchable.
    #[test]
    fn a_tag_with_only_an_artist_and_album_is_still_searchable() {
        let mut tag = Tag::with_version(Version::Id3v23);
        tag.set_album_artist("Daft Punk");
        tag.set_album("Discovery");
        tag.add_frame(Frame::text("TSRC", "not an isrc"));

        let evidence = album_evidence(&tag).unwrap();

        assert!(evidence.is_searchable());
        // Junk in the ISRC frame is dropped rather than searched with.
        assert_eq!(evidence.isrc, None);
        assert_eq!(evidence.year, None);
        assert_eq!(evidence.total_tracks, None);
    }

    #[test]
    fn a_tag_naming_no_album_yields_no_evidence() {
        let mut tag = Tag::with_version(Version::Id3v23);
        tag.set_album_artist("Daft Punk");
        assert!(album_evidence(&tag).is_none());
    }

    /// A catalogue that names the language is trusted over the detector: it
    /// describes the song, where the detector only reads whatever text the
    /// .lrc happens to hold.
    #[test]
    fn a_known_language_beats_what_the_lyrics_look_like() {
        let directory = TempDir::new("known-language");
        let audio = directory.0.join("song.mp3");
        write_spotdl_style_audio(&audio);
        // Unmistakably English lyrics, and a catalogue saying Korean.
        fs::write(
            directory.0.join("song.lrc"),
            "[00:01.00]Every night I dream of you\n[00:05.00]and the morning comes again\n",
        )
        .unwrap();

        let korean = parse_language("Korean").unwrap();
        let report = finalize(&audio, None, Some(&korean), &english());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.get("TLAN").and_then(|f| f.content().text()),
            Some("Korean")
        );
        assert_eq!(report.languages_looked_up, 1);
        // The detector never ran, so nothing was "detected".
        assert_eq!(report.languages_detected, 0);
        // The USLT frame carries the matching code.
        assert_eq!(tag.lyrics().next().unwrap().lang, korean.code);
    }

    /// MusicBrainz work data is patchy, so a miss has to leave the old
    /// behaviour intact rather than fall straight through to the default.
    #[test]
    fn without_a_known_language_the_lyrics_are_still_read() {
        let directory = TempDir::new("detected-language");
        let audio = directory.0.join("song.mp3");
        write_spotdl_style_audio(&audio);
        fs::write(
            directory.0.join("song.lrc"),
            "[00:01.00]오늘밤 달빛이 좀 좋네\n[00:05.00]너와 함께 걷는 이 밤\n",
        )
        .unwrap();

        let report = finalize(&audio, None, None, &english());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.get("TLAN").and_then(|f| f.content().text()),
            Some("Korean")
        );
        assert_eq!(report.languages_detected, 1);
        assert_eq!(report.languages_looked_up, 0);
    }

    /// And when neither knows, the configured default still stands.
    #[test]
    fn with_neither_the_default_is_recorded() {
        let directory = TempDir::new("default-language");
        let audio = directory.0.join("song.mp3");
        write_spotdl_style_audio(&audio);

        let report = finalize(&audio, None, None, &parse_language("Japanese").unwrap());

        let tag = Tag::read_from_path(&audio).unwrap();
        assert_eq!(
            tag.get("TLAN").and_then(|f| f.content().text()),
            Some("Japanese")
        );
        assert_eq!(report.languages_detected, 0);
        assert_eq!(report.languages_looked_up, 0);
    }
}
