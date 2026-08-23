//! Exact-source spotDL downloads, ID3 metadata rewriting, and recursive
//! ID3v2.3 tag deletion.

pub mod cli;
mod delete;
pub mod download;
mod files;
mod frames;
pub mod itunes;
mod lyrics;
pub mod metadata;

pub use delete::{DeleteReport, delete_tags_recursively};
pub use files::FileError;
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use metadata::{MetadataReport, album_of, finalize};
