//! Minimal Spotify Web API client used to fetch the tags spotDL leaves out.
//!
//! The track request carries the ISRC. Copyright is exposed only on the album
//! object, so a lookup is a track request followed by an album request. Albums
//! are cached, which makes a whole album's worth of tracks cost one album
//! request.

use std::collections::HashMap;
use std::env;
use std::io::{self, Write};
use std::time::Duration;

use serde::Deserialize;

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API_BASE: &str = "https://api.spotify.com/v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RATE_LIMIT_RETRIES: u32 = 5;
const MAX_RATE_LIMIT_WAIT: u64 = 300;
const MAX_CREDENTIAL_PROMPTS: u32 = 3;

/// Client-credentials application credentials.
///
/// These are the Client ID and Client Secret of a Spotify developer app. They
/// are never written to a file or printed back.
#[derive(Clone)]
pub struct Credentials {
    client_id: String,
    client_secret: String,
}

impl Credentials {
    /// Read the credentials from the environment, prompting when they are
    /// missing and a terminal is available.
    pub fn resolve(interactive: bool) -> Result<Self, String> {
        let client_id = environment("SPOTIFY_CLIENT_ID");
        let client_secret = environment("SPOTIFY_CLIENT_SECRET");
        if let (Some(client_id), Some(client_secret)) = (&client_id, &client_secret) {
            return Ok(Self {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
            });
        }

        if !interactive {
            return Err(
                "copyright and ISRC lookups need SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET. Set both, run from a terminal without --non-interactive so they can be typed in, or pass --no-spotify-metadata."
                    .into(),
            );
        }

        eprintln!();
        eprintln!(
            "Spotify credentials are needed to fetch the copyright and ISRC of each track. Create an app at"
        );
        eprintln!("https://developer.spotify.com/dashboard and paste its values below.");
        eprintln!(
            "They are used only for this run and are never saved. Pass --no-spotify-metadata to skip."
        );
        let client_id = match client_id {
            Some(value) => value,
            None => prompt("Client ID")?,
        };
        let client_secret = match client_secret {
            Some(value) => value,
            None => prompt("Client Secret")?,
        };
        Ok(Self {
            client_id,
            client_secret,
        })
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

fn environment(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn prompt(label: &str) -> Result<String, String> {
    for _ in 0..MAX_CREDENTIAL_PROMPTS {
        eprint!("Spotify {label}: ");
        io::stderr()
            .flush()
            .map_err(|error| format!("cannot flush the {label} prompt: {error}"))?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|error| format!("cannot read the {label}: {error}"))?;
        let value = input.trim();
        if !value.is_empty() {
            return Ok(value.to_owned());
        }
        eprintln!("The {label} cannot be empty.");
    }
    Err(format!(
        "no Spotify {label} was supplied after {MAX_CREDENTIAL_PROMPTS} attempts"
    ))
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// The facts this program stores in the tag but spotDL does not fill in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    /// The album's copyright message, for the `TCOP` frame.
    pub copyright: Option<String>,
    /// The recording's ISRC, for the `TSRC` frame.
    pub isrc: Option<String>,
}

impl TrackMetadata {
    pub fn is_empty(&self) -> bool {
        self.copyright.is_none() && self.isrc.is_none()
    }
}

#[derive(Debug, Deserialize)]
struct TrackResponse {
    album: Option<AlbumReference>,
    external_ids: Option<ExternalIds>,
}

#[derive(Debug, Deserialize)]
struct ExternalIds {
    isrc: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlbumReference {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlbumResponse {
    copyrights: Option<Vec<Copyright>>,
}

#[derive(Debug, Deserialize)]
struct Copyright {
    text: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// A blocking handle on the Spotify Web API.
pub struct Client {
    runtime: tokio::runtime::Runtime,
    http: reqwest::Client,
    token: String,
    albums: HashMap<String, Option<String>>,
}

impl Client {
    /// Authenticate with the client-credentials flow.
    pub fn connect(credentials: &Credentials) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the Spotify lookup runtime: {error}"))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| format!("cannot create the Spotify HTTP client: {error}"))?;

        let token = runtime.block_on(access_token(&http, credentials))?;
        Ok(Self {
            runtime,
            http,
            token,
            albums: HashMap::new(),
        })
    }

    /// The ISRC and copyright message for `track_id`.
    ///
    /// Either field is `None` when Spotify does not list it.
    pub fn track_metadata(&mut self, track_id: &str) -> Result<TrackMetadata, String> {
        let track = self.runtime.block_on(get::<TrackResponse>(
            &self.http,
            &self.token,
            &format!("{API_BASE}/tracks/{track_id}"),
        ))?;
        let isrc = track
            .external_ids
            .and_then(|ids| ids.isrc)
            .map(|isrc| isrc.trim().to_owned())
            .filter(|isrc| !isrc.is_empty());

        let Some(album_id) = track.album.and_then(|album| album.id) else {
            return Ok(TrackMetadata {
                copyright: None,
                isrc,
            });
        };
        let copyright = self.album_copyright(&album_id)?;
        Ok(TrackMetadata { copyright, isrc })
    }

    fn album_copyright(&mut self, album_id: &str) -> Result<Option<String>, String> {
        if let Some(cached) = self.albums.get(album_id) {
            return Ok(cached.clone());
        }
        let album = self.runtime.block_on(get::<AlbumResponse>(
            &self.http,
            &self.token,
            &format!("{API_BASE}/albums/{album_id}"),
        ))?;
        let copyright = preferred_copyright(album.copyrights.unwrap_or_default());
        self.albums.insert(album_id.to_owned(), copyright.clone());
        Ok(copyright)
    }
}

/// Pick the copyright message to store, preferring `C` over the `P` phonogram.
fn preferred_copyright(copyrights: Vec<Copyright>) -> Option<String> {
    let usable: Vec<Copyright> = copyrights
        .into_iter()
        .filter(|copyright| {
            copyright
                .text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .collect();
    usable
        .iter()
        .find(|copyright| copyright.kind.as_deref() == Some("C"))
        .or_else(|| {
            usable
                .iter()
                .find(|copyright| copyright.kind.as_deref() == Some("P"))
        })
        .or_else(|| usable.first())
        .and_then(|copyright| copyright.text.as_deref())
        .map(|text| text.trim().to_owned())
}

async fn access_token(http: &reqwest::Client, credentials: &Credentials) -> Result<String, String> {
    let response = http
        .post(TOKEN_URL)
        .basic_auth(&credentials.client_id, Some(&credentials.client_secret))
        .form(&[("grant_type", "client_credentials")])
        .send()
        .await
        .map_err(|error| format!("Spotify authentication request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Spotify authentication failed (HTTP {status}): {}. Check the Client ID and Client Secret.",
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

async fn get<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<T, String> {
    for retry in 0..=MAX_RATE_LIMIT_RETRIES {
        let response = http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| format!("Spotify request failed: {error}"))?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = retry_after(&response).unwrap_or(1).max(1);
            if retry == MAX_RATE_LIMIT_RETRIES {
                return Err(format!(
                    "Spotify kept rate-limiting the request after {} attempts",
                    MAX_RATE_LIMIT_RETRIES + 1
                ));
            }
            if wait > MAX_RATE_LIMIT_WAIT {
                return Err(format!(
                    "Spotify requested a {wait}-second wait, exceeding the {MAX_RATE_LIMIT_WAIT}-second safety limit"
                ));
            }
            eprintln!(
                "Spotify rate limited the metadata lookup; waiting {wait}s before attempt {}/{}...",
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
                "Spotify request failed (HTTP {status}): {}",
                one_line(&body)
            ));
        }

        return response
            .json()
            .await
            .map_err(|error| format!("Spotify returned an unreadable response: {error}"));
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

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{AlbumResponse, Credentials, TrackMetadata, TrackResponse, preferred_copyright};

    fn copyrights(json: &str) -> Vec<super::Copyright> {
        let album: AlbumResponse = serde_json::from_str(json).unwrap();
        album.copyrights.unwrap_or_default()
    }

    #[test]
    fn prefers_the_copyright_over_the_phonogram() {
        let album = copyrights(
            r#"{"copyrights":[
                {"text":"(P) 2013 Columbia Records","type":"P"},
                {"text":"2013 Daft Life Ltd.","type":"C"}
            ]}"#,
        );
        assert_eq!(
            preferred_copyright(album),
            Some("2013 Daft Life Ltd.".to_owned())
        );
    }

    #[test]
    fn falls_back_to_the_phonogram_and_then_to_anything_usable() {
        let phonogram = copyrights(r#"{"copyrights":[{"text":"(P) 2013 Columbia","type":"P"}]}"#);
        assert_eq!(
            preferred_copyright(phonogram),
            Some("(P) 2013 Columbia".to_owned())
        );

        let untyped = copyrights(r#"{"copyrights":[{"text":"  2001 Some Label  "}]}"#);
        assert_eq!(
            preferred_copyright(untyped),
            Some("2001 Some Label".to_owned())
        );
    }

    #[test]
    fn treats_missing_and_blank_copyrights_as_absent() {
        assert_eq!(
            preferred_copyright(copyrights(r#"{"copyrights":[]}"#)),
            None
        );
        assert_eq!(preferred_copyright(copyrights(r#"{}"#)), None);
        assert_eq!(
            preferred_copyright(copyrights(r#"{"copyrights":[{"text":"   ","type":"C"}]}"#)),
            None
        );
    }

    #[test]
    fn reads_the_isrc_from_the_track_response() {
        let track: TrackResponse = serde_json::from_str(
            r#"{"album":{"id":"alb1"},"external_ids":{"isrc":" GBAYE0601498 "}}"#,
        )
        .unwrap();
        let isrc = track
            .external_ids
            .and_then(|ids| ids.isrc)
            .map(|isrc| isrc.trim().to_owned())
            .filter(|isrc| !isrc.is_empty());
        assert_eq!(isrc, Some("GBAYE0601498".to_owned()));

        let without: TrackResponse = serde_json::from_str(r#"{"album":{"id":"alb1"}}"#).unwrap();
        assert!(without.external_ids.is_none());

        let blank: TrackResponse =
            serde_json::from_str(r#"{"external_ids":{"isrc":"  "}}"#).unwrap();
        assert!(
            blank
                .external_ids
                .and_then(|ids| ids.isrc)
                .map(|isrc| isrc.trim().to_owned())
                .filter(|isrc| !isrc.is_empty())
                .is_none()
        );
    }

    #[test]
    fn empty_metadata_is_recognised() {
        assert!(TrackMetadata::default().is_empty());
        assert!(
            !TrackMetadata {
                copyright: None,
                isrc: Some("GBAYE0601498".to_owned()),
            }
            .is_empty()
        );
    }

    #[test]
    fn credentials_never_leak_through_debug() {
        let credentials = Credentials {
            client_id: "id-value".to_owned(),
            client_secret: "secret-value".to_owned(),
        };
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("id-value"));
        assert!(!rendered.contains("secret-value"));
    }
}
