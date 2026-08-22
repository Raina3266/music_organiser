pub mod cli;
mod input;
mod output;
mod spotdl;
mod token;

use self::cli::Config;
use self::input::Entry;
use self::spotdl::Classification;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_TOKEN_REPLACEMENTS: u32 = 3;
const MAX_TOKEN_PROMPTS: u32 = 3;

/// Download every unique Spotify link in `config.input` through spotDL.
///
/// A return value of `0` means every entry completed; `1` means a retry list
/// was written for failed or unattempted entries.
pub fn run(config: Config) -> Result<i32, String> {
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
    if !config.official_api
        && let Some(risk) = spotdl::official_config_risk()?
    {
        return Err(format!(
            "spotDL config {} enables official-only setting(s): {}. The download command stopped before downloading so token-free mode is not silently overridden. Set load_config to false, clear those settings, or rerun with --official-api intentionally.",
            risk.path.display(),
            risk.settings.join(", ")
        ));
    }
    println!(
        "Loaded {} unique Spotify link(s) from {}.",
        input.entries.len(),
        config.input.display()
    );
    if input.duplicate_count > 0 {
        println!("Ignored {} duplicate link(s).", input.duplicate_count);
    }
    println!("Downloads will be stored in {}.", config.output.display());
    if config.official_api {
        println!("Spotify metadata mode: official Web API (quota limits apply).");
    } else {
        println!("Spotify metadata mode: spotDL token-free client.");
        if env::var_os("SPOTIFY_AUTH_TOKEN").is_some() {
            eprintln!("Note: SPOTIFY_AUTH_TOKEN is ignored unless --official-api is supplied.");
        }
    }

    let interactive = !config.non_interactive && io::stdin().is_terminal();
    let mut auth_token = initial_token(&config)?;
    let total = input.entries.len();
    let mut completed = 0usize;
    let mut failures = Vec::new();
    let mut stop = None;
    let mut deno_install_attempted = false;

    for (index, entry) in input.entries.iter().enumerate() {
        println!("\n[{}/{}] Downloading {}", index + 1, total, entry.url);
        match download_entry(
            &config,
            entry,
            &mut auth_token,
            interactive,
            &mut deno_install_attempted,
        )? {
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
    if let Some(stop) = &stop {
        let remaining = total.saturating_sub(stop.entry_index + 1);
        println!("Not attempted: {remaining}");
        println!("Reason for stopping: {}", stop.reason);
        println!(
            "Rerun the same command after fixing the reported problem; existing files will be skipped."
        );
    }

    let pending_start = stop
        .as_ref()
        .map(|stop| stop.entry_index + 1)
        .unwrap_or(total);
    let failed_urls_path = output::write_failed_urls(
        &config.output,
        failures.iter().map(|failure| failure.url.as_str()).chain(
            input.entries[pending_start..]
                .iter()
                .map(|entry| entry.url.as_str()),
        ),
    )?;
    println!("Retry URL list: {}", failed_urls_path.display());

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
            &entry.url,
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
                    "spotDL could not find downloadable audio for this Spotify link".into(),
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
    url: String,
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
            url: entry.url.clone(),
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
            failure.url,
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
