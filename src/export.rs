//! Read every ID3 frame under a folder and write them all to one CSV file.
//!
//! The scan is the same recursive walk the delete command uses, so it covers
//! the same containers and skips the same non-music files. Every music file
//! becomes one row, every frame ID found anywhere under the folder becomes one
//! column, and a file that does not carry a frame leaves that cell empty --
//! including a file with no ID3 tag at all, which is still listed with an
//! otherwise empty row.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use id3::{ErrorKind, Tag, frame::Content};

use crate::{
    files::{FileError, music_files_recursively},
    frames::SUPPORTED_TAGS,
};

/// The default file name when the command is given no destination.
pub const DEFAULT_CSV_NAME: &str = "id3-frames.csv";
/// The header of the first column, which holds the path of each file.
const PATH_COLUMN: &str = "File";
/// Joins the values of repeated frames that share one frame ID, such as the
/// several `TXXX` or `APIC` frames a tagger may write.
const VALUE_SEPARATOR: &str = " | ";
/// The record separator RFC 4180 asks for. Newlines inside a value -- lyrics,
/// mostly -- stay as line feeds inside the quoted cell.
const RECORD_SEPARATOR: &str = "\r\n";

/// What one export run found.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ExportReport {
    /// Music files the recursive scan visited.
    pub files_scanned: usize,
    /// Files that carried an ID3 tag.
    pub files_with_tag: usize,
    /// Files listed with an empty row because they carry no ID3 tag.
    pub files_without_tag: usize,
    /// Frames written across all rows.
    pub frames_exported: usize,
    /// Frame columns in the CSV file, excluding the leading path column.
    pub frame_columns: usize,
    /// Files that could not be read, and why. They are left out of the CSV.
    pub errors: Vec<FileError>,
}

#[derive(Debug)]
pub struct ExportError {
    message: String,
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ExportError {}

/// Collect the ID3 frames of every music file under `folder` into `destination`.
///
/// The destination is only replaced when `overwrite` is set; otherwise an
/// existing file is an error and nothing is written.
pub fn export_frames_to_csv(
    folder: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<ExportReport, ExportError> {
    if !overwrite && destination.exists() {
        return Err(ExportError {
            message: format!(
                "{} already exists; pass --overwrite to replace it",
                destination.display()
            ),
        });
    }

    let files = music_files_recursively(folder).map_err(|error| ExportError {
        message: format!("cannot scan {}: {error}", folder.display()),
    })?;

    let mut report = ExportReport {
        files_scanned: files.len(),
        ..ExportReport::default()
    };
    let mut columns = BTreeSet::new();
    let mut rows = Vec::with_capacity(files.len());

    for path in files {
        let label = relative_label(&path, folder);
        let tag = match Tag::read_from_path(&path) {
            Ok(tag) => tag,
            Err(error) if matches!(error.kind, ErrorKind::NoTag) => {
                report.files_without_tag += 1;
                rows.push((label, BTreeMap::new()));
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

        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for frame in tag.frames() {
            let value = describe(frame.content());
            columns.insert(frame.id().to_owned());
            report.frames_exported += 1;
            values
                .entry(frame.id().to_owned())
                .and_modify(|existing| append_value(existing, &value))
                .or_insert(value);
        }

        report.files_with_tag += 1;
        rows.push((label, values));
    }

    report.frame_columns = columns.len();
    write_csv(destination, &columns, &rows).map_err(|error| ExportError {
        message: format!("cannot write {}: {error}", destination.display()),
    })?;

    Ok(report)
}

/// The destination used when the caller names only a folder.
pub fn default_csv_path(folder: &Path) -> PathBuf {
    folder.join(DEFAULT_CSV_NAME)
}

/// Keep repeated frames readable without inventing a value for an empty one.
fn append_value(existing: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    if existing.is_empty() {
        existing.push_str(value);
        return;
    }
    existing.push_str(VALUE_SEPARATOR);
    existing.push_str(value);
}

fn write_csv(
    destination: &Path,
    columns: &BTreeSet<String>,
    rows: &[(String, BTreeMap<String, String>)],
) -> io::Result<()> {
    let mut writer = BufWriter::new(fs::File::create(destination)?);

    let headers = std::iter::once(PATH_COLUMN.to_owned())
        .chain(columns.iter().map(|frame_id| column_header(frame_id)))
        .collect::<Vec<_>>();
    write_record(&mut writer, headers.iter().map(String::as_str))?;

    for (label, values) in rows {
        let cells = std::iter::once(label.as_str()).chain(
            columns
                .iter()
                .map(|frame_id| values.get(frame_id).map_or("", String::as_str)),
        );
        write_record(&mut writer, cells)?;
    }

    writer.flush()
}

fn write_record<'a>(
    writer: &mut impl Write,
    cells: impl Iterator<Item = &'a str>,
) -> io::Result<()> {
    for (index, cell) in cells.enumerate() {
        if index > 0 {
            writer.write_all(b",")?;
        }
        writer.write_all(escape(cell).as_bytes())?;
    }
    writer.write_all(RECORD_SEPARATOR.as_bytes())
}

/// Quote a cell the way RFC 4180 asks, collapsing carriage returns so that
/// only the record separator carries one.
fn escape(value: &str) -> String {
    let value = if value.contains('\r') {
        value.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        value.to_owned()
    };

    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

/// `Title (TIT2)` for a frame this program knows a name for, the bare frame ID
/// for anything else a tagger left behind.
fn column_header(frame_id: &str) -> String {
    SUPPORTED_TAGS
        .iter()
        .find(|tag| tag.frame_id == frame_id)
        .map_or_else(
            || frame_id.to_owned(),
            |tag| format!("{} ({frame_id})", tag.name),
        )
}

fn relative_label(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// One frame as a single CSV cell.
///
/// Text frames export their text unchanged, so an empty frame stays an empty
/// cell. The frames that hold binary data are summarised instead of dumped,
/// and synchronised lyrics are rebuilt in the `.lrc` timestamp format the
/// download command started from.
fn describe(content: &Content) -> String {
    match content {
        Content::SynchronisedLyrics(lyrics) => lyrics
            .content
            .iter()
            .map(|(milliseconds, text)| {
                format!("[{}] {}", timestamp(*milliseconds), text.trim_matches('\n'))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Content::Picture(picture) => {
            let summary = format!(
                "{} ({}, {} bytes)",
                picture.picture_type,
                picture.mime_type,
                picture.data.len()
            );
            if picture.description.is_empty() {
                summary
            } else {
                format!("{}: {summary}", picture.description)
            }
        }
        Content::Private(private) => format!(
            "{}: {} bytes",
            private.owner_identifier,
            private.private_data.len()
        ),
        other => other.to_string(),
    }
}

/// `mm:ss.xx`, the timestamp `.lrc` files use.
fn timestamp(milliseconds: u32) -> String {
    let minutes = milliseconds / 60_000;
    let seconds = (milliseconds % 60_000) / 1_000;
    let hundredths = (milliseconds % 1_000) / 10;
    format!("{minutes:02}:{seconds:02}.{hundredths:02}")
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use id3::{
        Frame, TagLike, Version,
        frame::{
            Picture, PictureType, SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat,
        },
    };

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-export-{}-{}-{}",
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

    fn tagged_file(path: &Path, frames: &[(&str, &str)]) {
        fs::write(path, b"audio payload").unwrap();
        let mut tag = Tag::with_version(Version::Id3v23);
        for (id, value) in frames {
            tag.add_frame(Frame::text(*id, *value));
        }
        tag.write_to_path(path, Version::Id3v23).unwrap();
    }

    fn rows_of(csv: &str) -> Vec<&str> {
        csv.split_terminator(RECORD_SEPARATOR).collect()
    }

    #[test]
    fn exports_every_frame_and_leaves_missing_ones_empty() {
        let directory = TestDirectory::new();
        let nested = directory.0.join("artist").join("album");
        fs::create_dir_all(&nested).unwrap();

        tagged_file(
            &nested.join("full.mp3"),
            &[("TIT2", "Song"), ("TPE1", "Artist"), ("TENC", "")],
        );
        tagged_file(&directory.0.join("sparse.MP3"), &[("TIT2", "Other")]);
        fs::write(directory.0.join("no-tag.mp3"), b"audio payload").unwrap();
        fs::write(directory.0.join("cover.jpg"), b"not music").unwrap();

        let destination = directory.0.join("frames.csv");
        let report = export_frames_to_csv(&directory.0, &destination, false).unwrap();

        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.files_with_tag, 2);
        assert_eq!(report.files_without_tag, 1);
        assert_eq!(report.frames_exported, 4);
        assert_eq!(report.frame_columns, 3);
        assert!(report.errors.is_empty());

        let csv = fs::read_to_string(&destination).unwrap();
        assert_eq!(
            rows_of(&csv),
            vec![
                "File,Encoded-by (TENC),Title (TIT2),Artist (TPE1)",
                "artist/album/full.mp3,,Song,Artist",
                "no-tag.mp3,,,",
                "sparse.MP3,,Other,",
            ]
        );
    }

    #[test]
    fn quotes_separators_and_joins_repeated_frames() {
        let directory = TestDirectory::new();
        let path = directory.0.join("song.mp3");
        fs::write(&path, b"audio payload").unwrap();

        let mut tag = Tag::with_version(Version::Id3v23);
        tag.add_frame(Frame::text("TIT2", "Song, \"live\"\nreprise"));
        tag.add_frame(id3::frame::ExtendedText {
            description: "MOOD".to_owned(),
            value: "calm".to_owned(),
        });
        tag.add_frame(id3::frame::ExtendedText {
            description: "SOURCE".to_owned(),
            value: "web".to_owned(),
        });
        tag.write_to_path(&path, Version::Id3v23).unwrap();

        let destination = directory.0.join("frames.csv");
        export_frames_to_csv(&directory.0, &destination, false).unwrap();

        let csv = fs::read_to_string(&destination).unwrap();
        assert_eq!(
            rows_of(&csv),
            vec![
                "File,Title (TIT2),User Text (TXXX)",
                "song.mp3,\"Song, \"\"live\"\"\nreprise\",MOOD: calm | SOURCE: web",
            ]
        );
    }

    #[test]
    fn summarizes_pictures_and_rebuilds_synced_lyrics() {
        let directory = TestDirectory::new();
        let path = directory.0.join("song.mp3");
        fs::write(&path, b"audio payload").unwrap();

        let mut tag = Tag::with_version(Version::Id3v23);
        tag.add_frame(Picture {
            mime_type: "image/jpeg".to_owned(),
            picture_type: PictureType::CoverFront,
            description: String::new(),
            data: vec![0; 12],
        });
        tag.add_frame(SynchronisedLyrics {
            lang: "eng".to_owned(),
            timestamp_format: TimestampFormat::Ms,
            content_type: SynchronisedLyricsType::Lyrics,
            description: String::new(),
            content: vec![
                (1_000, "first line".to_owned()),
                (61_230, "second".to_owned()),
            ],
        });
        tag.write_to_path(&path, Version::Id3v23).unwrap();

        let destination = directory.0.join("frames.csv");
        export_frames_to_csv(&directory.0, &destination, false).unwrap();

        let csv = fs::read_to_string(&destination).unwrap();
        assert_eq!(
            rows_of(&csv),
            vec![
                "File,Picture (APIC),Synced Lyrics (SYLT)",
                "song.mp3,\"Front cover (image/jpeg, 12 bytes)\",\
                 \"[00:01.00] first line\n[01:01.23] second\"",
            ]
        );
    }

    #[test]
    fn refuses_to_replace_an_existing_file_without_overwrite() {
        let directory = TestDirectory::new();
        tagged_file(&directory.0.join("song.mp3"), &[("TIT2", "Song")]);
        let destination = directory.0.join("frames.csv");
        fs::write(&destination, "keep me").unwrap();

        let error = export_frames_to_csv(&directory.0, &destination, false).unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "keep me");

        export_frames_to_csv(&directory.0, &destination, true).unwrap();
        assert!(
            fs::read_to_string(&destination)
                .unwrap()
                .contains("song.mp3")
        );
    }

    #[test]
    fn reports_a_folder_that_cannot_be_scanned() {
        let error = export_frames_to_csv(
            Path::new("/definitely/not/a/folder"),
            Path::new("/definitely/not/a/folder/frames.csv"),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot scan"));
    }

    #[test]
    fn formats_lrc_timestamps() {
        assert_eq!(timestamp(0), "00:00.00");
        assert_eq!(timestamp(1_234), "00:01.23");
        assert_eq!(timestamp(605_000), "10:05.00");
    }
}
