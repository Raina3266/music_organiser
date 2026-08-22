//! Spotify lookup/download workflows and recursive ID3v2.3 tag deletion.

pub mod cli;
mod delete;
pub mod download;
mod files;
mod frames;
pub mod resolve;

pub use delete::{DeleteReport, FileError, delete_tags_recursively};
pub use frames::{SUPPORTED_TAGS, TagSpec};
