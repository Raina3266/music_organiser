use std::{collections::HashSet, ffi::OsString, path::PathBuf};

use crate::download::cli::{self as download_cli, ParsedCommand as DownloadCommand};
use crate::export::default_csv_path;
use crate::frames::{SUPPORTED_TAGS, TagSpec, find_tag};
use crate::sources::{Limits, Source};

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
    ",
    env!("CARGO_PKG_NAME"),
    " copyright <FOLDER> [--source NAMES] [--only-missing] [--dry-run]

COMMANDS:
    download    Download Spotify and/or YouTube Music links through spotDL
    delete      Remove selected ID3 tags from a folder recursively
    export      Write every ID3 frame under a folder to one CSV file
    copyright   Look the TCOP copyright message up again in a music catalogue

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
    ",
    env!("CARGO_PKG_NAME"),
    " copyright \"/music\"
    ",
    env!("CARGO_PKG_NAME"),
    " copyright \"/music\" --source musicbrainz --only-missing
    ",
    env!("CARGO_PKG_NAME"),
    " copyright \"/music\" --dry-run --csv changes.csv

The download command reads one link per line and forces MP3 with synced
lyrics. A line is a Spotify URL, a YouTube Music URL, or a
YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL exact-source pair, and one file may mix
all three. It then cleans and embeds the generated .lrc text and limits each
ID3v2.3 tag to the 15 supported metadata types.

Downloads are token-free by default. Supplying --auth-token, --token-file, or
SPOTIFY_AUTH_TOKEN automatically enables Spotify's official API; no separate
--official-api flag is required.

Tag names are case-sensitive. The delete command searches recursively, skips
files that do not contain a requested tag, and writes changed tags as ID3v2.3.
Supported music containers: MP3/MP2/MP1, WAV, AIFF, and AIF.

The export command reads instead of writing: it walks the same folders, gives
every file a row and every frame ID a column, and leaves a cell empty where a
file has no such frame. Without OUTPUT_CSV it writes id3-frames.csv inside
the scanned folder, and it refuses to replace an existing file unless
--overwrite is given.

The copyright command looks every album under a folder up in a music catalogue
and writes the message it finds to TCOP. Four catalogues are available and they
disagree on both wording and coverage, so an interactive run asks which one to
use; --source itunes|musicbrainz|discogs|spotify answers the question in
advance, and a run with no terminal uses iTunes. iTunes and MusicBrainz need no
account; Discogs and Spotify need a token, taken from --token-file, from
DISCOGS_TOKEN or SPOTIFY_ACCESS_TOKEN, or from a prompt.

--source takes several names to build a fallback chain, tried in order per
album: --source spotify,musicbrainz,itunes. A source that runs out of rate
limit, keeps throttling, or rejects its token is retired for the rest of the
run rather than asked again, and the albums it never reached go to the next
one. When every source is spent the scan stops instead of failing each
remaining album; re-run with --only-missing to carry on. --max-wait sets how
long a rate-limit pause may be before a source is given up on (default 60s).

A file is written only when a copyright was found: a lookup that matches
nothing, a lookup that fails, and a file naming no album all leave that file
exactly as it was. --only-missing skips files that already carry a message.

A throttled request is waited out and retried --max-throttle-retries times
(30 by default); running out of those skips that album and the scan carries
on, since throttling passes and the next album will very likely go through.
A source that says the requests are coming too often also has the gap between
them widened -- doubled each time it refuses, up to ten seconds, and eased
back once ten requests have got through -- so the next album is not sent at
the rate that was just refused. MusicBrainz in particular answers a breached
rate limit with 503, and at a busy hour it can refuse a rate it accepted
earlier in the same run.
A request that times out or hits a server error is retried with a growing
pause, --max-attempts times (5 by default), before that album is likewise
skipped.

The scan always visits every file. A source is only ever set aside -- stopped
being asked, while the run continues -- when it asks for a wait longer than
--max-wait, when its token is rejected, or when three albums in a row cannot
reach it at all or never get through its throttling.

--csv PATH writes one row per file showing what it held, what the run would
write, and what became of it. With --dry-run that is a preview to read before
letting a real run touch anything; without it, a record of what was written.
It refuses to replace an existing file unless --overwrite is given.

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
    Copyright {
        folder: PathBuf,
        /// The catalogues to ask, in order. `None` means nobody has chosen
        /// yet, so an interactive run asks and any other run takes the
        /// default. More than one is a fallback chain: if the first runs out
        /// of rate limit, the rest of the library goes to the next.
        sources: Option<Vec<Source>>,
        /// A token given on the command line. Every process on the machine can
        /// read this, so --token-file and the environment are preferred.
        token: Option<String>,
        token_file: Option<PathBuf>,
        only_missing: bool,
        dry_run: bool,
        /// Where to write the before-and-after report, when one was asked for.
        csv: Option<PathBuf>,
        /// Whether that report may replace an existing file.
        overwrite: bool,
        /// How hard to try before giving up on a request and on a source.
        limits: Limits,
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
        "copyright" => parse_copyright(&args[1..]),
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

fn parse_copyright(args: &[OsString]) -> Result<Command, String> {
    let mut positional = Vec::new();
    let mut sources = None;
    let mut token = None;
    let mut token_file = None;
    let mut only_missing = false;
    let mut dry_run = false;
    let mut csv = None;
    let mut overwrite = false;
    let mut limits = Limits::default();

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].to_string_lossy().into_owned();
        match argument.as_str() {
            "--only-missing" => only_missing = true,
            "--dry-run" => dry_run = true,
            "--overwrite" => overwrite = true,
            "--csv" => csv = Some(PathBuf::from(next_value(args, &mut index, "--csv")?)),
            "--source" => {
                sources = Some(Source::parse_list(&next_value(
                    args, &mut index, "--source",
                )?)?);
            }
            "--max-wait" => {
                limits.max_wait = next_value(args, &mut index, "--max-wait")?
                    .parse()
                    .map_err(|_| "--max-wait expects a number of seconds".to_owned())?;
            }
            "--max-throttle-retries" => {
                let retries: u32 = next_value(args, &mut index, "--max-throttle-retries")?
                    .parse()
                    .map_err(|_| "--max-throttle-retries expects a number".to_owned())?;
                if retries == 0 {
                    return Err("--max-throttle-retries must be at least 1".to_owned());
                }
                limits.max_throttle_retries = retries;
            }
            "--max-attempts" => {
                let attempts: u32 = next_value(args, &mut index, "--max-attempts")?
                    .parse()
                    .map_err(|_| "--max-attempts expects a number".to_owned())?;
                if attempts == 0 {
                    return Err("--max-attempts must be at least 1".to_owned());
                }
                limits.max_attempts = attempts;
            }
            "--token" => token = Some(next_value(args, &mut index, "--token")?),
            "--token-file" => {
                token_file = Some(PathBuf::from(next_value(args, &mut index, "--token-file")?));
            }
            _ if argument.starts_with("--csv=") => {
                csv = Some(PathBuf::from(&argument["--csv=".len()..]));
            }
            _ if argument.starts_with("--source=") => {
                sources = Some(Source::parse_list(&argument["--source=".len()..])?);
            }
            _ if argument.starts_with("--token=") => {
                token = Some(argument["--token=".len()..].to_owned());
            }
            _ if argument.starts_with("--token-file=") => {
                token_file = Some(PathBuf::from(&argument["--token-file=".len()..]));
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option {argument:?}"));
            }
            _ => positional.push(&args[index]),
        }
        index += 1;
    }

    if positional.len() != 1 {
        return Err("copyright requires exactly one folder".to_owned());
    }
    if token.is_some() && token_file.is_some() {
        return Err("give --token or --token-file, not both".to_owned());
    }
    if overwrite && csv.is_none() {
        return Err("--overwrite only means something with --csv".to_owned());
    }

    Ok(Command::Copyright {
        folder: PathBuf::from(positional[0]),
        sources,
        token,
        token_file,
        only_missing,
        dry_run,
        csv,
        overwrite,
        limits,
    })
}

/// The value that follows an option, advancing past it.
fn next_value(args: &[OsString], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{option} expects a value"))
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
    fn parses_the_copyright_command() {
        assert_eq!(
            parse_args(strings(&[
                "copyright",
                "/music",
                "--only-missing",
                "--dry-run"
            ]))
            .unwrap(),
            Command::Copyright {
                folder: PathBuf::from("/music"),
                sources: None,
                token: None,
                token_file: None,
                only_missing: true,
                dry_run: true,
                csv: None,
                overwrite: false,
                limits: Limits::default(),
            }
        );
        // No --source is not a default: it is the question left unanswered,
        // which an interactive run then asks.
        assert_eq!(
            parse_args(strings(&["copyright", "/music"])).unwrap(),
            Command::Copyright {
                folder: PathBuf::from("/music"),
                sources: None,
                token: None,
                token_file: None,
                only_missing: false,
                dry_run: false,
                csv: None,
                overwrite: false,
                limits: Limits::default(),
            }
        );
        assert!(parse_args(strings(&["copyright"])).is_err());
        assert!(parse_args(strings(&["copyright", "/music", "/other"])).is_err());
    }

    #[test]
    fn parses_a_chosen_source_and_its_token() {
        assert_eq!(
            parse_args(strings(&[
                "copyright",
                "/music",
                "--source",
                "musicbrainz",
                "--token-file",
                "contact.txt",
            ]))
            .unwrap(),
            Command::Copyright {
                folder: PathBuf::from("/music"),
                sources: Some(vec![Source::MusicBrainz]),
                token: None,
                token_file: Some(PathBuf::from("contact.txt")),
                only_missing: false,
                dry_run: false,
                csv: None,
                overwrite: false,
                limits: Limits::default(),
            }
        );
        // The --option=value spelling works too, and so do the short names.
        assert_eq!(
            parse_args(strings(&["copyright", "/music", "--source=discogs"])).unwrap(),
            Command::Copyright {
                folder: PathBuf::from("/music"),
                sources: Some(vec![Source::Discogs]),
                token: None,
                token_file: None,
                only_missing: false,
                dry_run: false,
                csv: None,
                overwrite: false,
                limits: Limits::default(),
            }
        );
    }

    #[test]
    fn parses_the_change_report_options() {
        let Command::Copyright {
            csv,
            overwrite,
            dry_run,
            ..
        } = parse_args(strings(&[
            "copyright",
            "/music",
            "--dry-run",
            "--csv",
            "changes.csv",
            "--overwrite",
        ]))
        .unwrap()
        else {
            panic!("expected the copyright command");
        };
        assert_eq!(csv, Some(PathBuf::from("changes.csv")));
        assert!(overwrite);
        assert!(dry_run);

        // The --option=value spelling works here too.
        let Command::Copyright { csv, .. } =
            parse_args(strings(&["copyright", "/music", "--csv=out.csv"])).unwrap()
        else {
            panic!("expected the copyright command");
        };
        assert_eq!(csv, Some(PathBuf::from("out.csv")));
    }

    #[test]
    fn rejects_a_report_option_that_would_do_nothing() {
        // --overwrite with no report to write is a typo worth catching rather
        // than a no-op worth ignoring.
        let error = parse_args(strings(&["copyright", "/music", "--overwrite"])).unwrap_err();
        assert!(error.contains("--overwrite only means something with --csv"));
        assert!(parse_args(strings(&["copyright", "/music", "--csv"])).is_err());
    }

    #[test]
    fn rejects_a_source_that_does_not_exist_or_a_token_given_twice() {
        let error =
            parse_args(strings(&["copyright", "/music", "--source", "bandcamp"])).unwrap_err();
        assert!(error.contains("unknown copyright source"));
        assert!(parse_args(strings(&["copyright", "/music", "--source"])).is_err());
        // A comma-separated list is a fallback chain, de-duplicated in order.
        let Command::Copyright { sources, .. } = parse_args(strings(&[
            "copyright",
            "/music",
            "--source",
            "spotify,musicbrainz,spotify,itunes",
        ]))
        .unwrap() else {
            panic!("expected the copyright command");
        };
        assert_eq!(
            sources,
            Some(vec![Source::Spotify, Source::MusicBrainz, Source::Itunes])
        );
        assert!(
            parse_args(strings(&[
                "copyright",
                "/music",
                "--token",
                "a",
                "--token-file",
                "b",
            ]))
            .is_err()
        );
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
