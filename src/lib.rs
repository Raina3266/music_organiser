//! spotDL downloads of Spotify and YouTube Music links, ID3 metadata
//! rewriting, recursive ID3v2.3 tag deletion, and recursive ID3 frame export.

pub mod cli;
mod delete;
pub mod download;
mod export;
mod files;
mod frames;
pub mod itunes;
mod lyrics;
pub mod metadata;

pub use delete::{DeleteReport, delete_tags_recursively};
pub use export::{
    DEFAULT_CSV_NAME, ExportError, ExportReport, default_csv_path, export_frames_to_csv,
};
pub use files::FileError;
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use lyrics::{Language, parse_language};
pub use metadata::{MetadataReport, album_of, finalize};
