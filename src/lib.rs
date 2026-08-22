//! Spotify lookup/download workflows and recursive ID3v2.3 metadata tools.

pub mod cli;
mod delete;
pub mod download;
mod files;
mod frames;
pub mod resolve;
mod transfer;

pub use delete::{DeleteReport, FileError, delete_tags_recursively};
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use transfer::{TransferReport, transfer_frame_recursively};
