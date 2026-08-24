//! Look the copyright message up again for music already on disk.
//!
//! The download command fills `TCOP` from the iTunes Search API as each file
//! arrives. This does the same for a folder that is already there: every album
//! under it is looked up once and its files are given the copyright message
//! that came back.
//!
//! A file is only ever written when a copyright was actually found. A lookup
//! that matches nothing, a lookup that fails, and a file whose tag names no
//! album all leave the file exactly as it was — an existing message is never
//! cleared by a miss.

use std::{collections::HashMap, error::Error, fmt, path::Path};

use id3::{ErrorKind, Tag, TagLike};

use crate::{
    files::{FileError, music_files_recursively, write_tag_safely},
    itunes,
    metadata::{COPYRIGHT_FRAME, TAG_VERSION, album_key, set_text},
};

/// Where a copyright message can be looked up.
///
/// The iTunes client is the only real implementation; the trait keeps this
/// module testable without reaching the network. Answers are remembered here
/// rather than assumed of the implementation, so each album costs one call
/// however many tracks it has.
pub trait CopyrightLookup {
    /// The copyright message for an album, or `None` when nothing matched
    /// confidently enough to use.
    fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, String>;
}

impl CopyrightLookup for itunes::Client {
    fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
        itunes::Client::copyright(self, artist, album)
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
    /// Albums iTunes had no confident match for.
    pub albums_without_match: usize,
    /// Albums whose lookup failed outright, which is a warning rather than a
    /// failure: their files keep whatever they had.
    pub albums_failed: usize,
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
    lookup: &mut impl CopyrightLookup,
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
    let mut answers: HashMap<(String, String), Result<Option<String>, String>> = HashMap::new();

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

        let Some((artist, album)) = album_key(&tag) else {
            println!(
                "{}: no album artist and album to search iTunes with; left unchanged.",
                path.display()
            );
            report.files_without_copyright += 1;
            continue;
        };

        let key = (artist.clone(), album.clone());
        let first_time = !answers.contains_key(&key);
        if first_time {
            report.albums_looked_up += 1;
            let answer = lookup.copyright(&artist, &album);
            answers.insert(key.clone(), answer);
        }
        let copyright = match answers[&key].clone() {
            Ok(Some(copyright)) => copyright,
            Ok(None) => {
                if first_time {
                    report.albums_without_match += 1;
                    println!("{artist} - {album}: iTunes has no matching album; left unchanged.");
                }
                report.files_without_copyright += 1;
                continue;
            }
            Err(error) => {
                if first_time {
                    report.albums_failed += 1;
                    eprintln!("{artist} - {album}: the lookup failed ({error}); left unchanged.");
                }
                report.files_without_copyright += 1;
                continue;
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
        answers: HashMap<(String, String), Result<Option<String>, String>>,
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
                            Err(error) => Err((*error).to_owned()),
                        };
                        (((*artist).to_owned(), (*album).to_owned()), answer)
                    })
                    .collect(),
                requests: Vec::new(),
            }
        }
    }

    impl CopyrightLookup for FakeLookup {
        fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
            self.requests.push((artist.to_owned(), album.to_owned()));
            self.answers
                .get(&(artist.to_owned(), album.to_owned()))
                .cloned()
                .unwrap_or(Ok(None))
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
}
