use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

const OUTPUT_TEMPLATE: &str = "{artists} - {title} [{track-id}].{output-ext}";
/// Every download uses the same audio format, overwrite policy, and lyrics
/// options so each run reproduces exactly what the input file asks for.
const FIXED_ARGUMENTS: &[&str] = &[
    "--overwrite",
    "force",
    "--format",
    "mp3",
    "--lyrics",
    "synced",
    "--generate-lrc",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcessResult {
    pub(super) success: bool,
    pub(super) code: Option<i32>,
    pub(super) output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OfficialConfigRisk {
    pub(super) path: PathBuf,
    pub(super) settings: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Classification {
    Success,
    PremiumRequired,
    Authentication,
    Forbidden,
    QuotaExceeded(Option<u64>),
    RateLimited(Option<u64>),
    FreeClientUnavailable,
    DenoRequired,
    Network,
    NotFound,
    Failed,
}

pub(super) fn verify(program: &str) -> Result<String, String> {
    let result = Command::new(resolve_program(program))
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!("cannot run `{program}`: {error}. Install spotDL or pass --spotdl <PATH>.")
        })?;

    if !result.status.success() {
        let details = String::from_utf8_lossy(&result.stderr).trim().to_owned();
        return Err(if details.is_empty() {
            format!("`{program} --version` exited unsuccessfully")
        } else {
            format!("`{program} --version` failed: {details}")
        });
    }

    let stdout = String::from_utf8_lossy(&result.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

fn resolve_program(program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() == 1 {
        return path.to_path_buf();
    }

    env::current_dir()
        .map(|directory| directory.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn official_config_risk() -> Result<Option<OfficialConfigRisk>, String> {
    let Some(path) = spotdl_config_paths()
        .into_iter()
        .find(|candidate| candidate.is_file())
    else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("cannot inspect spotDL config {}: {error}", path.display()))?;
    let settings = forcing_config_settings(&contents);
    Ok((!settings.is_empty()).then_some(OfficialConfigRisk { path, settings }))
}

fn spotdl_config_paths() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    let modern = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("spotdl")
        .join("config.json");
    let legacy = home.join(".spotdl").join("config.json");
    if cfg!(target_os = "windows") {
        vec![legacy, modern]
    } else {
        vec![modern, legacy]
    }
}

fn forcing_config_settings(contents: &str) -> Vec<&'static str> {
    if json_bool(contents, "load_config") == Some(false) {
        return Vec::new();
    }

    let mut settings = Vec::new();
    for key in ["use_official_api", "use_cache_file", "user_auth"] {
        if json_bool(contents, key) == Some(true) {
            settings.push(key);
        }
    }
    if json_value(contents, "auth_token").is_some_and(|value| !value.starts_with("null")) {
        settings.push("auth_token");
    }
    settings
}

fn json_bool(contents: &str, key: &str) -> Option<bool> {
    let value = json_value(contents, key)?;
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\"");
    let after_key = contents.get(contents.find(&marker)? + marker.len()..)?;
    let after_colon = after_key.get(after_key.find(':')? + 1..)?;
    Some(after_colon.trim_start())
}

pub(super) fn download(
    program: &str,
    output_dir: &Path,
    query: &str,
    official_api: bool,
    token: Option<&str>,
) -> Result<ProcessResult, String> {
    let mut command = download_command(program, output_dir, query, official_api, token);
    run_relayed(&mut command, program)
}

/// Build `spotdl ... download "YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL"`.
///
/// The pair is passed as a single argument; spotDL reads the part before `|`
/// as the audio source and the part after it as the metadata source.
fn download_command(
    program: &str,
    output_dir: &Path,
    query: &str,
    official_api: bool,
    token: Option<&str>,
) -> Command {
    let mut command = Command::new(resolve_program(program));
    if official_api {
        command.arg("--use-official-api");
        if let Some(token) = token {
            command.arg("--auth-token").arg(token);
        }
    }
    command
        .args(FIXED_ARGUMENTS)
        .arg("--print-errors")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(OUTPUT_TEMPLATE)
        .arg("download")
        .arg(query)
        .current_dir(output_dir);
    command
}

pub(super) fn download_deno(program: &str) -> Result<ProcessResult, String> {
    let mut command = Command::new(resolve_program(program));
    command.arg("--download-deno");
    run_relayed(&mut command, program)
}

fn run_relayed(command: &mut Command, program: &str) -> Result<ProcessResult, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start `{program}`: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cannot capture spotDL stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "cannot capture spotDL stderr".to_owned())?;
    let stdout_thread = relay(stdout, false);
    let stderr_thread = relay(stderr, true);

    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for spotDL: {error}"))?;
    let stdout_text = join_relay(stdout_thread, "stdout")?;
    let stderr_text = join_relay(stderr_thread, "stderr")?;
    let output = if stdout_text.is_empty() {
        stderr_text
    } else if stderr_text.is_empty() {
        stdout_text
    } else {
        format!("{stdout_text}\n{stderr_text}")
    };

    Ok(ProcessResult {
        success: status.success(),
        code: status.code(),
        output,
    })
}

fn relay<R>(reader: R, to_stderr: bool) -> thread::JoinHandle<Result<String, String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut collected = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|error| format!("cannot read spotDL output: {error}"))?;
            if read == 0 {
                break;
            }
            if to_stderr {
                let mut stream = io::stderr().lock();
                stream
                    .write_all(&line)
                    .and_then(|_| stream.flush())
                    .map_err(|error| format!("cannot relay spotDL stderr: {error}"))?;
            } else {
                let mut stream = io::stdout().lock();
                stream
                    .write_all(&line)
                    .and_then(|_| stream.flush())
                    .map_err(|error| format!("cannot relay spotDL stdout: {error}"))?;
            }
            collected.extend_from_slice(&line);
        }
        Ok(String::from_utf8_lossy(&collected).into_owned())
    })
}

fn join_relay(
    handle: thread::JoinHandle<Result<String, String>>,
    stream: &str,
) -> Result<String, String> {
    handle
        .join()
        .map_err(|_| format!("spotDL {stream} relay thread panicked"))?
}

pub(super) fn classify(result: &ProcessResult) -> Classification {
    let text = result.output.to_ascii_lowercase();
    let has_success_marker = text.contains("downloaded \"")
        || (text.contains("skipping ")
            && (text.contains("file already exists") || text.contains("duplicate")));
    let has_reported_failure = contains_any(
        &text,
        &[
            "audioprovidererror",
            "failed to download",
            "an error occurred",
            "traceback (most recent call last)",
        ],
    );

    if text.contains("active premium subscription required for the owner of the app")
        || (text.contains("premium subscription") && text.contains("owner of the app"))
    {
        return Classification::PremiumRequired;
    }
    if text.contains("your application has reached a rate/request limit")
        || text.contains("quota_exceeded")
    {
        return Classification::QuotaExceeded(parse_retry_after(&text));
    }
    if contains_any(
        &text,
        &[
            "http status: 429",
            "429 client error",
            "too many requests",
            "rate/request limit",
            "quota_exceeded",
            "retry will occur after",
        ],
    ) {
        return Classification::RateLimited(parse_retry_after(&text));
    }
    if contains_any(
        &text,
        &[
            "could not get session auth tokens",
            "couldn't get session auth tokens",
            "spotipyfree client is unavailable",
        ],
    ) {
        return Classification::FreeClientUnavailable;
    }
    if contains_any(
        &text,
        &[
            "http status: 401",
            "401 client error",
            "invalid access token",
            "access token expired",
            "token has expired",
            "authentication token is invalid",
        ],
    ) {
        return Classification::Authentication;
    }
    if contains_any(
        &text,
        &["http status: 403", "403 client error", "forbidden for url"],
    ) {
        return Classification::Forbidden;
    }
    if has_success_marker && !has_reported_failure {
        return Classification::Success;
    }
    let has_deno_hint = contains_any(
        &text,
        &[
            "some youtube downloads require deno",
            "run spotdl --download-deno",
            "install deno system-wide",
            "no supported javascript runtime could be found",
        ],
    );
    let has_youtube_download_failure =
        contains_any(&text, &["audioprovidererror", "yt-dlp download error"]);
    if has_deno_hint && has_youtube_download_failure {
        return Classification::DenoRequired;
    }
    if contains_any(
        &text,
        &[
            "temporary failure in name resolution",
            "network is unreachable",
            "connection refused",
            "connection reset",
            "connectionerror",
            "connecttimeout",
            "readtimeout",
            "timed out",
            "502 bad gateway",
            "503 service unavailable",
            "504 gateway timeout",
        ],
    ) {
        return Classification::Network;
    }
    if contains_any(
        &text,
        &[
            "no songs found",
            "no song matches found",
            "couldn't find a match",
            "could not find a match",
            "downloaded 0 song",
        ],
    ) {
        return Classification::NotFound;
    }
    if result.success && !has_reported_failure {
        return Classification::Success;
    }
    Classification::Failed
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn parse_retry_after(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    for marker in ["retry-after", "retry after", "retry will occur after"] {
        if let Some(position) = lower.find(marker) {
            let remainder = &lower[position + marker.len()..];
            let digits: String = remainder
                .chars()
                .skip_while(|character| !character.is_ascii_digit())
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(seconds) = digits.parse() {
                return Some(seconds);
            }
        }
    }
    None
}

pub(super) fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    for word in text.split_whitespace() {
        let candidate =
            word.trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
        let mut parts = candidate.split('.');
        let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse(), minor.parse()) else {
            continue;
        };
        let patch = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        return Some((major, minor, patch));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        Classification, ProcessResult, classify, download_command, forcing_config_settings,
        parse_retry_after, parse_version,
    };
    use std::path::Path;

    const PAIR: &str =
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/abc123";

    fn arguments(official_api: bool, token: Option<&str>) -> Vec<String> {
        download_command("spotdl", Path::new("downloads"), PAIR, official_api, token)
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    fn result(success: bool, output: &str) -> ProcessResult {
        ProcessResult {
            success,
            code: if success { Some(0) } else { Some(1) },
            output: output.into(),
        }
    }

    #[test]
    fn premium_error_wins_over_transient_wording() {
        let output = "Temporary network/service failure\nSpotifyException: http status: 403 - Active premium subscription required for the owner of the app";
        assert_eq!(
            classify(&result(false, output)),
            Classification::PremiumRequired
        );
    }

    #[test]
    fn classifies_long_rate_limit_and_delay() {
        let output = "429 Too Many Requests. Retry will occur after: 86400 s";
        assert_eq!(
            classify(&result(false, output)),
            Classification::RateLimited(Some(86_400))
        );
        assert_eq!(parse_retry_after("Retry-After: 17"), Some(17));
    }

    #[test]
    fn application_rate_limit_is_a_quota_stop() {
        let output =
            "Your application has reached a rate/request limit. Retry will occur after: 5 s";
        assert_eq!(
            classify(&result(false, output)),
            Classification::QuotaExceeded(Some(5))
        );
        assert_eq!(
            classify(&result(false, r#"{\"reason\":\"QUOTA_EXCEEDED\"}"#)),
            Classification::QuotaExceeded(None)
        );
    }

    #[test]
    fn every_download_forces_mp3_and_synced_lyrics_for_the_exact_pair() {
        let args = arguments(false, None);
        let expected = [
            "--overwrite",
            "force",
            "--format",
            "mp3",
            "--lyrics",
            "synced",
            "--generate-lrc",
        ];
        let start = args
            .iter()
            .position(|argument| argument == "--overwrite")
            .expect("the overwrite policy is always passed");
        assert_eq!(args[start..start + expected.len()], expected);

        let download = args
            .iter()
            .position(|argument| argument == "download")
            .expect("the download subcommand is always passed");
        assert_eq!(args[download + 1], PAIR);
        assert_eq!(args.len(), download + 2);
    }

    #[test]
    fn token_free_command_does_not_force_the_official_api() {
        let args = arguments(false, None);
        assert!(!args.iter().any(|argument| argument == "--use-official-api"));
        assert!(!args.iter().any(|argument| argument == "--auth-token"));
        assert!(!args.iter().any(|argument| argument == "--use-cache-file"));
    }

    #[test]
    fn official_command_is_an_explicit_opt_in_without_metadata_cache() {
        let args = arguments(true, Some("secret-token"));
        assert!(args.iter().any(|argument| argument == "--use-official-api"));
        assert!(args.iter().any(|argument| argument == "--auth-token"));
        assert!(args.iter().any(|argument| argument == "secret-token"));
        assert!(!args.iter().any(|argument| argument == "--use-cache-file"));
    }

    #[test]
    fn relative_spotdl_paths_survive_the_download_working_directory() {
        let command = download_command("./tools/spotdl", Path::new("downloads"), PAIR, false, None);
        assert!(Path::new(command.get_program()).is_absolute());
    }

    #[test]
    fn parses_spotdl_versions() {
        assert_eq!(parse_version("spotDL 4.5.2"), Some((4, 5, 2)));
        assert_eq!(parse_version("4.5"), Some((4, 5, 0)));
        assert_eq!(parse_version("spotDL unknown"), None);
    }

    #[test]
    fn spots_config_that_would_silently_restore_the_official_api() {
        let risky = r#"{
            "load_config": true,
            "auth_token": "old-token",
            "use_cache_file": true,
            "use_official_api": true,
            "user_auth": false
        }"#;
        assert_eq!(
            forcing_config_settings(risky),
            vec!["use_official_api", "use_cache_file", "auth_token"]
        );

        let disabled = r#"{
            "load_config": false,
            "auth_token": "old-token",
            "use_official_api": true
        }"#;
        assert!(forcing_config_settings(disabled).is_empty());

        let safe = r#"{
            "load_config": true,
            "auth_token": null,
            "use_cache_file": false,
            "use_official_api": false,
            "user_auth": false
        }"#;
        assert!(forcing_config_settings(safe).is_empty());
    }

    #[test]
    fn classifies_missing_deno_but_not_a_harmless_warning() {
        let failure = "Some YouTube downloads require Deno. Run spotdl --download-deno or install Deno system-wide.\nAudioProviderError: YT-DLP download error";
        assert_eq!(
            classify(&result(false, failure)),
            Classification::DenoRequired
        );

        let success = "Some YouTube downloads require Deno. Run spotdl --download-deno or install Deno system-wide.\nDownloaded \"Artist - Song\"";
        assert_eq!(classify(&result(true, success)), Classification::Success);
    }

    #[test]
    fn successful_download_and_skip_are_successes() {
        assert_eq!(
            classify(&result(true, "Downloaded \"Artist - Song\"")),
            Classification::Success
        );
        assert_eq!(
            classify(&result(
                true,
                "Skipping Artist - Song (file already exists)"
            )),
            Classification::Success
        );
    }

    #[test]
    fn status_zero_with_reported_failure_is_not_success() {
        assert_eq!(
            classify(&result(true, "An error occurred\nFailed to download song")),
            Classification::Failed
        );
    }

    #[test]
    fn partial_success_with_audio_provider_error_is_not_success() {
        let output = "Downloaded \"Artist - Song One\"\nAudioProviderError: YT-DLP download error";
        assert_eq!(classify(&result(true, output)), Classification::Failed);
    }
}
