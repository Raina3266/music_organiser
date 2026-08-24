//! Look the copyright message up again for music already on disk.
//!
//! The download command fills `TCOP` from the iTunes Search API as each file
//! arrives. This does the same for a folder that is already there: every album
//! under it is looked up once and its files are given the copyright message
//! that came back. Which catalogue answers is the caller's choice — see
//! [`crate::sources`] — because the four disagree on both wording and coverage.
//!
//! A file is only ever written when a copyright was actually found. A lookup
//! that matches nothing, a lookup that fails, and a file whose tag names no
//! album all leave the file exactly as it was — an existing message is never
//! cleared by a miss.

use std::{collections::HashMap, error::Error, fmt, path::Path};

use id3::{ErrorKind, Tag, TagLike};

use crate::{
    files::{FileError, music_files_recursively, write_tag_safely},
    metadata::{COPYRIGHT_FRAME, TAG_VERSION, album_evidence, set_text},
};

/// Why a lookup did not produce a copyright.
///
/// The distinction matters more than it looks. One album that cannot be found
/// says nothing about the next one, but a source that has started refusing
/// every request — a spent rate limit, a dead token — will refuse the rest of
/// the library too. Asking it again is not merely wasted: with a rate limit it
/// deepens the hole. So the two are reported separately and handled
/// differently.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LookupError {
    /// This album could not be looked up. Others may still succeed.
    Album(String),
    /// The source has stopped answering for longer than this run will wait,
    /// and must not be asked again.
    Exhausted(String),
}

impl fmt::Display for LookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::Album(message) | LookupError::Exhausted(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for LookupError {}

/// Everything a tag knows about the release its file belongs to.
///
/// The artist and album name alone are a weak key: names collide, catalogues
/// spell them differently, and an album and its deluxe edition differ by a
/// suffix. The rest of this is corroboration, taken from the first file seen
/// for an album and reused for the others — so a lookup still costs one
/// request per album, but has far more than a pair of strings to choose with.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AlbumEvidence {
    /// The album artist, or the track artist when there is no album artist.
    pub artist: String,
    pub album: String,
    /// From `TSRC`. A globally unique identifier for the *recording*, which
    /// pins it exactly on the sources that can search by it.
    ///
    /// It does not pin the release: one recording sits on the album, the
    /// single, and any number of compilations, each with its own copyright
    /// line. So this narrows the field to releases that genuinely contain the
    /// track, and the album name still chooses between them.
    pub isrc: Option<String>,
    /// The release year, from `TDRC`.
    pub year: Option<String>,
    /// How many tracks the album has, from the total in `TRCK`'s `5/12`.
    pub total_tracks: Option<u32>,
    /// One track known to be on the release, from `TIT2`.
    pub track_title: Option<String>,
}

impl AlbumEvidence {
    /// The cache key: one lookup per album, however many tracks it has.
    pub fn key(&self) -> (String, String) {
        (self.artist.clone(), self.album.clone())
    }

    /// Whether there is enough to search with at all.
    pub fn is_searchable(&self) -> bool {
        !self.artist.trim().is_empty() && !self.album.trim().is_empty()
    }
}

/// One catalogue that can be asked for an album's copyright message.
///
/// Every source in [`crate::sources`] implements this, and so does the fake in
/// this module's tests, which is what keeps the refresh itself testable without
/// reaching the network. Answers are remembered here rather than assumed of the
/// implementation, so each album costs one call however many tracks it has.
pub trait CopyrightLookup {
    /// The copyright message for an album, or `None` when nothing matched
    /// confidently enough to use.
    fn copyright(&mut self, album: &AlbumEvidence) -> Result<Option<String>, LookupError>;
}

/// What one refresh run did.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct CopyrightReport {
    /// Music files the recursive scan visited.
    pub files_scanned: usize,
    /// Files whose `TCOP` was written.
    pub files_updated: usize,
    /// Files that already held the message the lookup returned.
    pub files_unchanged: usize,
    /// Files left alone because they already had a message and `only_missing`
    /// was set.
    pub files_skipped: usize,
    /// Files left alone because their album could not be looked up: no ID3
    /// tag, no album name, no match, or a failed request.
    pub files_without_copyright: usize,
    /// Distinct albums a request was made for.
    pub albums_looked_up: usize,
    /// Albums the source had no confident match for.
    pub albums_without_match: usize,
    /// Albums whose lookup failed outright, which is a warning rather than a
    /// failure: their files keep whatever they had.
    pub albums_failed: usize,
    /// Set when every source stopped answering and the scan gave up part way.
    /// The files already written are still written; the rest were not visited.
    pub stopped_early: bool,
    /// Files that could not be read or written, and why.
    pub errors: Vec<FileError>,
}

#[derive(Debug)]
pub struct CopyrightError {
    message: String,
}

impl fmt::Display for CopyrightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CopyrightError {}

/// Refresh `TCOP` for every music file under `folder`.
///
/// With `only_missing`, files that already carry a non-empty copyright are
/// left alone and their album is never looked up. `dry_run` reports the same
/// counts without writing anything.
pub fn refresh_copyrights(
    folder: &Path,
    lookup: &mut dyn CopyrightLookup,
    only_missing: bool,
    dry_run: bool,
) -> Result<CopyrightReport, CopyrightError> {
    let files = music_files_recursively(folder).map_err(|error| CopyrightError {
        message: format!("cannot scan {}: {error}", folder.display()),
    })?;

    let mut report = CopyrightReport {
        files_scanned: files.len(),
        ..CopyrightReport::default()
    };
    // One answer per album, kept whether it was a hit, a miss, or a failure:
    // an album that could not be looked up is not worth asking about again for
    // every one of its tracks.
    let mut answers: HashMap<(String, String), Result<Option<String>, LookupError>> =
        HashMap::new();

    for path in files {
        let mut tag = match Tag::read_from_path(&path) {
            Ok(tag) => tag,
            Err(error) if matches!(error.kind, ErrorKind::NoTag) => {
                report.files_without_copyright += 1;
                continue;
            }
            Err(error) => {
                report.errors.push(FileError {
                    path,
                    message: error.to_string(),
                });
                continue;
            }
        };

        let existing = tag
            .get(COPYRIGHT_FRAME)
            .and_then(|frame| frame.content().text())
            .unwrap_or_default()
            .trim()
            .to_owned();
        if only_missing && !existing.is_empty() {
            report.files_skipped += 1;
            continue;
        }

        let Some(wanted) = album_evidence(&tag) else {
            println!(
                "{}: no album artist and album to search with; left unchanged.",
                path.display()
            );
            report.files_without_copyright += 1;
            continue;
        };

        let (artist, album) = (wanted.artist.clone(), wanted.album.clone());
        let key = wanted.key();
        let first_time = !answers.contains_key(&key);
        if first_time {
            report.albums_looked_up += 1;
            let answer = lookup.copyright(&wanted);
            answers.insert(key.clone(), answer);
        }
        let copyright = match answers[&key].clone() {
            Ok(Some(copyright)) => copyright,
            Ok(None) => {
                if first_time {
                    report.albums_without_match += 1;
                    println!("{artist} - {album}: no matching release; left unchanged.");
                }
                report.files_without_copyright += 1;
                continue;
            }
            Err(LookupError::Album(error)) => {
                if first_time {
                    report.albums_failed += 1;
                    eprintln!("{artist} - {album}: the lookup failed ({error}); left unchanged.");
                }
                report.files_without_copyright += 1;
                continue;
            }
            // Nothing left to ask. Stopping here keeps the remaining albums
            // unvisited rather than turning each one into an identical
            // failure, and leaves them for a later run to pick up.
            Err(LookupError::Exhausted(error)) => {
                eprintln!("{error}");
                report.stopped_early = true;
                break;
            }
        };

        if first_time {
            println!("{artist} - {album}: {copyright}");
        }
        if existing == copyright {
            report.files_unchanged += 1;
            continue;
        }

        if !dry_run {
            set_text(&mut tag, COPYRIGHT_FRAME, Some(&copyright));
            if let Err(error) = write_tag_safely(&path, &tag, TAG_VERSION) {
                report.errors.push(FileError {
                    path,
                    message: error.to_string(),
                });
                continue;
            }
        }
        report.files_updated += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use id3::{Frame, Version};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-copyright-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed),
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A lookup that answers from a table and counts what it was asked.
    #[derive(Default)]
    struct FakeLookup {
        answers: HashMap<(String, String), Result<Option<String>, LookupError>>,
        requests: Vec<(String, String)>,
    }

    /// One canned answer: the album artist, the album, and what to reply.
    type Answer<'a> = (&'a str, &'a str, Result<Option<&'a str>, &'a str>);

    impl FakeLookup {
        fn with(answers: &[Answer<'_>]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(artist, album, answer)| {
                        let answer = match answer {
                            Ok(Some(copyright)) => Ok(Some((*copyright).to_owned())),
                            Ok(None) => Ok(None),
                            Err(error) => Err(LookupError::Album((*error).to_owned())),
                        };
                        (((*artist).to_owned(), (*album).to_owned()), answer)
                    })
                    .collect(),
                requests: Vec::new(),
            }
        }
    }

    impl CopyrightLookup for FakeLookup {
        fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
            self.requests.push(wanted.key());
            self.answers.get(&wanted.key()).cloned().unwrap_or(Ok(None))
        }
    }

    fn tagged_file(path: &Path, album: &str, copyright: Option<&str>) {
        fs::write(path, b"audio payload").unwrap();
        let mut tag = Tag::with_version(Version::Id3v23);
        tag.set_album_artist("Daft Punk");
        tag.set_album(album);
        if let Some(copyright) = copyright {
            tag.add_frame(Frame::text(COPYRIGHT_FRAME, copyright));
        }
        tag.write_to_path(path, Version::Id3v23).unwrap();
    }

    fn copyright_of(path: &Path) -> Option<String> {
        Tag::read_from_path(path)
            .ok()?
            .get(COPYRIGHT_FRAME)?
            .content()
            .text()
            .map(str::to_owned)
    }

    #[test]
    fn writes_what_was_found_and_looks_each_album_up_once() {
        let directory = TestDirectory::new();
        let album = directory.0.join("Discovery");
        fs::create_dir_all(&album).unwrap();
        let one = album.join("01 One More Time.mp3");
        let two = album.join("02 Aerodynamic.mp3");
        tagged_file(&one, "Discovery", None);
        tagged_file(&two, "Discovery", Some("stale message"));

        let mut lookup = FakeLookup::with(&[(
            "Daft Punk",
            "Discovery",
            Ok(Some("\u{2117} 2001 Daft Life Limited")),
        )]);
        let report = refresh_copyrights(&directory.0, &mut lookup, false, false).unwrap();

        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.files_updated, 2);
        assert_eq!(report.albums_looked_up, 1);
        assert!(report.errors.is_empty());
        // One album, one request, however many tracks it holds.
        assert_eq!(lookup.requests.len(), 1);

        for path in [&one, &two] {
            assert_eq!(
                copyright_of(path).as_deref(),
                Some("\u{2117} 2001 Daft Life Limited")
            );
            assert_eq!(
                Tag::read_from_path(path).unwrap().version(),
                Version::Id3v23
            );
        }
    }

    #[test]
    fn a_miss_a_failure_and_a_missing_album_all_leave_the_file_alone() {
        let directory = TestDirectory::new();
        let unmatched = directory.0.join("unmatched.mp3");
        let failed = directory.0.join("failed.mp3");
        let albumless = directory.0.join("albumless.mp3");
        let untagged = directory.0.join("untagged.mp3");
        tagged_file(&unmatched, "Obscure", Some("keep me"));
        tagged_file(&failed, "Offline", Some("keep me too"));
        fs::write(&albumless, b"audio payload").unwrap();
        let mut tag = Tag::with_version(Version::Id3v23);
        tag.add_frame(Frame::text(COPYRIGHT_FRAME, "no album here"));
        tag.write_to_path(&albumless, Version::Id3v23).unwrap();
        fs::write(&untagged, b"audio payload").unwrap();

        let mut lookup = FakeLookup::with(&[
            ("Daft Punk", "Obscure", Ok(None)),
            ("Daft Punk", "Offline", Err("the lookup failed")),
        ]);
        let report = refresh_copyrights(&directory.0, &mut lookup, false, false).unwrap();

        assert_eq!(report.files_updated, 0);
        assert_eq!(report.albums_without_match, 1);
        assert_eq!(report.albums_failed, 1);
        assert_eq!(report.files_without_copyright, 4);
        assert!(report.errors.is_empty());

        assert_eq!(copyright_of(&unmatched).as_deref(), Some("keep me"));
        assert_eq!(copyright_of(&failed).as_deref(), Some("keep me too"));
        assert_eq!(copyright_of(&albumless).as_deref(), Some("no album here"));
    }

    #[test]
    fn an_unchanged_message_is_not_rewritten() {
        let directory = TestDirectory::new();
        let path = directory.0.join("song.mp3");
        tagged_file(&path, "Discovery", Some("\u{2117} 2001 Daft Life Limited"));

        let mut lookup = FakeLookup::with(&[(
            "Daft Punk",
            "Discovery",
            Ok(Some("\u{2117} 2001 Daft Life Limited")),
        )]);
        let report = refresh_copyrights(&directory.0, &mut lookup, false, false).unwrap();

        assert_eq!(report.files_updated, 0);
        assert_eq!(report.files_unchanged, 1);
    }

    #[test]
    fn only_missing_skips_files_that_already_have_one() {
        let directory = TestDirectory::new();
        let filled = directory.0.join("filled.mp3");
        let empty = directory.0.join("empty.mp3");
        tagged_file(&filled, "Discovery", Some("already here"));
        tagged_file(&empty, "Discovery", Some("   "));

        let mut lookup = FakeLookup::with(&[(
            "Daft Punk",
            "Discovery",
            Ok(Some("\u{2117} 2001 Daft Life Limited")),
        )]);
        let report = refresh_copyrights(&directory.0, &mut lookup, true, false).unwrap();

        assert_eq!(report.files_skipped, 1);
        assert_eq!(report.files_updated, 1);
        assert_eq!(copyright_of(&filled).as_deref(), Some("already here"));
        assert_eq!(
            copyright_of(&empty).as_deref(),
            Some("\u{2117} 2001 Daft Life Limited")
        );
    }

    #[test]
    fn a_dry_run_reports_without_writing() {
        let directory = TestDirectory::new();
        let path = directory.0.join("song.mp3");
        tagged_file(&path, "Discovery", None);

        let mut lookup = FakeLookup::with(&[(
            "Daft Punk",
            "Discovery",
            Ok(Some("\u{2117} 2001 Daft Life Limited")),
        )]);
        let report = refresh_copyrights(&directory.0, &mut lookup, false, true).unwrap();

        assert_eq!(report.files_updated, 1);
        assert_eq!(copyright_of(&path), None);
    }

    #[test]
    fn reports_a_folder_that_cannot_be_scanned() {
        let mut lookup = FakeLookup::default();
        let error = refresh_copyrights(
            Path::new("/definitely/not/a/folder"),
            &mut lookup,
            false,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot scan"));
    }

    /// The behaviour the Spotify rate limit exposed: once the source has
    /// given up, the scan must stop rather than turn every remaining album
    /// into an identical failure. What was written stays written.
    #[test]
    fn an_exhausted_source_stops_the_scan_instead_of_failing_every_album() {
        let directory = TestDirectory::new();
        for album in ["Discovery", "Homework", "Human After All", "Tron"] {
            let path = directory.0.join(format!("{album}.mp3"));
            tagged_file(&path, album, None);
        }

        struct Spent {
            asked: usize,
        }
        impl CopyrightLookup for Spent {
            fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
                self.asked += 1;
                match wanted.album.as_str() {
                    "Discovery" => Ok(Some("\u{2117} 2001 Daft Life".to_owned())),
                    _ => Err(LookupError::Exhausted("rate limited for 5h 21m".to_owned())),
                }
            }
        }

        let mut lookup = Spent { asked: 0 };
        let report = refresh_copyrights(&directory.0, &mut lookup, false, false).unwrap();

        assert!(report.stopped_early);
        // The album that worked is still written.
        assert_eq!(report.files_updated, 1);
        // The scan gave up at the first refusal rather than asking three more
        // times: that is the whole point, since asking again deepens a ban.
        assert_eq!(lookup.asked, 2);
        assert_eq!(report.albums_failed, 0);
    }
}
