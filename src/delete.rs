use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use id3::{ErrorKind, Tag, TagLike, Version};

use crate::{files::music_files_recursively, frames::TagSpec};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct DeleteReport {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub files_unchanged: usize,
    pub files_without_tag: usize,
    pub frames_removed: usize,
    pub errors: Vec<FileError>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct FileError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug)]
pub struct DeleteError {
    message: String,
}

impl fmt::Display for DeleteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DeleteError {}

pub fn delete_tags_recursively(
    folder: &Path,
    tags: &[TagSpec],
    dry_run: bool,
) -> Result<DeleteReport, DeleteError> {
    if tags.is_empty() {
        return Err(DeleteError {
            message: "at least one tag is required".to_owned(),
        });
    }

    let files = music_files_recursively(folder).map_err(|error| DeleteError {
        message: format!("cannot scan {}: {error}", folder.display()),
    })?;

    let mut report = DeleteReport {
        files_scanned: files.len(),
        ..DeleteReport::default()
    };

    for path in files {
        let mut tag = match Tag::read_from_path(&path) {
            Ok(tag) => tag,
            Err(error) if matches!(error.kind, ErrorKind::NoTag) => {
                report.files_without_tag += 1;
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

        let removed = tags
            .iter()
            .map(|requested| tag.remove(requested.frame_id).len())
            .sum::<usize>();

        if removed == 0 {
            report.files_unchanged += 1;
            continue;
        }

        if !dry_run {
            if let Err(error) = write_tag_safely(&path, &tag) {
                report.errors.push(FileError {
                    path,
                    message: error.to_string(),
                });
                continue;
            }
        }

        report.files_changed += 1;
        report.frames_removed += removed;
    }

    Ok(report)
}

fn write_tag_safely(path: &Path, tag: &Tag) -> Result<(), Box<dyn Error>> {
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.music-tag-transfer.tmp",
        std::process::id()
    ));

    fs::copy(path, &temporary)?;
    if let Err(error) = tag.write_to_path(&temporary, Version::Id3v23) {
        let _ = fs::remove_file(&temporary);
        return Err(Box::new(error));
    }

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(Box::new(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use id3::{Frame, TagLike};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-{}-{}-{}",
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

    fn requested_tags() -> Vec<TagSpec> {
        vec![
            TagSpec { name: "Encoded-by", frame_id: "TENC" },
            TagSpec { name: "Album Artist", frame_id: "TPE2" },
        ]
    }

    #[test]
    fn deletes_requested_frames_recursively_and_skips_missing_tags() {
        let directory = TestDirectory::new();
        let nested = directory.0.join("artist").join("album");
        fs::create_dir_all(&nested).unwrap();

        let changed = nested.join("changed.mp3");
        tagged_file(
            &changed,
            &[("TIT2", "Song"), ("TENC", "Encoder"), ("TPE2", "Artist")],
        );
        tagged_file(&directory.0.join("unchanged.MP3"), &[("TIT2", "Other")]);
        fs::write(directory.0.join("no-tag.mp3"), b"audio payload").unwrap();
        fs::write(directory.0.join("cover.jpg"), b"not music").unwrap();

        let report =
            delete_tags_recursively(&directory.0, &requested_tags(), false).unwrap();

        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.files_changed, 1);
        assert_eq!(report.files_unchanged, 1);
        assert_eq!(report.files_without_tag, 1);
        assert_eq!(report.frames_removed, 2);
        assert!(report.errors.is_empty());

        let result = Tag::read_from_path(changed).unwrap();
        assert_eq!(result.version(), Version::Id3v23);
        assert!(result.get("TENC").is_none());
        assert!(result.get("TPE2").is_none());
        assert_eq!(
            result.get("TIT2").and_then(|frame| frame.content().text()),
            Some("Song")
        );
    }

    #[test]
    fn dry_run_reports_without_modifying_the_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("song.mp3");
        tagged_file(&path, &[("TENC", "Encoder")]);

        let report =
            delete_tags_recursively(&directory.0, &requested_tags(), true).unwrap();

        assert_eq!(report.files_changed, 1);
        assert_eq!(report.frames_removed, 1);
        assert!(Tag::read_from_path(path).unwrap().get("TENC").is_some());
    }
}
