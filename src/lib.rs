//! Spotify lookup/download workflows, synced-lyrics embedding, and recursive
//! ID3v2.3 tag deletion.

pub mod cli;
mod delete;
pub mod download;
mod files;
mod frames;
pub mod lyrics;
pub mod resolve;

pub use delete::{DeleteReport, delete_tags_recursively};
pub use files::FileError;
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use lyrics::{LyricsReport, embed_synced_lyrics};
