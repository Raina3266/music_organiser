//! Odesli (song.link) as a Spotify-to-YouTube-Music link resolver.
//!
//! spotDL already finds a YouTube match for a bare Spotify link by searching
//! YouTube Music for the title and artist and scoring what comes back. That is
//! a good guess, and a guess is what an exact-source pair exists to replace —
//! so a pair is only worth writing down when it came from somewhere that did
//! not guess.
//!
//! Odesli is that somewhere: it cross-references the streaming catalogues by
//! their own identifiers rather than by matching text, so it disagrees with a
//! search precisely where a search goes wrong. Asking it for the YouTube Music
//! link that belongs to a Spotify track turns a searched download into a
//! pinned one.
//!
//! Only the `youtubeMusic` link is used. Odesli often also knows a plain
//! `youtube` link for the same track, but that is as likely to be the music
//! video — wrong length, spoken intro, live take — and pinning the audio to
//! the wrong recording is worse than letting spotDL search for the right one.
//!
//! Odesli has since withdrawn anonymous access to this API and deprecated the
//! `v1-alpha.1` namespace it lives in, so a key is required and the endpoint
//! is on borrowed time. Nothing here can work around that: without a key the
//! first request comes back `PUBLIC_API_ACCESS_DEPRECATED`, the run stops, and
//! every line is left bare for spotDL to search as it did before.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::LookupError;
use crate::sources::{Limits, http::Http};

pub const LABEL: &str = "Odesli";

const API_URL: &str = "https://api.song.link/v1-alpha.1/links";
/// The platform key whose link is a YouTube Music track.
const YOUTUBE_MUSIC: &str = "youtubeMusic";
/// Storefront to answer for. Availability differs by country, and `US` is the
/// most complete catalogue, matching the default the API itself applies.
pub const DEFAULT_COUNTRY: &str = "US";
/// The environment variable an API key may arrive in, named here so the
/// unauthorized message can point at it.
const KEY_VARIABLE: &str = super::resolve::KEY_VARIABLE;

/// Gap between requests without an API key.
///
/// The ten-per-minute anonymous tier this paces to has been withdrawn, so an
/// unkeyed run is now refused at its first request and never reaches the
/// second. The spacing is kept because the refusal is the server's to give:
/// should anonymous access return, this asks at the published rate rather
/// than at one that would earn a ban.
const FREE_INTERVAL: Duration = Duration::from_secs(6);
/// Gap between requests once a key is supplied.
///
/// Odesli issues keys with a raised limit but does not publish the number, so
/// this starts brisk rather than fast and leaves the correction to the
/// throttling: a 429 widens the spacing and it eases back once requests are
/// getting through again.
const KEYED_INTERVAL: Duration = Duration::from_millis(500);

/// The half of the Odesli response this needs: a URL per platform.
///
/// Deserialized as a map rather than a struct of every platform because the
/// list grows, and an unknown key must not turn a good answer into a parse
/// error.
#[derive(Debug, Deserialize)]
struct LinksResponse {
    #[serde(rename = "linksByPlatform", default)]
    links_by_platform: HashMap<String, PlatformLink>,
}

#[derive(Debug, Deserialize)]
struct PlatformLink {
    url: Option<String>,
}

/// What a 401 from Odesli means, which depends on what was sent.
///
/// A run that sent no credential has no stale token to blame, and saying it
/// has sends the reader hunting for one that does not exist. Since v1-alpha.1
/// was deprecated, an anonymous request is refused outright with
/// `PUBLIC_API_ACCESS_DEPRECATED`, so that is what the message says.
fn unauthorized_means(keyed: bool) -> String {
    if keyed {
        format!(
            "{LABEL} rejected the API key (HTTP 401). Check the key, or drop \
             --api-key/--api-key-file to try an anonymous request."
        )
    } else {
        format!(
            "{LABEL} no longer answers anonymous requests: access to its public \
             v1-alpha.1 API has been withdrawn, and a key is now required. Ask \
             developers@song.link for one, then set {KEY_VARIABLE} or pass --api-key. \
             That namespace is deprecated outright, so a key may not buy long."
        )
    }
}

/// A blocking handle on the Odesli links API.
pub struct Client {
    http: Http,
    /// Only the tests point this anywhere but Odesli.
    endpoint: String,
    key: Option<String>,
    country: String,
    /// Answers already given, so a link repeated in an input file costs one
    /// request. At six seconds each that is worth remembering.
    resolved: HashMap<String, Option<String>>,
}

impl Client {
    pub fn new(key: Option<&str>, country: &str, limits: Limits) -> Result<Self, String> {
        let country = country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.chars().all(|letter| letter.is_ascii_alphabetic()) {
            return Err(format!(
                "{country:?} is not a two-letter country code, such as US or GB"
            ));
        }
        let key = match key.map(str::trim) {
            Some("") => return Err("the Odesli API key is empty".to_owned()),
            other => other.map(str::to_owned),
        };
        let interval = if key.is_some() {
            KEYED_INTERVAL
        } else {
            FREE_INTERVAL
        };
        let user_agent = concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
            " +https://github.com/raina3266/music_organiser"
        );
        Ok(Self {
            http: Http::new(LABEL, user_agent, interval)?
                .unauthorized_means(unauthorized_means(key.is_some()))
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            endpoint: API_URL.to_owned(),
            key,
            country,
            resolved: HashMap::new(),
        })
    }

    /// How long one request is being spaced from the next, for the run to say
    /// out loud before it starts: the free rate makes a long file a long wait,
    /// and that is better known at the start than discovered at the end.
    pub fn interval(&self) -> Duration {
        if self.key.is_some() {
            KEYED_INTERVAL
        } else {
            FREE_INTERVAL
        }
    }

    /// The YouTube Music URL for a Spotify track, or `None` when Odesli knows
    /// the track but has no YouTube Music link for it.
    pub fn youtube_music_url(&mut self, spotify_url: &str) -> Result<Option<String>, LookupError> {
        if let Some(cached) = self.resolved.get(spotify_url) {
            return Ok(cached.clone());
        }

        let endpoint = self.endpoint.clone();
        let mut query = vec![("url", spotify_url), ("userCountry", self.country.as_str())];
        if let Some(key) = &self.key {
            query.push(("key", key.as_str()));
        }

        // A 404 is Odesli saying it does not know this track. That is an
        // answer, not a failure: the line stays a bare Spotify link and spotDL
        // searches for it as it always did.
        let found: Option<LinksResponse> = self.http.get_json(&endpoint, &query, &[])?;
        let url = found.and_then(|response| {
            response
                .links_by_platform
                .get(YOUTUBE_MUSIC)
                .and_then(|link| link.url.clone())
                .filter(|url| !url.trim().is_empty())
        });

        self.resolved.insert(spotify_url.to_owned(), url.clone());
        Ok(url)
    }
}

#[cfg(test)]
impl Client {
    /// A client pointed at a local test server instead of Odesli, paced as
    /// fast as the plumbing allows so tests do not sit through a rate limit.
    pub(crate) fn testing(base: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            endpoint: base.to_owned(),
            key: None,
            country: DEFAULT_COUNTRY.to_owned(),
            resolved: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::testing::{Server, status};

    fn pointing_at(base: &str) -> Client {
        Client::testing(base)
    }

    const TRACK: &str = "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW";
    const LINKS: &str = r#"{
        "entityUniqueId": "SPOTIFY_SONG::02Q0SXOsk74oV4hesiL6JW",
        "linksByPlatform": {
            "spotify": {"url": "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"},
            "youtube": {"url": "https://www.youtube.com/watch?v=OTHERVIDEO"},
            "youtubeMusic": {"url": "https://music.youtube.com/watch?v=dQw4w9WgXcQ"}
        }
    }"#;

    #[test]
    fn asks_for_the_track_and_returns_its_youtube_music_link() {
        let server = Server::answering(&[LINKS]);
        let mut client = pointing_at(&server.address);

        assert_eq!(
            client.youtube_music_url(TRACK).unwrap().as_deref(),
            Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
        );

        let asked = server.requests();
        assert_eq!(asked.len(), 1);
        assert!(
            asked[0]
                .target
                .contains("url=https%3A%2F%2Fopen.spotify.com%2Ftrack%2F02Q0SXOsk74oV4hesiL6JW"),
            "the Spotify URL is sent url-encoded: {}",
            asked[0].target
        );
        assert!(asked[0].target.contains("userCountry=US"));
        assert!(
            !asked[0].target.contains("key="),
            "no key is sent when none was given"
        );
    }

    #[test]
    fn sends_the_key_and_country_when_they_were_given() {
        let server = Server::answering(&[LINKS]);
        let mut client = Client {
            key: Some("secret-key".to_owned()),
            country: "GB".to_owned(),
            ..pointing_at(&server.address)
        };

        client.youtube_music_url(TRACK).unwrap();

        let asked = server.requests();
        assert!(asked[0].target.contains("key=secret-key"));
        assert!(asked[0].target.contains("userCountry=GB"));
    }

    /// The music video is deliberately not a fallback: pinning the audio to
    /// the wrong recording is worse than letting spotDL search for it.
    #[test]
    fn ignores_a_plain_youtube_link_when_there_is_no_youtube_music_one() {
        let server = Server::answering(&[
            r#"{"linksByPlatform": {"youtube": {"url": "https://www.youtube.com/watch?v=VIDEO"}}}"#,
        ]);
        let mut client = pointing_at(&server.address);

        assert_eq!(client.youtube_music_url(TRACK).unwrap(), None);
    }

    #[test]
    fn an_unknown_track_is_an_answer_rather_than_a_failure() {
        let server = Server::replying(&[status("404 Not Found", "{}")]);
        let mut client = pointing_at(&server.address);

        assert_eq!(client.youtube_music_url(TRACK).unwrap(), None);
    }

    #[test]
    fn tolerates_platforms_it_does_not_know_and_a_missing_url() {
        let server = Server::answering(&[
            r#"{"linksByPlatform": {"someNewService": {"url": "https://example.com/x",
                "extra": 1}, "youtubeMusic": {"entityUniqueId": "x"}}}"#,
        ]);
        let mut client = pointing_at(&server.address);

        assert_eq!(client.youtube_music_url(TRACK).unwrap(), None);
    }

    /// One request per distinct track, however often the file repeats it.
    #[test]
    fn a_track_is_only_looked_up_once() {
        let server = Server::answering(&[LINKS]);
        let mut client = pointing_at(&server.address);

        let first = client.youtube_music_url(TRACK).unwrap();
        let second = client.youtube_music_url(TRACK).unwrap();

        assert_eq!(first, second);
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn rejects_a_country_that_is_not_a_two_letter_code() {
        assert!(Client::new(None, "United States", Limits::default()).is_err());
        assert!(Client::new(None, "U1", Limits::default()).is_err());
        assert!(Client::new(None, "", Limits::default()).is_err());
        assert!(Client::new(Some("  "), "US", Limits::default()).is_err());
    }

    #[test]
    fn a_key_buys_a_shorter_gap_between_requests() {
        let free = Client::new(None, "us", Limits::default()).unwrap();
        let keyed = Client::new(Some("key"), "US", Limits::default()).unwrap();

        assert_eq!(free.interval(), FREE_INTERVAL);
        assert!(keyed.interval() < free.interval());
        // A lowercase code is accepted and sent in the form the API uses.
        assert_eq!(free.country, "US");
    }
}
