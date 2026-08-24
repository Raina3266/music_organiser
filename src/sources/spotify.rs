//! The Spotify Web API as a copyright source.
//!
//! Spotify is the only source here that stores the copyright as a finished
//! sentence: an album carries `copyrights`, each a line of text and a type,
//! `P` for the ℗ line and `C` for the © one. Nothing is assembled — the text is
//! taken as it stands, gaining only the symbol when Spotify left it off.
//!
//! Every request needs an OAuth access token. It is the same kind of token the
//! download command asks for, so one pasted there works here, and a token
//! copied from the open.spotify.com web player expires within the hour.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::AlbumEvidence;
use crate::sources::{
    http::Http,
    naming::{Score, confidence},
};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "Spotify";
/// Environment variable holding the access token.
pub const TOKEN_VARIABLE: &str = "SPOTIFY_ACCESS_TOKEN";

const SEARCH_URL: &str = "https://api.spotify.com/v1/search";
const ALBUM_URL: &str = "https://api.spotify.com/v1/albums";
/// Spotify measures its limit over a rolling window rather than per request,
/// and answers a breach with a `Retry-After` of hours rather than seconds. Two
/// requests per album at 200ms was fast enough to earn one on a real library,
/// so the pace here is deliberately slower than the API strictly requires.
const MIN_INTERVAL: Duration = Duration::from_millis(500);
const RESULT_LIMIT: u8 = 10;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    albums: Option<AlbumPage>,
    /// Present when the search asked for tracks, which is how an ISRC is
    /// looked up: the recording is found, and its album comes back with it.
    tracks: Option<TrackPage>,
}

#[derive(Debug, Deserialize)]
struct TrackPage {
    #[serde(default)]
    items: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    album: Option<AlbumStub>,
}

#[derive(Debug, Deserialize)]
struct AlbumPage {
    #[serde(default)]
    items: Vec<AlbumStub>,
}

#[derive(Debug, Deserialize)]
struct AlbumStub {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    artists: Vec<Named>,
    /// Corroboration, carried by the simplified album the search returns, so
    /// ranking costs no extra request.
    #[serde(rename = "total_tracks")]
    total_tracks: Option<u32>,
    #[serde(rename = "release_date")]
    release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Album {
    #[serde(default)]
    copyrights: Vec<Copyright>,
}

#[derive(Debug, Deserialize)]
struct Copyright {
    text: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// A blocking handle on the Spotify Web API.
pub struct Client {
    http: Http,
    authorization: String,
    /// The search and album endpoints. Only the tests point them anywhere but
    /// Spotify.
    search: String,
    album_url: String,
    albums: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new(token: &str, max_wait: u64) -> Result<Self, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("the Spotify access token is empty".to_owned());
        }
        let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            http: Http::new(LABEL, user_agent, MIN_INTERVAL)?.waiting_at_most(max_wait),
            authorization: format!("Bearer {token}"),
            search: SEARCH_URL.to_owned(),
            album_url: ALBUM_URL.to_owned(),
            albums: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(base: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            authorization: "Bearer test".to_owned(),
            search: format!("{base}/search"),
            album_url: format!("{base}/albums"),
            albums: HashMap::new(),
        }
    }

    fn lookup(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        // An ISRC names the recording exactly, so it is worth asking with
        // first: it cannot come back with the wrong artist's album the way a
        // name search can. It identifies a recording rather than a release,
        // though — the same one sits on the album, the single and any number
        // of compilations — so the releases it turns up are still ranked by
        // name.
        let mut chosen = match &wanted.isrc {
            Some(isrc) => self.album_carrying(isrc, wanted)?,
            None => None,
        };
        if chosen.is_none() {
            chosen = self.album_named(wanted)?;
        }
        let Some(id) = chosen else {
            return Ok(None);
        };

        // The search answers with a simplified album that carries no
        // copyrights, so the match has to be fetched in full.
        let url = format!("{}/{id}", self.album_url);
        let authorization = self.authorization.clone();
        let Some(album): Option<Album> =
            self.http
                .get_json(&url, &[], &[("Authorization", authorization.as_str())])?
        else {
            return Ok(None);
        };
        Ok(copyright_of(&album))
    }

    /// The best release carrying a given recording, found by its ISRC.
    ///
    /// Every album this returns provably contains the recording, which makes a
    /// resembling name much safer evidence than it would be from a plain
    /// search.
    fn album_carrying(
        &mut self,
        isrc: &str,
        wanted: &AlbumEvidence,
    ) -> Result<Option<String>, LookupError> {
        let query = format!("isrc:{isrc}");
        let Some(found) = self.search(&query, "track")? else {
            return Ok(None);
        };
        Ok(best_of(
            found
                .tracks
                .iter()
                .flat_map(|page| &page.items)
                .filter_map(|track| track.album.as_ref()),
            wanted,
        ))
    }

    /// The best release matching the artist and album name.
    fn album_named(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        // Spotify's field filters keep a search for an album from answering
        // with a track or a playlist that merely mentions it.
        let query = format!(
            "album:{} artist:{}",
            quote_for_search(&wanted.album),
            quote_for_search(&wanted.artist)
        );
        let Some(found) = self.search(&query, "album")? else {
            return Ok(None);
        };
        Ok(best_of(
            found.albums.iter().flat_map(|page| &page.items),
            wanted,
        ))
    }

    fn search(&mut self, query: &str, kind: &str) -> Result<Option<SearchResponse>, LookupError> {
        let limit = RESULT_LIMIT.to_string();
        let authorization = self.authorization.clone();
        let search = self.search.clone();
        self.http.get_json(
            &search,
            &[("q", query), ("type", kind), ("limit", limit.as_str())],
            &[("Authorization", authorization.as_str())],
        )
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

/// How well a candidate matches, or `None` when it is a different release.
///
/// The album is ranked first: a search constrained to one artist separates its
/// results by album name, so that is what decides between them.
fn rank(candidate: &AlbumStub, wanted: &AlbumEvidence) -> Option<Score> {
    candidate.id.as_ref()?;
    let name = confidence(candidate.name.as_deref(), &wanted.album)?;
    let artist = candidate
        .artists
        .iter()
        .filter_map(|credited| confidence(credited.name.as_deref(), &wanted.artist))
        .min()?;
    Some(Score::new(
        name,
        artist,
        candidate.total_tracks,
        wanted.total_tracks,
        candidate.release_date.as_deref(),
        wanted.year.as_deref(),
    ))
}

/// The id of the best-ranked album among some candidates.
fn best_of<'a>(
    candidates: impl Iterator<Item = &'a AlbumStub>,
    wanted: &AlbumEvidence,
) -> Option<String> {
    candidates
        .filter_map(|candidate| Some((rank(candidate, wanted)?, candidate)))
        .min_by_key(|(rank, _)| *rank)
        .and_then(|(_, candidate)| candidate.id.clone())
}

/// The album's copyright line, preferring ℗ over ©.
fn copyright_of(album: &Album) -> Option<String> {
    for (kind, symbol) in [("P", '\u{2117}'), ("C", '\u{a9}')] {
        let line = album
            .copyrights
            .iter()
            .filter(|copyright| {
                copyright
                    .kind
                    .as_deref()
                    .is_some_and(|it| it.eq_ignore_ascii_case(kind))
            })
            .find_map(|copyright| with_symbol(copyright.text.as_deref()?, symbol));
        if line.is_some() {
            return line;
        }
    }
    None
}

/// The text as Spotify wrote it, given its symbol if it has none.
///
/// Spotify is inconsistent: some lines open with `℗`, some with `(P)`, and
/// many with the bare year. Only the last kind needs the symbol added.
fn with_symbol(text: &str, symbol: char) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let already_marked = text.starts_with(['\u{2117}', '\u{a9}'])
        || text.len() >= 3 && matches!(&text.to_uppercase()[..3], "(P)" | "(C)");
    Some(if already_marked {
        text.to_owned()
    } else {
        format!("{symbol} {text}")
    })
}

/// Wrap a value as a search phrase, dropping what would break the query.
fn quote_for_search(value: &str) -> String {
    let escaped: String = value
        .chars()
        .filter(|character| *character != '"')
        .collect();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{DEFAULT_MAX_WAIT, testing::Server};

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

    fn album(json: &str) -> Album {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn prefers_the_p_line_and_takes_the_text_as_it_stands() {
        let discovery = album(
            r#"{"copyrights":[
                {"text":"© 2001 Virgin Records Ltd.","type":"C"},
                {"text":"℗ 2001 Daft Life Ltd.","type":"P"}
            ]}"#,
        );
        assert_eq!(
            copyright_of(&discovery).as_deref(),
            Some("\u{2117} 2001 Daft Life Ltd.")
        );
    }

    #[test]
    fn adds_the_symbol_to_a_line_that_has_none() {
        let bare = album(r#"{"copyrights":[{"text":"2001 Daft Life Ltd.","type":"P"}]}"#);
        assert_eq!(
            copyright_of(&bare).as_deref(),
            Some("\u{2117} 2001 Daft Life Ltd.")
        );
        // A line already marked the ASCII way is left exactly as it is.
        let ascii = album(r#"{"copyrights":[{"text":"(P) 2001 Daft Life Ltd.","type":"P"}]}"#);
        assert_eq!(
            copyright_of(&ascii).as_deref(),
            Some("(P) 2001 Daft Life Ltd.")
        );
    }

    #[test]
    fn falls_back_to_the_c_line() {
        let only_copyright =
            album(r#"{"copyrights":[{"text":"1997 Virgin Records Ltd.","type":"C"}]}"#);
        assert_eq!(
            copyright_of(&only_copyright).as_deref(),
            Some("\u{a9} 1997 Virgin Records Ltd.")
        );
    }

    #[test]
    fn an_album_with_no_usable_copyright_yields_nothing() {
        assert_eq!(copyright_of(&album(r#"{"copyrights":[]}"#)), None);
        assert_eq!(copyright_of(&album(r#"{}"#)), None);
        assert_eq!(
            copyright_of(&album(r#"{"copyrights":[{"text":"   ","type":"P"}]}"#)),
            None
        );
        assert_eq!(
            copyright_of(&album(r#"{"copyrights":[{"text":"anything"}]}"#)),
            None
        );
    }

    #[test]
    fn only_a_matching_album_with_an_id_is_fetched() {
        let page: SearchResponse = serde_json::from_str(
            r#"{"albums":{"items":[
                {"id":null,"name":"Discovery","artists":[{"name":"Daft Punk"}]},
                {"id":"one","name":"Discovery","artists":[{"name":"Pink Floyd"}]},
                {"id":"two","name":"Discovery (Deluxe)","artists":[{"name":"Daft Punk"}]}
            ]}}"#,
        )
        .unwrap();

        let chosen: Vec<&str> = page
            .albums
            .iter()
            .flat_map(|page| &page.items)
            .filter(|candidate| rank(candidate, &wanting("Daft Punk", "Discovery")).is_some())
            .filter_map(|candidate| candidate.id.as_deref())
            .collect();
        assert_eq!(chosen, ["two"]);
    }

    #[test]
    fn a_token_is_required() {
        assert!(Client::new("  ", DEFAULT_MAX_WAIT).is_err());
    }

    /// The search answers with a simplified album that has no copyrights, so
    /// the matching one is fetched in full — and only that one.
    #[test]
    fn searches_then_fetches_the_matching_album_in_full() {
        let server = Server::answering(&[
            r#"{"albums":{"items":[
                {"id":"wrong","name":"Discovery","artists":[{"name":"Pink Floyd"}]},
                {"id":"right","name":"Discovery","artists":[{"name":"Daft Punk"}]}
            ]}}"#,
            r#"{"copyrights":[{"text":"2001 Daft Life Ltd.","type":"P"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let copyright = client
            .copyright(&wanting("Daft Punk", "Discovery"))
            .unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/search?q=album%3A%22Discovery%22+artist%3A%22Daft+Punk%22&type=album&limit=10"
        );
        assert_eq!(asked[1].target, "/albums/right");
        for request in &asked {
            assert_eq!(request.header("Authorization"), Some("Bearer test"));
        }
    }

    #[test]
    fn a_search_that_matches_nothing_costs_one_request() {
        let server = Server::answering(&[
            r#"{"albums":{"items":[{"id":"wrong","name":"Homework","artists":[{"name":"Daft Punk"}]}]}}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap(),
            None
        );

        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn an_expired_token_is_reported_rather_than_read_as_a_miss() {
        let server = Server::replying(&[crate::sources::testing::status("401 Unauthorized", "")]);
        let mut client = Client::pointing_at(&server.address);

        let error = client
            .copyright(&wanting("Daft Punk", "Discovery"))
            .expect_err("an expired token must not look like an album nobody has");

        assert!(error.to_string().contains("expired"), "{error}");
        server.requests();
    }

    #[test]
    fn an_album_is_only_looked_up_once() {
        let server = Server::answering(&[r#"{"albums":{"items":[]}}"#]);
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

    /// An ISRC is asked with first, as a track search: it names the recording
    /// exactly, so it cannot come back with another artist's album.
    #[test]
    fn an_isrc_is_searched_for_before_a_name() {
        let server = Server::answering(&[
            r#"{"tracks":{"items":[
                {"album":{"id":"right","name":"Discovery","artists":[{"name":"Daft Punk"}],
                          "total_tracks":14,"release_date":"2001-03-12"}}
            ]}}"#,
            r#"{"copyrights":[{"text":"℗ 2001 Daft Life Ltd.","type":"P"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.isrc = Some("GBUM71029604".to_owned());

        let copyright = client.copyright(&wanted).unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/search?q=isrc%3AGBUM71029604&type=track&limit=10"
        );
        assert_eq!(asked[1].target, "/albums/right");
        // Still two requests per album, the same as a name lookup.
        assert_eq!(asked.len(), 2);
    }

    /// A recording sits on the album, the single and the compilations alike,
    /// so the releases an ISRC turns up are still ranked by name.
    #[test]
    fn the_album_name_chooses_between_the_releases_an_isrc_finds() {
        let server = Server::answering(&[
            r#"{"tracks":{"items":[
                {"album":{"id":"compilation","name":"Now That's What I Call Music! 48",
                          "artists":[{"name":"Various Artists"}]}},
                {"album":{"id":"album","name":"Discovery","artists":[{"name":"Daft Punk"}]}}
            ]}}"#,
            r#"{"copyrights":[{"text":"℗ 2001 Daft Life Ltd.","type":"P"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.isrc = Some("GBUM71029604".to_owned());

        assert!(client.copyright(&wanted).unwrap().is_some());
        assert_eq!(server.requests()[1].target, "/albums/album");
    }

    /// An ISRC that finds nothing must not lose the album: the name search is
    /// still there to fall back on.
    #[test]
    fn a_fruitless_isrc_falls_back_to_the_name() {
        let server = Server::answering(&[
            r#"{"tracks":{"items":[]}}"#,
            r#"{"albums":{"items":[
                {"id":"named","name":"Discovery","artists":[{"name":"Daft Punk"}]}
            ]}}"#,
            r#"{"copyrights":[{"text":"℗ 2001 Daft Life Ltd.","type":"P"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.isrc = Some("GBUM71029604".to_owned());

        assert!(client.copyright(&wanted).unwrap().is_some());
        let asked = server.requests();
        assert!(asked[0].target.contains("isrc"), "{}", asked[0].target);
        assert!(asked[1].target.contains("album%3A"), "{}", asked[1].target);
        assert_eq!(asked[2].target, "/albums/named");
    }

    /// Without an ISRC nothing changes: one search, by name.
    #[test]
    fn a_file_with_no_isrc_searches_by_name_alone() {
        let server = Server::answering(&[
            r#"{"albums":{"items":[
                {"id":"named","name":"Discovery","artists":[{"name":"Daft Punk"}]}
            ]}}"#,
            r#"{"copyrights":[{"text":"℗ 2001 Daft Life Ltd.","type":"P"}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        assert!(
            client
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap()
                .is_some()
        );
        let asked = server.requests();
        assert_eq!(asked.len(), 2);
        assert!(!asked[0].target.contains("isrc"), "{}", asked[0].target);
    }
}
