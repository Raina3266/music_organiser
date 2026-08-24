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

use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use id3::{ErrorKind, Tag, TagLike};

use crate::{
    csv::{relative_label, write_record},
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

    /// Which catalogue supplied the last answer, for the report.
    ///
    /// Only a chain of sources has anything interesting to say here, so the
    /// default is silence.
    fn answered_by(&self) -> Option<&'static str> {
        None
    }
}

/// What happened to one file.
///
/// Every visited file gets one of these, whether or not it changed, so the
/// report can show what was left alone as well as what was written. A dry run
/// producing these is the point of the exercise: it is the thing to read
/// before letting the real run touch anything.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Outcome {
    /// A copyright was found that differs from the one in the file.
    Written,
    /// The file already held exactly the message the lookup returned.
    Unchanged,
    /// Left alone: it already had a message and `only_missing` was set.
    Skipped,
    /// Every source was asked and none had a matching release.
    NoMatch,
    /// The lookup failed outright, so nothing could be written.
    Failed,
    /// The tag names no album artist and album to search with.
    NoAlbum,
    /// The file carries no ID3 tag at all.
    NoTag,
    /// The file could not be read, or its new tag could not be written.
    Error,
}

impl Outcome {
    /// The word this outcome appears as in the report.
    pub const fn label(self) -> &'static str {
        match self {
            Outcome::Written => "written",
            Outcome::Unchanged => "unchanged",
            Outcome::Skipped => "skipped",
            Outcome::NoMatch => "no match",
            Outcome::Failed => "lookup failed",
            Outcome::NoAlbum => "no album in tag",
            Outcome::NoTag => "no ID3 tag",
            Outcome::Error => "error",
        }
    }
}

/// One file's before and after, for the report.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    /// The album artist and album the lookup searched with, when the tag named
    /// them.
    pub artist: String,
    pub album: String,
    /// The copyright the file carried before the run.
    pub before: String,
    /// The copyright the run wrote, or would have written on a dry run. Empty
    /// whenever nothing was found, which is also what keeps the report honest
    /// about a message being kept rather than replaced.
    pub after: String,
    pub outcome: Outcome,
    /// Which catalogue supplied the message, when one did.
    pub source: Option<&'static str>,
    /// Why the file could not be handled, for the outcomes that have a reason.
    pub note: String,
}

impl Change {
    fn of(path: &Path, outcome: Outcome) -> Self {
        Self {
            path: path.to_path_buf(),
            artist: String::new(),
            album: String::new(),
            before: String::new(),
            after: String::new(),
            outcome,
            source: None,
            note: String::new(),
        }
    }

    fn searching(mut self, artist: &str, album: &str) -> Self {
        self.artist = artist.to_owned();
        self.album = album.to_owned();
        self
    }

    fn carrying(mut self, before: &str) -> Self {
        self.before = before.to_owned();
        self
    }

    fn writing(mut self, after: &str, source: Option<&'static str>) -> Self {
        self.after = after.to_owned();
        self.source = source;
        self
    }

    fn because(mut self, note: &str) -> Self {
        self.note = note.to_owned();
        self
    }
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
    /// Every visited file's before and after, in the order they were scanned.
    pub changes: Vec<Change>,
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
                report.changes.push(Change::of(&path, Outcome::NoTag));
                continue;
            }
            Err(error) => {
                report
                    .changes
                    .push(Change::of(&path, Outcome::Error).because(&error.to_string()));
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
            report
                .changes
                .push(Change::of(&path, Outcome::Skipped).carrying(&existing));
            continue;
        }

        let Some(wanted) = album_evidence(&tag) else {
            println!(
                "{}: no album artist and album to search with; left unchanged.",
                path.display()
            );
            report.files_without_copyright += 1;
            report
                .changes
                .push(Change::of(&path, Outcome::NoAlbum).carrying(&existing));
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
                report.changes.push(
                    Change::of(&path, Outcome::NoMatch)
                        .searching(&artist, &album)
                        .carrying(&existing),
                );
                continue;
            }
            Err(LookupError::Album(error)) => {
                if first_time {
                    report.albums_failed += 1;
                    eprintln!("{artist} - {album}: the lookup failed ({error}); left unchanged.");
                }
                report.files_without_copyright += 1;
                report.changes.push(
                    Change::of(&path, Outcome::Failed)
                        .searching(&artist, &album)
                        .carrying(&existing)
                        .because(&error),
                );
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
        let source = lookup.answered_by();
        if existing == copyright {
            report.files_unchanged += 1;
            report.changes.push(
                Change::of(&path, Outcome::Unchanged)
                    .searching(&artist, &album)
                    .carrying(&existing)
                    .writing(&copyright, source),
            );
            continue;
        }

        if !dry_run {
            set_text(&mut tag, COPYRIGHT_FRAME, Some(&copyright));
            if let Err(error) = write_tag_safely(&path, &tag, TAG_VERSION) {
                report.changes.push(
                    Change::of(&path, Outcome::Error)
                        .searching(&artist, &album)
                        .carrying(&existing)
                        .because(&error.to_string()),
                );
                report.errors.push(FileError {
                    path,
                    message: error.to_string(),
                });
                continue;
            }
        }
        report.files_updated += 1;
        report.changes.push(
            Change::of(&path, Outcome::Written)
                .searching(&artist, &album)
                .carrying(&existing)
                .writing(&copyright, source),
        );
    }

    Ok(report)
}

/// The header of the copyright report, in the order the columns are written.
const REPORT_COLUMNS: &[&str] = &[
    "File",
    "Album Artist",
    "Album",
    "Copyright Before",
    "Copyright After",
    "Outcome",
    "Source",
    "Note",
];

/// Write one row per visited file, showing what it held and what the run made
/// of it.
///
/// Paired with `--dry-run` this is a preview: nothing on disk has changed, and
/// the `Copyright After` column is what a real run would write. Without it the
/// same file is a record of what was written.
///
/// The destination is only replaced when `overwrite` is set; otherwise an
/// existing file is left alone and this fails.
pub fn write_change_report(
    report: &CopyrightReport,
    root: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<(), CopyrightError> {
    if !overwrite && destination.exists() {
        return Err(CopyrightError {
            message: format!(
                "{} already exists; pass --overwrite to replace it",
                destination.display()
            ),
        });
    }

    write_rows(report, root, destination).map_err(|error| CopyrightError {
        message: format!("cannot write {}: {error}", destination.display()),
    })
}

fn write_rows(report: &CopyrightReport, root: &Path, destination: &Path) -> io::Result<()> {
    let mut writer = BufWriter::new(fs::File::create(destination)?);
    write_record(&mut writer, REPORT_COLUMNS.iter().copied())?;

    for change in &report.changes {
        let path = relative_label(&change.path, root);
        let cells = [
            path.as_str(),
            change.artist.as_str(),
            change.album.as_str(),
            change.before.as_str(),
            change.after.as_str(),
            change.outcome.label(),
            change.source.unwrap_or(""),
            change.note.as_str(),
        ];
        write_record(&mut writer, cells.into_iter())?;
    }

    writer.flush()
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

    fn report_of(directory: &Path) -> String {
        let csv = directory.join("changes.csv");
        let mut lookup = FakeLookup::with(&[
            (
                "Daft Punk",
                "Discovery",
                Ok(Some("\u{2117} 2001 Daft Life")),
            ),
            ("Daft Punk", "Obscure", Ok(None)),
            ("Daft Punk", "Offline", Err("the network is down")),
        ]);
        let report = refresh_copyrights(directory, &mut lookup, false, true).unwrap();
        write_change_report(&report, directory, &csv, false).unwrap();
        fs::read_to_string(&csv).unwrap()
    }

    /// The point of the feature: a dry run says what it would do, per file,
    /// without touching anything.
    #[test]
    fn the_report_shows_every_files_before_and_after() {
        let directory = TestDirectory::new();
        let changed = directory.0.join("changed.mp3");
        let same = directory.0.join("same.mp3");
        tagged_file(&changed, "Discovery", Some("stale message"));
        tagged_file(&same, "Discovery", Some("\u{2117} 2001 Daft Life"));

        let csv = report_of(&directory.0);
        let rows: Vec<&str> = csv.split_terminator("\r\n").collect();

        assert_eq!(
            rows[0],
            "File,Album Artist,Album,Copyright Before,Copyright After,Outcome,Source,Note"
        );
        let changed_row = rows.iter().find(|r| r.starts_with("changed.mp3")).unwrap();
        assert!(changed_row.contains("stale message"), "{changed_row}");
        assert!(
            changed_row.contains("\u{2117} 2001 Daft Life"),
            "{changed_row}"
        );
        assert!(changed_row.contains("written"), "{changed_row}");

        let same_row = rows.iter().find(|r| r.starts_with("same.mp3")).unwrap();
        assert!(same_row.contains("unchanged"), "{same_row}");

        // A dry run wrote the report and nothing else.
        assert_eq!(copyright_of(&changed).as_deref(), Some("stale message"));
    }

    /// A file that gets no copyright still gets a row, and its existing
    /// message is shown as kept rather than blank.
    #[test]
    fn the_report_accounts_for_the_files_that_were_left_alone() {
        let directory = TestDirectory::new();
        tagged_file(
            &directory.0.join("unmatched.mp3"),
            "Obscure",
            Some("keep me"),
        );
        tagged_file(
            &directory.0.join("failed.mp3"),
            "Offline",
            Some("keep me too"),
        );
        fs::write(directory.0.join("untagged.mp3"), b"audio payload").unwrap();

        let csv = report_of(&directory.0);

        let row = |name: &str| {
            csv.split_terminator("\r\n")
                .find(|r| r.starts_with(name))
                .unwrap_or_else(|| panic!("no row for {name}"))
                .to_owned()
        };
        let unmatched = row("unmatched.mp3");
        assert!(unmatched.contains("no match"), "{unmatched}");
        assert!(unmatched.contains("keep me"), "{unmatched}");

        let failed = row("failed.mp3");
        assert!(failed.contains("lookup failed"), "{failed}");
        assert!(failed.contains("the network is down"), "{failed}");

        assert!(row("untagged.mp3").contains("no ID3 tag"));
    }

    #[test]
    fn a_message_holding_a_comma_survives_the_round_trip() {
        let directory = TestDirectory::new();
        let path = directory.0.join("comma.mp3");
        tagged_file(
            &path,
            "Discovery",
            Some("\u{2117} 2020 Artist Partner Group, Inc."),
        );

        let csv = report_of(&directory.0);

        // Quoted, so a spreadsheet still sees one cell.
        assert!(
            csv.contains("\"\u{2117} 2020 Artist Partner Group, Inc.\""),
            "{csv}"
        );
    }

    #[test]
    fn the_report_refuses_to_replace_a_file_without_overwrite() {
        let directory = TestDirectory::new();
        tagged_file(&directory.0.join("song.mp3"), "Discovery", None);
        let csv = directory.0.join("changes.csv");
        fs::write(&csv, b"existing").unwrap();

        let mut lookup = FakeLookup::default();
        let report = refresh_copyrights(&directory.0, &mut lookup, false, true).unwrap();

        let error = write_change_report(&report, &directory.0, &csv, false).unwrap_err();
        assert!(error.to_string().contains("--overwrite"));
        assert_eq!(fs::read_to_string(&csv).unwrap(), "existing");

        write_change_report(&report, &directory.0, &csv, true).unwrap();
        assert!(fs::read_to_string(&csv).unwrap().starts_with("File,"));
    }

    /// Skipped files are in the report too, so --only-missing shows what it
    /// passed over rather than silently omitting it.
    #[test]
    fn the_report_lists_what_only_missing_skipped() {
        let directory = TestDirectory::new();
        tagged_file(
            &directory.0.join("filled.mp3"),
            "Discovery",
            Some("already here"),
        );

        let mut lookup = FakeLookup::with(&[(
            "Daft Punk",
            "Discovery",
            Ok(Some("\u{2117} 2001 Daft Life")),
        )]);
        let report = refresh_copyrights(&directory.0, &mut lookup, true, true).unwrap();

        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].outcome, Outcome::Skipped);
        assert_eq!(report.changes[0].before, "already here");
        // Nothing would be written, so nothing is promised in the after column.
        assert!(report.changes[0].after.is_empty());
    }
}
