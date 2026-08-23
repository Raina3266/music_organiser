use std::env;
use std::path::{Path, PathBuf};

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

    Ok(ParsedCommand::Run(Config {
        input,
        output,
        spotdl,
        official_api,
        auth_token,
        token_file,
        non_interactive,
        auto_download_deno,
        max_attempts,
        max_rate_limit_wait,
    }))
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
        "{package} download - download one YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL pair per line

USAGE:
    {package} download [OPTIONS] <INPUT_FILE>

OPTIONS:
    -o, --output <DIR>                Output directory [default: ~/Documents/Music]
        --spotdl <PROGRAM>            spotDL executable [default: spotdl]
        --official-api                Opt in to Spotify's official Web API and its quotas
        --auth-token <TOKEN>          Official mode: short-lived Spotify access token
        --token-file <FILE>           Official mode: read an access token from a file
        --non-interactive             Never prompt for Deno or an expired official token
        --auto-download-deno          Let spotDL install Deno if YouTube requires it
        --max-attempts <N>            Attempts for genuine network failures [default: 3]
        --max-rate-limit-wait <SECS>  Longest Retry-After delay to wait [default: 300]
    -h, --help                        Print help
    -V, --version                     Print version

INPUT FORMAT:
    Every non-comment line must be an exact-source pair:

        https://music.youtube.com/watch?v=VIDEO_ID|https://open.spotify.com/track/TRACK_ID

    The left URL pins the audio spotDL downloads; the right URL supplies the
    Spotify track metadata. Each pair runs as:

        spotdl download \"PAIR\" --overwrite force --format mp3 \\
              --lyrics synced --generate-lrc

SYNCED LYRICS:
    spotDL embeds only untimed USLT lyrics, so every generated .lrc file is
    parsed into an ID3v2.3 SYLT frame with millisecond timestamps. The USLT
    frame is removed, the frame is read back from the file to verify it, and
    only then is the .lrc file deleted. If any of that fails, the .lrc file is
    kept and the pair is added to the retry list.

SPOTIFY MODE:
    The default uses spotDL's token-free client. --official-api is an explicit
    fallback; only then are --auth-token, --token-file, and SPOTIFY_AUTH_TOKEN used.

ENVIRONMENT:
    SPOTDL_PROGRAM                    Alternative spotDL executable or path

Blank lines and lines beginning with # are ignored.",
        package = env!("CARGO_PKG_NAME")
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_positive_u32, set_input, validate_official_options};
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
    fn token_options_require_explicit_official_mode() {
        assert!(validate_official_options(false, Some("token"), None).is_err());
        assert!(validate_official_options(false, None, Some(Path::new("token.txt"))).is_err());
        assert!(validate_official_options(true, Some("token"), None).is_ok());
        assert!(validate_official_options(false, None, None).is_ok());
    }
}
