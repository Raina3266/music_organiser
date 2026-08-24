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

use crate::CopyrightLookup;
use crate::sources::{http::Http, naming::matches_name};

pub const LABEL: &str = "Spotify";
/// Environment variable holding the access token.
pub const TOKEN_VARIABLE: &str = "SPOTIFY_ACCESS_TOKEN";

const SEARCH_URL: &str = "https://api.spotify.com/v1/search";
const ALBUM_URL: &str = "https://api.spotify.com/v1/albums";
const MIN_INTERVAL: Duration = Duration::from_millis(200);
const RESULT_LIMIT: u8 = 10;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    albums: Option<AlbumPage>,
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
    pub fn new(token: &str) -> Result<Self, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("the Spotify access token is empty".to_owned());
        }
        let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            http: Http::new(LABEL, user_agent, MIN_INTERVAL)?,
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

    fn lookup(&mut self, artist: &str, album: &str) -> Result<Option<String>, String> {
        // Spotify's field filters keep a search for an album from answering
        // with a track or a playlist that merely mentions it.
        let query = format!(
            "album:{} artist:{}",
            quote_for_search(album),
            quote_for_search(artist)
        );
        let limit = RESULT_LIMIT.to_string();
        let authorization = self.authorization.clone();
        let search = self.search.clone();
        let Some(found): Option<SearchResponse> = self.http.get_json(
            &search,
            &[
                ("q", query.as_str()),
                ("type", "album"),
                ("limit", limit.as_str()),
            ],
            &[("Authorization", authorization.as_str())],
        )?
        else {
            return Ok(None);
        };

        // The search answers with a simplified album that carries no
        // copyrights, so the match has to be fetched in full.
        let Some(id) = found
            .albums
            .iter()
            .flat_map(|page| &page.items)
            .find(|candidate| is_the_album(candidate, artist, album))
            .and_then(|candidate| candidate.id.clone())
        else {
            return Ok(None);
        };

        let url = format!("{}/{id}", self.album_url);
        let Some(album): Option<Album> =
            self.http
                .get_json(&url, &[], &[("Authorization", authorization.as_str())])?
        else {
            return Ok(None);
        };
        Ok(copyright_of(&album))
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

fn is_the_album(candidate: &AlbumStub, artist: &str, album: &str) -> bool {
    candidate.id.is_some()
        && matches_name(candidate.name.as_deref(), album)
        && candidate
            .artists
            .iter()
            .any(|credited| matches_name(credited.name.as_deref(), artist))
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
    use crate::sources::testing::Server;

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
            .filter(|candidate| is_the_album(candidate, "Daft Punk", "Discovery"))
            .filter_map(|candidate| candidate.id.as_deref())
            .collect();
        assert_eq!(chosen, ["two"]);
    }

    #[test]
    fn a_token_is_required() {
        assert!(Client::new("  ").is_err());
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

        let copyright = client.copyright("Daft Punk", "Discovery").unwrap();

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

        assert_eq!(client.copyright("Daft Punk", "Discovery").unwrap(), None);

        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn an_expired_token_is_reported_rather_than_read_as_a_miss() {
        let server = Server::replying(&[crate::sources::testing::status("401 Unauthorized", "")]);
        let mut client = Client::pointing_at(&server.address);

        let error = client
            .copyright("Daft Punk", "Discovery")
            .expect_err("an expired token must not look like an album nobody has");

        assert!(error.contains("expired"), "{error}");
        server.requests();
    }

    #[test]
    fn an_album_is_only_looked_up_once() {
        let server = Server::answering(&[r#"{"albums":{"items":[]}}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(client.copyright("Daft Punk", "Discovery").unwrap(), None);
        assert_eq!(client.copyright("Daft Punk", "Discovery").unwrap(), None);

        assert_eq!(server.requests().len(), 1);
    }
}
