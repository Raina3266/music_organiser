//! Download Spotify and YouTube Music links through spotDL, clean and embed
//! synced lyrics, standardise ID3 metadata, and maintain tags recursively.

pub mod cli;
mod copyright;
mod csv;
mod delete;
pub mod download;
mod export;
mod files;
mod frames;
mod lyrics;
pub mod metadata;
pub mod sources;

pub use copyright::{
    AlbumEvidence, Change, CopyrightError, CopyrightLookup, CopyrightReport, LookupError, Outcome,
    refresh_copyrights, write_change_report,
};
pub use delete::{DeleteReport, delete_tags_recursively};
pub use export::{
    DEFAULT_CSV_NAME, ExportError, ExportReport, default_csv_path, export_frames_to_csv,
};
pub use files::FileError;
pub use frames::{SUPPORTED_TAGS, TagSpec};
pub use lyrics::{Language, parse_language};
pub use metadata::{MetadataReport, album_of, evidence_of, finalize};
