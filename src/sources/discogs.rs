//! Discogs as a copyright source.
//!
//! Like MusicBrainz, Discogs stores the copyright as a company credited on the
//! release rather than as a sentence: an entry whose role is
//! `Phonographic Copyright (p)` or `Copyright (c)`. The line written to `TCOP`
//! is built from that company and the release's year.
//!
//! Discogs requires a token. A personal access token is the simplest kind and
//! is generated at <https://www.discogs.com/settings/developers>; it is not a
//! password and grants only what the account can already see.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::AlbumEvidence;
use crate::sources::{
    Limits,
    http::Http,
    naming::{Agreement, Score, confidence, copyright_line},
};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "Discogs";
/// Environment variable holding the personal access token.
pub const TOKEN_VARIABLE: &str = "DISCOGS_TOKEN";

const SEARCH_URL: &str = "https://api.discogs.com/database/search";
const RELEASE_URL: &str = "https://api.discogs.com/releases";
/// An authenticated key is allowed 60 requests a minute.
const MIN_INTERVAL: Duration = Duration::from_millis(1_100);
const RESULT_LIMIT: u8 = 10;
/// How many matching releases to open before giving up. Each one is a request.
const MAX_RELEASES_FETCHED: usize = 3;

/// The company roles that name a copyright holder.
const PHONOGRAPHIC_ROLE: &str = "phonographic copyright";
const COPYRIGHT_ROLE: &str = "copyright";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct Release {
    title: Option<String>,
    /// Corroboration: the tracklist's length is the album's track count.
    #[serde(default)]
    tracklist: Vec<serde_json::Value>,
    /// Discogs has spelled this as a number and as a string over the years.
    #[serde(default)]
    year: Option<serde_json::Value>,
    #[serde(default)]
    artists: Vec<Named>,
    #[serde(default)]
    companies: Vec<Company>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Company {
    name: Option<String>,
    #[serde(rename = "entity_type_name")]
    entity_type_name: Option<String>,
}

/// A blocking handle on the Discogs API.
pub struct Client {
    http: Http,
    token: String,
    /// The search and release endpoints. Only the tests point them anywhere
    /// but Discogs.
    search: String,
    releases: String,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new(token: &str, limits: Limits) -> Result<Self, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("the Discogs token is empty".to_owned());
        }
        let user_agent = concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
            " +https://github.com/raina3266/music_organiser"
        );
        Ok(Self {
            http: Http::new(LABEL, user_agent, MIN_INTERVAL)?
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            // Discogs takes the token in a header rather than the query
            // string, which keeps it out of any proxy's access log.
            token: format!("Discogs token={token}"),
            search: SEARCH_URL.to_owned(),
            releases: RELEASE_URL.to_owned(),
            albums: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(base: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            token: "Discogs token=test".to_owned(),
            search: format!("{base}/search"),
            releases: base.to_owned(),
            albums: HashMap::new(),
        }
    }

    fn lookup(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        let limit = RESULT_LIMIT.to_string();
        let authorization = self.token.clone();
        let search = self.search.clone();
        let Some(found): Option<SearchResponse> = self.http.get_json(
            &search,
            &[
                ("artist", wanted.artist.as_str()),
                ("release_title", wanted.album.as_str()),
                ("type", "release"),
                ("per_page", limit.as_str()),
            ],
            &[("Authorization", authorization.as_str())],
        )?
        else {
            return Ok(None);
        };

        // A search result carries neither the credits nor a reliably split
        // artist and title, so each candidate has to be opened to be judged.
        let mut opened = 0;
        for result in &found.results {
            if opened == MAX_RELEASES_FETCHED {
                break;
            }
            let url = format!("{}/{}", self.releases, result.id);
            let Some(release): Option<Release> =
                self.http
                    .get_json(&url, &[], &[("Authorization", authorization.as_str())])?
            else {
                continue;
            };
            opened += 1;
            // Candidates are opened in the order the search returned them, so
            // ranking cannot reorder them; what it can do is refuse one that
            // the evidence says is a different release.
            let Some(score) = rank(&release, wanted) else {
                continue;
            };
            if score.tracks == Agreement::Differs {
                continue;
            }
            if let Some(copyright) = copyright_of(&release) {
                return Ok(Some(copyright));
            }
        }
        Ok(None)
    }
}

impl CopyrightLookup for Client {
    fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        if !wanted.is_searchable() {
            return Ok(None);
        }
        let key = wanted.key();
        if let Some(cached) = self.albums.get(&key) {
            return Ok(cached.clone());
        }
        let copyright = self.lookup(wanted)?;
        self.albums.insert(key, copyright.clone());
        Ok(copyright)
    }
}

fn rank(release: &Release, wanted: &AlbumEvidence) -> Option<Score> {
    let name = confidence(release.title.as_deref(), &wanted.album)?;
    let artist = release
        .artists
        .iter()
        .filter_map(|credited| confidence(credited.name.as_deref(), &wanted.artist))
        .min()?;
    Some(Score::new(
        name,
        artist,
        Some(release.tracklist.len() as u32).filter(|count| *count > 0),
        wanted.total_tracks,
        year_of(release).as_deref(),
        wanted.year.as_deref(),
    ))
}

/// The copyright line for a release, preferring ℗ over ©.
fn copyright_of(release: &Release) -> Option<String> {
    let year = year_of(release);
    for (role, symbol) in [(PHONOGRAPHIC_ROLE, '\u{2117}'), (COPYRIGHT_ROLE, '\u{a9}')] {
        let line = release
            .companies
            .iter()
            .filter(|company| has_role(company, role))
            .find_map(|company| copyright_line(symbol, year.as_deref(), company.name.as_deref()?));
        if line.is_some() {
            return line;
        }
    }
    None
}

/// Whether a company is credited in a given role.
///
/// The role reads `Phonographic Copyright (p)`, so the match is on the words
/// rather than on the whole string; `Copyright (c)` must not also answer to
/// the phonographic role, which is why it is checked as a prefix.
fn has_role(company: &Company, role: &str) -> bool {
    let Some(name) = &company.entity_type_name else {
        return false;
    };
    name.trim().to_lowercase().starts_with(role)
}

/// The release year, from either spelling Discogs uses. `0` means unknown.
fn year_of(release: &Release) -> Option<String> {
    let year = match release.year.as_ref()? {
        serde_json::Value::Number(number) => number.as_u64()?,
        serde_json::Value::String(text) => text.trim().parse().ok()?,
        _ => return None,
    };
    (year > 0).then(|| year.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Limits, testing::Server};

    fn wanting(artist: &str, album: &str) -> AlbumEvidence {
        AlbumEvidence {
            artist: artist.to_owned(),
            album: album.to_owned(),
            isrc: None,
            year: None,
            total_tracks: None,
            track_title: None,
        }
    }

    fn release(json: &str) -> Release {
        serde_json::from_str(json).unwrap()
    }

    const DISCOVERY: &str = r#"{
        "title": "Discovery",
        "year": 2001,
        "artists": [{ "name": "Daft Punk (2)" }],
        "companies": [
            { "name": "EMI", "entity_type_name": "Distributed By" },
            { "name": "Virgin Records Ltd.", "entity_type_name": "Copyright (c)" },
            { "name": "Daft Life Ltd.", "entity_type_name": "Phonographic Copyright (p)" }
        ]
    }"#;

    #[test]
    fn builds_the_phonographic_line_from_the_credited_company() {
        assert_eq!(
            copyright_of(&release(DISCOVERY)).as_deref(),
            Some("\u{2117} 2001 Daft Life Ltd.")
        );
    }

    #[test]
    fn matches_a_disambiguated_artist_name() {
        let matches = |artist, album| rank(&release(DISCOVERY), &wanting(artist, album)).is_some();
        assert!(matches("Daft Punk", "Discovery"));
        assert!(!matches("Stardust", "Discovery"));
        assert!(!matches("Daft Punk", "Homework"));
    }

    #[test]
    fn falls_back_to_the_c_line_and_survives_a_missing_year() {
        let only_copyright = release(
            r#"{
                "title": "Homework",
                "year": "0",
                "companies": [
                    { "name": "Virgin Records Ltd.", "entity_type_name": "Copyright (c)" }
                ]
            }"#,
        );
        assert_eq!(
            copyright_of(&only_copyright).as_deref(),
            Some("\u{a9} Virgin Records Ltd.")
        );
    }

    #[test]
    fn a_release_crediting_no_copyright_holder_yields_nothing() {
        let unrelated = release(
            r#"{"title":"Discovery","year":2001,
                "companies":[{"name":"EMI","entity_type_name":"Pressed By"}]}"#,
        );
        assert_eq!(copyright_of(&unrelated), None);
        assert_eq!(copyright_of(&release(r#"{"title":"Discovery"}"#)), None);
    }

    #[test]
    fn reads_a_year_spelled_either_way() {
        assert_eq!(
            year_of(&release(r#"{"year":2001}"#)).as_deref(),
            Some("2001")
        );
        assert_eq!(
            year_of(&release(r#"{"year":"2001"}"#)).as_deref(),
            Some("2001")
        );
        assert_eq!(year_of(&release(r#"{"year":0}"#)), None);
        assert_eq!(year_of(&release(r#"{}"#)), None);
    }

    #[test]
    fn a_token_is_required() {
        assert!(Client::new("   ", Limits::default()).is_err());
    }

    /// The search, then each candidate in turn — a search result carries no
    /// credits, so every one has to be opened — until one matches and names a
    /// copyright holder.
    #[test]
    fn searches_then_opens_candidates_until_one_answers() {
        let server = Server::answering(&[
            r#"{"results":[{"id":11},{"id":22},{"id":33}]}"#,
            // The right album by the wrong artist: opened, then rejected.
            r#"{"title":"Discovery","year":2001,"artists":[{"name":"Pink Floyd"}],
                "companies":[{"name":"EMI","entity_type_name":"Phonographic Copyright (p)"}]}"#,
            r#"{"title":"Discovery","year":2001,"artists":[{"name":"Daft Punk (2)"}],
                "companies":[{"name":"EMI","entity_type_name":"Pressed By"}]}"#,
            DISCOVERY,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let copyright = client
            .copyright(&wanting("Daft Punk", "Discovery"))
            .unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/search?artist=Daft+Punk&release_title=Discovery&type=release&per_page=10"
        );
        assert_eq!(asked[1].target, "/11");
        assert_eq!(asked[3].target, "/33");
        // Every request carries the token, in the header rather than the URL.
        for request in &asked {
            assert_eq!(request.header("Authorization"), Some("Discogs token=test"));
        }
    }

    /// Opening candidates is capped: each one is a request against a rate
    /// limit that a whole library's worth of albums has to share.
    #[test]
    fn gives_up_after_a_few_candidates_rather_than_opening_them_all() {
        let mut script = vec![r#"{"results":[{"id":1},{"id":2},{"id":3},{"id":4},{"id":5}]}"#];
        let unmatched = r#"{"title":"Homework","artists":[{"name":"Daft Punk"}]}"#;
        script.extend(std::iter::repeat_n(unmatched, MAX_RELEASES_FETCHED));
        let server = Server::answering(&script);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap(),
            None
        );

        assert_eq!(server.requests().len(), 1 + MAX_RELEASES_FETCHED);
    }

    #[test]
    fn an_album_is_only_looked_up_once() {
        let server = Server::answering(&[r#"{"results":[]}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap(),
            None
        );
        assert_eq!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap(),
            None
        );

        assert_eq!(server.requests().len(), 1);
    }

    /// Discogs opens candidates in the order its search returned them, so
    /// ranking cannot reorder them. What it can do is refuse one the evidence
    /// contradicts, and carry on to the next.
    #[test]
    fn a_release_whose_track_count_contradicts_the_tag_is_passed_over() {
        let server = Server::answering(&[
            r#"{"results":[{"id":11},{"id":22}]}"#,
            // Right name, wrong record: twenty tracks against the tag's
            // fourteen.
            r#"{"title":"Discovery","year":2021,"artists":[{"name":"Daft Punk"}],
                "tracklist":[{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}],
                "companies":[{"name":"the deluxe reissue",
                              "entity_type_name":"Phonographic Copyright (p)"}]}"#,
            r#"{"title":"Discovery","year":2001,"artists":[{"name":"Daft Punk"}],
                "tracklist":[{},{},{},{},{},{},{},{},{},{},{},{},{},{}],
                "companies":[{"name":"Daft Life Ltd.",
                              "entity_type_name":"Phonographic Copyright (p)"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.total_tracks = Some(14);

        let copyright = client.copyright(&wanted).unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        assert_eq!(server.requests().len(), 3);
    }

    /// A tag with no track count cannot contradict anything, so the first
    /// matching release still answers.
    #[test]
    fn without_a_track_count_the_first_match_still_answers() {
        let server = Server::answering(&[
            r#"{"results":[{"id":11}]}"#,
            r#"{"title":"Discovery","year":2021,"artists":[{"name":"Daft Punk"}],
                "tracklist":[{},{}],
                "companies":[{"name":"whoever","entity_type_name":"Phonographic Copyright (p)"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        assert!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap()
                .is_some()
        );
    }
}
