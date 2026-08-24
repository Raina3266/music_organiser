//! MusicBrainz as a copyright source.
//!
//! MusicBrainz stores no copyright *string*. What it stores is the
//! relationship between a release and the label that holds the copyright:
//! `phonographic copyright` for the ℗ line and `copyright` for the © one, each
//! with the year it began. The line written to `TCOP` is built from those
//! parts, which is why a message from here can read slightly differently from
//! the same album's message on a store.
//!
//! The API needs no account. It does ask for two things in return: a User-Agent
//! that identifies the application and a way to reach whoever runs it, and no
//! more than one request a second. Both are honoured here — set
//! `MUSICBRAINZ_CONTACT` to an email address or URL so a MusicBrainz admin can
//! reach you instead of blocking the whole application.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::CopyrightLookup;
use crate::sources::{
    http::Http,
    naming::{copyright_line, matches_name, year_of},
};

pub const LABEL: &str = "MusicBrainz";
/// Environment variable naming whoever is running this copy.
pub const CONTACT_VARIABLE: &str = "MUSICBRAINZ_CONTACT";

const SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release";
/// One request a second is the published limit; the extra 100ms is slack for a
/// clock that rounds the wrong way.
const MIN_INTERVAL: Duration = Duration::from_millis(1_100);
/// How many search results to weigh. Releases are per-pressing, so one album
/// can have dozens and only some carry the copyright relationships.
const RESULT_LIMIT: u8 = 10;
/// How many matching releases to open before giving up. Each one is a request,
/// and the rate limit makes them expensive.
const MAX_RELEASES_FETCHED: usize = 3;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    releases: Vec<ReleaseStub>,
}

#[derive(Debug, Deserialize)]
struct ReleaseStub {
    id: String,
    title: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
}

#[derive(Debug, Deserialize)]
struct ArtistCredit {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    date: Option<String>,
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Debug, Deserialize)]
struct Relation {
    #[serde(rename = "type")]
    kind: Option<String>,
    begin: Option<String>,
    label: Option<Label>,
}

#[derive(Debug, Deserialize)]
struct Label {
    name: Option<String>,
}

/// A blocking handle on the MusicBrainz web service.
pub struct Client {
    http: Http,
    /// The release endpoint. Searches go to it and a release is read from
    /// under it; only the tests point it anywhere but MusicBrainz.
    releases: String,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    /// `contact` is the email address or URL to advertise in the User-Agent.
    pub fn new(contact: Option<&str>) -> Result<Self, String> {
        let contact = contact
            .map(str::trim)
            .filter(|contact| !contact.is_empty())
            .unwrap_or("https://github.com/raina3266/music_organiser");
        let user_agent = format!(
            "{}/{} ( {contact} )",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        );
        Ok(Self {
            http: Http::new(LABEL, &user_agent, MIN_INTERVAL)?,
            releases: SEARCH_URL.to_owned(),
            albums: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(releases: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            releases: releases.to_owned(),
            albums: HashMap::new(),
        }
    }

    fn lookup(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
        let query = format!(
            "artist:{} AND release:{}",
            quote_for_lucene(artist),
            quote_for_lucene(album)
        );
        let limit = RESULT_LIMIT.to_string();
        let releases = self.releases.clone();
        let Some(found): Option<SearchResponse> = self.http.get_json(
            &releases,
            &[
                ("query", query.as_str()),
                ("limit", limit.as_str()),
                ("fmt", "json"),
            ],
            &[],
        )?
        else {
            return Ok(None);
        };

        let candidates = found
            .releases
            .iter()
            .filter(|release| is_the_release(release, artist, album))
            .take(MAX_RELEASES_FETCHED);
        // Releases are per-pressing and only some carry the relationships, so
        // the first few matches are opened until one of them has a copyright.
        for release in candidates {
            let url = format!("{releases}/{}", release.id);
            let Some(release): Option<Release> =
                self.http
                    .get_json(&url, &[("inc", "label-rels"), ("fmt", "json")], &[])?
            else {
                continue;
            };
            if let Some(copyright) = copyright_of(&release) {
                return Ok(Some(copyright));
            }
        }
        Ok(None)
    }
}

impl CopyrightLookup for Client {
    fn copyright(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
        if artist.trim().is_empty() || album.trim().is_empty() {
            return Ok(None);
        }
        let key = (artist.to_owned(), album.to_owned());
        if let Some(cached) = self.albums.get(&key) {
            return Ok(cached.clone());
        }
        let copyright = self.lookup(artist, album)?;
        self.albums.insert(key, copyright.clone());
        Ok(copyright)
    }
}

/// Whether a search result is the release that was asked for.
fn is_the_release(release: &ReleaseStub, artist: &str, album: &str) -> bool {
    matches_name(release.title.as_deref(), album)
        && release
            .artist_credit
            .iter()
            .any(|credit| matches_name(credit.name.as_deref(), artist))
}

/// The copyright line for a release, preferring ℗ over ©.
///
/// `TCOP` is the recording's copyright, so the phonographic line is the right
/// one; the © line is only used when a release records nothing else.
fn copyright_of(release: &Release) -> Option<String> {
    for (kind, symbol) in [
        ("phonographic copyright", '\u{2117}'),
        ("copyright", '\u{a9}'),
    ] {
        let line = release
            .relations
            .iter()
            .filter(|relation| relation.kind.as_deref() == Some(kind))
            .find_map(|relation| {
                let owner = relation.label.as_ref()?.name.as_deref()?;
                let year =
                    year_of(relation.begin.as_deref()).or_else(|| year_of(release.date.as_deref()));
                copyright_line(symbol, year.as_deref(), owner)
            });
        if line.is_some() {
            return line;
        }
    }
    None
}

/// Wrap a value as a Lucene phrase, dropping what would break the query.
///
/// The search field is a Lucene query, so a quote or a backslash in an album
/// name would end the phrase early; neither carries meaning for the match.
fn quote_for_lucene(value: &str) -> String {
    let escaped: String = value
        .chars()
        .filter(|character| !matches!(character, '"' | '\\'))
        .collect();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::testing::Server;

    const SEARCH_HIT: &str = r#"{"releases":[
        {"id":"wrong","title":"Homework","artist-credit":[{"name":"Daft Punk"}]},
        {"id":"bare","title":"Discovery","artist-credit":[{"name":"Daft Punk"}]},
        {"id":"credited","title":"Discovery","artist-credit":[{"name":"Daft Punk"}]}
    ]}"#;

    fn release(json: &str) -> Release {
        serde_json::from_str(json).unwrap()
    }

    const DISCOVERY: &str = r#"{
        "id": "0a2b3c",
        "title": "Discovery",
        "date": "2001-03-12",
        "relations": [
            { "type": "distributed by", "begin": null, "label": { "name": "EMI" } },
            {
                "type": "phonographic copyright",
                "begin": "2001",
                "label": { "name": "Daft Life Ltd." }
            },
            { "type": "copyright", "begin": "2001", "label": { "name": "Virgin Records" } }
        ]
    }"#;

    #[test]
    fn builds_the_phonographic_line_from_the_label_relationship() {
        assert_eq!(
            copyright_of(&release(DISCOVERY)).as_deref(),
            Some("\u{2117} 2001 Daft Life Ltd.")
        );
    }

    #[test]
    fn falls_back_to_the_copyright_relationship_and_the_release_date() {
        let only_copyright = release(
            r#"{
                "date": "1997-01-20",
                "relations": [
                    { "type": "copyright", "label": { "name": "Virgin Records" } }
                ]
            }"#,
        );
        assert_eq!(
            copyright_of(&only_copyright).as_deref(),
            Some("\u{a9} 1997 Virgin Records")
        );
    }

    #[test]
    fn a_release_with_no_copyright_relationship_yields_nothing() {
        let unrelated = release(
            r#"{"date":"2001","relations":[{"type":"manufactured by","label":{"name":"EMI"}}]}"#,
        );
        assert_eq!(copyright_of(&unrelated), None);
        assert_eq!(copyright_of(&release(r#"{"date":"2001"}"#)), None);
        // A relationship to no label at all cannot name an owner.
        let unnamed = release(r#"{"relations":[{"type":"copyright","label":{}}]}"#);
        assert_eq!(copyright_of(&unnamed), None);
    }

    #[test]
    fn only_the_release_that_was_asked_for_is_opened() {
        let found: SearchResponse = serde_json::from_str(
            r#"{"releases":[
                {"id":"one","title":"Homework","artist-credit":[{"name":"Daft Punk"}]},
                {"id":"two","title":"Discovery","artist-credit":[{"name":"Daft Punk"}]},
                {"id":"three","title":"Discovery","artist-credit":[{"name":"Pink Floyd"}]}
            ]}"#,
        )
        .unwrap();

        let matching: Vec<&str> = found
            .releases
            .iter()
            .filter(|release| is_the_release(release, "Daft Punk", "Discovery"))
            .map(|release| release.id.as_str())
            .collect();
        assert_eq!(matching, ["two"]);
    }

    #[test]
    fn quotes_a_name_that_would_otherwise_break_the_query() {
        assert_eq!(quote_for_lucene("Discovery"), "\"Discovery\"");
        assert_eq!(quote_for_lucene(r#"He said "hi"\"#), "\"He said hi\"");
    }

    /// The search, then each matching release in turn until one of them names
    /// a copyright holder. The release that is not a match is never opened.
    #[test]
    fn searches_then_opens_matching_releases_until_one_answers() {
        let server = Server::answering(&[
            SEARCH_HIT,
            r#"{"date":"2001","relations":[{"type":"manufactured by","label":{"name":"EMI"}}]}"#,
            r#"{"date":"2001-03-12","relations":[
                {"type":"phonographic copyright","begin":"2001","label":{"name":"Daft Life Ltd."}}
            ]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let copyright = client.copyright("Daft Punk", "Discovery").unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        let asked: Vec<String> = server
            .requests()
            .into_iter()
            .map(|request| request.target)
            .collect();
        assert_eq!(
            asked,
            [
                "/?query=artist%3A%22Daft+Punk%22+AND+release%3A%22Discovery%22&limit=10&fmt=json",
                "/bare?inc=label-rels&fmt=json",
                "/credited?inc=label-rels&fmt=json",
            ]
        );
    }

    /// A second track from the same album must not cost a second request.
    #[test]
    fn an_album_is_only_looked_up_once() {
        let server = Server::answering(&[r#"{"releases":[]}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(client.copyright("Daft Punk", "Discovery").unwrap(), None);
        assert_eq!(client.copyright("Daft Punk", "Discovery").unwrap(), None);

        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn an_album_with_no_artist_is_not_searched_for_at_all() {
        let server = Server::answering(&[]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(client.copyright("  ", "Discovery").unwrap(), None);
        assert_eq!(client.copyright("Daft Punk", "").unwrap(), None);

        assert!(server.requests().is_empty());
    }
}
