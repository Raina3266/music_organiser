use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// Songs are grouped into one folder per album, named `{Album Artist} || {Album}`.
///
/// spotDL creates the folder and sanitizes both values for the filesystem, so a
/// track lands beside the rest of its album without any file moving here.
const OUTPUT_TEMPLATE: &str =
    "{album-artist} || {album}/{artists} - {title} [{track-id}].{output-ext}";
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
    /// YouTube Music answered spotDL's search with something its API client
    /// could not read, so no candidate recording was ever considered.
    AudioSearchUnavailable,
    /// The search named a recording and yt-dlp could not fetch audio from it.
    AudioUndownloadable,
    DenoRequired,
    Network,
    NotFound,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AudioSearch {
    /// The input already contains the exact YouTube recording to download.
    Pinned,
    /// Search YouTube Music, accepting only verified results.
    Verified,
    /// Search YouTube Music without spotDL's verified-result restriction.
    Unverified,
    /// Search YouTube itself, through spotDL's yt-dlp provider.
    ///
    /// spotDL reaches YouTube Music through the ytmusicapi package and YouTube
    /// through yt-dlp, so this is the only search left when the YouTube Music
    /// API stops answering.
    PlainYouTube,
}

impl AudioSearch {
    /// The provider left when YouTube Music itself will not answer.
    ///
    /// Both YouTube Music modes go through the same API, so dropping the
    /// verified-result restriction would only ask it the same question again
    /// and collect the same refusal: plain YouTube is the one search that does
    /// not depend on it.
    ///
    /// A pinned input falls back too, even though it searches for nothing.
    /// Naming no provider leaves spotDL on its default, which is YouTube
    /// Music, and spotDL runs a YouTube Music connectivity check on startup
    /// whenever that provider is loaded — before it wraps the work in a
    /// handler, so a refusal there takes the process down with it. Asking for
    /// plain YouTube skips that check and cannot change what is downloaded:
    /// the recording is already pinned, and the audio is fetched with yt-dlp
    /// either way.
    pub(super) fn without_youtube_music(self) -> Option<Self> {
        match self {
            AudioSearch::Pinned | AudioSearch::Verified | AudioSearch::Unverified => {
                Some(AudioSearch::PlainYouTube)
            }
            AudioSearch::PlainYouTube => None,
        }
    }

    /// The next search to try when the recording this one found could not be
    /// downloaded.
    ///
    /// Unlike a silent YouTube Music, the API answered perfectly well here —
    /// what it named was refused by YouTube itself. Each rung offers a wider
    /// field of recordings to pick a different one from, so verification goes
    /// first and YouTube Music only after that. A pinned input is never
    /// widened: it asked for one exact recording, and quietly fetching some
    /// other one is not a fallback but a different song.
    pub(super) fn widened(self) -> Option<Self> {
        match self {
            AudioSearch::Verified => Some(AudioSearch::Unverified),
            AudioSearch::Unverified => Some(AudioSearch::PlainYouTube),
            AudioSearch::Pinned | AudioSearch::PlainYouTube => None,
        }
    }

    /// The next provider to try when this search returned no candidates.
    ///
    /// An empty verified search only proves that no official recording was
    /// found, so the next attempt keeps YouTube Music and relaxes that filter.
    /// An empty unverified search has exhausted YouTube Music altogether; the
    /// remaining useful fallback is yt-dlp's plain YouTube search, which also
    /// avoids ytmusicapi and its startup connectivity check.
    pub(super) fn after_not_found(self) -> Option<Self> {
        self.widened()
    }

    /// How a run names this search to the person watching it.
    pub(super) fn describe(self) -> &'static str {
        match self {
            AudioSearch::Pinned => "the YouTube recording the input pinned",
            AudioSearch::Verified => "verified YouTube Music results",
            AudioSearch::Unverified => "unverified YouTube Music results",
            AudioSearch::PlainYouTube => "spotDL's plain YouTube provider",
        }
    }
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

/// The yt-dlp options a run may hand to spotDL untouched.
///
/// YouTube serves audio to an anonymous request less and less reliably, and
/// neither answer to that lives in this program: cookies come from a browser
/// the person is already signed into, and the rest is whatever yt-dlp needs on
/// the day. Both are passed straight through.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct YtDlpOptions<'a> {
    pub(super) cookie_file: Option<&'a Path>,
    pub(super) extra_arguments: Option<&'a str>,
}

pub(super) fn download(
    program: &str,
    output_dir: &Path,
    query: &str,
    audio_search: AudioSearch,
    official_api: bool,
    token: Option<&str>,
    yt_dlp: YtDlpOptions<'_>,
) -> Result<ProcessResult, String> {
    let mut command = download_command(
        program,
        output_dir,
        query,
        audio_search,
        official_api,
        token,
        yt_dlp,
    );
    run_relayed(&mut command, program)
}

/// Build `spotdl ... download "YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL"`.
///
/// The pair is passed as a single argument; spotDL reads the part before `|`
/// as the audio source and the part after it as the metadata source, so the
/// input parser has already put the two URLs in that order.
fn download_command(
    program: &str,
    output_dir: &Path,
    query: &str,
    audio_search: AudioSearch,
    official_api: bool,
    token: Option<&str>,
    yt_dlp: YtDlpOptions<'_>,
) -> Command {
    let mut command = Command::new(resolve_program(program));
    if official_api {
        command.arg("--use-official-api");
        if let Some(token) = token {
            command.arg("--auth-token").arg(token);
        }
    }
    // A bare Spotify link leaves the audio choice to spotDL. Search YouTube
    // Music's verified results first, then let the caller relax verification
    // after a miss and leave YouTube Music altogether when its API will not
    // answer. Exact-source pairs and YouTube URLs already pin the audio.
    //
    // `--only-verified-results` is never passed with the plain YouTube
    // provider: nothing yt-dlp finds is marked verified, so the flag would
    // discard every result it returned.
    match audio_search {
        AudioSearch::Pinned => {}
        AudioSearch::Verified => {
            command
                .arg("--audio")
                .arg("youtube-music")
                .arg("--only-verified-results");
        }
        AudioSearch::Unverified => {
            command.arg("--audio").arg("youtube-music");
        }
        AudioSearch::PlainYouTube => {
            command.arg("--audio").arg("youtube");
        }
    }
    if let Some(path) = yt_dlp.cookie_file {
        command.arg("--cookie-file").arg(path);
    }
    if let Some(arguments) = yt_dlp.extra_arguments {
        // Written as one `--option=value` argument rather than two. These are
        // yt-dlp options, so the value all but always begins with a dash, and
        // spotDL's argument parser reads a separate word beginning with a dash
        // as the next option rather than as this one's value. Joining them
        // leaves no such word: everything after the first `=` is the value,
        // its own `=` signs included.
        command.arg(format!("--yt-dlp-args={arguments}"));
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
    let song_errors = song_error_classes(&result.output);
    let has_success_marker = text.contains("downloaded \"")
        || (text.contains("skipping ")
            && (text.contains("file already exists") || text.contains("duplicate")));
    let has_reported_failure = !song_errors.is_empty()
        || contains_any(
            &text,
            &[
                "audioprovidererror",
                "failed to download",
                "an error occurred",
                "traceback (most recent call last)",
                "song is missing required fields",
                "error occurred while reinitializing song",
            ],
        );

    if youtube_music_would_not_answer(&text, &song_errors) {
        return Classification::AudioSearchUnavailable;
    }
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
    // Checked after the Deno case, which is this same provider error with a
    // cause spotDL names outright. Everything left is YouTube refusing to
    // serve a recording the search had already found, so the answer is another
    // recording rather than another search engine.
    if song_errors
        .iter()
        .any(|class| class.eq_ignore_ascii_case("AudioProviderError"))
    {
        return Classification::AudioUndownloadable;
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
            "no results found",
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

/// Whether the run failed because YouTube Music would not answer spotDL.
///
/// spotDL reaches YouTube Music through ytmusicapi, which decodes a reply
/// before it looks at the status code, so a page served in place of results
/// surfaces as a JSON error rather than as an HTTP one. It reaches spotDL from
/// two places, and neither can be read from the exit status alone:
///
/// * a search made for one song, which spotDL catches and reports on that
///   song's `--print-errors` line;
/// * its startup connectivity check, which runs before spotDL wraps the work
///   in a handler at all, so it escapes as a traceback and takes the whole
///   process down.
///
/// The second is why the frames are read as well as the report lines. The same
/// JSON error raised while reading Spotify **metadata** also arrives as a
/// traceback, and no audio provider would rescue it, so a traceback counts
/// only when its frames name ytmusicapi as the thing that could not be read.
fn youtube_music_would_not_answer(text: &str, song_errors: &[&str]) -> bool {
    let unreadable_reply = contains_any(
        text,
        &[
            "jsondecodeerror",
            "expecting value: line 1 column 1",
            "ytmusicservererror",
            "ytmusicgatederror",
        ],
    );
    if song_errors.iter().any(|class| {
        matches!(
            class.to_ascii_lowercase().as_str(),
            "jsondecodeerror" | "ytmusicservererror" | "ytmusicgatederror"
        )
    }) {
        return true;
    }

    // A rich traceback boxes its frames and wraps long paths mid-word, so a
    // marker is matched wherever it survives that intact rather than as a
    // whole path. Each of these appears several times in the frames the
    // startup check produces.
    unreadable_reply
        && contains_any(
            text,
            &["ytmusicapi", "check_ytmusic_connection", "ytmusic.py"],
        )
}

/// The exception classes spotDL named on its `--print-errors` report lines.
///
/// spotDL exits successfully even when every song in a run failed, so a zero
/// exit status says nothing on its own. What it does print, once per failed
/// song, is `SOURCE_URL - ExceptionClass: message`, and that line is the only
/// dependable sign that a run which reported no other trouble still downloaded
/// nothing. The class name also says which half of the job failed, so an audio
/// search that could not be read is not confused with a metadata one.
fn song_error_classes(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| {
            // The source URL comes first and holds no spaces, so the first
            // ` - ` always separates it from the exception, whatever the
            // message that follows contains.
            let (source, error) = line.split_once(" - ")?;
            if !source.contains("://") {
                return None;
            }
            let class = error.split_once(':')?.0;
            (!class.is_empty()
                && !class.contains(char::is_whitespace)
                && (class.ends_with("Error") || class.ends_with("Exception")))
            .then_some(class)
        })
        .collect()
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
        AudioSearch, Classification, ProcessResult, YtDlpOptions, classify, download_command,
        forcing_config_settings, parse_retry_after, parse_version,
    };
    use std::path::Path;

    const PAIR: &str =
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/abc123";

    fn arguments(official_api: bool, token: Option<&str>) -> Vec<String> {
        download_command(
            "spotdl",
            Path::new("downloads"),
            PAIR,
            AudioSearch::Pinned,
            official_api,
            token,
            YtDlpOptions::default(),
        )
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
    fn a_missing_audio_result_is_available_for_the_unverified_fallback() {
        let output = "LookupError: No results found for song: Artist - Song";
        assert_eq!(classify(&result(false, output)), Classification::NotFound);
    }

    #[test]
    fn an_empty_youtube_music_search_eventually_leaves_youtube_music() {
        assert_eq!(
            AudioSearch::Verified.after_not_found(),
            Some(AudioSearch::Unverified)
        );
        assert_eq!(
            AudioSearch::Unverified.after_not_found(),
            Some(AudioSearch::PlainYouTube)
        );
        assert_eq!(AudioSearch::PlainYouTube.after_not_found(), None);
        assert_eq!(AudioSearch::Pinned.after_not_found(), None);
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
    fn groups_downloads_into_one_folder_per_album() {
        let args = arguments(false, None);
        let template = args
            .iter()
            .position(|argument| argument == "--output")
            .map(|index| args[index + 1].clone())
            .expect("the output template is always passed");
        let (folder, file) = template
            .split_once('/')
            .expect("the template puts each song in an album folder");
        assert_eq!(folder, "{album-artist} || {album}");
        assert_eq!(file, "{artists} - {title} [{track-id}].{output-ext}");
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
    fn a_spotify_audio_search_accepts_only_verified_youtube_music_results() {
        let query = "https://open.spotify.com/track/abc123";
        let args = download_command(
            "spotdl",
            Path::new("downloads"),
            query,
            AudioSearch::Verified,
            false,
            None,
            YtDlpOptions::default(),
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        let audio = args
            .iter()
            .position(|argument| argument == "--audio")
            .expect("Spotify searches always choose the YouTube Music provider");
        assert_eq!(args[audio + 1], "youtube-music");
        assert!(
            args.iter()
                .any(|argument| argument == "--only-verified-results")
        );
    }

    #[test]
    fn the_fallback_keeps_youtube_music_but_relaxes_verification() {
        let query = "https://open.spotify.com/track/abc123";
        let args = download_command(
            "spotdl",
            Path::new("downloads"),
            query,
            AudioSearch::Unverified,
            false,
            None,
            YtDlpOptions::default(),
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        let audio = args
            .iter()
            .position(|argument| argument == "--audio")
            .expect("the fallback still searches YouTube Music");
        assert_eq!(args[audio + 1], "youtube-music");
        assert!(
            !args
                .iter()
                .any(|argument| argument == "--only-verified-results")
        );
    }

    #[test]
    fn yt_dlp_options_are_handed_to_spotdl_untouched() {
        let args = download_command(
            "spotdl",
            Path::new("downloads"),
            PAIR,
            AudioSearch::Pinned,
            false,
            None,
            YtDlpOptions {
                cookie_file: Some(Path::new("/home/me/cookies.txt")),
                extra_arguments: Some("--extractor-args youtube:player_client=web"),
            },
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        let cookies = args
            .iter()
            .position(|argument| argument == "--cookie-file")
            .expect("the cookie file is passed when one was given");
        assert_eq!(args[cookies + 1], "/home/me/cookies.txt");

        // Joined into one argument: spotDL parses these with Python's argparse,
        // which refuses a separate value beginning with a dash, and a yt-dlp
        // option always begins with one. The value keeps its own `=` intact.
        assert!(
            args.iter()
                .any(|argument| argument
                    == "--yt-dlp-args=--extractor-args youtube:player_client=web"),
            "the value is joined to its option, dashes and all: {args:?}"
        );
    }

    #[test]
    fn a_lone_yt_dlp_flag_is_joined_to_its_option_too() {
        let args = download_command(
            "spotdl",
            Path::new("downloads"),
            PAIR,
            AudioSearch::Pinned,
            false,
            None,
            YtDlpOptions {
                cookie_file: None,
                extra_arguments: Some("--ignore-no-formats-error"),
            },
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(
            args.iter()
                .any(|argument| argument == "--yt-dlp-args=--ignore-no-formats-error"),
            "{args:?}"
        );
    }

    #[test]
    fn a_run_that_asks_for_neither_passes_neither() {
        let args = arguments(false, None);
        assert!(!args.iter().any(|argument| argument == "--cookie-file"));
        assert!(
            !args
                .iter()
                .any(|argument| argument.starts_with("--yt-dlp-args"))
        );
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
        let command = download_command(
            "./tools/spotdl",
            Path::new("downloads"),
            PAIR,
            AudioSearch::Pinned,
            false,
            None,
            YtDlpOptions::default(),
        );
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
    fn a_json_error_reported_per_song_is_a_failed_audio_search() {
        // spotDL exits 0 here: every song failed, and only the report lines say so.
        let output = "Processing query: https://open.spotify.com/track/7kg7gCtbQF6zPk0dKpsWTY\n\
             JSONDecodeError: Expecting value: line 1 column 1 (char 0)\n\
             https://open.spotify.com/track/7kg7gCtbQF6zPk0dKpsWTY - JSONDecodeError: Expecting value: line 1 column 1 (char 0)";
        assert_eq!(
            classify(&result(true, output)),
            Classification::AudioSearchUnavailable
        );

        let gated = "https://open.spotify.com/track/abc123 - YTMusicServerError: Server returned HTTP 400: Bad Request.";
        assert_eq!(
            classify(&result(true, gated)),
            Classification::AudioSearchUnavailable
        );
    }

    #[test]
    fn the_same_json_error_from_the_metadata_half_is_not_an_audio_search_failure() {
        // Reading Spotify metadata happens before the download loop, so its
        // failure escapes as a traceback rather than a per-song report line.
        // No audio search was reached, so changing the provider would not help.
        let output = "Processing query: https://open.spotify.com/track/abc123\n\
             An error occurred\n\
             Traceback (most recent call last):\n\
             site-packages/spotdl/types/song.py:84 in from_url\n\
             site-packages/spotdl/utils/spotify.py:141 in _get\n\
             JSONDecodeError: Expecting value: line 1 column 1 (char 0)";
        assert_eq!(classify(&result(false, output)), Classification::Failed);
    }

    #[test]
    fn the_startup_check_crashing_on_ytmusicapi_is_a_failed_audio_search() {
        // spotDL runs its YouTube Music connectivity check before it wraps the
        // work in a handler, so a refusal there escapes as a traceback and
        // exits non-zero. Naming plain YouTube skips the check entirely, which
        // is exactly the fallback this classification asks for.
        let output = "Traceback (most recent call last):\n\
             site-packages/spotdl/console/entry_point.py:100 in entry_point\n\
             if not check_ytmusic_connection():\n\
             site-packages/spotdl/providers/audio/ytmusic.py:73 in get_results\n\
             python3.14-ytmusicapi-1.12.2/ytmusicapi/ytmusic.py:246 in _send_request\n\
             JSONDecodeError: Expecting value: line 1 column 1 (char 0)";
        assert_eq!(
            classify(&result(false, output)),
            Classification::AudioSearchUnavailable
        );
    }

    #[test]
    fn a_reported_song_error_outweighs_a_successful_exit_status() {
        let output =
            "https://open.spotify.com/track/abc123 - DownloaderError: Failed to embed metadata";
        assert_eq!(classify(&result(true, output)), Classification::Failed);
        assert_eq!(
            classify(&result(
                true,
                "Song is missing required fields: Artist - Song"
            )),
            Classification::Failed
        );
    }

    #[test]
    fn an_ordinary_download_line_is_not_read_as_an_error_report() {
        let output = "Downloaded \"Artist - Song\": https://music.youtube.com/watch?v=dQw4w9WgXcQ";
        assert!(super::song_error_classes(output).is_empty());
        assert_eq!(classify(&result(true, output)), Classification::Success);
    }

    #[test]
    fn a_recording_that_will_not_download_is_not_a_search_failure() {
        let output = "https://open.spotify.com/track/abc123 - AudioProviderError: YT-DLP download error - https://www.youtube.com/watch?v=XTjgRONsYLg";
        assert_eq!(
            classify(&result(true, output)),
            Classification::AudioUndownloadable
        );

        let format = "https://open.spotify.com/track/abc123 - AudioProviderError: ERROR: [youtube] mcDLVzATOnY: Requested format is not available";
        assert_eq!(
            classify(&result(true, format)),
            Classification::AudioUndownloadable
        );
    }

    #[test]
    fn a_missing_deno_still_wins_over_the_provider_error_it_arrives_as() {
        // Deno is that same yt-dlp failure with a cause spotDL names, and it
        // has a fix of its own, so widening the search must not swallow it.
        let output = "Some YouTube downloads require Deno. Run spotdl --download-deno or install Deno system-wide.\n\
             https://open.spotify.com/track/abc123 - AudioProviderError: YT-DLP download error - https://www.youtube.com/watch?v=abc";
        assert_eq!(
            classify(&result(false, output)),
            Classification::DenoRequired
        );
    }

    #[test]
    fn an_undownloadable_recording_widens_the_field_a_rung_at_a_time() {
        // The API answered here, so unlike a silent YouTube Music the
        // unverified rung is worth having: it offers different recordings.
        assert_eq!(
            AudioSearch::Verified.widened(),
            Some(AudioSearch::Unverified)
        );
        assert_eq!(
            AudioSearch::Unverified.widened(),
            Some(AudioSearch::PlainYouTube)
        );
        assert_eq!(AudioSearch::PlainYouTube.widened(), None);
        // A pinned input asked for one recording; another one is a different
        // song, not a fallback.
        assert_eq!(AudioSearch::Pinned.widened(), None);
    }

    #[test]
    fn a_silent_youtube_music_takes_both_of_its_modes_down_together() {
        // Relaxing verification would ask the same unanswering API again, so
        // either YouTube Music mode falls straight through to plain YouTube.
        assert_eq!(
            AudioSearch::Verified.without_youtube_music(),
            Some(AudioSearch::PlainYouTube)
        );
        assert_eq!(
            AudioSearch::Unverified.without_youtube_music(),
            Some(AudioSearch::PlainYouTube)
        );
        assert_eq!(AudioSearch::PlainYouTube.without_youtube_music(), None);
    }

    #[test]
    fn a_pinned_input_names_a_provider_only_to_escape_the_startup_check() {
        // Nothing is searched for, but spotDL's default provider is YouTube
        // Music and loading it costs a connectivity check that can take the
        // process down.
        assert_eq!(
            AudioSearch::Pinned.without_youtube_music(),
            Some(AudioSearch::PlainYouTube)
        );

        let pinned = download_command(
            "spotdl",
            Path::new("downloads"),
            PAIR,
            AudioSearch::Pinned,
            false,
            None,
            YtDlpOptions::default(),
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert!(
            !pinned.iter().any(|argument| argument == "--audio"),
            "a first attempt still leaves the provider to spotDL"
        );
    }

    #[test]
    fn the_last_audio_search_leaves_youtube_music_and_asks_for_no_verification() {
        let query = "https://open.spotify.com/track/abc123";
        let args = download_command(
            "spotdl",
            Path::new("downloads"),
            query,
            AudioSearch::PlainYouTube,
            false,
            None,
            YtDlpOptions::default(),
        )
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        let audio = args
            .iter()
            .position(|argument| argument == "--audio")
            .expect("the last fallback still names a provider");
        assert_eq!(args[audio + 1], "youtube");
        assert!(
            !args
                .iter()
                .any(|argument| argument == "--only-verified-results")
        );
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
