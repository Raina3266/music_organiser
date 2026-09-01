//! Deezer as an ISRC source.
//!
//! A token-free spotDL session writes no `TSRC`, so the ISRC has to be found
//! elsewhere. MusicBrainz answers first because its catalogue is the fuller
//! one, but it is a volunteer database and a track it has never had entered
//! simply is not there. Deezer is a commercial catalogue with a different
//! shape of coverage — strong on recent and mainstream releases, which is
//! where MusicBrainz is thinnest — so the two miss different tracks.
//!
//! The API needs no account, key, or token.
//!
//! Only the ISRC is taken. Deezer publishes no copyright line at all: its
//! album object carries a `label`, which is the marketing imprint rather than
//! the phonographic copyright holder, and the two are routinely different
//! companies. Assembling a `℗` line from it would produce something that reads
//! right and names the wrong entity, and a wrong copyright is worse than none.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::AlbumEvidence;
use crate::metadata::normalize_isrc;
use crate::sources::{Limits, http::Http, naming::confidence};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "Deezer";

const SEARCH_URL: &str = "https://api.deezer.com/search/track";
const TRACK_URL: &str = "https://api.deezer.com/track";
/// Deezer throttles per address at around fifty requests every five seconds.
/// This stays well inside that, since a library shares the allowance with
/// whatever else on this address is asking.
const MIN_INTERVAL: Duration = Duration::from_millis(200);
/// How many candidates to weigh before giving up on a confident match.
const RESULT_LIMIT: u8 = 10;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<TrackStub>,
}

/// A search hit. Deezer's search omits the ISRC, so a match has to be opened
/// to read it — the same two-step the Discogs source makes.
#[derive(Debug, Clone, Deserialize)]
struct TrackStub {
    id: u64,
    title: Option<String>,
    artist: Option<Named>,
}

#[derive(Debug, Clone, Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Track {
    isrc: Option<String>,
}

/// A blocking handle on the Deezer API.
pub struct Client {
    http: Http,
    /// The search and track endpoints. Only the tests point them anywhere but
    /// Deezer.
    search: String,
    tracks: String,
    /// Answers already given, keyed by performer and title.
    isrcs: HashMap<(String, String), Option<String>>,
}

impl Client {
    pub fn new(limits: Limits) -> Result<Self, String> {
        let user_agent = concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
            " +https://github.com/raina3266/music_organiser"
        );
        Ok(Self {
            http: Http::new(LABEL, user_agent, MIN_INTERVAL)?
                .forbidden_is_throttling()
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            search: SEARCH_URL.to_owned(),
            tracks: TRACK_URL.to_owned(),
            isrcs: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(base: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            search: format!("{base}/search"),
            tracks: base.to_owned(),
            isrcs: HashMap::new(),
        }
    }

    /// The ISRC for one track, or `None` when nothing matched confidently.
    ///
    /// A wrong ISRC is worse than none — it would send every later lookup
    /// confidently after the wrong recording — so a candidate is used only
    /// when both its title and its artist match what was asked for.
    pub fn isrc(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        let Some(title) = wanted.track_title.as_deref() else {
            return Ok(None);
        };
        // The performer, for the same reason the MusicBrainz recording lookup
        // uses it: Deezer credits a track to whoever played it, not to whoever
        // the compilation it sits on is credited to.
        let performer = wanted.performer();
        if performer.trim().is_empty() {
            return Ok(None);
        }

        let key = (performer.to_owned(), title.to_owned());
        if let Some(cached) = self.isrcs.get(&key) {
            return Ok(cached.clone());
        }

        let found = self.look_the_track_up(performer, title)?;
        self.isrcs.insert(key, found.clone());
        Ok(found)
    }

    fn look_the_track_up(
        &mut self,
        performer: &str,
        title: &str,
    ) -> Result<Option<String>, LookupError> {
        // Deezer's own field syntax, which searches the artist and track
        // fields rather than one bag of words, so a title that happens to
        // contain the artist's name cannot stand in for a real match.
        let query = format!(
            r#"artist:"{}" track:"{}""#,
            escape(performer),
            escape(title)
        );
        let limit = RESULT_LIMIT.to_string();
        let search = self.search.clone();
        let Some(results): Option<SearchResponse> = self.http.get_json(
            &search,
            &[("q", query.as_str()), ("limit", limit.as_str())],
            &[],
        )?
        else {
            return Ok(None);
        };

        let Some(id) = best_track(&results.data, performer, title) else {
            return Ok(None);
        };
        let url = format!("{}/{id}", self.tracks);
        let Some(track): Option<Track> = self.http.get_json(&url, &[], &[])? else {
            return Ok(None);
        };
        Ok(track.isrc.as_deref().and_then(normalize_isrc))
    }
}

/// Deezer's search treats `"` as the field delimiter, so one inside a name
/// would end the term early and search for something else entirely.
fn escape(value: &str) -> String {
    value.replace('"', " ")
}

/// The closest search hit whose title *and* artist both match.
fn best_track(tracks: &[TrackStub], performer: &str, title: &str) -> Option<u64> {
    tracks
        .iter()
        .filter_map(|track| {
            let name = confidence(track.title.as_deref(), title)?;
            let artist = confidence(
                track
                    .artist
                    .as_ref()
                    .and_then(|artist| artist.name.as_deref()),
                performer,
            )?;
            Some(((name, artist), track.id))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, id)| id)
}

/// Deezer answers no copyright at all, so it never supplies one.
///
/// Implemented so it can sit in the same collections as the other sources
/// without every caller special-casing it; it is wired in for the ISRC only.
impl CopyrightLookup for Client {
    fn copyright(&mut self, _album: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::testing::{Server, status};

    fn track(performer: &str, title: &str) -> AlbumEvidence {
        AlbumEvidence {
            artist: performer.to_owned(),
            album: "Discovery".to_owned(),
            track_artist: None,
            isrc: None,
            year: None,
            total_tracks: None,
            track_title: Some(title.to_owned()),
        }
    }

    const RESULTS: &str = r#"{"data":[
        {"id":11,"title":"One More Time","artist":{"name":"Stardust"}},
        {"id":22,"title":"One More Time","artist":{"name":"Daft Punk"}}
    ]}"#;

    #[test]
    fn searches_then_opens_the_matching_track_for_its_isrc() {
        let server = Server::answering(&[RESULTS, r#"{"id":22,"isrc":"GBUM71029604"}"#]);
        let mut client = Client::pointing_at(&server.address);

        let isrc = client.isrc(&track("Daft Punk", "One More Time")).unwrap();

        assert_eq!(isrc.as_deref(), Some("GBUM71029604"));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/search?q=artist%3A%22Daft+Punk%22+track%3A%22One+More+Time%22&limit=10"
        );
        // The right title by the wrong artist is passed over.
        assert_eq!(asked[1].target, "/22");
    }

    /// A compilation credits the release to one name and the track to another,
    /// and Deezer credits the track to whoever played it.
    #[test]
    fn searches_for_the_performer_rather_than_the_album_artist() {
        let server = Server::answering(&[RESULTS, r#"{"isrc":"GBUM71029604"}"#]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = track("Various Artists", "One More Time");
        wanted.track_artist = Some("Daft Punk".to_owned());

        assert_eq!(
            client.isrc(&wanted).unwrap().as_deref(),
            Some("GBUM71029604")
        );
        assert!(server.requests()[0].target.contains("Daft+Punk"));
    }

    #[test]
    fn a_title_by_the_wrong_artist_is_not_used() {
        let server = Server::answering(&[RESULTS]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client.isrc(&track("Pink Floyd", "One More Time")).unwrap(),
            None
        );
        // Nothing matched, so no track was opened.
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn a_track_deezer_does_not_know_is_no_answer_rather_than_a_failure() {
        let server = Server::answering(&[r#"{"data":[]}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client.isrc(&track("Daft Punk", "One More Time")).unwrap(),
            None
        );
    }

    /// A malformed ISRC is dropped rather than written: it would search
    /// confidently for a recording that does not exist.
    #[test]
    fn junk_in_the_isrc_field_is_refused() {
        let server = Server::answering(&[RESULTS, r#"{"isrc":"not an isrc"}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client.isrc(&track("Daft Punk", "One More Time")).unwrap(),
            None
        );
    }

    #[test]
    fn a_track_with_no_isrc_field_at_all_is_tolerated() {
        let server = Server::answering(&[RESULTS, r#"{"id":22,"title":"One More Time"}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client.isrc(&track("Daft Punk", "One More Time")).unwrap(),
            None
        );
    }

    #[test]
    fn a_tag_with_no_title_is_not_searched_for() {
        let server = Server::answering(&[]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = track("Daft Punk", "One More Time");
        wanted.track_title = None;

        assert_eq!(client.isrc(&wanted).unwrap(), None);
        assert_eq!(server.requests().len(), 0);
    }

    /// One request pair per distinct track, however many files share it.
    #[test]
    fn a_track_is_only_looked_up_once() {
        let server = Server::answering(&[RESULTS, r#"{"isrc":"GBUM71029604"}"#]);
        let mut client = Client::pointing_at(&server.address);

        let first = client.isrc(&track("Daft Punk", "One More Time")).unwrap();
        let second = client.isrc(&track("Daft Punk", "One More Time")).unwrap();

        assert_eq!(first, second);
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn a_quote_in_a_name_cannot_end_the_search_term_early() {
        let server = Server::answering(&[r#"{"data":[]}"#]);
        let mut client = Client::pointing_at(&server.address);

        client
            .isrc(&track(r#"Guns N" Roses"#, "Sweet Child"))
            .unwrap();

        // The quote becomes a space, so the only quotes left are the four
        // that delimit the two fields.
        assert_eq!(
            server.requests()[0].target,
            "/search?q=artist%3A%22Guns+N++Roses%22+track%3A%22Sweet+Child%22&limit=10"
        );
    }

    #[test]
    fn a_failing_request_is_a_failure_rather_than_a_missing_isrc() {
        let server = Server::replying(&[status("400 Bad Request", "{}")]);
        let mut client = Client::pointing_at(&server.address);

        assert!(client.isrc(&track("Daft Punk", "One More Time")).is_err());
    }

    #[test]
    fn deezer_never_supplies_a_copyright() {
        let server = Server::answering(&[]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .copyright(&track("Daft Punk", "One More Time"))
                .unwrap(),
            None
        );
        assert_eq!(server.requests().len(), 0);
    }
}
