use std::env;
use std::path::{Path, PathBuf};

use crate::lyrics::{Language, parse_language};

/// Language recorded when the lyrics cannot settle one.
const DEFAULT_LANGUAGE: &str = "English";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
    pub spotdl: String,
    pub official_api: bool,
    pub auth_token: Option<String>,
    pub token_file: Option<PathBuf>,
    pub non_interactive: bool,
    pub auto_download_deno: bool,
    /// Skip the iTunes copyright lookup.
    pub no_copyright: bool,
    /// Skip the MusicBrainz language lookup and read the lyrics instead.
    pub no_language_lookup: bool,
    /// Recorded when the lyrics cannot settle the language.
    pub language: Language,
    pub max_attempts: u32,
    pub max_rate_limit_wait: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    Run(Config),
    Help,
    Version,
}

pub fn parse_args<I>(args: I) -> Result<ParsedCommand, String>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut spotdl = env::var("SPOTDL_PROGRAM").unwrap_or_else(|_| "spotdl".into());
    let mut official_api = false;
    let mut auth_token = None;
    let mut token_file = None;
    let mut non_interactive = false;
    let mut auto_download_deno = false;
    let mut no_copyright = false;
    let mut no_language_lookup = false;
    let mut language = DEFAULT_LANGUAGE.to_owned();
    let mut max_attempts = 3;
    let mut max_rate_limit_wait = 300;

    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-h" | "--help" => return Ok(ParsedCommand::Help),
            "-V" | "--version" => return Ok(ParsedCommand::Version),
            "--official-api" => official_api = true,
            "--non-interactive" => non_interactive = true,
            "--auto-download-deno" => auto_download_deno = true,
            "--no-copyright" => no_copyright = true,
            "--no-language-lookup" => no_language_lookup = true,
            "--language" => language = next_value(&args, &mut index, argument)?,
            "-o" | "--output" => {
                output = Some(PathBuf::from(next_value(&args, &mut index, argument)?));
            }
            "--spotdl" => spotdl = next_value(&args, &mut index, argument)?,
            "--auth-token" => auth_token = Some(next_value(&args, &mut index, argument)?),
            "--token-file" => {
                token_file = Some(PathBuf::from(next_value(&args, &mut index, argument)?));
            }
            "--max-attempts" => {
                max_attempts =
                    parse_positive_u32(&next_value(&args, &mut index, argument)?, argument)?;
            }
            "--max-rate-limit-wait" => {
                max_rate_limit_wait = next_value(&args, &mut index, argument)?
                    .parse::<u64>()
                    .map_err(|_| format!("{argument} expects a number of seconds"))?;
            }
            "--" => {
                index += 1;
                while index < args.len() {
                    set_input(&mut input, &args[index])?;
                    index += 1;
                }
                break;
            }
            _ if argument.starts_with("--output=") => {
                output = Some(PathBuf::from(&argument["--output=".len()..]));
            }
            _ if argument.starts_with("--spotdl=") => {
                spotdl = argument["--spotdl=".len()..].to_owned();
            }
            _ if argument.starts_with("--auth-token=") => {
                auth_token = Some(argument["--auth-token=".len()..].to_owned());
            }
            _ if argument.starts_with("--token-file=") => {
                token_file = Some(PathBuf::from(&argument["--token-file=".len()..]));
            }
            _ if argument.starts_with("--language=") => {
                language = argument["--language=".len()..].to_owned();
            }
            _ if argument.starts_with('-') => {
                return Err(format!(
                    "unknown option: {argument}\n\nRun with --help for usage."
                ));
            }
            _ => set_input(&mut input, argument)?,
        }
        index += 1;
    }

    let input =
        input.ok_or_else(|| "missing input file\n\nRun with --help for usage.".to_owned())?;
    let home = home_dir()?;
    let output = expand_tilde(
        output
            .unwrap_or_else(|| home.join("Documents").join("Music"))
            .as_path(),
        &home,
    );
    let input = expand_tilde(&input, &home);
    let token_file = token_file.map(|path| expand_tilde(&path, &home));

    if spotdl.trim().is_empty() {
        return Err("--spotdl cannot be empty".into());
    }
    validate_official_options(official_api, auth_token.as_deref(), token_file.as_deref())?;
    let language = validate_language(&language)?;

    Ok(ParsedCommand::Run(Config {
        input,
        output,
        spotdl,
        official_api,
        auth_token,
        token_file,
        non_interactive,
        auto_download_deno,
        no_copyright,
        no_language_lookup,
        language,
        max_attempts,
        max_rate_limit_wait,
    }))
}

/// Accept a language by English name or by ISO-639-2/639-3 code.
///
/// The tag shows the name, so that is the natural thing to type, but the codes
/// keep working because they are what the frames themselves carry.
fn validate_language(value: &str) -> Result<Language, String> {
    parse_language(value).ok_or_else(|| {
        format!(
            "--language expects a language name or an ISO-639-2 code such as {DEFAULT_LANGUAGE}, Chinese, eng, or zho, not {value:?}"
        )
    })
}

fn validate_official_options(
    official_api: bool,
    auth_token: Option<&str>,
    token_file: Option<&Path>,
) -> Result<(), String> {
    if !official_api && (auth_token.is_some() || token_file.is_some()) {
        return Err(
            "--auth-token and --token-file require --official-api; normal downloads are token-free"
                .into(),
        );
    }
    Ok(())
}

fn next_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_positive_u32(value: &str, option: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{option} expects a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{option} must be at least 1"));
    }
    Ok(parsed)
}

fn set_input(input: &mut Option<PathBuf>, value: &str) -> Result<(), String> {
    if input.is_some() {
        return Err(format!("unexpected second input file: {value}"));
    }
    *input = Some(PathBuf::from(value));
    Ok(())
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot determine the home directory (HOME/USERPROFILE is unset)".into())
}

fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.join(rest);
    }
    path.to_path_buf()
}

pub fn help_text() -> String {
    format!(
        "{package} download - download one Spotify or YouTube Music link per line

USAGE:
    {package} download [OPTIONS] <INPUT_FILE>

OPTIONS:
    -o, --output <DIR>                Output directory [default: ~/Documents/Music]
        --spotdl <PROGRAM>            spotDL executable [default: spotdl]
        --official-api                Opt in to Spotify's official Web API and its quotas
        --auth-token <TOKEN>          Official mode: short-lived Spotify access token
        --token-file <FILE>           Official mode: read an access token from a file
        --non-interactive             Never prompt for a token, Deno, or a replacement
        --auto-download-deno          Let spotDL install Deno if YouTube requires it
        --no-copyright                Skip the iTunes copyright lookup entirely
        --no-language-lookup          Skip the MusicBrainz language lookup and read the
                                      lyrics instead
        --language <LANGUAGE>         Fallback language, by name or code [default: English]
        --max-attempts <N>            Attempts for genuine network failures [default: 3]
        --max-rate-limit-wait <SECS>  Longest Retry-After delay to wait [default: 300]
    -h, --help                        Print help
    -V, --version                     Print version

INPUT FORMAT:
    Every non-comment line is one of three forms, and a single file may mix
    them freely:

        https://open.spotify.com/track/TRACK_ID
        https://music.youtube.com/watch?v=VIDEO_ID
        https://music.youtube.com/watch?v=VIDEO_ID|https://open.spotify.com/track/TRACK_ID

    A Spotify link pins the metadata and lets spotDL search YouTube for the
    audio; track, album, and playlist links are accepted, and an album or a
    playlist downloads every song on it. A YouTube link pins the audio and
    lets spotDL search Spotify for the metadata; /watch and /playlist links
    are accepted, as are youtu.be and www.youtube.com. The pair is
    spotDL's exact-source syntax: the left URL pins the audio, the right URL
    supplies the metadata, and neither side is searched. spotify: URIs work
    wherever a Spotify URL does.

    Each line runs as:

        spotdl download \"LINE\" --overwrite force --format mp3 \\
              --lyrics synced --generate-lrc

    Duplicate lines are ignored after the tracking parameters are stripped.

FILE LAYOUT:
    Songs are grouped into one folder per album, so everything from the same
    album lands together:

        <OUTPUT_DIR>/{{Album Artist}} || {{Album}}/{{artists}} - {{title}}.mp3

    output.txt and the failure report stay in the output directory itself.
    Whatever a line downloads is found by comparing the output directory
    against what was there before it ran, so an album or playlist line has
    every one of its songs tagged.

    spotDL is asked to end each name with the Spotify track ID so a file can
    be tied back to its line. Once the tag is written the suffix has done its
    job and is removed, leaving \"i-dle - Luv U.mp3\". A file already using the
    trimmed name is an earlier download of the same track and is replaced.

METADATA:
    After each download the ID3v2.3 tag is rewritten:

        POPM   the rating frame spotDL writes from Spotify's popularity
               score is removed
        TSSE   the encoder-settings string FFmpeg leaves behind is removed
        TYER   the year spotDL writes beside the whole TDRC date is removed,
               leaving one date frame instead of two
        TCOP   the copyright message, looked up per album on the iTunes
               Search API, which needs no account, key, or token
        TLAN   the language as a readable name - English, Chinese,
               Korean - detected from the lyric text with its timestamps
               stripped, falling back to --language when the lyrics do not
               settle it. ID3v2.3 specifies an ISO-639-2 code here, but the
               name is what a tagger shows; the three-byte language field
               inside the USLT frame keeps the code.
        SYLT   any synchronised-lyrics frame is removed, spotDL's included

    TSRC is left exactly as spotDL wrote it. iTunes publishes no ISRCs, and
    spotDL already fills that frame from its own metadata.

    A copyright is stored only when an iTunes album matches both the album
    artist and the album name in the tag, so a wrong one is never written.
    Use --no-copyright to skip the lookup.

SYNCED LYRICS:
    Every download asks spotDL for time-synced lyrics and for the .lrc file
    that carries them:

        --lyrics synced --generate-lrc

    That .lrc file is then pasted, verbatim and with its [mm:ss.xx] timestamps
    intact, into the ordinary USLT lyrics frame — the frame players actually
    read, and the one whose text the players that understand timed lyrics
    parse the timestamps out of. Any SYLT synchronised-lyrics frame is removed,
    spotDL's own included. The frame is read back from the file to verify it,
    and only then is the .lrc file deleted. If any of that fails, the .lrc file
    is kept and the line is added to the retry list.

SPOTIFY MODE:
    Before the first download, an interactive run asks for a Spotify access
    token:

        Paste an access token, or press Enter to download token-free:

    Pasting one switches the run to Spotify's official Web API, which is the
    only source for the ISRC (TSRC) frame; pressing Enter keeps spotDL's
    token-free client, where TSRC stays empty. A token from a developer-mode
    app needs the app owner to hold Spotify Premium; one copied from the
    open.spotify.com web player does not, but expires within the hour, and a
    replacement is asked for when it does.

    The prompt is skipped when --auth-token, --token-file, or
    SPOTIFY_AUTH_TOKEN already supplied a token, when --non-interactive is
    used, and when stdin is not a terminal, so scripted runs are unchanged.
    --official-api forces official mode whether or not a token is supplied;
    --auth-token and --token-file still require it.

ENVIRONMENT:
    SPOTDL_PROGRAM                    Alternative spotDL executable or path

Blank lines and lines beginning with # are ignored.",
        package = env!("CARGO_PKG_NAME")
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_positive_u32, set_input, validate_language, validate_official_options};
    use std::path::{Path, PathBuf};

    #[test]
    fn positive_integer_rejects_zero() {
        assert!(parse_positive_u32("0", "--max-attempts").is_err());
        assert_eq!(parse_positive_u32("3", "--max-attempts").unwrap(), 3);
    }

    #[test]
    fn only_one_input_is_allowed() {
        let mut input = None;
        set_input(&mut input, "a.txt").unwrap();
        assert_eq!(input, Some(PathBuf::from("a.txt")));
        assert!(set_input(&mut input, "b.txt").is_err());
    }

    #[test]
    fn language_is_taken_by_name_or_by_code() {
        assert_eq!(validate_language("JPN").unwrap().name, "Japanese");
        assert_eq!(validate_language(" fra ").unwrap().name, "French");
        assert_eq!(validate_language("english").unwrap().code, "eng");
        assert_eq!(validate_language("Chinese").unwrap().code, "zho");
        assert!(validate_language("en").is_err());
        assert!(validate_language("e1g").is_err());
    }

    #[test]
    fn token_options_require_explicit_official_mode() {
        assert!(validate_official_options(false, Some("token"), None).is_err());
        assert!(validate_official_options(false, None, Some(Path::new("token.txt"))).is_err());
        assert!(validate_official_options(true, Some("token"), None).is_ok());
        assert!(validate_official_options(false, None, None).is_ok());
    }
}
