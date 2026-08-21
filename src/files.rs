use std::{
    fs, io,
    path::{Path, PathBuf},
};

const MUSIC_EXTENSIONS: &[&str] = &["mp1", "mp2", "mp3", "wav", "aif", "aiff"];

pub fn music_files_recursively(root: &Path) -> io::Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a directory", root.display()),
        ));
    }

    let mut files = Vec::new();
    visit(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn visit(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            visit(&entry.path(), files)?;
        } else if file_type.is_file() && is_music_file(&entry.path()) {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn is_music_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MUSIC_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_extensions_case_insensitively() {
        assert!(is_music_file(Path::new("track.MP3")));
        assert!(is_music_file(Path::new("track.aiff")));
        assert!(!is_music_file(Path::new("cover.jpg")));
        assert!(!is_music_file(Path::new("track.flac")));
    }
}
