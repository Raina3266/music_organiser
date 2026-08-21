//! Recursive ID3v2.3 metadata tools.

pub mod cli;
mod delete;
mod files;
mod frames;
mod transfer;

pub use delete::{DeleteReport, FileError, delete_tags_recursively};
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use transfer::{TransferReport, transfer_frame_recursively};
