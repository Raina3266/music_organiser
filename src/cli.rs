use std::{collections::HashSet, ffi::OsString, path::PathBuf};

use crate::frames::{SUPPORTED_TAGS, TagSpec, find_tag};

pub const HELP: &str = "\
music-tag-transfer

USAGE:
    music-tag-transfer delete <FOLDER> \"[Tag Name, Other Tag]\" [--dry-run]
    music-tag-transfer transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>
    music-tag-transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>

EXAMPLE:
    music-tag-transfer delete \"/music\" \"[Encoded-by, Album Artist]\"

Tag names are case-sensitive. The delete command searches recursively, skips
files that do not contain a requested tag, and writes changed tags as ID3v2.3.
Supported music containers: MP3/MP2/MP1, WAV, AIFF, and AIF.
";

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Delete {
        folder: PathBuf,
        tags: Vec<TagSpec>,
        dry_run: bool,
    },
    Transfer {
        source: PathBuf,
        destination: PathBuf,
        frame_id: String,
    },
    Help,
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
        "delete" => parse_delete(&args[1..]),
        "transfer" => parse_transfer(&args[1..]),
        _ if args.len() == 3 => parse_transfer(&args),
        _ => Err(format!("unknown command {command:?}")),
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

fn parse_transfer(args: &[OsString]) -> Result<Command, String> {
    if args.len() != 3 {
        return Err("transfer requires source folder, destination folder, and frame ID".to_owned());
    }

    let frame_id = args[2]
        .to_str()
        .ok_or_else(|| "frame ID must be valid UTF-8".to_owned())?
        .to_owned();
    if frame_id.len() != 4
        || !frame_id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("frame ID must be four uppercase ASCII letters/digits, such as USLT".to_owned());
    }

    Ok(Command::Transfer {
        source: PathBuf::from(&args[0]),
        destination: PathBuf::from(&args[1]),
        frame_id,
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
                    TagSpec { name: "Encoded-by", frame_id: "TENC" },
                    TagSpec { name: "Album Artist", frame_id: "TPE2" },
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
            vec![TagSpec { name: "Encoded-by", frame_id: "TENC" }]
        );
    }

    #[test]
    fn supports_legacy_transfer_invocation() {
        assert_eq!(
            parse_args(strings(&["source", "destination", "USLT"])).unwrap(),
            Command::Transfer {
                source: PathBuf::from("source"),
                destination: PathBuf::from("destination"),
                frame_id: "USLT".to_owned(),
            }
        );
    }
}
