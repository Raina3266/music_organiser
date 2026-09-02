//! MusicBrainz as a copyright source.
//!
//! MusicBrainz stores copyright mainly as relationships from a release to the
//! label or artist that holds the copyright: `phonographic copyright` for the
//! ℗ line and `copyright` for the © one, each with the year it began. Some
//! releases instead carry an explicit copyright line in their annotation. The
//! line written to `TCOP` is built from a structured relationship when one is
//! present, and otherwise an explicit annotation line is used.
//!
//! The API needs no account. It does ask for two things in return: a User-Agent
//! that identifies the application and a way to reach whoever runs it, and no
//! more than one request a second. Both are honoured here — set
//! `MUSICBRAINZ_CONTACT` to an email address or URL so a MusicBrainz admin can
//! reach you instead of blocking the whole application.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::metadata::normalize_isrc;
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
    annotation: Option<String>,
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Debug, Deserialize)]
struct Relation {
    #[serde(rename = "type")]
    kind: Option<String>,
    begin: Option<String>,
    label: Option<Label>,
    artist: Option<Artist>,
}

#[derive(Debug, Deserialize)]
struct Label {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Artist {
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
    isrcs: Vec<String>,
    #[serde(default)]
    relations: Vec<WorkRelation>,
}

/// Recording-level metadata that MusicBrainz can add without a Spotify token.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TrackMetadata {
    pub isrc: Option<String>,
    pub language: Option<Language>,
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
    /// One answer per track. The language flag is part of the key so a lookup
    /// that intentionally skipped work relationships cannot hide them later.
    tracks: HashMap<(String, bool), TrackMetadata>,
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
                .attempting_at_most(limits.max_attempts)
                .waiting_out_throttling(limits.max_throttle_retries),
            releases: SEARCH_URL.to_owned(),
            recordings: RECORDING_URL.to_owned(),
            albums: HashMap::new(),
            tracks: HashMap::new(),
        })
    }

    #[cfg(test)]
    fn pointing_at(releases: &str) -> Self {
        Self {
            http: Http::new(LABEL, "test", Duration::from_millis(1)).unwrap(),
            releases: releases.to_owned(),
            recordings: releases.to_owned(),
            albums: HashMap::new(),
            tracks: HashMap::new(),
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
        Ok(self.track_metadata(track, true)?.language)
    }

    /// Look up the ISRC and, when requested, the work language in one pair of
    /// MusicBrainz requests. This is used after token-free downloads, where
    /// spotDL ordinarily has no ISRC to put in `TSRC`.
    pub fn track_metadata(
        &mut self,
        track: &AlbumEvidence,
        include_language: bool,
    ) -> Result<TrackMetadata, LookupError> {
        let Some(title) = track.track_title.as_deref() else {
            return Ok(TrackMetadata::default());
        };
        let key = match &track.isrc {
            Some(isrc) => format!("isrc:{isrc}"),
            None => format!("{} - {title}", track.performer()),
        };
        let cache_key = (key, include_language);
        if let Some(cached) = self.tracks.get(&cache_key) {
            return Ok(cached.clone());
        }

        let metadata = self.look_the_recording_up(track, title, include_language)?;
        self.tracks.insert(cache_key, metadata.clone());
        Ok(metadata)
    }

    fn look_the_recording_up(
        &mut self,
        track: &AlbumEvidence,
        title: &str,
        include_language: bool,
    ) -> Result<TrackMetadata, LookupError> {
        // An ISRC names the recording outright; without one the artist and
        // title have to do, ranked the same way releases are.
        let query = match &track.isrc {
            Some(isrc) => format!("isrc:{}", quote_for_lucene(isrc)),
            // The performer, not the album artist: MusicBrainz credits a
            // recording to whoever played it, so a compilation searched by
            // "Various Artists" matches nothing at all.
            None => format!(
                "artist:{} AND recording:{}",
                quote_for_lucene(track.performer()),
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
            return Ok(TrackMetadata::default());
        };

        let Some(id) = best_recording(&found.recordings, track, title) else {
            return Ok(TrackMetadata::default());
        };
        let url = format!("{recordings}/{id}");
        let includes = if include_language {
            "isrcs+work-rels"
        } else {
            "isrcs"
        };
        let Some(recording): Option<Recording> =
            self.http
                .get_json(&url, &[("inc", includes), ("fmt", "json")], &[])?
        else {
            return Ok(TrackMetadata::default());
        };
        Ok(TrackMetadata {
            isrc: track.isrc.clone().or_else(|| isrc_of_recording(&recording)),
            language: include_language
                .then(|| language_of_works(&recording))
                .flatten(),
        })
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
            let Some(release): Option<Release> = self.http.get_json(
                &url,
                &[
                    ("inc", "label-rels+artist-rels+annotation"),
                    ("fmt", "json"),
                ],
                &[],
            )?
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
/// one; the © line is only used when a release records nothing else. A release
/// annotation is considered only after structured relationships have no answer.
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
                let owner = relation_owner(relation)?;
                let year =
                    year_of(relation.begin.as_deref()).or_else(|| year_of(release.date.as_deref()));
                copyright_line(symbol, year.as_deref(), owner)
            });
        if line.is_some() {
            return line;
        }
    }
    annotation_copyright(release.annotation.as_deref())
}

fn relation_owner(relation: &Relation) -> Option<&str> {
    relation
        .label
        .as_ref()
        .and_then(|label| label.name.as_deref())
        .or_else(|| {
            relation
                .artist
                .as_ref()
                .and_then(|artist| artist.name.as_deref())
        })
}

/// Accept only annotation text that explicitly declares itself to be a
/// copyright line. An annotation is otherwise free-form prose, so guessing an
/// owner from arbitrary text would be much less reliable than returning none.
fn annotation_copyright(annotation: Option<&str>) -> Option<String> {
    annotation?.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with(['\u{2117}', '\u{a9}']) {
            return Some(line.to_owned());
        }

        let lower = line.to_lowercase();
        for (prefix, symbol) in [
            ("phonographic copyright:", '\u{2117}'),
            ("copyright:", '\u{a9}'),
        ] {
            if lower.starts_with(prefix) {
                let rest = line[prefix.len()..].trim();
                if !rest.is_empty() {
                    return Some(format!("{symbol} {rest}"));
                }
            }
        }
        None
    })
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
                .filter_map(|credit| confidence(credit.name.as_deref(), track.performer()))
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

/// Pick the first valid recording identifier MusicBrainz returns.
///
/// A recording can legitimately carry more than one ISRC (for example, codes
/// assigned in different territories). `TSRC` holds one value, and each code
/// identifies the same recording, so the API's first valid value is suitable.
fn isrc_of_recording(recording: &Recording) -> Option<String> {
    recording.isrcs.iter().find_map(|isrc| normalize_isrc(isrc))
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
            track_artist: None,
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
    fn builds_the_line_from_an_artist_relationship_too() {
        let artist_owned = release(
            r#"{
                "date":"2026-01-01",
                "relations":[{
                    "type":"phonographic copyright",
                    "begin":"2026",
                    "artist":{"name":"Independent Artist"}
                }]
            }"#,
        );
        assert_eq!(
            copyright_of(&artist_owned).as_deref(),
            Some("\u{2117} 2026 Independent Artist")
        );
    }

    #[test]
    fn falls_back_to_an_explicit_annotation_line() {
        let annotated = release(
            r#"{
                "date":"2026",
                "annotation":"Imported notes\n℗ 2026 SM Entertainment, under exclusive license to UNIVERSAL MUSIC LLC\nOther prose"
            }"#,
        );
        assert_eq!(
            copyright_of(&annotated).as_deref(),
            Some("\u{2117} 2026 SM Entertainment, under exclusive license to UNIVERSAL MUSIC LLC")
        );

        let labelled = release(r#"{"annotation":"Copyright: 2026 Example Rights Ltd."}"#);
        assert_eq!(
            copyright_of(&labelled).as_deref(),
            Some("\u{a9} 2026 Example Rights Ltd.")
        );
    }

    #[test]
    fn arbitrary_annotation_prose_is_not_guessed_as_copyright() {
        let annotated = release(
            r#"{"annotation":"Released by Example Records. Licensed in several territories."}"#,
        );
        assert_eq!(copyright_of(&annotated), None);
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
                "/bare?inc=label-rels%2Bartist-rels%2Bannotation&fmt=json",
                "/credited?inc=label-rels%2Bartist-rels%2Bannotation&fmt=json",
            ]
        );
    }

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
        assert_eq!(
            asked[1].target,
            "/right?inc=label-rels%2Bartist-rels%2Bannotation&fmt=json"
        );
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
        let asked = server.requests();
        assert_eq!(
            asked[1].target,
            "/original?inc=label-rels%2Bartist-rels%2Bannotation&fmt=json"
        );
        assert_eq!(asked.len(), 2);
    }

    fn track(artist: &str, album: &str, title: &str) -> AlbumEvidence {
        let mut evidence = wanting(artist, album);
        evidence.track_title = Some(title.to_owned());
        evidence
    }

    #[test]
    fn a_recording_is_looked_up_by_its_performer_not_the_album_artist() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"right","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"isrcs":["GBUM71029604"],"relations":[]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let mut evidence = track(
            "Various Artists",
            "Now That's What I Call Music",
            "One More Time",
        );
        evidence.track_artist = Some("Daft Punk".to_owned());

        let found = client.track_metadata(&evidence, false).unwrap();

        assert_eq!(found.isrc.as_deref(), Some("GBUM71029604"));
        let asked = server.requests();
        assert_eq!(
            asked[0].target,
            "/?query=artist%3A%22Daft+Punk%22+AND+recording%3A%22One+More+Time%22&limit=10&fmt=json",
            "the performer is searched for, not Various Artists"
        );
    }

    #[test]
    fn the_album_artist_is_used_when_the_tag_names_no_performer() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"right","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"isrcs":["GBUM71029604"],"relations":[]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let found = client
            .track_metadata(&track("Daft Punk", "Discovery", "One More Time"), false)
            .unwrap();

        assert_eq!(found.isrc.as_deref(), Some("GBUM71029604"));
    }

    #[test]
    fn the_language_comes_from_the_works_of_the_recording() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"right","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"isrcs":["GBUM71029604"],"relations":[
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
                "/right?inc=isrcs%2Bwork-rels&fmt=json",
            ]
        );
        assert_eq!(
            client
                .track_metadata(&track("Daft Punk", "Discovery", "One More Time"), true)
                .unwrap()
                .isrc
                .as_deref(),
            Some("GBUM71029604")
        );
    }

    #[test]
    fn an_isrc_names_the_recording_outright() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"exact","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"isrcs":["GBUM71029604"],"relations":[{"work":{"languages":["eng"]}}]}"#,
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

    #[test]
    fn every_gap_in_the_data_is_simply_no_answer() {
        let server = Server::answering(&[r#"{"recordings":[]}"#]);
        let mut client = Client::pointing_at(&server.address);
        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        server.requests();

        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"isrcs":[],"relations":[]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        assert_eq!(
            client
                .language(&track("Daft Punk", "Discovery", "One More Time"))
                .unwrap(),
            None
        );
        server.requests();

        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"isrcs":[],"relations":[{"work":{}}]}"#,
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

    #[test]
    fn a_track_is_only_looked_up_once() {
        let server = Server::answering(&[
            r#"{"recordings":[{"id":"a","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}]}"#,
            r#"{"isrcs":[],"relations":[{"work":{"languages":["fra"]}}]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);
        let wanted = track("Daft Punk", "Discovery", "One More Time");

        assert_eq!(client.language(&wanted).unwrap().unwrap().name, "French");
        assert_eq!(client.language(&wanted).unwrap().unwrap().name, "French");

        assert_eq!(server.requests().len(), 2);
    }

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

    #[test]
    fn token_free_lookup_returns_a_normalized_isrc_without_work_data() {
        let server = Server::answering(&[
            r#"{"recordings":[
                {"id":"right","title":"One More Time","artist-credit":[{"name":"Daft Punk"}]}
            ]}"#,
            r#"{"isrcs":["gb-um7-10-29604"]}"#,
        ]);
        let mut client = Client::pointing_at(&server.address);

        let metadata = client
            .track_metadata(&track("Daft Punk", "Discovery", "One More Time"), false)
            .unwrap();

        assert_eq!(metadata.isrc.as_deref(), Some("GBUM71029604"));
        assert_eq!(metadata.language, None);
        assert_eq!(server.requests()[1].target, "/right?inc=isrcs&fmt=json");
    }
}
