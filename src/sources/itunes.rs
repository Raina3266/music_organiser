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

use crate::AlbumEvidence;
use crate::sources::{
    Limits,
    http::Http,
    naming::{Confidence, Score, confidence},
};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "iTunes";

const SEARCH_URL: &str = "https://itunes.apple.com/search";
/// Storefront to search. The `℗` line is the label's and rarely differs
/// between storefronts, and the US catalogue is the most complete.
const COUNTRY: &str = "US";
/// Apple's legacy Search API documents roughly twenty calls a minute. Three
/// seconds keeps a library scan inside that limit instead of immediately
/// relying on 403/429 backoff to discover it.
const MIN_INTERVAL: Duration = Duration::from_secs(3);
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
    /// Corroboration: how many tracks the store lists, and when it came out.
    #[serde(rename = "trackCount")]
    track_count: Option<u32>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
}

/// A blocking handle on the iTunes Search API.
pub struct Client {
    http: Http,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new(limits: Limits) -> Result<Self, String> {
        let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            http: Http::new("iTunes", user_agent, MIN_INTERVAL)?
                .forbidden_is_throttling()
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            albums: HashMap::new(),
        })
    }

    /// The copyright message for an album, or `None` when no confident match
    /// was found.
    ///
    /// A wrong copyright is worse than none, so a result is used only when both
    /// its artist and its album name match what was asked for.
    pub fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        if !wanted.is_searchable() {
            return Ok(None);
        }
        let key = wanted.key();
        if let Some(cached) = self.albums.get(&key) {
            return Ok(cached.clone());
        }

        // The Search API has no ISRC field, so the name is all there is to
        // search with; the rest of the evidence sorts what comes back.
        let term = format!("{} {}", wanted.artist, wanted.album);
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
            .and_then(|response| best_match(&response.results, wanted));
        self.albums.insert(key, copyright.clone());
        Ok(copyright)
    }
}

impl CopyrightLookup for Client {
    fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        Client::copyright(self, wanted)
    }
}

/// The copyright of the closest result whose artist and album both match.
///
/// Closest, not first: a search for `Discovery` can return the deluxe edition
/// ahead of the plain one, and the plain one is what was asked for.
fn best_match(results: &[AlbumResult], wanted: &AlbumEvidence) -> Option<String> {
    results
        .iter()
        .filter_map(|result| Some((rank(result, wanted)?, result)))
        .min_by_key(|(rank, _)| *rank)
        .and_then(|(_, result)| result.copyright.as_deref())
        .map(str::trim)
        .filter(|copyright| !copyright.is_empty())
        .map(str::to_owned)
}

/// How well a result matches everything the tag knows.
fn rank(result: &AlbumResult, wanted: &AlbumEvidence) -> Option<Score> {
    Some(Score::new(
        itunes_release_confidence(result.collection_name.as_deref(), &wanted.album)?,
        confidence(result.artist_name.as_deref(), &wanted.artist)?,
        result.track_count,
        wanted.total_tracks,
        result.release_date.as_deref(),
        wanted.year.as_deref(),
    ))
}

/// Apple decorates collection names with a media kind that spotDL/Spotify
/// commonly omit, notably ` - Single` and ` - EP`. Those suffixes identify how
/// Apple sells the same release; they are not edition names and should not be
/// added to the shared matcher, where a literal album title ending in "Single"
/// could otherwise become a false positive.
fn itunes_release_confidence(candidate: Option<&str>, expected: &str) -> Option<Confidence> {
    let candidate = candidate?;
    confidence(Some(candidate), expected).or_else(|| {
        let stripped = strip_apple_media_kind(candidate);
        (stripped != candidate)
            .then(|| confidence(Some(stripped), expected))
            .flatten()
    })
}

fn strip_apple_media_kind(value: &str) -> &str {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    for suffix in [" - single", " - ep"] {
        if lower.ends_with(suffix) {
            return trimmed[..trimmed.len() - suffix.len()].trim_end();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tag that knows only the artist and album, as most do.
    fn wanting(artist: &str, album: &str) -> AlbumEvidence {
        AlbumEvidence {
            artist: artist.to_owned(),
            album: album.to_owned(),
            track_artist: None,
            isrc: None,
            year: None,
            total_tracks: None,
            track_title: None,
        }
    }

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
            best_match(&results(DISCOVERY), &wanting("Daft Punk", "Discovery")),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );
    }

    #[test]
    fn ignores_results_that_are_a_different_release() {
        // The right album by the wrong artist must not supply a copyright.
        assert_eq!(
            best_match(&results(DISCOVERY), &wanting("Pink Floyd", "Discovery")),
            None
        );
        assert_eq!(
            best_match(&results(DISCOVERY), &wanting("Daft Punk", "Homework")),
            None
        );
        assert_eq!(best_match(&[], &wanting("Daft Punk", "Discovery")), None);
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
            best_match(&deluxe, &wanting("Daft Punk", "Random Access Memories")),
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
        assert_eq!(
            best_match(&punctuated, &wanting("Sigur R\u{f3}s", "( )")),
            None
        );
    }

    #[test]
    fn accepts_apples_single_and_ep_catalogue_suffixes() {
        let single = results(
            r#"{"results":[{
                "artistName":"Hearts2Hearts",
                "collectionName":"RUDE! - Single",
                "copyright":"℗ 2026 SM Entertainment"
            }]}"#,
        );
        assert_eq!(
            best_match(&single, &wanting("Hearts2Hearts", "RUDE!")),
            Some("\u{2117} 2026 SM Entertainment".to_owned())
        );

        let ep = results(
            r#"{"results":[{
                "artistName":"Hearts2Hearts",
                "collectionName":"FOCUS - The 1st Mini Album - EP",
                "copyright":"℗ 2025 SM Entertainment"
            }]}"#,
        );
        assert_eq!(
            best_match(
                &ep,
                &wanting("Hearts2Hearts", "FOCUS - The 1st Mini Album")
            ),
            Some("\u{2117} 2025 SM Entertainment".to_owned())
        );
    }

    #[test]
    fn does_not_treat_single_as_a_global_edition_word() {
        let different = results(
            r#"{"results":[{
                "artistName":"Example Artist",
                "collectionName":"Single",
                "copyright":"℗ 2026 Wrong"
            }]}"#,
        );
        assert_eq!(
            best_match(&different, &wanting("Example Artist", "Single Life")),
            None
        );
    }

    #[test]
    fn treats_a_missing_or_blank_copyright_as_absent() {
        let blank = results(
            r#"{"results":[{"artistName":"Daft Punk","collectionName":"Discovery","copyright":"   "}]}"#,
        );
        assert_eq!(best_match(&blank, &wanting("Daft Punk", "Discovery")), None);

        let missing =
            results(r#"{"results":[{"artistName":"Daft Punk","collectionName":"Discovery"}]}"#);
        assert_eq!(
            best_match(&missing, &wanting("Daft Punk", "Discovery")),
            None
        );
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
            best_match(&listed, &wanting("Daft Punk", "Discovery")),
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
            best_match(
                &listed,
                &wanting("Earth, Wind & Fire", "Earth, Wind & Fire")
            ),
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
            best_match(&listed, &wanting("Earth, Wind and Fire", "I Am")),
            Some("\u{2117} 1979 Columbia".to_owned())
        );
    }

    /// The case that names alone cannot settle. Both releases are called
    /// `Discovery` by Daft Punk; only the track count says which is the album
    /// and which is the deluxe edition.
    #[test]
    fn the_track_count_separates_an_album_from_its_deluxe_edition() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "trackCount":20,"releaseDate":"2021-01-01T00:00:00Z",
                 "copyright":"℗ 2021 the deluxe reissue"},
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "trackCount":14,"releaseDate":"2001-03-12T00:00:00Z",
                 "copyright":"℗ 2001 Daft Life Limited"}
            ]}"#,
        );

        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.total_tracks = Some(14);
        assert_eq!(
            best_match(&listed, &wanted),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );

        // Ask for twenty and the other one is right instead.
        wanted.total_tracks = Some(20);
        assert_eq!(
            best_match(&listed, &wanted),
            Some("\u{2117} 2021 the deluxe reissue".to_owned())
        );
    }

    /// Where the track count is unknown, the year still separates a reissue
    /// from the original.
    #[test]
    fn the_year_separates_a_reissue_from_the_original() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "releaseDate":"2021-01-01T00:00:00Z","copyright":"℗ 2021 the reissue"},
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "releaseDate":"2001-03-12T00:00:00Z","copyright":"℗ 2001 Daft Life Limited"}
            ]}"#,
        );

        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.year = Some("2001".to_owned());
        assert_eq!(
            best_match(&listed, &wanted),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );
    }

    /// Evidence ranks candidates; it never rejects the only one there is. A
    /// tag whose year disagrees with the sole match still gets a copyright,
    /// because catalogues genuinely disagree about release dates.
    #[test]
    fn conflicting_evidence_still_beats_no_answer_at_all() {
        let listed = results(
            r#"{"results":[
                {"artistName":"Daft Punk","collectionName":"Discovery",
                 "trackCount":14,"releaseDate":"2001-03-12T00:00:00Z",
                 "copyright":"℗ 2001 Daft Life Limited"}
            ]}"#,
        );

        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.year = Some("1998".to_owned());
        wanted.total_tracks = Some(9);
        assert_eq!(
            best_match(&listed, &wanted),
            Some("\u{2117} 2001 Daft Life Limited".to_owned())
        );
    }
}
