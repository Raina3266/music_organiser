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

use crate::sources::{
    http::Http,
    naming::{Confidence, confidence},
};
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

/// The copyright of the closest result whose artist and album both match.
///
/// Closest, not first: a search for `Discovery` can return the deluxe edition
/// ahead of the plain one, and the plain one is what was asked for.
fn best_match(results: &[AlbumResult], artist: &str, album: &str) -> Option<String> {
    results
        .iter()
        .filter_map(|result| Some((rank(result, artist, album)?, result)))
        .min_by_key(|(rank, _)| *rank)
        .and_then(|(_, result)| result.copyright.as_deref())
        .map(str::trim)
        .filter(|copyright| !copyright.is_empty())
        .map(str::to_owned)
}

/// How well a result matches, album first: two releases by one artist differ
/// by their album name, so that is the more telling of the two.
fn rank(result: &AlbumResult, artist: &str, album: &str) -> Option<(Confidence, Confidence)> {
    Some((
        confidence(result.collection_name.as_deref(), album)?,
        confidence(result.artist_name.as_deref(), artist)?,
    ))
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

    /// Ranking, not order: iTunes lists the deluxe edition first here, and the
    /// plain release is the one that was asked for.
    #[test]
    fn the_closest_release_wins_over_the_one_listed_first() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Daft Punk","collectionName":"Discovery (Deluxe Edition)",
                 "copyright":"℗ 2015 the wrong one"},
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "copyright":"℗ 2001 Daft Life Limited"}
            ]}"#,
        );
        assert_eq!(
            best_match(&listed, "Daft Punk", "Discovery"),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );
    }

    /// The false positive that made this worth changing: a longer name is a
    /// different record, and must not supply its copyright.
    #[test]
    fn a_longer_album_name_is_a_different_record() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Earth, Wind & Fire",
                 "collectionName":"The Best Of Earth, Wind & Fire Vol. 1",
                 "copyright":"℗ 1978 a different record"}
            ]}"#,
        );
        assert_eq!(
            best_match(&listed, "Earth, Wind & Fire", "Earth, Wind & Fire"),
            None
        );
    }

    /// Spelling differences between catalogues are not differences of record.
    #[test]
    fn matches_across_ampersands_and_accents() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Earth, Wind & Fire","collectionName":"I Am",
                 "copyright":"℗ 1979 Columbia"}
            ]}"#,
        );
        assert_eq!(
            best_match(&listed, "Earth, Wind and Fire", "I Am"),
            Some("\u{2117} 1979 Columbia".to_owned())
        );
    }
}
