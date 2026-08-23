use std::{
    collections::HashMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
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

/// What the music files under a directory looked like at one moment.
///
/// The download command takes a snapshot before every download so it can tell
/// afterwards which files spotDL just wrote. That works for any input line,
/// including the ones that name no single Spotify track: a bare YouTube link,
/// or an album or playlist that expands into many songs.
#[derive(Debug, Default)]
pub(crate) struct MusicSnapshot {
    stamps: HashMap<PathBuf, Stamp>,
}

/// Enough of a file's metadata to notice a rewrite.
///
/// spotDL downloads with `--overwrite force`, so a song already on disk is
/// written again under the same path; the modification time and length change
/// even though the path does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    length: u64,
}

pub(crate) fn snapshot_music_files(root: &Path) -> io::Result<MusicSnapshot> {
    let mut stamps = HashMap::new();
    for path in music_files_recursively(root)? {
        if let Ok(stamp) = stamp_of(&path) {
            stamps.insert(path, stamp);
        }
    }
    Ok(MusicSnapshot { stamps })
}

impl MusicSnapshot {
    /// The music files that appeared or were rewritten since the snapshot.
    pub(crate) fn files_written_since(&self, root: &Path) -> io::Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for path in music_files_recursively(root)? {
            let Ok(stamp) = stamp_of(&path) else {
                continue;
            };
            if self.stamps.get(&path) != Some(&stamp) {
                written.push(path);
            }
        }
        Ok(written)
    }
}

fn stamp_of(path: &Path) -> io::Result<Stamp> {
    let metadata = fs::metadata(path)?;
    Ok(Stamp {
        modified: metadata.modified().ok(),
        length: metadata.len(),
    })
}

/// The audio files spotDL wrote for one Spotify track.
///
/// The download command forces spotDL's `[{track-id}]` output template, so the
/// track ID in the file name ties a file back to an input line that names a
/// single track. It is the fallback for when the snapshot comparison comes up
/// empty, and it does not apply to lines without a track ID.
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

/// A file renamed out of spotDL's `[{track-id}]` naming.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Rename {
    /// Where the file now lives.
    pub(crate) target: PathBuf,
    /// Whether an earlier file of that name was replaced.
    pub(crate) replaced: bool,
}

/// How long the bracketed run of letters and digits at the end of a downloaded
/// file name may be for it to count as a track ID.
///
/// A Spotify track ID is 22 base62 characters. The range leaves room either
/// side of that without ever reaching a bracketed word belonging to the title,
/// such as `[Live]` or `[Remix]`.
const TRACK_ID_LENGTH: std::ops::RangeInclusive<usize> = 16..=32;

/// A file stem with spotDL's trailing `[{track-id}]` removed, if it has one.
pub(crate) fn stem_without_track_id(stem: &str) -> Option<&str> {
    let (name, track_id) = stem.strip_suffix(']')?.rsplit_once('[')?;
    if !TRACK_ID_LENGTH.contains(&track_id.len())
        || !track_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }

    let name = name.trim_end();
    (!name.is_empty()).then_some(name)
}

/// Rename a downloaded file to the name without its `[{track-id}]` suffix.
///
/// Any `.lrc` sidecar still sitting next to the audio moves with it, so a file
/// whose metadata could not be finished keeps its lyrics within reach.
/// `Ok(None)` means the name carries no track ID and nothing was moved.
///
/// spotDL runs with `--overwrite force`, so a file already using the trimmed
/// name is an earlier download of the same track in the same album folder, and
/// replacing it is what that overwrite policy asks for. It is reported so the
/// run says out loud that a file was replaced.
pub(crate) fn drop_track_id_suffix(audio: &Path) -> io::Result<Option<Rename>> {
    let Some(trimmed) = audio
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(stem_without_track_id)
    else {
        return Ok(None);
    };

    // The trimmed stem may itself contain a dot, as in "Artist - Song feat.
    // Someone", so the extension is joined on by hand rather than through
    // `set_extension`, which would overwrite everything after that dot.
    let file_name = match audio.extension().and_then(|extension| extension.to_str()) {
        Some(extension) => format!("{trimmed}.{extension}"),
        None => trimmed.to_owned(),
    };
    let target = audio.with_file_name(file_name);
    if target == audio {
        return Ok(None);
    }

    let replaced = target.exists();
    if replaced {
        fs::remove_file(&target)?;
    }

    let sidecar = sibling_lyrics_file(audio);
    fs::rename(audio, &target)?;
    if let Some(sidecar) = sidecar {
        fs::rename(&sidecar, target.with_extension(LYRICS_EXTENSION))?;
    }

    Ok(Some(Rename { target, replaced }))
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
    fn a_snapshot_reports_new_and_rewritten_music_files() {
        let root = std::env::temp_dir().join(format!("music-snapshot-{}", std::process::id()));
        let album = root.join("Artist || Album");
        fs::create_dir_all(&album).unwrap();
        let existing = album.join("Artist - Old [aaa].mp3");
        fs::write(&existing, b"old").unwrap();

        let snapshot = snapshot_music_files(&root).unwrap();
        assert!(snapshot.files_written_since(&root).unwrap().is_empty());

        let added = album.join("Artist - New [bbb].mp3");
        fs::write(&added, b"new").unwrap();
        fs::write(album.join("cover.jpg"), b"art").unwrap();
        assert_eq!(
            snapshot.files_written_since(&root).unwrap(),
            vec![added.clone()]
        );

        fs::write(&existing, b"rewritten").unwrap();
        assert_eq!(
            snapshot.files_written_since(&root).unwrap(),
            vec![added, existing]
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recognizes_only_a_trailing_track_id() {
        assert_eq!(
            stem_without_track_id("i-dle - Luv U [2Mvdcda3pVMDASD7oZWPr4]"),
            Some("i-dle - Luv U")
        );
        // A bracketed part of the title is kept; only the last group goes.
        assert_eq!(
            stem_without_track_id("Artist - Song [Live] [2Mvdcda3pVMDASD7oZWPr4]"),
            Some("Artist - Song [Live]")
        );
        // Nothing that is not a track ID is touched.
        assert_eq!(stem_without_track_id("Artist - Song [Live]"), None);
        assert_eq!(stem_without_track_id("Artist - Song [Remastered]"), None);
        assert_eq!(stem_without_track_id("Artist - Song"), None);
        assert_eq!(stem_without_track_id("Artist - Song []"), None);
        // A name that is nothing but a track ID would be renamed to nothing.
        assert_eq!(stem_without_track_id("[2Mvdcda3pVMDASD7oZWPr4]"), None);
        // Track IDs are base62; punctuation means this is part of the title.
        assert_eq!(
            stem_without_track_id("Artist - Song [a very long note here!]"),
            None
        );
    }

    #[test]
    fn renaming_moves_the_lyrics_sidecar_and_keeps_a_dotted_name() {
        let root = std::env::temp_dir().join(format!("music-rename-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();

        let audio = root.join("Artist - Song feat. Someone [2Mvdcda3pVMDASD7oZWPr4].mp3");
        let sidecar = root.join("Artist - Song feat. Someone [2Mvdcda3pVMDASD7oZWPr4].lrc");
        fs::write(&audio, b"audio").unwrap();
        fs::write(&sidecar, b"[00:01.00]line").unwrap();

        let rename = drop_track_id_suffix(&audio).unwrap().unwrap();

        assert_eq!(rename.target, root.join("Artist - Song feat. Someone.mp3"));
        assert!(!rename.replaced);
        assert!(rename.target.is_file());
        assert!(!audio.exists());
        assert!(!sidecar.exists());
        assert!(root.join("Artist - Song feat. Someone.lrc").is_file());

        // A file without a track ID is left exactly where it is.
        assert_eq!(drop_track_id_suffix(&rename.target).unwrap(), None);
        assert!(rename.target.is_file());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn renaming_replaces_an_earlier_download_of_the_same_name() {
        let root = std::env::temp_dir().join(format!("music-replace-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();

        let existing = root.join("Artist - Song.mp3");
        fs::write(&existing, b"old download").unwrap();
        let audio = root.join("Artist - Song [2Mvdcda3pVMDASD7oZWPr4].mp3");
        fs::write(&audio, b"new download").unwrap();

        let rename = drop_track_id_suffix(&audio).unwrap().unwrap();

        assert_eq!(rename.target, existing);
        assert!(rename.replaced);
        assert_eq!(fs::read_to_string(&existing).unwrap(), "new download");
        assert!(!audio.exists());

        fs::remove_dir_all(&root).unwrap();
    }

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
