//! LRCLIB as a synced-lyrics source.
//!
//! spotDL asks the `syncedlyrics` aggregator for an `.lrc`, and that search
//! goes out on the track's name and artist alone. Nothing in it checks how long
//! the recording is, so a remaster, a radio edit, or a live take answers as
//! readily as the cut actually on disk: the words come back right and the
//! timings drift.
//!
//! LRCLIB is built the other way round. It answers only when the duration asked
//! about matches its record within a couple of seconds, so the wrong version of
//! a song is a miss rather than a plausible-looking answer. That is the same
//! bargain the copyright lookups strike -- a wrong copyright is worse than none
//! -- applied to lyrics, and it is the whole reason this source is worth asking
//! before falling back to whatever spotDL wrote.
//!
//! The API needs no account, key, or token. It asks only that callers identify
//! themselves in the `User-Agent`, which [`Http`] already does.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::AlbumEvidence;
use crate::LookupError;
use crate::sources::{Limits, http::Http};

pub const LABEL: &str = "LRCLIB";

const GET_URL: &str = "https://lrclib.net/api/get";
/// LRCLIB publishes no hard rate limit and asks only that clients be
/// reasonable. This is the same spacing the Deezer source keeps, which a
/// library-sized run has never been refused at.
const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// The half of a record this needs. LRCLIB has already done the matching by
/// the time it answers, so there is nothing here to weigh.
#[derive(Debug, Clone, Deserialize)]
struct Record {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    /// A track LRCLIB knows to have no words at all. Distinct from having no
    /// lyrics on file, and a reason to stop asking rather than to fall back.
    #[serde(default)]
    instrumental: bool,
}

/// A blocking handle on the LRCLIB API.
pub struct Client {
    http: Http,
    /// The `api/get` endpoint. Only the tests point this anywhere but LRCLIB.
    get: String,
    /// Answers already given, keyed by everything that was asked.
    lyrics: HashMap<(String, String, u64), Option<String>>,
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
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            get: GET_URL.to_owned(),
            lyrics: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(base: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            get: format!("{base}/api/get"),
            lyrics: HashMap::new(),
        }
    }

    /// The synced lyrics for one track, or `None` when LRCLIB has none that
    /// belongs to a recording this long.
    ///
    /// `seconds` is the length of the file on disk, not a length any catalogue
    /// reported: the point is to check the candidate against the audio, and a
    /// duration taken from the same search that found the candidate would agree
    /// with it however wrong the match was.
    ///
    /// Plain lyrics are refused. They would replace the timed text spotDL
    /// wrote with untimed text, which is a loss even when the words are better.
    pub fn synced_lyrics(
        &mut self,
        wanted: &AlbumEvidence,
        seconds: u64,
    ) -> Result<Option<String>, LookupError> {
        let Some(title) = wanted.track_title.as_deref() else {
            return Ok(None);
        };
        // The performer, for the reason the Deezer and MusicBrainz recording
        // lookups use it: lyrics belong to the recording, and a compilation
        // credits the release to somebody who never played on it.
        let performer = wanted.performer();
        if performer.trim().is_empty() || title.trim().is_empty() {
            return Ok(None);
        }

        let key = (performer.to_owned(), title.to_owned(), seconds);
        if let Some(cached) = self.lyrics.get(&key) {
            return Ok(cached.clone());
        }

        let found = self.look_the_track_up(performer, title, &wanted.album, seconds)?;
        self.lyrics.insert(key, found.clone());
        Ok(found)
    }

    fn look_the_track_up(
        &mut self,
        performer: &str,
        title: &str,
        album: &str,
        seconds: u64,
    ) -> Result<Option<String>, LookupError> {
        let duration = seconds.to_string();
        let get = self.get.clone();
        // A 404 is LRCLIB's answer for "nothing this long matches", which is
        // the ordinary outcome rather than a failure, and `get_json` already
        // reads it as `None`.
        let Some(record): Option<Record> = self.http.get_json(
            &get,
            &[
                ("artist_name", performer),
                ("track_name", title),
                ("album_name", album),
                ("duration", duration.as_str()),
            ],
            &[],
        )?
        else {
            return Ok(None);
        };

        if record.instrumental {
            return Ok(None);
        }
        Ok(record
            .synced_lyrics
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::testing::Server;

    fn evidence(title: Option<&str>) -> AlbumEvidence {
        AlbumEvidence {
            artist: "Daft Punk".to_owned(),
            album: "Discovery".to_owned(),
            track_artist: None,
            isrc: None,
            year: None,
            total_tracks: None,
            track_title: title.map(str::to_owned),
        }
    }

    #[test]
    fn the_length_on_disk_is_what_it_asks_about() {
        let server = Server::answering(&[
            r#"{"syncedLyrics":"[00:12.00]One more time","instrumental":false}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let found = client
            .synced_lyrics(&evidence(Some("One More Time")), 320)
            .unwrap();

        assert_eq!(found.as_deref(), Some("[00:12.00]One more time"));
        let requests = server.requests();
        assert!(
            requests[0].target.contains("duration=320"),
            "{:?}",
            requests[0].target
        );
        assert!(requests[0].target.contains("track_name=One+More+Time"));
        assert!(requests[0].target.contains("artist_name=Daft+Punk"));
        assert!(requests[0].target.contains("album_name=Discovery"));
    }

    #[test]
    fn nothing_of_that_length_is_an_answer_not_a_failure() {
        let server =
            Server::replying(&["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned()]);
        let mut client = Client::pointing_at(&server.address);

        let found = client
            .synced_lyrics(&evidence(Some("One More Time")), 320)
            .unwrap();

        assert_eq!(found, None);
    }

    #[test]
    fn plain_lyrics_alone_are_refused() {
        let server = Server::answering(&[
            r#"{"syncedLyrics":null,"plainLyrics":"One more time","instrumental":false}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .synced_lyrics(&evidence(Some("One More Time")), 320)
                .unwrap(),
            None
        );
    }

    #[test]
    fn an_instrumental_has_no_lyrics_to_write() {
        let server = Server::answering(&[r#"{"syncedLyrics":"[00:01.00] ","instrumental":true}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .synced_lyrics(&evidence(Some("Nightvision")), 104)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_tag_naming_no_track_is_never_asked_about() {
        // The server answers nothing, so reaching it at all would hang or fail.
        let mut client = Client::pointing_at("http://127.0.0.1:1");
        assert_eq!(client.synced_lyrics(&evidence(None), 320).unwrap(), None);
    }

    #[test]
    fn the_same_track_is_only_asked_about_once() {
        let server = Server::answering(&[
            r#"{"syncedLyrics":"[00:12.00]One more time","instrumental":false}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let wanted = evidence(Some("One More Time"));
        let first = client.synced_lyrics(&wanted, 320).unwrap();
        let second = client.synced_lyrics(&wanted, 320).unwrap();

        assert_eq!(first, second);
        assert_eq!(server.requests().len(), 1);
    }
}
