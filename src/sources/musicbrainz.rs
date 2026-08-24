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

use crate::sources::{
    Limits,
    http::Http,
    naming::{Score, confidence, copyright_line, year_of},
};
use crate::{AlbumEvidence, Language, parse_language};
use crate::{CopyrightLookup, LookupError};

pub const LABEL: &str = "MusicBrainz";
/// Environment variable naming whoever is running this copy.
pub const CONTACT_VARIABLE: &str = "MUSICBRAINZ_CONTACT";

const SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release";
const RECORDING_URL: &str = "https://musicbrainz.org/ws/2/recording";
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
    /// Corroboration carried by the search result itself.
    date: Option<String>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
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

#[derive(Debug, Deserialize)]
struct RecordingSearch {
    #[serde(default)]
    recordings: Vec<RecordingStub>,
}

#[derive(Debug, Deserialize)]
struct RecordingStub {
    id: String,
    title: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
}

#[derive(Debug, Deserialize)]
struct Recording {
    #[serde(default)]
    relations: Vec<WorkRelation>,
}

#[derive(Debug, Deserialize)]
struct WorkRelation {
    work: Option<Work>,
}

/// A work is the song as written, which is where MusicBrainz records the
/// language it is sung in.
#[derive(Debug, Deserialize)]
struct Work {
    /// The current spelling, a list because a song can be sung in more than
    /// one.
    #[serde(default)]
    languages: Vec<String>,
    /// The older singular field, still present on plenty of works.
    language: Option<String>,
}

/// A blocking handle on the MusicBrainz web service.
pub struct Client {
    http: Http,
    /// The release endpoint. Searches go to it and a release is read from
    /// under it; only the tests point it anywhere but MusicBrainz.
    releases: String,
    /// The recording endpoint, used to reach a track's work and so its
    /// language.
    recordings: String,
    albums: HashMap<(String, String), Option<String>>,
    /// One answer per track, since language is a property of the song rather
    /// than of the release it sits on.
    languages: HashMap<String, Option<Language>>,
}

impl Client {
    /// `contact` is the email address or URL to advertise in the User-Agent.
    pub fn new(contact: Option<&str>, limits: Limits) -> Result<Self, String> {
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
            http: Http::new(LABEL, &user_agent, MIN_INTERVAL)?
                .waiting_at_most(limits.max_wait)
                .attempting_at_most(limits.max_attempts),
            releases: SEARCH_URL.to_owned(),
            recordings: RECORDING_URL.to_owned(),
            albums: HashMap::new(),
            languages: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(releases: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            releases: releases.to_owned(),
            recordings: releases.to_owned(),
            albums: HashMap::new(),
            languages: HashMap::new(),
        }
    }

    /// The language one track is sung in, according to MusicBrainz.
    ///
    /// The answer comes from the *work* — the song as written — rather than
    /// from the release, because a release's language describes the text on
    /// its track list. An English song on a Korean album has a Korean track
    /// list and English lyrics, and it is the lyrics that `TLAN` is about.
    ///
    /// `None` covers every ordinary gap: no recording found, no work linked to
    /// it, no language recorded on the work. Work data is patchy, so a caller
    /// should have something else to fall back on.
    pub fn language(&mut self, track: &AlbumEvidence) -> Result<Option<Language>, LookupError> {
        let Some(title) = track.track_title.as_deref() else {
            return Ok(None);
        };
        let key = match &track.isrc {
            Some(isrc) => format!("isrc:{isrc}"),
            None => format!("{} - {title}", track.artist),
        };
        if let Some(cached) = self.languages.get(&key) {
            return Ok(cached.clone());
        }

        let language = self.look_the_work_up(track, title)?;
        self.languages.insert(key, language.clone());
        Ok(language)
    }

    fn look_the_work_up(
        &mut self,
        track: &AlbumEvidence,
        title: &str,
    ) -> Result<Option<Language>, LookupError> {
        // An ISRC names the recording outright; without one the artist and
        // title have to do, ranked the same way releases are.
        let query = match &track.isrc {
            Some(isrc) => format!("isrc:{}", quote_for_lucene(isrc)),
            None => format!(
                "artist:{} AND recording:{}",
                quote_for_lucene(&track.artist),
                quote_for_lucene(title)
            ),
        };
        let limit = RESULT_LIMIT.to_string();
        let recordings = self.recordings.clone();
        let Some(found): Option<RecordingSearch> = self.http.get_json(
            &recordings,
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

        let Some(id) = best_recording(&found.recordings, track, title) else {
            return Ok(None);
        };
        let url = format!("{recordings}/{id}");
        let Some(recording): Option<Recording> =
            self.http
                .get_json(&url, &[("inc", "work-rels"), ("fmt", "json")], &[])?
        else {
            return Ok(None);
        };
        Ok(language_of_works(&recording))
    }

    fn lookup(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        // The release index is searchable by ISRC, which finds the releases
        // that provably carry the recording instead of the ones whose name
        // merely resembles the tag's. The album name still ranks them, since
        // a recording sits on the album, the single and the compilations
        // alike.
        if let Some(isrc) = &wanted.isrc
            && let Some(copyright) =
                self.search_releases(&format!("isrc:{}", quote_for_lucene(isrc)), wanted)?
        {
            return Ok(Some(copyright));
        }
        let query = format!(
            "artist:{} AND release:{}",
            quote_for_lucene(&wanted.artist),
            quote_for_lucene(&wanted.album)
        );
        self.search_releases(&query, wanted)
    }

    /// Run one release search and open the closest matches it returns.
    fn search_releases(
        &mut self,
        query: &str,
        wanted: &AlbumEvidence,
    ) -> Result<Option<String>, LookupError> {
        let limit = RESULT_LIMIT.to_string();
        let releases = self.releases.clone();
        let Some(found): Option<SearchResponse> = self.http.get_json(
            &releases,
            &[("query", query), ("limit", limit.as_str()), ("fmt", "json")],
            &[],
        )?
        else {
            return Ok(None);
        };

        // Only a few releases can be opened, so they are opened closest
        // first: a search for `Discovery` lists dozens of pressings and the
        // deluxe edition is as likely as not to come back ahead of the plain
        // one.
        let mut candidates: Vec<(_, &ReleaseStub)> = found
            .releases
            .iter()
            .filter_map(|release| Some((rank(release, wanted)?, release)))
            .collect();
        candidates.sort_by_key(|(rank, _)| *rank);
        let candidates = candidates
            .into_iter()
            .map(|(_, release)| release)
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

/// How well a search result matches, or `None` when it is a different release.
fn rank(release: &ReleaseStub, wanted: &AlbumEvidence) -> Option<Score> {
    let name = confidence(release.title.as_deref(), &wanted.album)?;
    let artist = release
        .artist_credit
        .iter()
        .filter_map(|credit| confidence(credit.name.as_deref(), &wanted.artist))
        .min()?;
    Some(Score::new(
        name,
        artist,
        release.track_count,
        wanted.total_tracks,
        release.date.as_deref(),
        wanted.year.as_deref(),
    ))
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

/// The closest recording to what the tag says, or `None` when none match.
///
/// An ISRC search returns the right recording already, but the ranking costs
/// nothing and guards the artist-and-title path, where a search for a common
/// title can return anybody's song of that name.
fn best_recording(
    recordings: &[RecordingStub],
    track: &AlbumEvidence,
    title: &str,
) -> Option<String> {
    recordings
        .iter()
        .filter_map(|recording| {
            let name = confidence(recording.title.as_deref(), title)?;
            let artist = recording
                .artist_credit
                .iter()
                .filter_map(|credit| confidence(credit.name.as_deref(), &track.artist))
                .min()?;
            Some(((name, artist), recording))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, recording)| recording.id.clone())
}

/// The language of the first work that records one.
///
/// A recording can link to several works, and a work can list several
/// languages when a song is sung in more than one. `TLAN` holds a single
/// language, so the first that parses is the one written; anything MusicBrainz
/// spells with a code this program does not know is passed over rather than
/// guessed at.
fn language_of_works(recording: &Recording) -> Option<Language> {
    recording
        .relations
        .iter()
        .filter_map(|relation| relation.work.as_ref())
        .flat_map(|work| work.languages.iter().chain(work.language.iter()))
        .find_map(|code| parse_language(code))
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
            .filter(|release| rank(release, &wanting("Daft Punk", "Discovery")).is_some())
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

        let copyright = client
            .copyright(&wanting("Daft Punk", "Discovery"))
            .unwrap();

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

    #[test]
    fn an_album_with_no_artist_is_not_searched_for_at_all() {
        let server = Server::answering(&[]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(client.copyright(&wanting("  ", "Discovery")).unwrap(), None);
        assert_eq!(client.copyright(&wanting("Daft Punk", "")).unwrap(), None);

        assert!(server.requests().is_empty());
    }

    /// MusicBrainz indexes releases by the ISRCs they carry, so the exact
    /// identifier replaces the name search rather than adding a request.
    #[test]
    fn an_isrc_is_searched_for_before_a_name() {
        let server = Server::answering(&[
            r#"{"releases":[
                {"id":"right","title":"Discovery","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"date":"2001-03-12","relations":[
                {"type":"phonographic copyright","begin":"2001","label":{"name":"Daft Life Ltd."}}
            ]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.isrc = Some("GBUM71029604".to_owned());

        let copyright = client.copyright(&wanted).unwrap();

        assert_eq!(copyright.as_deref(), Some("\u{2117} 2001 Daft Life Ltd."));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/?query=isrc%3A%22GBUM71029604%22&limit=10&fmt=json"
        );
        assert_eq!(asked[1].target, "/right?inc=label-rels&fmt=json");
    }

    #[test]
    fn a_fruitless_isrc_falls_back_to_the_name() {
        let server = Server::answering(&[
            r#"{"releases":[]}"#,
            r#"{"releases":[
                {"id":"named","title":"Discovery","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"date":"2001","relations":[
                {"type":"phonographic copyright","label":{"name":"Daft Life Ltd."}}
            ]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.isrc = Some("GBUM71029604".to_owned());

        assert!(client.copyright(&wanted).unwrap().is_some());
        let asked = server.requests();
        assert!(asked[0].target.contains("isrc"), "{}", asked[0].target);
        assert!(asked[1].target.contains("artist"), "{}", asked[1].target);
    }

    /// Only three releases are ever opened, so the evidence decides which
    /// three: MusicBrainz lists a release per pressing.
    #[test]
    fn the_track_count_puts_the_right_pressing_first() {
        let server = Server::answering(&[
            r#"{"releases":[
                {"id":"deluxe","title":"Discovery","artist-credit":[{"name":"Daft Punk"}],
                 "track-count":20,"date":"2021"},
                {"id":"original","title":"Discovery","artist-credit":[{"name":"Daft Punk"}],
                 "track-count":14,"date":"2001-03-12"}
            ]}"#,
            r#"{"date":"2001-03-12","relations":[
                {"type":"phonographic copyright","begin":"2001","label":{"name":"Daft Life Ltd."}}
            ]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = wanting("Daft Punk", "Discovery");
        wanted.total_tracks = Some(14);

        assert!(client.copyright(&wanted).unwrap().is_some());
        // The fourteen-track pressing is opened first and answers, so the
        // twenty-track one is never fetched at all.
        let asked = server.requests();
        assert_eq!(asked[1].target, "/original?inc=label-rels&fmt=json");
        assert_eq!(asked.len(), 2);
    }

    fn track(artist: &str, album: &str, title: &str) -> AlbumEvidence {
        let mut evidence = wanting(artist, album);
        evidence.track_title = Some(title.to_owned());
        evidence
    }

    /// The language comes from the work -- the song as written -- reached
    /// through the recording, because a release's own language describes the
    /// text on its track list rather than what is sung.
    #[test]
    fn the_language_comes_from_the_works_of_the_recording() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"right","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"relations":[
                {"work":{"languages":["kor"],"language":"kor"}}
            ]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let language = client
            .language(&track("Daft Punk", "Discovery", "One More Time"))
            .unwrap()
            .unwrap();

        assert_eq!(language.name, "Korean");
        assert_eq!(language.code, "kor");
        let asked: Vec<String> = server
            .requests()
            .into_iter()
            .map(|request| request.target)
            .collect();
        assert_eq!(
            asked,
            [
                "/?query=artist%3A%22Daft+Punk%22+AND+recording%3A%22One+More+Time%22&limit=10&fmt=json",
                "/right?inc=work-rels&fmt=json",
            ]
        );
    }

    /// An ISRC names the recording outright, so it replaces the fuzzy search
    /// rather than adding to it.
    #[test]
    fn an_isrc_names_the_recording_outright() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"exact","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"relations":[{"work":{"languages":["eng"]}}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut wanted = track("Daft Punk", "Discovery", "One More Time");
        wanted.isrc = Some("GBUM71029604".to_owned());

        assert_eq!(client.language(&wanted).unwrap().unwrap().name, "English");
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/?query=isrc%3A%22GBUM71029604%22&limit=10&fmt=json"
        );
    }

    /// Work data is patchy. Every ordinary gap has to come back as "no
    /// answer", so the caller can fall back to reading the lyrics.
    #[test]
    fn every_gap_in_the_data_is_simply_no_answer() {
        // No recording found.
        let server = Server::answering(&[r#"{"recordings":[]}"#]);
        let mut client = Client::pointing_at(&server.address);
        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        server.requests();

        // A recording with no work linked to it, which is the common case.
        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"relations":[]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        server.requests();

        // A work with no language recorded on it.
        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"relations":[{"work":{}}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        server.requests();
    }

    /// A tag with no title has nothing to search a recording by, and must not
    /// spend a request finding that out.
    #[test]
    fn a_track_with_no_title_is_not_searched_for() {
        let server = Server::answering(&[]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client.language(&wanting("Daft Punk", "Discovery")).unwrap(),
            None
        );

        assert!(server.requests().is_empty());
    }

    /// Language belongs to the song, so it is cached per track rather than per
    /// album -- but asking twice for the same track still costs one lookup.
    #[test]
    fn a_track_is_only_looked_up_once() {
        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"relations":[{"work":{"languages":["fra"]}}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let wanted = track("Daft Punk", "Discovery", "One More Time");

        assert_eq!(client.language(&wanted).unwrap().unwrap().name, "French");
        assert_eq!(client.language(&wanted).unwrap().unwrap().name, "French");

        assert_eq!(server.requests().len(), 2);
    }

    /// A search for a common title can return anybody's song of that name, so
    /// the artist still has to agree.
    #[test]
    fn a_recording_by_the_wrong_artist_is_refused() {
        let server = Server::answering(&[r#"{"recordings":[
                {"id":"wrong","title":"One More Time","artist-credit":[{"name":"Britney Spears"}]}
            ]}"#]);
        let mut client = Client::pointing_at(&server.address);

        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        assert_eq!(server.requests().len(), 1);
    }
}
