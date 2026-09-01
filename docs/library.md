# Using the Rust library

[← Back to the README](../README.md)

The package also exposes the recursive metadata deletion operation:

```rust
use std::path::Path;
use music_tag_transfer::{SUPPORTED_TAGS, delete_tags_recursively};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let encoded_by = SUPPORTED_TAGS
        .iter()
        .copied()
        .find(|tag| tag.name == "Encoded-by")
        .expect("supported tag");

    let report = delete_tags_recursively(
        Path::new("/path/to/music"),
        &[encoded_by],
        true,
    )?;
    println!("Would update {} file(s)", report.files_changed);
    Ok(())
}
```

The copyright refresh is available as a library call, taking anything that
implements `CopyrightLookup` so a caller can supply its own source:

```rust
use std::path::Path;
use music_tag_transfer::{
    refresh_copyrights,
    sources::{DEFAULT_MAX_WAIT, Source},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lookup = Source::MusicBrainz.open(None, DEFAULT_MAX_WAIT)?;
    let report = refresh_copyrights(Path::new("/path/to/music"), lookup.as_mut(), false, true)?;
    println!("Would write {} file(s)", report.files_updated);
    Ok(())
}
```

The same metadata rules can be applied to a single file that already has a
`.lrc` sidecar next to it:

```rust
use std::path::Path;
use music_tag_transfer::finalize;

fn main() {
    let report = finalize(
        Path::new("/path/to/Artist - Song [id].mp3"),
        Some("\u{2117} 2001 Daft Life Limited"),
        "eng",
    );
    println!(
        "Removed {} rating frame(s), embedded {} line(s)",
        report.ratings_removed, report.lines_embedded
    );
    for failure in &report.failures {
        eprintln!("{}: {}", failure.path.display(), failure.message);
    }
}
```

The recursive frame export is available too:

```rust
use std::path::Path;
use music_tag_transfer::{default_csv_path, export_frames_to_csv};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = Path::new("/path/to/music");
    let report = export_frames_to_csv(folder, &default_csv_path(folder), true)?;
    println!(
        "Exported {} frame(s) across {} column(s)",
        report.frames_exported, report.frame_columns
    );
    Ok(())
}
```

`DeleteReport`, `ExportReport`, `ExportError`, `CopyrightReport`, `CopyrightError`,
`MetadataReport`, `FileError`, and `TagSpec` are also exported,
along with `album_of` for reading the album key a lookup searches with. The
`cli`, `download`, `sources`, and `metadata` modules expose the executable's
command configuration and entry points.
