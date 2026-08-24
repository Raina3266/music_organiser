//! spotDL downloads of Spotify and YouTube Music links, ID3 metadata
//! rewriting, recursive ID3v2.3 tag deletion, recursive ID3 frame export, and
//! copyright refreshes for music already on disk.

pub mod cli;
mod copyright;
mod delete;
pub mod download;
mod export;
mod files;
mod frames;
mod lyrics;
pub mod metadata;
pub mod sources;

pub use copyright::{CopyrightError, CopyrightLookup, CopyrightReport, refresh_copyrights};
pub use delete::{DeleteReport, delete_tags_recursively};
pub use export::{
    DEFAULT_CSV_NAME, ExportError, ExportReport, default_csv_path, export_frames_to_csv,
};
pub use files::FileError;
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use lyrics::{Language, parse_language};
pub use metadata::{MetadataReport, album_of, finalize};
