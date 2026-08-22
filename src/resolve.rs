use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use serde::Deserialize;

pub const HELP: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " resolve - turn free-text track descriptions into Spotify URLs

USAGE:
    ",
    env!("CARGO_PKG_NAME"),
    " resolve <INPUT_FILE> <OUTPUT_FILE>

ENVIRONMENT:
    SPOTIFY_CLIENT_ID       Spotify app client ID
    SPOTIFY_CLIENT_SECRET   Spotify app client secret

Each non-empty, non-comment input line is searched as a track. The first
Spotify result is written to the corresponding output line. Missing tracks and
per-line errors become # comments, so the output can be passed directly to the
download command.
"
);

const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_SEARCH_URL: &str = "https://api.spotify.com/v1/search";
const DELAY_BETWEEN_REQUESTS: Duration = Duration::from_millis(150);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RATE_LIMIT_RETRIES: u32 = 5;
const MAX_RATE_LIMIT_WAIT: u64 = 300;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Report {
    pub total: usize,
    pub resolved: usize,
    pub not_found: usize,
    pub errors: usize,
    pub output: PathBuf,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    tracks: Option<TracksObject>,
}

#[derive(Debug, Deserialize)]
struct TracksObject {
    items: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    name: String,
    artists: Vec<Artist>,
    external_urls: ExternalUrls,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ExternalUrls {
    spotify: String,
}

#[derive(Debug, Eq, PartialEq)]
struct TrackMatch {
    url: String,
    description: String,
}

pub fn run(config: Config) -> Result<Report, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the Spotify lookup runtime: {error}"))?;
    runtime.block_on(run_async(config))
}

async fn run_async(config: Config) -> Result<Report, String> {
    let content = fs::read_to_string(&config.input)
        .map_err(|error| format!("cannot read {}: {error}", config.input.display()))?;
    let client_id = required_environment("SPOTIFY_CLIENT_ID")?;
    let client_secret = required_environment("SPOTIFY_CLIENT_SECRET")?;

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|error| format!("cannot create the Spotify HTTP client: {error}"))?;

    eprintln!("Authenticating with Spotify...");
    let token = get_access_token(&client, &client_id, &client_secret).await?;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let mut resolved = 0;
    let mut not_found = 0;
    let mut errors = 0;
    let mut results = Vec::with_capacity(total);

    for (index, raw_line) in lines.iter().enumerate() {
        let query = raw_line.trim();
        if query.is_empty() {
            results.push(String::new());
            continue;
        }
        if query.starts_with('#') {
            results.push(raw_line.to_string());
            continue;
        }

        eprint!("[{}/{}] \"{}\" -> ", index + 1, total, query);
        let _ = io::stderr().flush();

        match search_track(&client, &token, query).await {
            Ok(Some(track)) => {
                eprintln!("{}\n         {}", track.description, track.url);
                results.push(track.url);
                resolved += 1;
            }
            Ok(None) => {
                eprintln!("NOT FOUND");
                results.push(format!("# NOT FOUND: {query}"));
                not_found += 1;
            }
            Err(error) => {
                eprintln!("ERROR: {error}");
                results.push(format!("# ERROR ({}): {query}", one_line(&error)));
                errors += 1;
            }
        }

        tokio::time::sleep(DELAY_BETWEEN_REQUESTS).await;
    }

    let output = if results.is_empty() {
        String::new()
    } else {
        format!("{}\n", results.join("\n"))
    };
    fs::write(&config.output, output)
        .map_err(|error| format!("cannot write {}: {error}", config.output.display()))?;

    Ok(Report {
        total,
        resolved,
        not_found,
        errors,
        output: config.output,
    })
}

fn required_environment(name: &str) -> Result<String, String> {
    let value = env::var(name).map_err(|_| format!("missing {name} environment variable"))?;
    if value.trim().is_empty() {
        return Err(format!("{name} cannot be empty"));
    }
    Ok(value)
}

async fn get_access_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
) -> Result<String, String> {
    let response = client
        .post(SPOTIFY_TOKEN_URL)
        .basic_auth(client_id, Some(client_secret))
        .form(&[("grant_type", "client_credentials")])
        .send()
        .await
        .map_err(|error| format!("Spotify authentication request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Spotify authentication failed (HTTP {status}): {}. Check SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET.",
            one_line(&body)
        ));
    }

    let token: TokenResponse = response
        .json()
        .await
        .map_err(|error| format!("Spotify returned an invalid token response: {error}"))?;
    if token.access_token.trim().is_empty() {
        return Err("Spotify returned an empty access token".to_owned());
    }
    Ok(token.access_token)
}

async fn search_track(
    client: &reqwest::Client,
    token: &str,
    query: &str,
) -> Result<Option<TrackMatch>, String> {
    for retry in 0..=MAX_RATE_LIMIT_RETRIES {
        let response = client
            .get(SPOTIFY_SEARCH_URL)
            .bearer_auth(token)
            .query(&[("q", query), ("type", "track"), ("limit", "1")])
            .send()
            .await
            .map_err(|error| format!("Spotify search request failed: {error}"))?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = retry_after(&response).unwrap_or(1).max(1);
            if retry == MAX_RATE_LIMIT_RETRIES {
                return Err(format!(
                    "Spotify kept rate-limiting the search after {} attempts",
                    MAX_RATE_LIMIT_RETRIES + 1
                ));
            }
            if wait > MAX_RATE_LIMIT_WAIT {
                return Err(format!(
                    "Spotify requested a {wait}-second wait, exceeding the {MAX_RATE_LIMIT_WAIT}-second safety limit"
                ));
            }

            eprintln!(
                "rate limited; waiting {wait}s before attempt {}/{}...",
                retry + 2,
                MAX_RATE_LIMIT_RETRIES + 1
            );
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Spotify search failed (HTTP {status}): {}",
                one_line(&body)
            ));
        }

        let response: SearchResponse = response
            .json()
            .await
            .map_err(|error| format!("Spotify returned an invalid search response: {error}"))?;
        return Ok(first_track(response));
    }

    unreachable!("the rate-limit loop always returns or continues")
}

fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn first_track(response: SearchResponse) -> Option<TrackMatch> {
    response.tracks?.items.into_iter().next().map(|track| {
        let artists = track
            .artists
            .into_iter()
            .map(|artist| artist.name)
            .collect::<Vec<_>>()
            .join(", ");
        TrackMatch {
            url: track.external_urls.spotify,
            description: format!("\"{}\" by {artists}", track.name),
        }
    })
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{SearchResponse, TrackMatch, first_track, one_line};

    #[test]
    fn extracts_the_first_track() {
        let response: SearchResponse = serde_json::from_str(
            r#"{
                "tracks": {
                    "items": [
                        {
                            "name": "Get Lucky",
                            "artists": [{"name": "Daft Punk"}, {"name": "Pharrell Williams"}],
                            "external_urls": {
                                "spotify": "https://open.spotify.com/track/abc123"
                            }
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            first_track(response),
            Some(TrackMatch {
                url: "https://open.spotify.com/track/abc123".to_owned(),
                description: "\"Get Lucky\" by Daft Punk, Pharrell Williams".to_owned(),
            })
        );
    }

    #[test]
    fn supports_an_empty_search_result() {
        let response: SearchResponse = serde_json::from_str(r#"{"tracks":{"items":[]}}"#).unwrap();
        assert_eq!(first_track(response), None);
    }

    #[test]
    fn flattens_error_messages_for_comment_output() {
        assert_eq!(
            one_line("first line\n  second line"),
            "first line second line"
        );
    }
}
