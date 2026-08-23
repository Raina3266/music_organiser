use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use id3::{Tag, Version};

pub(crate) const MUSIC_EXTENSIONS: &[&str] = &["mp3", "mp1", "mp2", "wav", "aif", "aiff"];
pub(crate) const LYRICS_EXTENSION: &str = "lrc";

/// A path that could not be processed and the reason why.
#[derive(Debug, Eq, PartialEq)]
pub struct FileError {
    pub path: PathBuf,
    pub message: String,
}

pub(crate) fn music_files_recursively(root: &Path) -> io::Result<Vec<PathBuf>> {
    files_recursively(root, MUSIC_EXTENSIONS)
}

/// The `.lrc` sidecar sitting next to an audio file, if spotDL wrote one.
pub(crate) fn sibling_lyrics_file(audio: &Path) -> Option<PathBuf> {
    let candidate = audio.with_extension(LYRICS_EXTENSION);
    candidate.is_file().then_some(candidate)
}

/// The audio files spotDL wrote for one Spotify track.
///
/// The download command forces spotDL's `[{track-id}]` output template, so the
/// track ID in the file name is what ties a file back to its input pair.
pub(crate) fn music_files_for_track(root: &Path, track_id: &str) -> io::Result<Vec<PathBuf>> {
    let suffix = format!("[{track_id}]");
    Ok(music_files_recursively(root)?
        .into_iter()
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.ends_with(&suffix))
        })
        .collect())
}

fn files_recursively(root: &Path, extensions: &[&str]) -> io::Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a directory", root.display()),
        ));
    }

    let mut files = Vec::new();
    visit(root, extensions, &mut files)?;
    files.sort();
    Ok(files)
}

fn visit(directory: &Path, extensions: &[&str], files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            visit(&entry.path(), extensions, files)?;
        } else if file_type.is_file() && has_extension(&entry.path(), extensions) {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Replace `path`'s tag by updating a copy and renaming it over the original.
pub(crate) fn write_tag_safely(
    path: &Path,
    tag: &Tag,
    version: Version,
) -> Result<(), Box<dyn Error>> {
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.music-tag-transfer.tmp",
        std::process::id()
    ));

    fs::copy(path, &temporary)?;
    if let Err(error) = tag.write_to_path(&temporary, version) {
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
    use super::*;

    #[test]
    fn recognizes_supported_extensions_case_insensitively() {
        assert!(has_extension(Path::new("track.MP3"), MUSIC_EXTENSIONS));
        assert!(has_extension(Path::new("track.aiff"), MUSIC_EXTENSIONS));
        assert!(!has_extension(Path::new("cover.jpg"), MUSIC_EXTENSIONS));
        assert!(!has_extension(Path::new("track.flac"), MUSIC_EXTENSIONS));
        assert!(has_extension(Path::new("track.LRC"), &[LYRICS_EXTENSION]));
        assert!(!has_extension(Path::new("track.mp3"), &[LYRICS_EXTENSION]));
    }
}
