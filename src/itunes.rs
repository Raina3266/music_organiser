//! Minimal iTunes Search API client used to fetch an album's copyright.
//!
//! The API needs no account, key, or token — it is the reason this project no
//! longer asks for Spotify credentials. It offers no ISRC, so the `TSRC` frame
//! is left to spotDL.
//!
//! Lookups are by album rather than by track, because the copyright belongs to
//! the album and because the downloads are already grouped that way. Results
//! are cached, so an album's worth of tracks costs one request.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

const SEARCH_URL: &str = "https://itunes.apple.com/search";
/// Storefront to search. The `℗` line is the label's and rarely differs
/// between storefronts, and the US catalogue is the most complete.
const COUNTRY: &str = "US";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The API is throttled per IP, so requests are spaced out a little.
const DELAY_BETWEEN_REQUESTS: Duration = Duration::from_millis(200);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RATE_LIMIT_WAIT: u64 = 60;
/// How many candidates to weigh before giving up on a confident match.
const RESULT_LIMIT: u8 = 10;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<AlbumResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct AlbumResult {
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "collectionName")]
    collection_name: Option<String>,
    copyright: Option<String>,
}

/// A blocking handle on the iTunes Search API.
pub struct Client {
    runtime: tokio::runtime::Runtime,
    http: reqwest::Client,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the iTunes lookup runtime: {error}"))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| format!("cannot create the iTunes HTTP client: {error}"))?;
        Ok(Self {
            runtime,
            http,
            albums: HashMap::new(),
        })
    }

    /// The copyright message for an album, or `None` when no confident match
    /// was found.
    ///
    /// A wrong copyright is worse than none, so a result is used only when both
    /// its artist and its album name match what was asked for.
    pub fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
        if artist.trim().is_empty() || album.trim().is_empty() {
            return Ok(None);
        }
        let key = (artist.to_owned(), album.to_owned());
        if let Some(cached) = self.albums.get(&key) {
            return Ok(cached.clone());
        }

        let response = self.runtime.block_on(search(&self.http, artist, album))?;
        let copyright = best_match(&response.results, artist, album);
        self.albums.insert(key, copyright.clone());
        Ok(copyright)
    }
}

async fn search(
    http: &reqwest::Client,
    artist: &str,
    album: &str,
) -> Result<SearchResponse, String> {
    let term = format!("{artist} {album}");
    let limit = RESULT_LIMIT.to_string();

    for retry in 0..=MAX_RATE_LIMIT_RETRIES {
        tokio::time::sleep(DELAY_BETWEEN_REQUESTS).await;
        let response = http
            .get(SEARCH_URL)
            .query(&[
                ("term", term.as_str()),
                ("entity", "album"),
                ("limit", limit.as_str()),
                ("country", COUNTRY),
            ])
            .send()
            .await
            .map_err(|error| format!("iTunes search request failed: {error}"))?;

        // The API answers 403 as well as 429 when it is throttling.
        let throttled = matches!(
            response.status(),
            reqwest::StatusCode::TOO_MANY_REQUESTS | reqwest::StatusCode::FORBIDDEN
        );
        if throttled {
            if retry == MAX_RATE_LIMIT_RETRIES {
                return Err(format!(
                    "iTunes kept throttling the lookup after {} attempts",
                    MAX_RATE_LIMIT_RETRIES + 1
                ));
            }
            let wait = retry_after(&response).unwrap_or(2 << retry).max(1);
            if wait > MAX_RATE_LIMIT_WAIT {
                return Err(format!(
                    "iTunes requested a {wait}-second wait, exceeding the {MAX_RATE_LIMIT_WAIT}-second safety limit"
                ));
            }
            eprintln!("iTunes throttled the lookup; waiting {wait}s before retrying...");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }

        if !response.status().is_success() {
            return Err(format!("iTunes search failed (HTTP {})", response.status()));
        }

        // The endpoint answers with text/javascript, so the body is decoded
        // rather than taken as JSON directly.
        let body = response
            .text()
            .await
            .map_err(|error| format!("iTunes returned an unreadable response: {error}"))?;
        return serde_json::from_str(&body)
            .map_err(|error| format!("iTunes returned malformed JSON: {error}"));
    }

    unreachable!("the throttling loop always returns or continues")
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

/// The copyright of the first result whose artist and album both match.
fn best_match(results: &[AlbumResult], artist: &str, album: &str) -> Option<String> {
    results
        .iter()
        .find(|result| {
            matches_name(result.artist_name.as_deref(), artist)
                && matches_name(result.collection_name.as_deref(), album)
        })
        .and_then(|result| result.copyright.as_deref())
        .map(str::trim)
        .filter(|copyright| !copyright.is_empty())
        .map(str::to_owned)
}

/// Compare two names loosely enough to survive edition suffixes.
///
/// Punctuation and case are ignored, and one name may extend the other so that
/// `Random Access Memories` matches `Random Access Memories (Deluxe Edition)`.
/// Anything less similar is treated as a different release.
fn matches_name(candidate: Option<&str>, expected: &str) -> bool {
    let (Some(candidate), expected) = (candidate.map(normalize), normalize(expected)) else {
        return false;
    };
    if candidate.is_empty() || expected.is_empty() {
        return false;
    }
    candidate == expected
        || candidate.starts_with(&format!("{expected} "))
        || expected.starts_with(&format!("{candidate} "))
}

fn normalize(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        // Apostrophes vanish rather than splitting a word, so that "Pepper's"
        // still matches a store listing spelled "Peppers".
        if matches!(character, '\'' | '\u{2019}' | '\u{02bc}') {
            continue;
        }
        if character.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            pending_space = false;
            normalized.extend(character.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(json: &str) -> Vec<AlbumResult> {
        serde_json::from_str::<SearchResponse>(json)
            .unwrap()
            .results
    }

    const DISCOVERY: &str = r#"{
        "resultCount": 2,
        "results": [
            {
                "artistName": "Stardust",
                "collectionName": "Music Sounds Better With You",
                "copyright": "℗ 1998 Roule"
            },
            {
                "artistName": "Daft Punk",
                "collectionName": "Discovery",
                "copyright": "℗ 2001 Daft Life Limited"
            }
        ]
    }"#;

    #[test]
    fn takes_the_copyright_of_the_matching_album() {
        assert_eq!(
            best_match(&results(DISCOVERY), "Daft Punk", "Discovery"),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );
    }

    #[test]
    fn ignores_results_that_are_a_different_release() {
        // The right album by the wrong artist must not supply a copyright.
        assert_eq!(
            best_match(&results(DISCOVERY), "Pink Floyd", "Discovery"),
            None
        );
        assert_eq!(
            best_match(&results(DISCOVERY), "Daft Punk", "Homework"),
            None
        );
        assert_eq!(best_match(&[], "Daft Punk", "Discovery"), None);
    }

    #[test]
    fn survives_edition_suffixes_and_punctuation() {
        let deluxe = results(
            r#"{"results":[{
                "artistName":"Daft Punk",
                "collectionName":"Random Access Memories (Deluxe Edition)",
                "copyright":"℗ 2013 Daft Life Limited"
            }]}"#,
        );
        assert_eq!(
            best_match(&deluxe, "Daft Punk", "Random Access Memories"),
            Some("\u{2117} 2013 Daft Life Limited".to_owned())
        );

        let punctuated = results(
            r#"{"results":[{
                "artistName":"Sigur Rós",
                "collectionName":"( )",
                "copyright":"℗ 2002 Sigur Rós"
            }]}"#,
        );
        // An album whose name normalizes to nothing cannot be matched safely.
        assert_eq!(best_match(&punctuated, "Sigur R\u{f3}s", "( )"), None);
    }

    #[test]
    fn treats_a_missing_or_blank_copyright_as_absent() {
        let blank = results(
            r#"{"results":[{"artistName":"Daft Punk","collectionName":"Discovery","copyright":"   "}]}"#,
        );
        assert_eq!(best_match(&blank, "Daft Punk", "Discovery"), None);

        let missing =
            results(r#"{"results":[{"artistName":"Daft Punk","collectionName":"Discovery"}]}"#);
        assert_eq!(best_match(&missing, "Daft Punk", "Discovery"), None);
    }

    #[test]
    fn tolerates_a_response_with_no_results_field() {
        assert!(results(r#"{"resultCount":0}"#).is_empty());
    }

    #[test]
    fn normalizes_case_punctuation_and_spacing() {
        assert_eq!(normalize("  Sgt. Pepper's  Lonely  "), "sgt peppers lonely");
        assert_eq!(normalize("Sgt Peppers Lonely"), "sgt peppers lonely");
        assert_eq!(normalize("Don\u{2019}t Stop"), "dont stop");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("()"), "");
    }
}
