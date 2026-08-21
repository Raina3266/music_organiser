use std::{
    collections::HashMap,
    error::Error,
    fmt,
    path::Path,
};

use id3::{ErrorKind, Frame, Tag, TagLike, Version};

use crate::{delete::FileError, files::music_files_recursively};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct TransferReport {
    pub source_files_scanned: usize,
    pub destination_files_scanned: usize,
    pub files_changed: usize,
    pub frames_copied: usize,
    pub errors: Vec<FileError>,
}

#[derive(Debug)]
pub struct TransferError {
    message: String,
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransferError {}

pub fn transfer_frame_recursively(
    source_folder: &Path,
    destination_folder: &Path,
    frame_id: &str,
) -> Result<TransferReport, TransferError> {
    let source_files =
        music_files_recursively(source_folder).map_err(|error| TransferError {
            message: format!("cannot scan {}: {error}", source_folder.display()),
        })?;
    let destination_files =
        music_files_recursively(destination_folder).map_err(|error| TransferError {
            message: format!("cannot scan {}: {error}", destination_folder.display()),
        })?;

    let mut report = TransferReport {
        source_files_scanned: source_files.len(),
        destination_files_scanned: destination_files.len(),
        ..TransferReport::default()
    };
    let mut frames_by_title: HashMap<String, Vec<Frame>> = HashMap::new();

    for path in source_files {
        match Tag::read_from_path(&path) {
            Ok(tag) => {
                let Some(title) = tag.title() else {
                    continue;
                };
                let frames = tag
                    .frames()
                    .filter(|frame| frame.id() == frame_id)
                    .cloned()
                    .collect::<Vec<_>>();
                if !frames.is_empty() {
                    frames_by_title.entry(title.to_owned()).or_insert(frames);
                }
            }
            Err(error) if matches!(error.kind, ErrorKind::NoTag) => {}
            Err(error) => report.errors.push(FileError {
                path,
                message: error.to_string(),
            }),
        }
    }

    for path in destination_files {
        let mut tag = match Tag::read_from_path(&path) {
            Ok(tag) => tag,
            Err(error) if matches!(error.kind, ErrorKind::NoTag) => continue,
            Err(error) => {
                report.errors.push(FileError {
                    path,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let Some(title) = tag.title() else {
            continue;
        };
        let Some(frames) = frames_by_title.get(title) else {
            continue;
        };

        let copied = frames.len();
        for frame in frames.iter().cloned() {
            tag.add_frame(frame);
        }

        if let Err(error) = tag.write_to_path(&path, Version::Id3v23) {
            report.errors.push(FileError {
                path,
                message: error.to_string(),
            });
            continue;
        }
        report.files_changed += 1;
        report.frames_copied += copied;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_matching_frame_replaces_only_that_frame() {
        let mut source = Tag::new();
        source.set_title("Song");
        source.add_frame(Frame::text("TENC", "Encoder"));

        let mut destination = Tag::new();
        destination.set_title("Song");
        destination.set_album("Album");

        let source_frames = source
            .frames()
            .filter(|frame| frame.id() == "TENC")
            .cloned()
            .collect::<Vec<_>>();
        for frame in source_frames {
            destination.add_frame(frame);
        }

        assert_eq!(
            destination
                .get("TENC")
                .and_then(|frame| frame.content().text()),
            Some("Encoder")
        );
        assert_eq!(destination.album(), Some("Album"));
    }
}
