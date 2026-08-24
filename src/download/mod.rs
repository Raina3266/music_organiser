pub mod cli;
mod input;
mod output;
mod spotdl;
pub(crate) mod token;

use self::cli::Config;
use self::input::Entry;
use self::spotdl::Classification;
use crate::files::{self, MusicSnapshot};
use crate::metadata::{self, MetadataReport};
use crate::sources::{DEFAULT_MAX_WAIT, itunes::Client as ItunesClient};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_TOKEN_REPLACEMENTS: u32 = 3;
const MAX_TOKEN_PROMPTS: u32 = 3;

/// Download every unique line in `config.input` through spotDL, then rewrite
/// each file's ID3v2.3 tag: drop the POPM rating, store the Spotify copyright,
/// and paste the generated `.lrc` file into the ordinary USLT lyrics frame.
///
/// A line is a Spotify URL, a YouTube Music URL, or a
/// `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` exact-source pair, and the three may
/// be mixed freely in one file.
///
/// A return value of `0` means every line downloaded and had its metadata
/// applied; `1` means a retry list was written for failed or unattempted
/// lines.
pub fn run(mut config: Config) -> Result<i32, String> {
    let input = input::load(&config.input)?;
    fs::create_dir_all(&config.output)
        .map_err(|error| format!("cannot create {}: {error}", config.output.display()))?;

    let version = spotdl::verify(&config.spotdl)?;
    if version.is_empty() {
        println!("Found spotDL.");
    } else {
        println!("Found {version}.");
    }
    if let Some((major, minor, _)) = spotdl::parse_version(&version) {
        if (major, minor) < (4, 5) {
            return Err(format!(
                "spotDL 4.5.0 or newer is required for token-free mode; found {version}"
            ));
        }
    } else {
        eprintln!(
            "Warning: could not parse the spotDL version. The download command requires spotDL 4.5.0 or newer."
        );
    }
    println!(
        "Loaded {} unique download(s) from {}: {}.",
        input.entries.len(),
        config.input.display(),
        input.summary()
    );
    if input.duplicate_count > 0 {
        println!("Ignored {} duplicate line(s).", input.duplicate_count);
    }
    println!("Downloads will be stored in {}.", config.output.display());

    // The token is asked for before anything is downloaded, because it decides
    // which Spotify metadata source the whole run uses.
    let interactive = !config.non_interactive && io::stdin().is_terminal();
    let mut auth_token = startup_token(&mut config, interactive)?;

    // Only a token-free run can be silently turned into an official one by
    // spotDL's own config, so this check waits until the mode is settled.
    if !config.official_api
        && let Some(risk) = spotdl::official_config_risk()?
    {
        return Err(format!(
            "spotDL config {} enables official-only setting(s): {}. The download command stopped before downloading so token-free mode is not silently overridden. Set load_config to false, clear those settings, or rerun with --official-api intentionally.",
            risk.path.display(),
            risk.settings.join(", ")
        ));
    }

    if config.official_api {
        println!("Spotify metadata mode: official Web API (quota limits apply).");
        if auth_token.is_none() {
            eprintln!(
                "Note: official mode was requested without a token, so spotDL falls back to its own credentials."
            );
        }
    } else {
        println!("Spotify metadata mode: spotDL token-free client; TSRC stays empty.");
        if env::var_os("SPOTIFY_AUTH_TOKEN").is_some() {
            eprintln!("Note: SPOTIFY_AUTH_TOKEN is ignored unless --official-api is supplied.");
        }
    }

    let mut itunes = if config.no_copyright {
        println!("Copyright lookup: disabled with --no-copyright.");
        None
    } else {
        println!("Copyright will be looked up per album through the iTunes Search API.");
        Some(ItunesClient::new(DEFAULT_MAX_WAIT)?)
    };
    let total = input.entries.len();
    let mut completed = 0usize;
    let mut failures = Vec::new();
    let mut stop = None;
    let mut deno_install_attempted = false;
    let mut metadata_totals = MetadataReport::default();

    for (index, entry) in input.entries.iter().enumerate() {
        println!("\n[{}/{}] Downloading {}", index + 1, total, entry.query);
        // The snapshot is what tells this run apart from everything already in
        // the output directory, so a line that names no single Spotify track
        // still finds the files it just produced.
        let outcome = match files::snapshot_music_files(&config.output) {
            Err(error) => EntryOutcome::Abort(format!(
                "cannot scan {} before downloading: {error}",
                config.output.display()
            )),
            Ok(before) => {
                let outcome = download_entry(
                    &config,
                    entry,
                    &mut auth_token,
                    interactive,
                    &mut deno_install_attempted,
                )?;
                if matches!(outcome, EntryOutcome::Completed) {
                    apply_metadata(
                        &config,
                        entry,
                        &before,
                        itunes.as_mut(),
                        &mut metadata_totals,
                    )
                } else {
                    outcome
                }
            }
        };
        match outcome {
            EntryOutcome::Completed => completed += 1,
            EntryOutcome::Failed(reason) => {
                eprintln!("Failed: {reason}");
                failures.push(Failure::new(entry, reason));
            }
            EntryOutcome::Abort(reason) => {
                eprintln!("Stopped: {reason}");
                failures.push(Failure::new(entry, reason.clone()));
                stop = Some(Stop {
                    entry_index: index,
                    reason,
                });
                break;
            }
        }
    }

    println!("\nCompleted: {completed}");
    println!("Failed: {}", failures.len());
    println!(
        "Metadata: updated {} file(s); stripped {} rating/encoder/year frame(s); wrote {} copyright message(s).",
        metadata_totals.files_updated,
        metadata_totals.frames_stripped,
        metadata_totals.copyrights_written
    );
    if metadata_totals.copyright_lookups_failed > 0 {
        println!(
            "The iTunes copyright lookup failed for {} file(s); everything else in their tags was still written.",
            metadata_totals.copyright_lookups_failed
        );
    }
    println!(
        "Language: detected from the lyrics for {} of {} file(s); the rest use {}.",
        metadata_totals.languages_detected, metadata_totals.files_updated, config.language.name
    );
    println!(
        "Lyrics: pasted {} .lrc file(s) into USLT ({} line(s)); removed {} SYLT frame(s).",
        metadata_totals.lyrics_embedded,
        metadata_totals.lines_embedded,
        metadata_totals.sylt_frames_removed
    );
    println!(
        "Names: dropped the track ID from {} file name(s); {} replaced an earlier download.",
        metadata_totals.files_renamed, metadata_totals.files_replaced
    );
    if !metadata_totals.failures.is_empty() {
        println!(
            "{} file(s) could not be finished; any .lrc file was kept and the line is in the retry list.",
            metadata_totals.failures.len()
        );
    }
    if let Some(stop) = &stop {
        let remaining = total.saturating_sub(stop.entry_index + 1);
        println!("Not attempted: {remaining}");
        println!("Reason for stopping: {}", stop.reason);
        println!(
            "Rerun the same command after fixing the reported problem; spotDL uses --overwrite force, so every retried line is downloaded again."
        );
    }

    let pending_start = stop
        .as_ref()
        .map(|stop| stop.entry_index + 1)
        .unwrap_or(total);
    let retry_path = output::write_retry_queries(
        &config.output,
        failures.iter().map(|failure| failure.query.as_str()).chain(
            input.entries[pending_start..]
                .iter()
                .map(|entry| entry.query.as_str()),
        ),
    )?;
    println!("Retry list: {}", retry_path.display());

    if !failures.is_empty() {
        let report = write_failure_report(&config.output, &failures, stop.as_ref())?;
        println!("Failure report: {}", report.display());
    }

    Ok(if failures.is_empty() && stop.is_none() {
        0
    } else {
        1
    })
}

/// Rewrite the tag of every file spotDL just wrote for this entry.
///
/// The copyright lookup searches iTunes with the album artist and album spotDL
/// recorded in the tag. A file that cannot be finished is reported as a failure
/// so its line lands in the retry list, and its `.lrc` sidecar is left on disk.
fn apply_metadata(
    config: &Config,
    entry: &Entry,
    before: &MusicSnapshot,
    mut itunes: Option<&mut ItunesClient>,
    totals: &mut MetadataReport,
) -> EntryOutcome {
    let files = match downloaded_files(config, entry, before) {
        Ok(files) => files,
        Err(error) => {
            return EntryOutcome::Abort(format!(
                "cannot scan {} for downloaded audio: {error}",
                config.output.display()
            ));
        }
    };
    if files.is_empty() {
        return EntryOutcome::Failed(format!(
            "spotDL reported success but wrote no audio file into {}",
            config.output.display()
        ));
    }

    let mut report = MetadataReport::default();
    let mut lookups_failed = 0;
    for file in &files {
        // A failed lookup must not cost the rest of the tag work. iTunes needs
        // no account and throttles freely, so a miss is left as a warning and
        // the copyright frame is simply not touched.
        let copyright = match itunes.as_deref_mut() {
            Some(client) => match album_copyright(client, file) {
                Ok(copyright) => copyright,
                Err(error) => {
                    eprintln!("Copyright: {error}");
                    lookups_failed += 1;
                    None
                }
            },
            None => None,
        };
        report.absorb(metadata::finalize(
            file,
            copyright.as_deref(),
            &config.language,
        ));
        // The track ID is only there to tie a file back to the line that asked
        // for it, which is done by now. A file whose tag could not be finished
        // is renamed too, so a retry of that line replaces it instead of
        // leaving both names behind.
        match files::drop_track_id_suffix(file) {
            Ok(Some(rename)) => {
                report.files_renamed += 1;
                if rename.replaced {
                    report.files_replaced += 1;
                    println!(
                        "Renamed to {}, replacing an earlier download of the same name.",
                        rename.target.display()
                    );
                }
            }
            Ok(None) => {}
            // The audio and its tag are finished; only the name is untidy.
            Err(error) => eprintln!(
                "Name: cannot drop the track ID from {}: {error}",
                file.display()
            ),
        }
    }
    report.copyright_lookups_failed = lookups_failed;

    let reasons = report
        .failures
        .iter()
        .map(|failure| format!("{}: {}", failure.path.display(), failure.message))
        .collect::<Vec<_>>();
    for reason in &reasons {
        eprintln!("Metadata: {reason}");
    }
    if report.files_updated > 0 {
        let lyrics = if report.lyrics_embedded > 0 {
            format!(
                ", pasted {} line(s) of synced lyrics into USLT and deleted the .lrc file",
                report.lines_embedded
            )
        } else {
            String::new()
        };
        let language = if report.languages_detected > 0 {
            "detected from the lyrics"
        } else {
            "taken from --language"
        };
        println!(
            "Updated the tag: stripped {} rating/encoder/year frame(s), wrote {} copyright message(s), language {language}{lyrics}.",
            report.frames_stripped, report.copyrights_written
        );
    }

    let outcome = if reasons.is_empty() {
        EntryOutcome::Completed
    } else {
        EntryOutcome::Failed(format!(
            "the audio downloaded but its metadata was not finished ({})",
            reasons.join("; ")
        ))
    };
    totals.absorb(report);
    outcome
}

/// The audio spotDL just wrote for one entry.
///
/// Comparing the output directory against the snapshot taken before the
/// download covers every line form, including an album or playlist that
/// expanded into many songs. spotDL's `[{track-id}]` file name is only a
/// fallback for a line that names a single track, in case the comparison
/// missed a rewrite of a file that was already there.
fn downloaded_files(
    config: &Config,
    entry: &Entry,
    before: &MusicSnapshot,
) -> io::Result<Vec<PathBuf>> {
    let written = before.files_written_since(&config.output)?;
    if !written.is_empty() {
        return Ok(written);
    }
    match entry.source.track_id() {
        Some(track_id) => files::music_files_for_track(&config.output, track_id),
        None => Ok(Vec::new()),
    }
}

/// Look up one file's album on iTunes, using the tag spotDL wrote.
fn album_copyright(client: &mut ItunesClient, audio: &Path) -> Result<Option<String>, String> {
    let Some((artist, album)) = metadata::album_of(audio) else {
        println!("The downloaded file has no album artist and album to search iTunes with.");
        return Ok(None);
    };
    let copyright = client
        .copyright(&artist, &album)
        .map_err(|error| error.to_string())?;
    if copyright.is_none() {
        println!("iTunes has no matching album for \"{artist} - {album}\".");
    }
    Ok(copyright)
}

/// The token this run starts with, asking for one when nothing supplied it.
///
/// The prompt is skipped when a token already came from an option, a file, or
/// the environment, and when the run is not interactive, so scripted runs
/// behave exactly as before.
fn startup_token(config: &mut Config, interactive: bool) -> Result<Option<String>, String> {
    if let Some(token) = initial_token(config)? {
        return Ok(Some(token));
    }
    if !interactive {
        return Ok(None);
    }

    let mut attempt = 1;
    loop {
        match token::prompt_before_downloading() {
            Ok(token) => return Ok(adopt_token(config, token)),
            Err(error) if attempt < MAX_TOKEN_PROMPTS => {
                eprintln!("Invalid token: {error}");
                attempt += 1;
            }
            Err(error) => {
                return Err(format!(
                    "invalid token after {MAX_TOKEN_PROMPTS} attempts: {error}"
                ));
            }
        }
    }
}

/// Fold the answer to the startup prompt into the run's configuration.
///
/// Supplying a token is what turns official mode on, because a token is only
/// ever passed to spotDL alongside `--use-official-api`. An empty answer leaves
/// the default token-free mode alone.
fn adopt_token(config: &mut Config, token: Option<String>) -> Option<String> {
    if token.is_some() {
        config.official_api = true;
    }
    token
}

fn initial_token(config: &Config) -> Result<Option<String>, String> {
    if !config.official_api {
        return Ok(None);
    }
    if let Some(raw_token) = &config.auth_token {
        return token::clean_token(raw_token).map(Some);
    }
    if let Some(path) = &config.token_file {
        return token::read_token_file(path).map(Some);
    }
    if let Ok(raw_token) = env::var("SPOTIFY_AUTH_TOKEN")
        && !raw_token.trim().is_empty()
    {
        return token::clean_token(&raw_token).map(Some);
    }
    Ok(None)
}

fn download_entry(
    config: &Config,
    entry: &Entry,
    auth_token: &mut Option<String>,
    interactive: bool,
    deno_install_attempted: &mut bool,
) -> Result<EntryOutcome, String> {
    let mut network_attempt = 1u32;
    let mut token_replacements = 0u32;
    let mut waited_for_rate_limit = false;

    loop {
        let result = spotdl::download(
            &config.spotdl,
            &config.output,
            &entry.query,
            config.official_api,
            auth_token.as_deref(),
        )?;
        match spotdl::classify(&result) {
            Classification::Success => return Ok(EntryOutcome::Completed),
            Classification::PremiumRequired => {
                return Ok(EntryOutcome::Abort(
                    "Spotify returned 403: the official API app owner needs an active Premium subscription. A replacement token from the same app will not fix this; rerun without --official-api to use token-free mode."
                        .into(),
                ));
            }
            Classification::Authentication => {
                if !config.official_api {
                    return Ok(EntryOutcome::Abort(
                        "spotDL's token-free Spotify metadata session was rejected. Retry later, update spotDL, or explicitly use --official-api with valid credentials."
                            .into(),
                    ));
                }
                let reason = "Spotify rejected or expired the official-API access token.";
                if !interactive || token_replacements >= MAX_TOKEN_REPLACEMENTS {
                    return Ok(EntryOutcome::Abort(format!(
                        "{reason} Supply a fresh token with SPOTIFY_AUTH_TOKEN, --token-file, or --auth-token."
                    )));
                }
                if !replace_token(auth_token, reason)? {
                    return Ok(EntryOutcome::Abort(reason.into()));
                }
                token_replacements += 1;
                waited_for_rate_limit = false;
            }
            Classification::Forbidden => {
                return Ok(EntryOutcome::Abort(
                    "Spotify returned 403. In official mode, verify the Premium app owner and user allowlist; otherwise retry token-free mode. Token rotation is not attempted."
                        .into(),
                ));
            }
            Classification::QuotaExceeded(retry_after) => {
                let delay = retry_after
                    .map(|seconds| format!(" Retry-After was {seconds} seconds."))
                    .unwrap_or_default();
                return Ok(EntryOutcome::Abort(format!(
                    "Spotify reported an application quota limit.{delay} The download command will not sleep for a day or request another token because tokens from the same developer account can share this quota. The current and remaining URLs were preserved in output.txt."
                )));
            }
            Classification::RateLimited(retry_after) => {
                if let Some(seconds) = retry_after
                    && !waited_for_rate_limit
                    && seconds <= config.max_rate_limit_wait
                {
                    let seconds = seconds.max(1);
                    eprintln!(
                        "Spotify asked us to wait {seconds} second(s); respecting Retry-After before one retry."
                    );
                    thread::sleep(Duration::from_secs(seconds));
                    waited_for_rate_limit = true;
                    continue;
                }

                let delay = retry_after
                    .map(|seconds| format!(" Retry-After was {seconds} seconds."))
                    .unwrap_or_default();
                let reason = format!(
                    "Spotify rate-limited metadata requests.{delay} The download command will not wait longer than {} seconds or rotate tokens. The current and remaining URLs were preserved in output.txt.",
                    config.max_rate_limit_wait
                );
                return Ok(EntryOutcome::Abort(reason));
            }
            Classification::FreeClientUnavailable => {
                return Ok(EntryOutcome::Abort(
                    "spotDL's token-free Spotify client could not create a metadata session. Update spotDL and retry later, or use --official-api as an explicit fallback. The current and remaining URLs were preserved in output.txt."
                        .into(),
                ));
            }
            Classification::DenoRequired => {
                let setup = "spotDL needs Deno for this YouTube download.";
                if *deno_install_attempted {
                    return Ok(EntryOutcome::Abort(format!(
                        "{setup} The automatic setup already ran, but spotDL still cannot use Deno. Run `spotdl --download-deno` manually or install Deno system-wide, then rerun this command."
                    )));
                }

                let approved = if config.auto_download_deno {
                    true
                } else if interactive {
                    approve_deno_download()?
                } else {
                    false
                };
                if !approved {
                    return Ok(EntryOutcome::Abort(format!(
                        "{setup} Run `spotdl --download-deno`, then rerun this command. For unattended setup, rerun `{} download` with `--auto-download-deno`.",
                        env!("CARGO_PKG_NAME")
                    )));
                }

                *deno_install_attempted = true;
                eprintln!("Running `{} --download-deno`...", config.spotdl);
                let setup_result = spotdl::download_deno(&config.spotdl)?;
                if !setup_result.success {
                    let status = setup_result
                        .code
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| "terminated by a signal".into());
                    return Ok(EntryOutcome::Abort(format!(
                        "spotDL could not install Deno ({status}). Run `spotdl --download-deno` manually or install Deno system-wide, then rerun this command."
                    )));
                }
                eprintln!("Deno setup completed; retrying the current Spotify link.");
                network_attempt = 1;
            }
            Classification::Network => {
                if network_attempt >= config.max_attempts {
                    return Ok(EntryOutcome::Failed(format!(
                        "temporary network/service failure after {network_attempt} attempt(s)"
                    )));
                }
                let exponent = network_attempt.min(5);
                let delay = 1u64 << exponent;
                network_attempt += 1;
                eprintln!(
                    "Temporary network/service failure; waiting {delay} seconds before retry {network_attempt}/{}...",
                    config.max_attempts
                );
                thread::sleep(Duration::from_secs(delay));
            }
            Classification::NotFound => {
                return Ok(EntryOutcome::Failed(
                    "spotDL could not find downloadable audio for this pair".into(),
                ));
            }
            Classification::Failed => {
                let status = result
                    .code
                    .map(|code| format!("exit code {code}"))
                    .unwrap_or_else(|| "terminated by a signal".into());
                return Ok(EntryOutcome::Failed(format!(
                    "spotDL reported a failure ({status})"
                )));
            }
        }
    }
}

fn approve_deno_download() -> Result<bool, String> {
    eprintln!();
    eprintln!(
        "This track requires Deno. spotDL can download a private copy into its user configuration directory."
    );
    for _ in 0..3 {
        eprint!("Run `spotdl --download-deno` now and retry this track? [Y/n]: ");
        io::stderr()
            .flush()
            .map_err(|error| format!("cannot flush the Deno setup prompt: {error}"))?;
        let mut response = String::new();
        io::stdin()
            .read_line(&mut response)
            .map_err(|error| format!("cannot read the Deno setup response: {error}"))?;
        match response.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("Please answer y or n."),
        }
    }
    Err("no valid answer was supplied for Deno setup after 3 attempts".into())
}

fn replace_token(current: &mut Option<String>, reason: &str) -> Result<bool, String> {
    match prompt_for_new_token(current.as_deref(), reason)? {
        Some(replacement) => {
            *current = Some(replacement);
            Ok(true)
        }
        None => Ok(false),
    }
}

fn prompt_for_new_token(current: Option<&str>, reason: &str) -> Result<Option<String>, String> {
    for attempt in 1..=MAX_TOKEN_PROMPTS {
        match token::prompt_for_token(reason) {
            Ok(Some(candidate)) => {
                if current.is_some_and(|existing| existing == candidate.as_str()) {
                    eprintln!("That is the same token; copy a newly issued value.");
                    continue;
                }
                return Ok(Some(candidate));
            }
            Ok(None) => return Ok(None),
            Err(error) if attempt < MAX_TOKEN_PROMPTS => {
                eprintln!("Invalid token: {error}");
            }
            Err(error) => {
                return Err(format!(
                    "invalid token after {MAX_TOKEN_PROMPTS} attempts: {error}"
                ));
            }
        }
    }
    Err(format!(
        "no new token was supplied after {MAX_TOKEN_PROMPTS} attempts"
    ))
}

#[derive(Debug)]
enum EntryOutcome {
    Completed,
    Failed(String),
    Abort(String),
}

#[derive(Debug)]
struct Failure {
    line: usize,
    query: String,
    reason: String,
}

#[derive(Debug)]
struct Stop {
    entry_index: usize,
    reason: String,
}

impl Failure {
    fn new(entry: &Entry, reason: String) -> Self {
        Self {
            line: entry.line,
            query: entry.query.clone(),
            reason,
        }
    }
}

fn write_failure_report(
    output_dir: &Path,
    failures: &[Failure],
    stop: Option<&Stop>,
) -> Result<std::path::PathBuf, String> {
    let path = output_dir.join(format!("{}-download-failures.txt", env!("CARGO_PKG_NAME")));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut report = format!(
        "{} download failure report\nrun_unix_time={timestamp}\n",
        env!("CARGO_PKG_NAME")
    );
    if let Some(stop) = stop {
        report.push_str(&format!(
            "stopped_after_input_index={}\nstop_reason={}\n",
            stop.entry_index + 1,
            one_line(&stop.reason)
        ));
    }
    report.push('\n');
    for failure in failures {
        report.push_str(&format!(
            "line {}\t{}\t{}\n",
            failure.line,
            failure.query,
            one_line(&failure.reason)
        ));
    }
    fs::write(&path, report)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{adopt_token, initial_token};
    use crate::download::cli::Config;
    use std::path::PathBuf;

    fn config() -> Config {
        Config {
            input: PathBuf::from("links.txt"),
            output: PathBuf::from("downloads"),
            spotdl: "spotdl".into(),
            official_api: false,
            auth_token: None,
            token_file: None,
            non_interactive: false,
            auto_download_deno: false,
            no_copyright: false,
            language: crate::lyrics::parse_language("English").unwrap(),
            max_attempts: 3,
            max_rate_limit_wait: 300,
        }
    }

    #[test]
    fn a_pasted_token_turns_on_official_mode() {
        let mut config = config();
        let token = adopt_token(&mut config, Some("test-token-12345678901234567890".into()));

        assert_eq!(token.as_deref(), Some("test-token-12345678901234567890"));
        assert!(config.official_api);
    }

    #[test]
    fn declining_the_prompt_keeps_the_token_free_default() {
        let mut config = config();
        assert_eq!(adopt_token(&mut config, None), None);
        assert!(!config.official_api);
    }

    #[test]
    fn an_option_supplied_token_is_used_without_prompting() {
        let mut config = config();
        config.official_api = true;
        config.auth_token = Some("Bearer test-token-12345678901234567890".into());

        assert_eq!(
            initial_token(&config).unwrap().as_deref(),
            Some("test-token-12345678901234567890")
        );
    }
}
