//! YouTube Music search, through ytmusicapi.
//!
//! YouTube Music has no public search API. ytmusicapi speaks the private one
//! the web player uses, and keeps up with it as it changes, which is why this
//! borrows the Python library rather than reimplementing an undocumented
//! protocol that would break silently — returning no matches rather than an
//! error — the next time YouTube moved it.
//!
//! The library is a library and not a command, so a small script bridges the
//! two: it is started once, and each search is a line of JSON down the pipe
//! and a line of JSON back. One process for the whole run matters because the
//! import and the handshake cost far more than a search does; per row they
//! would be most of the runtime of a long CSV.
//!
//! Deciding which result is the wanted recording happens here rather than in
//! the script, so it uses the same name matching the copyright sources use.
//! The rule is theirs too: a wrong link is worse than none, so a result counts
//! only when both its title and one of its credited artists match. The album
//! never decides — the same recording is on the album, the single, and a dozen
//! compilations, and YouTube Music will happily name a different one of them —
//! but it does separate two results that are otherwise equally good.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use serde::Deserialize;

use crate::LookupError;
use crate::sources::naming::{self, Agreement, Confidence};

pub const LABEL: &str = "YouTube Music";
/// The interpreter that runs the bridge, when none was named.
pub const DEFAULT_PYTHON: &str = "python3";
/// The Python package the bridge needs, named in the message that asks for it.
pub const PACKAGE: &str = "ytmusicapi";
/// The script itself, compiled in so that the binary carries its own bridge
/// and there is no file to install beside it.
const BRIDGE: &str = include_str!("ytmusic_bridge.py");
/// How many results to weigh per search.
///
/// Enough that the right recording is in there when the top hit is a remix or
/// a live cut, and not so many that the tail is all covers and karaoke.
const RESULT_LIMIT: u8 = 10;
/// How much of the bridge's stderr to quote when something goes wrong.
const STDERR_EXCERPT: usize = 400;

/// What a link is being looked for, as the CSV row described it.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Wanted {
    pub song: String,
    pub album: String,
    pub artist: String,
}

impl Wanted {
    /// Whether there is enough here to search on at all.
    ///
    /// A song name and an artist, because those are what a match is checked
    /// against; a row missing either could only ever produce a guess.
    pub fn is_searchable(&self) -> bool {
        !self.song.trim().is_empty() && !self.artist.trim().is_empty()
    }

    /// The search text, which is what a person would type: the song, the
    /// artist, and the album to break a tie between two pressings.
    fn query(&self) -> String {
        [self.song.trim(), self.artist.trim(), self.album.trim()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Something that can be asked for a recording's YouTube Music link.
///
/// A trait rather than the client itself, so that the command driving a CSV
/// can be tested without a Python process, a network, or YouTube Music --
/// exactly as the copyright scan is tested without a catalogue.
pub trait UrlLookup {
    /// The YouTube Music URL for one row, or `None` when nothing in the
    /// results is confidently the same recording.
    fn url_for(&mut self, wanted: &Wanted) -> Result<Option<String>, LookupError>;

    /// Whether the lookup has stopped answering, so the rows after this one
    /// were never asked about rather than answered "not found".
    fn is_spent(&self) -> bool {
        false
    }
}

/// One search result, reduced by the bridge to what a decision needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Found {
    #[serde(rename = "videoId")]
    pub video_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: String,
}

impl Found {
    /// The watch link for this recording.
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.video_id)
    }
}

/// One reply from the bridge.
#[derive(Debug, Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default)]
    error: String,
    /// Set when the bridge cannot serve any request, rather than just this one.
    #[serde(default)]
    fatal: bool,
    #[serde(default)]
    results: Vec<Found>,
}

/// How well one result matches the row that was searched for.
///
/// Ordered best first, so the closest of several matches wins: an exact title
/// beats a title that only matches once an edition suffix is set aside, and
/// between two of those the one on the right album wins.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Score {
    title: Confidence,
    artist: Confidence,
    album: Agreement,
}

/// The conversation with the bridge, apart from the process running it.
///
/// Separated so the protocol -- what a reply means, and which failures end the
/// run rather than the row -- can be tested over a pair of buffers instead of
/// a Python process that a test machine may not have.
struct Session<W, R> {
    to_bridge: W,
    from_bridge: R,
    /// Set once the bridge has failed in a way the next request would hit too.
    /// Every row after that is answered without spending a request on it.
    spent: bool,
    searches: usize,
}

impl<W: Write, R: BufRead> Session<W, R> {
    fn new(to_bridge: W, from_bridge: R) -> Self {
        Self {
            to_bridge,
            from_bridge,
            spent: false,
            searches: 0,
        }
    }

    /// Wait for the bridge to say it has ytmusicapi loaded and is ready.
    fn handshake(&mut self) -> Result<(), String> {
        match self.read_reply() {
            Ok(reply) if reply.ok => Ok(()),
            Ok(reply) => Err(reply.error),
            Err(error) => Err(error),
        }
    }

    fn search(&mut self, query: &str) -> Result<Vec<Found>, LookupError> {
        if self.spent {
            return Err(LookupError::Exhausted(format!(
                "{LABEL} is no longer answering"
            )));
        }
        self.searches += 1;

        let request = serde_json::json!({ "query": query, "limit": RESULT_LIMIT });
        if let Err(error) = self.write_request(&request.to_string()) {
            // The pipe is gone, so every request after this one would fail the
            // same way. Nothing is served by trying them.
            return Err(self.give_up(error));
        }

        match self.read_reply() {
            Ok(reply) if reply.ok => Ok(reply.results),
            // The script reports a failed search in-band and stays up, so one
            // unanswerable row does not take the rest of the file down.
            Ok(reply) if !reply.fatal => Err(LookupError::Album(format!(
                "{LABEL} could not answer: {}",
                reply.error
            ))),
            Ok(reply) => Err(self.give_up(reply.error)),
            Err(error) => Err(self.give_up(error)),
        }
    }

    /// Stop asking, and say why.
    fn give_up(&mut self, reason: String) -> LookupError {
        self.spent = true;
        LookupError::Exhausted(reason)
    }

    fn write_request(&mut self, request: &str) -> Result<(), String> {
        writeln!(self.to_bridge, "{request}")
            .and_then(|()| self.to_bridge.flush())
            .map_err(|error| format!("cannot send a search to the {LABEL} bridge: {error}"))
    }

    fn read_reply(&mut self) -> Result<Reply, String> {
        let mut line = String::new();
        match self.from_bridge.read_line(&mut line) {
            Ok(0) => Err(format!("the {LABEL} bridge stopped before answering")),
            Ok(_) => serde_json::from_str(line.trim()).map_err(|error| {
                format!("the {LABEL} bridge answered with something unreadable: {error}")
            }),
            Err(error) => Err(format!("cannot read from the {LABEL} bridge: {error}")),
        }
    }
}

/// A running bridge process.
pub struct Client {
    child: Child,
    /// The pipes are inside the session; the child is kept out here so that
    /// dropping the session's end of stdin is what stops the script.
    session: Option<Session<ChildStdin, BufReader<ChildStdout>>>,
    stderr: mpsc::Receiver<String>,
}

impl Client {
    /// Start the bridge and wait for it to say it is ready.
    ///
    /// Starting is where a missing interpreter and a missing ytmusicapi both
    /// show up, and either is worth failing the whole run over: nothing would
    /// be found, and saying so once beats saying it per row.
    pub fn start(python: &str) -> Result<Self, String> {
        let mut child = Command::new(python)
            .arg("-c")
            .arg(BRIDGE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot run `{python}`: {error}. Install Python 3, or name another \
                     interpreter with --python <PATH>."
                )
            })?;

        // All three pipes were just asked for, so none can be missing; taking
        // them by hand keeps every path below from having to say so.
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
        // Drained on a thread rather than read on demand: a full stderr pipe
        // would block the script mid-search, and this way whatever it wrote is
        // still there to quote when something goes wrong.
        let stderr = drain(child.stderr.take().expect("stderr was piped"));

        let mut client = Self {
            child,
            session: Some(Session::new(stdin, stdout)),
            stderr,
        };

        match client.session().handshake() {
            Ok(()) => Ok(client),
            Err(error) => Err(client.annotate(error)),
        }
    }

    /// How many searches have actually been sent, which is fewer than the rows
    /// read whenever a row was too empty to search on.
    pub fn searches(&self) -> usize {
        self.session.as_ref().map_or(0, |session| session.searches)
    }

    fn session(&mut self) -> &mut Session<ChildStdin, BufReader<ChildStdout>> {
        self.session
            .as_mut()
            .expect("the session is only taken when the client is dropped")
    }

    /// A failure, with whatever the script printed about it.
    ///
    /// A Python traceback goes to stderr and the reason is its last line, so a
    /// message without it usually says nothing more useful than "it stopped".
    fn annotate(&mut self, reason: String) -> String {
        let noise: String = self.stderr.try_iter().collect();
        let noise = noise.trim();
        if noise.is_empty() {
            return reason;
        }
        format!("{reason}. It printed: {}", tail_of(noise, STDERR_EXCERPT))
    }
}

impl UrlLookup for Client {
    fn url_for(&mut self, wanted: &Wanted) -> Result<Option<String>, LookupError> {
        if !wanted.is_searchable() {
            return Ok(None);
        }
        let query = wanted.query();
        match self.session().search(&query) {
            Ok(found) => Ok(best_of(&found, wanted).map(Found::url)),
            // Only a failure that ends the run is worth reading a traceback
            // for; one bad row explains itself.
            Err(LookupError::Exhausted(reason)) => {
                Err(LookupError::Exhausted(self.annotate(reason)))
            }
            Err(error) => Err(error),
        }
    }

    fn is_spent(&self) -> bool {
        self.session.as_ref().is_some_and(|session| session.spent)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Closing the pipe is what ends the script: its loop is reading stdin,
        // so end-of-input is how it is told there is nothing more to do.
        self.session.take();
        // A script that ignores that is killed rather than left behind, and
        // either way it is waited on so nothing is orphaned.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The last `most` characters of some text, on a character boundary.
fn tail_of(text: &str, most: usize) -> &str {
    text.char_indices()
        .rev()
        .take(most)
        .last()
        .map_or(text, |(index, _)| &text[index..])
}

/// Read a pipe to its end on a thread, handing back what it held.
///
/// Line by line rather than all at once, so that a channel is receiving from
/// the moment the child starts writing.
fn drain(reader: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if sender.send(format!("{line}\n")).is_err() {
                break;
            }
        }
    });
    receiver
}

/// How well one result matches, or `None` when it is a different recording.
fn rank(candidate: &Found, wanted: &Wanted) -> Option<Score> {
    if candidate.video_id.trim().is_empty() {
        return None;
    }
    let title = naming::confidence(Some(&candidate.title), &wanted.song)?;
    // Any one of the credited artists matching is enough, and the closest of
    // them is the one that counts: YouTube Music credits a featured artist
    // that Spotify folded into the title, and either way it is the same
    // recording. It is `min` because Confidence is ordered best first.
    let artist = candidate
        .artists
        .iter()
        .filter_map(|credited| naming::confidence(Some(credited), &wanted.artist))
        .min()?;
    Some(Score {
        title,
        artist,
        album: album_agreement(candidate, wanted),
    })
}

/// Whether the result is on the album the row named.
///
/// Corroboration only, never a requirement: a recording sits on its album, its
/// single, and any number of compilations, and YouTube Music routinely names a
/// different one of them than Spotify does. A row that named no album, or a
/// result carrying none, is unknown rather than wrong.
fn album_agreement(candidate: &Found, wanted: &Wanted) -> Agreement {
    if wanted.album.trim().is_empty() || candidate.album.trim().is_empty() {
        return Agreement::Unknown;
    }
    if naming::matches_name(Some(&candidate.album), &wanted.album) {
        Agreement::Agrees
    } else {
        Agreement::Differs
    }
}

/// The best-ranked result, or `None` when none of them is the recording.
fn best_of<'a>(candidates: &'a [Found], wanted: &Wanted) -> Option<&'a Found> {
    candidates
        .iter()
        .filter_map(|candidate| Some((rank(candidate, wanted)?, candidate)))
        // `min_by_key` keeps the first of equals, which is YouTube Music's own
        // order: among results this cannot tell apart, its ranking is better
        // evidence than the order they happened to be parsed in.
        .min_by_key(|(score, _)| *score)
        .map(|(_, candidate)| candidate)
}
#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    /// A session over two buffers, so the protocol can be exercised without a
    /// Python process: what goes to the bridge is collected, and what comes
    /// back is scripted.
    fn session(replies: &str) -> Session<Vec<u8>, io::Cursor<Vec<u8>>> {
        Session::new(Vec::new(), io::Cursor::new(replies.as_bytes().to_vec()))
    }

    const READY: &str = r#"{"ok": true, "ready": true}"#;

    #[test]
    fn a_ready_bridge_hands_its_results_back() {
        let replies = format!(
            "{READY}\n{}\n",
            r#"{"ok": true, "results": [{"videoId": "id", "title": "Get Lucky",
               "artists": ["Daft Punk"], "album": "Random Access Memories"}]}"#
                .replace('\n', " ")
        );
        let mut session = session(&replies);

        session.handshake().unwrap();
        let found = session.search("Get Lucky Daft Punk").unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].video_id, "id");
        assert_eq!(found[0].artists, ["Daft Punk"]);
        // One line of JSON per request, and the query is in it.
        let sent = String::from_utf8(session.to_bridge.clone()).unwrap();
        assert_eq!(sent.lines().count(), 1);
        assert!(sent.contains("Get Lucky Daft Punk"), "sent: {sent}");
    }

    /// A bridge that never says it is ready has not loaded ytmusicapi, and the
    /// reason it gives is the only thing that explains the run failing.
    #[test]
    fn a_bridge_that_cannot_start_says_why() {
        let mut session =
            session(r#"{"ok": false, "error": "cannot import ytmusicapi", "fatal": true}"#);

        let error = session.handshake().unwrap_err();

        assert!(error.contains("cannot import ytmusicapi"), "{error}");
    }

    /// One search YouTube Music would not answer is this row's problem, not
    /// the file's: the bridge is still up, so the next row is still asked.
    #[test]
    fn a_failed_search_leaves_the_bridge_in_play() {
        let replies = format!(
            "{READY}\n{}\n{}\n",
            r#"{"ok": false, "error": "YouTube said no", "fatal": false}"#,
            r#"{"ok": true, "results": []}"#,
        );
        let mut session = session(&replies);
        session.handshake().unwrap();

        let error = session.search("first").unwrap_err();

        assert!(matches!(error, LookupError::Album(_)), "{error:?}");
        assert!(error.to_string().contains("YouTube said no"), "{error}");
        assert!(!session.spent);
        assert!(
            session.search("second").is_ok(),
            "the next row is still asked"
        );
        assert_eq!(session.searches, 2);
    }

    /// A fatal reply, and a bridge that has stopped talking, both mean every
    /// remaining row would fail the same way. Asking anyway would only be slow.
    #[test]
    fn a_bridge_that_gives_up_is_not_asked_again() {
        for replies in [
            format!(
                "{READY}\n{}\n",
                r#"{"ok": false, "error": "it died", "fatal": true}"#
            ),
            format!("{READY}\n"),
            format!("{READY}\nnot json at all\n"),
        ] {
            let mut session = session(&replies);
            session.handshake().unwrap();

            let error = session.search("first").unwrap_err();

            assert!(matches!(error, LookupError::Exhausted(_)), "{error:?}");
            assert!(session.spent);

            let next = session.search("second").unwrap_err();
            assert!(matches!(next, LookupError::Exhausted(_)), "{next:?}");
            // The second row was never sent: only the first request is on the
            // pipe.
            let sent = String::from_utf8(session.to_bridge.clone()).unwrap();
            assert_eq!(sent.lines().count(), 1, "sent: {sent}");
        }
    }

    /// Only the tail is quoted, because a Python traceback's reason is its
    /// last line and the frames above it say nothing a reader needs.
    #[test]
    fn a_long_traceback_is_quoted_from_its_end() {
        let noise = format!("{}the reason", "frame\n".repeat(200));

        assert_eq!(tail_of(&noise, "the reason".len()), "the reason");
        assert_eq!(tail_of("short", 400), "short");
        // Multi-byte characters are not cut in half.
        assert_eq!(tail_of("héllo", 3), "llo");
    }

    fn found(title: &str, artists: &[&str], album: &str, video: &str) -> Found {
        Found {
            video_id: video.to_owned(),
            title: title.to_owned(),
            artists: artists.iter().map(|name| (*name).to_owned()).collect(),
            album: album.to_owned(),
        }
    }

    fn wanted(song: &str, album: &str, artist: &str) -> Wanted {
        Wanted {
            song: song.to_owned(),
            album: album.to_owned(),
            artist: artist.to_owned(),
        }
    }

    fn chosen(candidates: &[Found], wanted: &Wanted) -> Option<String> {
        best_of(candidates, wanted).map(|found| found.video_id.clone())
    }

    #[test]
    fn a_matching_title_and_artist_is_the_link() {
        let results = [found(
            "Get Lucky",
            &["Daft Punk", "Pharrell Williams"],
            "Random Access Memories",
            "5NV6Rdv1a3I",
        )];

        let url = best_of(
            &results,
            &wanted("Get Lucky", "Random Access Memories", "Daft Punk"),
        )
        .map(Found::url);

        assert_eq!(
            url.as_deref(),
            Some("https://music.youtube.com/watch?v=5NV6Rdv1a3I")
        );
    }

    /// The whole point of ranking rather than taking the first hit: YouTube
    /// Music puts a cover or a remix above the recording often enough that
    /// trusting its order would write the wrong link.
    #[test]
    fn the_right_recording_wins_over_a_better_placed_wrong_one() {
        let results = [
            found("Get Lucky", &["Some Cover Band"], "Tribute", "wrong"),
            found("Get Lucky (Remix)", &["Daft Punk"], "Remixes", "remix"),
            found(
                "Get Lucky",
                &["Daft Punk"],
                "Random Access Memories",
                "right",
            ),
        ];

        let wanted = wanted("Get Lucky", "Random Access Memories", "Daft Punk");

        assert_eq!(chosen(&results, &wanted).as_deref(), Some("right"));
    }

    /// Nothing matching is an empty cell, which is what the caller asked for:
    /// a wrong link is worse than none.
    #[test]
    fn nothing_matching_is_no_link_at_all() {
        let results = [
            found("Something Else", &["Daft Punk"], "Discovery", "a"),
            found("Get Lucky", &["A Different Artist"], "Covers", "b"),
        ];

        assert_eq!(
            chosen(&results, &wanted("Get Lucky", "Discovery", "Daft Punk")),
            None
        );
        assert_eq!(chosen(&[], &wanted("Get Lucky", "", "Daft Punk")), None);
    }

    /// Case, accents, and `&` differ between Spotify and YouTube Music for the
    /// same recording, and the shared normalizing is what makes them agree.
    #[test]
    fn spelling_differences_between_the_two_catalogues_still_match() {
        let results = [found(
            "DÉJÀ VU",
            &["Beyoncé", "Earth, Wind and Fire"],
            "COWBOY CARTER",
            "id",
        )];

        let url = chosen(
            &results,
            &wanted("Deja Vu", "Cowboy Carter", "Earth, Wind & Fire"),
        );

        assert_eq!(url.as_deref(), Some("id"));
    }

    /// Being on the album the row named is corroboration, so it separates two
    /// results that are otherwise equally good.
    #[test]
    fn the_named_album_breaks_a_tie() {
        let results = [
            found(
                "Get Lucky",
                &["Daft Punk"],
                "Now That's What I Call Music",
                "compilation",
            ),
            found(
                "Get Lucky",
                &["Daft Punk"],
                "Random Access Memories",
                "album",
            ),
        ];

        let url = chosen(
            &results,
            &wanted("Get Lucky", "Random Access Memories", "Daft Punk"),
        );

        assert_eq!(url.as_deref(), Some("album"));
    }

    /// But it never decides on its own: a recording sits on its album, its
    /// single, and any number of compilations, and refusing every one whose
    /// album is spelled differently would lose songs that did match.
    #[test]
    fn a_different_album_is_still_a_match_when_it_is_all_there_is() {
        let results = [found(
            "Get Lucky",
            &["Daft Punk"],
            "Get Lucky (Single)",
            "single",
        )];

        let url = chosen(
            &results,
            &wanted("Get Lucky", "Random Access Memories", "Daft Punk"),
        );

        assert_eq!(url.as_deref(), Some("single"));
    }

    /// YouTube Music credits everyone on the recording, and Spotify's "artist
    /// name" column is usually just the first of them.
    #[test]
    fn any_one_of_the_credited_artists_matching_is_enough() {
        let results = [found(
            "Get Lucky",
            &["Pharrell Williams", "Daft Punk", "Nile Rodgers"],
            "Random Access Memories",
            "id",
        )];

        assert_eq!(
            chosen(&results, &wanted("Get Lucky", "", "Nile Rodgers")).as_deref(),
            Some("id")
        );
    }

    /// A result with no video id links nowhere, so it cannot be the answer
    /// however well it reads.
    #[test]
    fn a_result_that_links_nowhere_is_never_chosen() {
        let results = [
            found("Get Lucky", &["Daft Punk"], "Random Access Memories", ""),
            found("Get Lucky", &["Daft Punk"], "Random Access Memories", "  "),
        ];

        assert_eq!(
            chosen(&results, &wanted("Get Lucky", "", "Daft Punk")),
            None
        );
    }

    #[test]
    fn a_row_without_a_song_or_an_artist_is_not_worth_searching_for() {
        assert!(wanted("Get Lucky", "", "Daft Punk").is_searchable());
        assert!(!wanted("", "Random Access Memories", "Daft Punk").is_searchable());
        assert!(!wanted("Get Lucky", "Random Access Memories", "  ").is_searchable());
    }

    /// The query is what a person would type into the search box, and an
    /// absent album must not leave a double space in the middle of it.
    #[test]
    fn the_query_reads_as_a_person_would_type_it() {
        assert_eq!(
            wanted("Get Lucky", "Random Access Memories", "Daft Punk").query(),
            "Get Lucky Daft Punk Random Access Memories"
        );
        assert_eq!(
            wanted("Get Lucky", "  ", "Daft Punk").query(),
            "Get Lucky Daft Punk"
        );
    }

    #[test]
    fn a_video_id_becomes_a_youtube_music_watch_link() {
        assert_eq!(
            found("t", &[], "", "dQw4w9WgXcQ").url(),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }
}
