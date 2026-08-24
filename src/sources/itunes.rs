//! The iTunes Search API as a copyright source.
//!
//! The API needs no account, key, or token, which makes it the default. It
//! offers no ISRC, so the `TSRC` frame is left to spotDL.
//!
//! Lookups are by album rather than by track, because the copyright belongs to
//! the album and because the downloads are already grouped that way. Results
//! are cached, so an album's worth of tracks costs one request.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::sources::{http::Http, naming::matches_name};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "iTunes";

const SEARCH_URL: &str = "https://itunes.apple.com/search";
/// Storefront to search. The `℗` line is the label's and rarely differs
/// between storefronts, and the US catalogue is the most complete.
const COUNTRY: &str = "US";
/// The API is throttled per IP, so requests are spaced out a little.
const MIN_INTERVAL: Duration = Duration::from_millis(200);
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
    http: Http,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new(max_wait: u64) -> Result<Self, String> {
        let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            http: Http::new("iTunes", user_agent, MIN_INTERVAL)?
                .forbidden_is_throttling()
                .waiting_at_most(max_wait),
            albums: HashMap::new(),
        })
    }

    /// The copyright message for an album, or `None` when no confident match
    /// was found.
    ///
    /// A wrong copyright is worse than none, so a result is used only when both
    /// its artist and its album name match what was asked for.
    pub fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, LookupError> {
        if artist.trim().is_empty() || album.trim().is_empty() {
            return Ok(None);
        }
        let key = (artist.to_owned(), album.to_owned());
        if let Some(cached) = self.albums.get(&key) {
            return Ok(cached.clone());
        }

        let term = format!("{artist} {album}");
        let limit = RESULT_LIMIT.to_string();
        let response: Option<SearchResponse> = self.http.get_json(
            SEARCH_URL,
            &[
                ("term", term.as_str()),
                ("entity", "album"),
                ("limit", limit.as_str()),
                ("country", COUNTRY),
            ],
            &[],
        )?;

        let copyright = response
            .as_ref()
            .and_then(|response| best_match(&response.results, artist, album));
        self.albums.insert(key, copyright.clone());
        Ok(copyright)
    }
}

impl CopyrightLookup for Client {
    fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, LookupError> {
        Client::copyright(self, artist, album)
    }
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
}
