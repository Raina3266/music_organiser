use std::{collections::HashSet, ffi::OsString, path::PathBuf};

use crate::download::cli::{self as download_cli, ParsedCommand as DownloadCommand};
use crate::export::default_csv_path;
use crate::frames::{SUPPORTED_TAGS, TagSpec, find_tag};

pub const HELP: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "

USAGE:
    ",
    env!("CARGO_PKG_NAME"),
    " download [OPTIONS] <INPUT_FILE>
    ",
    env!("CARGO_PKG_NAME"),
    " delete <FOLDER> \"[Tag Name, Other Tag]\" [--dry-run]
    ",
    env!("CARGO_PKG_NAME"),
    " export <FOLDER> [OUTPUT_CSV] [--overwrite]

COMMANDS:
    download    Download Spotify and/or YouTube Music links through spotDL
    delete      Remove selected ID3 tags from a folder recursively
    export      Write every ID3 frame under a folder to one CSV file

EXAMPLES:
    ",
    env!("CARGO_PKG_NAME"),
    " download links.txt --output ~/Documents/Music
    ",
    env!("CARGO_PKG_NAME"),
    " delete \"/music\" \"[Encoded-by, Album Artist]\"
    ",
    env!("CARGO_PKG_NAME"),
    " export \"/music\" frames.csv

The download command reads one link per line and forces MP3 with synced
lyrics. A line is a Spotify URL, a YouTube Music URL, or a
YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL exact-source pair, and one file may mix
all three. It then rewrites each file's ID3v2.3 tag: the POPM rating frame is
removed, the TCOP copyright message is looked up on iTunes, and the generated
.lrc file is pasted into the ordinary USLT lyrics frame.

Before the first download, an interactive run asks for a Spotify access token.
Pasting one uses Spotify's official Web API, the only source for the ISRC
frame; pressing Enter downloads token-free.

Tag names are case-sensitive. The delete command searches recursively, skips
files that do not contain a requested tag, and writes changed tags as ID3v2.3.
Supported music containers: MP3/MP2/MP1, WAV, AIFF, and AIF.

The export command reads instead of writing: it walks the same folders, gives
every file a row and every frame ID a column, and leaves a cell empty where a
file has no such frame. Without OUTPUT_CSV it writes id3-frames.csv inside
the scanned folder, and it refuses to replace an existing file unless
--overwrite is given.

Run `",
    env!("CARGO_PKG_NAME"),
    " download --help` for command-specific help.
"
);

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Download(download_cli::Config),
    DownloadHelp,
    Delete {
        folder: PathBuf,
        tags: Vec<TagSpec>,
        dry_run: bool,
    },
    Export {
        folder: PathBuf,
        output: PathBuf,
        overwrite: bool,
    },
    Help,
    Version,
}

pub fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    let Some(command) = args.first().and_then(|value| value.to_str()) else {
        return Err("missing command".to_owned());
    };

    match command {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "-V" | "--version" => Ok(Command::Version),
        "download" => parse_download(&args[1..]),
        "delete" => parse_delete(&args[1..]),
        "export" => parse_export(&args[1..]),
        _ => Err(format!("unknown command {command:?}")),
    }
}

fn parse_download(args: &[OsString]) -> Result<Command, String> {
    let args = args
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "download arguments must be valid UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;

    match download_cli::parse_args(args)? {
        DownloadCommand::Run(config) => Ok(Command::Download(config)),
        DownloadCommand::Help => Ok(Command::DownloadHelp),
        DownloadCommand::Version => Ok(Command::Version),
    }
}

fn parse_delete(args: &[OsString]) -> Result<Command, String> {
    let mut positional = Vec::new();
    let mut dry_run = false;

    for argument in args {
        if argument == "--dry-run" {
            dry_run = true;
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option {:?}", argument.to_string_lossy()));
        } else {
            positional.push(argument);
        }
    }

    if positional.len() != 2 {
        return Err("delete requires a folder and one bracketed tag list".to_owned());
    }

    let raw_tags = positional[1]
        .to_str()
        .ok_or_else(|| "tag list must be valid UTF-8".to_owned())?;
    let tags = parse_tag_list(raw_tags)?;

    Ok(Command::Delete {
        folder: PathBuf::from(positional[0]),
        tags,
        dry_run,
    })
}

fn parse_export(args: &[OsString]) -> Result<Command, String> {
    let mut positional = Vec::new();
    let mut overwrite = false;

    for argument in args {
        if argument == "--overwrite" {
            overwrite = true;
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option {:?}", argument.to_string_lossy()));
        } else {
            positional.push(argument);
        }
    }

    let folder = match positional.len() {
        1 | 2 => PathBuf::from(positional[0]),
        _ => return Err("export requires a folder and an optional CSV path".to_owned()),
    };
    let output = positional
        .get(1)
        .map_or_else(|| default_csv_path(&folder), PathBuf::from);

    Ok(Command::Export {
        folder,
        output,
        overwrite,
    })
}

pub fn parse_tag_list(raw: &str) -> Result<Vec<TagSpec>, String> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| "tag list must be enclosed in '[' and ']'".to_owned())?;

    if inner.trim().is_empty() {
        return Err("tag list cannot be empty".to_owned());
    }

    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    for raw_name in inner.split(',') {
        let name = raw_name.trim();
        if name.is_empty() {
            return Err("tag names cannot be empty".to_owned());
        }
        let tag = find_tag(name).ok_or_else(|| {
            let supported = SUPPORTED_TAGS
                .iter()
                .map(|tag| tag.name)
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown case-sensitive tag name {name:?}; supported names: {supported}")
        })?;
        if seen.insert(tag.frame_id) {
            tags.push(tag);
        }
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_requested_delete_syntax() {
        let command = parse_args(strings(&[
            "delete",
            "/music",
            "[Encoded-by, Album Artist]",
            "--dry-run",
        ]))
        .unwrap();

        assert_eq!(
            command,
            Command::Delete {
                folder: PathBuf::from("/music"),
                tags: vec![
                    TagSpec {
                        name: "Encoded-by",
                        frame_id: "TENC"
                    },
                    TagSpec {
                        name: "Album Artist",
                        frame_id: "TPE2"
                    },
                ],
                dry_run: true,
            }
        );
    }

    #[test]
    fn rejects_wrong_case() {
        let error = parse_tag_list("[encoded-by, Album Artist]").unwrap_err();
        assert!(error.contains("unknown case-sensitive tag name"));
    }

    #[test]
    fn de_duplicates_frame_ids() {
        assert_eq!(
            parse_tag_list("[Encoded-by, Encoded-by]").unwrap(),
            vec![TagSpec {
                name: "Encoded-by",
                frame_id: "TENC"
            }]
        );
    }

    #[test]
    fn parses_export_with_an_explicit_destination() {
        let command =
            parse_args(strings(&["export", "/music", "frames.csv", "--overwrite"])).unwrap();

        assert_eq!(
            command,
            Command::Export {
                folder: PathBuf::from("/music"),
                output: PathBuf::from("frames.csv"),
                overwrite: true,
            }
        );
    }

    #[test]
    fn export_defaults_to_a_csv_inside_the_scanned_folder() {
        let command = parse_args(strings(&["export", "/music"])).unwrap();

        assert_eq!(
            command,
            Command::Export {
                folder: PathBuf::from("/music"),
                output: PathBuf::from("/music/id3-frames.csv"),
                overwrite: false,
            }
        );
    }

    #[test]
    fn rejects_export_without_a_folder() {
        assert!(parse_args(strings(&["export"])).is_err());
        assert!(parse_args(strings(&["export", "/music", "a.csv", "b.csv"])).is_err());
    }

    #[test]
    fn delegates_download_options() {
        let command = parse_args(strings(&[
            "download",
            "pairs.txt",
            "--output",
            "downloads",
            "--non-interactive",
        ]))
        .unwrap();

        let Command::Download(config) = command else {
            panic!("expected the download command");
        };
        assert_eq!(config.input, PathBuf::from("pairs.txt"));
        assert_eq!(config.output, PathBuf::from("downloads"));
        assert!(config.non_interactive);
    }

    #[test]
    fn delegates_download_help() {
        assert_eq!(
            parse_args(strings(&["download", "--help"])).unwrap(),
            Command::DownloadHelp
        );
    }
}
