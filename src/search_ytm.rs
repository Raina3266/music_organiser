//! Give a CSV of Spotify tracks a column of YouTube Music links.
//!
//! The input names a song, its album, its artist, and its Spotify URL — the
//! four columns a Spotify export carries — and the output is the same table
//! with one column added. Every original column and every row is copied
//! through in the order it arrived, so the result is the input file plus an
//! answer rather than a new table to reconcile with it.
//!
//! A row nothing matched gets an empty cell. That is the whole contract: the
//! search is a name search, names are ambiguous, and a link to the wrong
//! recording is worse than no link at all — it would be downloaded as if it
//! were right. So an empty cell means "not found", and a filled one means the
//! title and the artist both matched.
//!
//! The Spotify URL is not looked up. It is carried through as the key that
//! lines each row up with wherever it came from, and it is what makes the
//! output usable as a pairing: song, Spotify link, YouTube Music link.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::LookupError;
use crate::csv::{self, Table, write_record};
use crate::sources::ytmusic::{UrlLookup, Wanted};

/// The header given to the column this command adds.
pub const URL_COLUMN: &str = "youtube_music_url";
/// Joins the three names into the identity of a row that has no Spotify link.
/// A unit separator cannot appear in a title, so two different tracks cannot
/// be run together into one key.
const NAME_SEPARATOR: &str = "\u{1f}";

/// The spellings of each column that are recognised, best first.
///
/// The names the caller was asked for come first, and the ones a Spotify
/// export or a spreadsheet is likely to have written follow, because a file
/// that says `track_name` means the same column and refusing it would only
/// make somebody rename a header by hand.
const SONG_COLUMN: &[&str] = &["song name", "song", "track name", "track", "title", "name"];
const ALBUM_COLUMN: &[&str] = &["album name", "album", "album title"];
const ARTIST_COLUMN: &[&str] = &["artist name", "artist", "artists", "album artist"];
const SPOTIFY_COLUMN: &[&str] = &[
    "spotify_url",
    "spotify url",
    "spotify link",
    "spotify track url",
    "url",
];

/// What one `search-ytm-url` run was asked to do.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Whether the output may replace an existing file.
    pub overwrite: bool,
    /// The interpreter that runs the ytmusicapi bridge.
    pub python: String,
}

/// What one run found.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct SearchReport {
    /// Rows read out of the input, header excluded.
    pub rows: usize,
    /// Rows that came back with a YouTube Music link.
    pub matched: usize,
    /// Rows searched for that nothing matched confidently. Their cell is
    /// empty, which is what the command promises rather than a failure.
    pub unmatched: usize,
    /// Rows with no song name or no artist to search on. Nothing was asked
    /// about these, so they are not evidence that the song is missing.
    pub not_searchable: usize,
    /// Rows that already carried a YouTube Music link, and so were not looked
    /// up again.
    pub already_linked: usize,
    /// Rows naming a track an earlier row had already asked about, answered
    /// from that answer rather than by searching again.
    pub repeated: usize,
    /// Searches that failed outright rather than answering "nothing matched".
    pub failed: usize,
    /// Rows never asked about because the bridge had already stopped
    /// answering. Counted apart from `unmatched`: nobody has said anything
    /// about these, so a rerun may well place them.
    pub not_asked: usize,
    /// Whether the bridge stopped answering partway through.
    pub gave_up_early: bool,
}

impl SearchReport {
    /// Whether the run got a real answer for everything it asked about.
    fn complete(&self) -> bool {
        self.failed == 0 && !self.gave_up_early
    }
}

#[derive(Debug)]
pub struct SearchError {
    message: String,
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SearchError {}

impl From<String> for SearchError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

/// Which column of the input holds what.
///
/// Only the song and the artist have to be there: they are what a match is
/// checked against, and without them every row would be unsearchable, which is
/// worth saying once up front rather than row by row.
#[derive(Debug)]
struct Columns {
    song: Option<usize>,
    album: Option<usize>,
    artist: Option<usize>,
    /// Where a YouTube Music link already lives, when the input carries the
    /// column -- a file this command has already been run over.
    existing_url: Option<usize>,
    /// The Spotify link, which is never looked up: it identifies the track, so
    /// two rows carrying it are the same recording asked for twice.
    spotify_url: Option<usize>,
}

impl Columns {
    fn of(table: &Table) -> Result<Self, SearchError> {
        let song = table.column(SONG_COLUMN);
        let artist = table.column(ARTIST_COLUMN);
        if song.is_none() || artist.is_none() {
            let missing = [(song, "song name"), (artist, "artist name")]
                .iter()
                .filter(|(found, _)| found.is_none())
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(" or ");
            return Err(SearchError {
                message: format!(
                    "the input has no {missing} column; its headers are: {}",
                    if table.headers.is_empty() {
                        "none at all".to_owned()
                    } else {
                        table.headers.join(", ")
                    }
                ),
            });
        }
        Ok(Self {
            song,
            album: table.column(ALBUM_COLUMN),
            artist,
            existing_url: table.column(&[URL_COLUMN, "youtube music url", "youtube url"]),
            spotify_url: table.column(SPOTIFY_COLUMN),
        })
    }

    /// What one row is asking to have looked up.
    fn wanted(&self, row: &[String]) -> Wanted {
        Wanted {
            song: csv::cell(row, self.song).to_owned(),
            album: csv::cell(row, self.album).to_owned(),
            artist: csv::cell(row, self.artist).to_owned(),
        }
    }

    /// What makes two rows the same track.
    ///
    /// The Spotify link when there is one, because it names the recording
    /// exactly and a playlist export lists a track once per playlist it is on.
    /// Otherwise the three names, which is the same question the search would
    /// have been asked, so asking it twice could only get the same answer.
    fn identity(&self, row: &[String], wanted: &Wanted) -> String {
        let spotify = csv::cell(row, self.spotify_url);
        if spotify.is_empty() {
            [
                wanted.song.as_str(),
                wanted.album.as_str(),
                wanted.artist.as_str(),
            ]
            .join(NAME_SEPARATOR)
        } else {
            spotify.to_owned()
        }
    }
}

pub fn run(config: Config) -> Result<i32, String> {
    if !config.overwrite && config.output.exists() {
        return Err(format!(
            "{} already exists; pass --overwrite to replace it",
            config.output.display()
        ));
    }

    let table = read(&config.input)?;
    let columns = Columns::of(&table).map_err(|error| error.to_string())?;
    announce(&table, &columns);

    let mut client = crate::sources::ytmusic::Client::start(&config.python).map_err(|error| {
        format!(
            "{error}\n\nThis command searches YouTube Music through the {} Python \
             package. Install it with `pip install {}`.",
            crate::sources::ytmusic::PACKAGE,
            crate::sources::ytmusic::PACKAGE,
        )
    })?;

    let (rows, report) = look_up(&table, &columns, &mut client);
    write(&config.output, &table, &columns, &rows)?;
    summarize(&report, &config.output);

    Ok(if report.complete() { 0 } else { 1 })
}

/// Look every row up, handing back the URL cell each one earned.
///
/// Separated from the file handling so it can be driven by a fake lookup in
/// the tests, which is the only way to check the counting without YouTube
/// Music on the other end.
fn look_up(
    table: &Table,
    columns: &Columns,
    client: &mut impl UrlLookup,
) -> (Vec<String>, SearchReport) {
    let mut report = SearchReport {
        rows: table.rows.len(),
        ..SearchReport::default()
    };
    let mut urls = Vec::with_capacity(table.rows.len());
    let mut answered: HashMap<String, String> = HashMap::new();

    for (index, row) in table.rows.iter().enumerate() {
        // A file this command has already been run over keeps the links it
        // found, so a rerun only spends requests on the rows that came back
        // empty. That is what makes a second pass over a long file cheap.
        let existing = csv::cell(row, columns.existing_url);
        if !existing.is_empty() {
            report.already_linked += 1;
            urls.push(existing.to_owned());
            continue;
        }

        let wanted = columns.wanted(row);
        if !wanted.is_searchable() {
            report.not_searchable += 1;
            urls.push(String::new());
            continue;
        }
        // A track this run has already asked about is answered from that
        // answer. A playlist export lists a song once per playlist it is on,
        // and searching for it again could only get the same result slower.
        let identity = columns.identity(row, &wanted);
        if let Some(url) = answered.get(&identity) {
            report.repeated += 1;
            if !url.is_empty() {
                report.matched += 1;
            } else {
                report.unmatched += 1;
            }
            urls.push(url.clone());
            continue;
        }
        // Once the bridge has stopped answering, the rest of the file is
        // written out rather than spending a doomed request on every line.
        if report.gave_up_early {
            report.not_asked += 1;
            urls.push(String::new());
            continue;
        }

        let answer = client.url_for(&wanted);
        let failed = answer.is_err();
        let url = match answer {
            Ok(Some(url)) => {
                report.matched += 1;
                url
            }
            Ok(None) => {
                report.unmatched += 1;
                String::new()
            }
            Err(LookupError::Album(message)) => {
                report.failed += 1;
                eprintln!("row {}: {message}", index + 1);
                String::new()
            }
            Err(LookupError::Exhausted(message)) => {
                report.failed += 1;
                report.gave_up_early = true;
                eprintln!("row {}: {message}", index + 1);
                String::new()
            }
        };
        // A failed search is deliberately not remembered: it says nothing
        // about the track, so a later row naming it gets its own attempt
        // rather than inheriting an empty cell nobody established. A search
        // that simply matched nothing is remembered -- asking again could
        // only get the same answer.
        if !failed {
            answered.insert(identity, url.clone());
        }
        urls.push(url);
    }

    (urls, report)
}

fn read(input: &Path) -> Result<Table, String> {
    let contents = fs::read_to_string(input)
        .map_err(|error| format!("cannot read {}: {error}", input.display()))?;
    // A file saved by a spreadsheet opens with a byte order mark, which would
    // otherwise become part of the first header and hide that column.
    let table = csv::parse(contents.trim_start_matches('\u{feff}'));
    if table.headers.is_empty() {
        return Err(format!("{} is empty", input.display()));
    }
    Ok(table)
}

/// Write the input back out with the URL column filled in.
///
/// When the input already had the column it is replaced in place, so that
/// running the command twice leaves one column rather than two.
fn write(output: &Path, table: &Table, columns: &Columns, urls: &[String]) -> Result<(), String> {
    let cannot = |error: std::io::Error| format!("cannot write {}: {error}", output.display());
    let file = fs::File::create(output).map_err(cannot)?;
    let mut writer = BufWriter::new(file);

    let mut headers: Vec<&str> = table.headers.iter().map(String::as_str).collect();
    match columns.existing_url {
        Some(index) => headers[index] = URL_COLUMN,
        None => headers.push(URL_COLUMN),
    }
    write_record(&mut writer, headers.iter().copied()).map_err(cannot)?;

    for (row, url) in table.rows.iter().zip(urls) {
        let mut cells: Vec<&str> = row.iter().map(String::as_str).collect();
        match columns.existing_url {
            Some(index) => cells[index] = url,
            None => cells.push(url),
        }
        write_record(&mut writer, cells.iter().copied()).map_err(cannot)?;
    }

    writer
        .flush()
        .map_err(|error| format!("cannot finish writing {}: {error}", output.display()))
}

/// Say what was found in the file and what is about to happen, because a long
/// CSV takes a while and a silent run looks hung.
fn announce(table: &Table, columns: &Columns) {
    let named = |what: &str, column: Option<usize>| {
        column.map_or_else(
            || format!("no {what} column"),
            |index| format!("{what} from {:?}", table.headers[index]),
        )
    };
    println!(
        "Read {} row(s): {}, {}, {}.",
        table.rows.len(),
        named("song", columns.song),
        named("artist", columns.artist),
        named("album", columns.album),
    );
    if columns.album.is_none() {
        println!(
            "  Without an album the search still runs; the album only ever \
             separates two results that match equally well."
        );
    }
    if columns.existing_url.is_some() {
        println!(
            "  The input already has a {URL_COLUMN} column, so only the rows \
             still empty will be searched for."
        );
    }
}

fn summarize(report: &SearchReport, output: &Path) {
    // The headline counts the cells that ended up filled, not the searches
    // that filled them: on a rerun most of them were already there, and a
    // headline of "found 0" would read as a failed run rather than a finished
    // one. Where they came from is what the lines under it are for.
    let linked = report.matched + report.already_linked;
    println!(
        "Wrote {} row(s) to {}. {linked} row(s) carry a YouTube Music link and \
         {} are empty.",
        report.rows,
        output.display(),
        report.rows.saturating_sub(linked),
    );
    if report.matched > 0 || report.unmatched > 0 {
        println!(
            "  Searched YouTube Music for {} row(s): {} matched, {} matched nothing.",
            report.matched + report.unmatched,
            report.matched,
            report.unmatched,
        );
    }
    if report.already_linked > 0 {
        println!(
            "  {} row(s) already carried a link and were kept as they were.",
            report.already_linked,
        );
    }
    if report.repeated > 0 {
        println!(
            "  {} row(s) named a track an earlier row had already asked about, and \
             were answered without searching again.",
            report.repeated,
        );
    }
    if report.not_searchable > 0 {
        println!(
            "  {} row(s) had no song name or no artist to search on, so nothing \
             was asked about them.",
            report.not_searchable,
        );
    }
    if report.failed > 0 {
        println!(
            "  {} search(es) failed; those rows are empty and can be filled in by \
             running the command again over its own output.",
            report.failed,
        );
    }
    if report.gave_up_early {
        println!(
            "  The bridge stopped answering, so {} further row(s) were never asked \
             about. Nothing is known about those yet, so a rerun may still place them.",
            report.not_asked,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-search-ytm-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed),
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A lookup that answers from a table instead of from YouTube Music, and
    /// remembers what it was asked so a test can prove a row was never sent.
    #[derive(Default)]
    struct FakeLookup {
        answers: HashMap<String, Result<Option<String>, LookupError>>,
        asked: Vec<String>,
        spent: bool,
    }

    impl FakeLookup {
        fn answering(pairs: &[(&str, &str)]) -> Self {
            Self {
                answers: pairs
                    .iter()
                    .map(|(song, url)| ((*song).to_owned(), Ok(Some((*url).to_owned()))))
                    .collect(),
                ..Self::default()
            }
        }

        fn failing(song: &str, error: LookupError) -> Self {
            Self {
                answers: HashMap::from([(song.to_owned(), Err(error))]),
                ..Self::default()
            }
        }
    }

    impl UrlLookup for FakeLookup {
        fn url_for(&mut self, wanted: &Wanted) -> Result<Option<String>, LookupError> {
            self.asked.push(wanted.song.clone());
            match self.answers.get(&wanted.song) {
                Some(Ok(url)) => Ok(url.clone()),
                Some(Err(LookupError::Album(message))) => Err(LookupError::Album(message.clone())),
                Some(Err(LookupError::Exhausted(message))) => {
                    self.spent = true;
                    Err(LookupError::Exhausted(message.clone()))
                }
                None => Ok(None),
            }
        }

        fn is_spent(&self) -> bool {
            self.spent
        }
    }

    const HEADER: &str = "song name,album name,artist name,spotify_url";
    const LUCKY: &str = "https://music.youtube.com/watch?v=5NV6Rdv1a3I";

    /// Drive a whole table through a fake lookup, handing back the CSV that
    /// would be written and what the run counted.
    fn search(contents: &str, lookup: &mut FakeLookup) -> (String, SearchReport) {
        let directory = TestDirectory::new();
        let output = directory.0.join("out.csv");
        let table = csv::parse(contents);
        let columns = Columns::of(&table).unwrap();

        let (urls, report) = look_up(&table, &columns, lookup);
        write(&output, &table, &columns, &urls).unwrap();

        (fs::read_to_string(&output).unwrap(), report)
    }

    /// The whole point of the command: a matched row gets its link, and every
    /// column that came in comes back out beside it.
    fn cells(written: &str) -> Vec<Vec<String>> {
        let table = csv::parse(written);
        std::iter::once(table.headers.clone())
            .chain(table.rows.clone())
            .collect()
    }

    #[test]
    fn adds_a_url_column_and_keeps_every_column_it_was_given() {
        let input =
            format!("{HEADER}\r\nGet Lucky,RAM,Daft Punk,https://open.spotify.com/track/a\r\n");
        let mut lookup = FakeLookup::answering(&[("Get Lucky", LUCKY)]);

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(
            cells(&written),
            vec![
                vec![
                    "song name".to_owned(),
                    "album name".to_owned(),
                    "artist name".to_owned(),
                    "spotify_url".to_owned(),
                    URL_COLUMN.to_owned(),
                ],
                vec![
                    "Get Lucky".to_owned(),
                    "RAM".to_owned(),
                    "Daft Punk".to_owned(),
                    "https://open.spotify.com/track/a".to_owned(),
                    LUCKY.to_owned(),
                ],
            ]
        );
        assert_eq!(report.matched, 1);
        assert_eq!(report.rows, 1);
    }

    /// The contract the caller asked for: nothing matched leaves the cell
    /// empty rather than guessing at a link.
    #[test]
    fn a_row_nothing_matched_is_left_empty() {
        let input =
            format!("{HEADER}\r\nUnknown Song,Album,Nobody,https://open.spotify.com/track/a\r\n");
        let mut lookup = FakeLookup::default();

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(cells(&written)[1][4], "");
        assert_eq!(report.unmatched, 1);
        assert_eq!(report.matched, 0);
    }

    /// A row with no song or no artist could only ever be guessed at, so it is
    /// left empty without spending a search on it.
    #[test]
    fn a_row_with_nothing_to_search_on_is_never_asked_about() {
        let input = format!(
            "{HEADER}\r\n,Album,Daft Punk,https://open.spotify.com/track/a\r\nGet Lucky,Album,,https://open.spotify.com/track/b\r\n"
        );
        let mut lookup = FakeLookup::answering(&[("Get Lucky", LUCKY)]);

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(report.not_searchable, 2);
        assert!(lookup.asked.is_empty(), "asked: {:?}", lookup.asked);
        assert_eq!(cells(&written)[1][4], "");
        assert_eq!(cells(&written)[2][4], "");
    }

    /// A playlist export lists a song once per playlist it is on. Asking again
    /// could only get the same answer, more slowly.
    #[test]
    fn the_same_track_twice_is_only_searched_for_once() {
        let row = "Get Lucky,RAM,Daft Punk,https://open.spotify.com/track/a";
        let input = format!("{HEADER}\r\n{row}\r\n{row}\r\n");
        let mut lookup = FakeLookup::answering(&[("Get Lucky", LUCKY)]);

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(lookup.asked, ["Get Lucky"], "asked twice for one track");
        assert_eq!(report.repeated, 1);
        // Both rows still carry the link: the second is answered, not skipped.
        assert_eq!(cells(&written)[1][4], LUCKY);
        assert_eq!(cells(&written)[2][4], LUCKY);
        assert_eq!(report.matched, 2);
    }

    /// Two rows the same in every name are the same track even when the file
    /// carries no Spotify link to say so.
    #[test]
    fn a_repeat_is_recognised_without_a_spotify_url() {
        let input = "song name,album name,artist name\r\nGet Lucky,RAM,Daft Punk\r\nGet Lucky,RAM,Daft Punk\r\n";
        let mut lookup = FakeLookup::answering(&[("Get Lucky", LUCKY)]);

        let (_, report) = search(input, &mut lookup);

        assert_eq!(lookup.asked, ["Get Lucky"]);
        assert_eq!(report.repeated, 1);
    }

    /// Running the command over its own output is how a run that half failed
    /// is finished, so the links already found must not be searched for again.
    #[test]
    fn a_rerun_keeps_the_links_it_has_and_only_fills_the_empty_rows() {
        let input = format!(
            "{HEADER},{URL_COLUMN}\r\n\
             Get Lucky,RAM,Daft Punk,https://open.spotify.com/track/a,{LUCKY}\r\n\
             Instant Crush,RAM,Daft Punk,https://open.spotify.com/track/b,\r\n"
        );
        let mut lookup =
            FakeLookup::answering(&[("Instant Crush", "https://music.youtube.com/watch?v=second")]);

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(
            lookup.asked,
            ["Instant Crush"],
            "the filled row was asked again"
        );
        assert_eq!(report.already_linked, 1);
        assert_eq!(report.matched, 1);
        let rows = cells(&written);
        // One URL column, not two: the existing one is filled in place.
        assert_eq!(rows[0].len(), 5);
        assert_eq!(rows[0][4], URL_COLUMN);
        assert_eq!(rows[1][4], LUCKY);
        assert_eq!(rows[2][4], "https://music.youtube.com/watch?v=second");
    }

    /// One row that could not be looked up is that row's problem: the rest of
    /// the file is still searched for, and the run reports the failure.
    #[test]
    fn a_failed_row_stays_empty_and_the_file_carries_on() {
        let input = format!(
            "{HEADER}\r\n\
             Broken,RAM,Daft Punk,https://open.spotify.com/track/a\r\n\
             Get Lucky,RAM,Daft Punk,https://open.spotify.com/track/b\r\n"
        );
        let mut lookup = FakeLookup::failing("Broken", LookupError::Album("it broke".to_owned()));
        lookup
            .answers
            .insert("Get Lucky".to_owned(), Ok(Some(LUCKY.to_owned())));

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(report.failed, 1);
        assert_eq!(report.matched, 1);
        assert!(!report.complete(), "a failed row fails the run");
        assert_eq!(cells(&written)[1][4], "");
        assert_eq!(cells(&written)[2][4], LUCKY);
    }

    /// A failed search says nothing about the track, so a later row naming the
    /// same one is searched for again rather than inheriting an empty cell
    /// nobody ever established.
    #[test]
    fn a_failed_row_is_not_remembered_as_an_answer_for_its_repeats() {
        let row = "Broken,RAM,Daft Punk,https://open.spotify.com/track/a";
        let input = format!("{HEADER}\r\n{row}\r\n{row}\r\n");
        let mut lookup = FakeLookup::failing("Broken", LookupError::Album("it broke".to_owned()));

        let (_, report) = search(&input, &mut lookup);

        assert_eq!(
            lookup.asked,
            ["Broken", "Broken"],
            "the repeat was not retried"
        );
        assert_eq!(report.failed, 2);
        assert_eq!(report.repeated, 0);
        assert_eq!(
            report.unmatched, 0,
            "a failure is not a matched-nothing row"
        );
    }

    /// A row that was asked about and matched nothing is remembered, though:
    /// asking again could only get the same answer.
    #[test]
    fn a_matched_nothing_row_is_remembered_for_its_repeats() {
        let row = "Unknown,RAM,Nobody,https://open.spotify.com/track/a";
        let input = format!("{HEADER}\r\n{row}\r\n{row}\r\n");
        let mut lookup = FakeLookup::default();

        let (_, report) = search(&input, &mut lookup);

        assert_eq!(lookup.asked, ["Unknown"]);
        assert_eq!(report.repeated, 1);
        assert_eq!(report.unmatched, 2);
    }

    /// Once the bridge is gone every remaining row would fail the same way, so
    /// they are written out unasked -- and counted apart from the rows that
    /// were asked about and matched nothing.
    #[test]
    fn a_bridge_that_stops_leaves_the_rest_unasked_rather_than_unmatched() {
        let input = format!(
            "{HEADER}\r\n\
             Broken,RAM,Daft Punk,https://open.spotify.com/track/a\r\n\
             Get Lucky,RAM,Daft Punk,https://open.spotify.com/track/b\r\n\
             Instant Crush,RAM,Daft Punk,https://open.spotify.com/track/c\r\n"
        );
        let mut lookup =
            FakeLookup::failing("Broken", LookupError::Exhausted("it died".to_owned()));

        let (written, report) = search(&input, &mut lookup);

        assert_eq!(lookup.asked, ["Broken"]);
        assert!(report.gave_up_early);
        assert_eq!(report.not_asked, 2);
        assert_eq!(report.unmatched, 0, "nothing was said about those rows");
        // Every row is still written, so the output is the whole table.
        assert_eq!(cells(&written).len(), 4);
    }

    /// A file whose headers this cannot read is worth stopping over: every row
    /// would be unsearchable, and saying so once is more use than a file of
    /// empty cells.
    #[test]
    fn a_file_without_a_song_or_artist_column_is_refused_with_its_headers() {
        let table =
            csv::parse("album name,spotify_url\r\nRAM,https://open.spotify.com/track/a\r\n");

        let error = Columns::of(&table).unwrap_err().to_string();

        assert!(error.contains("song name or artist name"), "{error}");
        assert!(error.contains("album name, spotify_url"), "{error}");
    }

    /// However a tool spelled the four headers, they mean the same columns.
    #[test]
    fn the_columns_are_found_under_the_names_other_tools_write() {
        let table = csv::parse("Track Name,Album,Artist,Spotify URL\r\n");
        let columns = Columns::of(&table).unwrap();

        assert_eq!(columns.song, Some(0));
        assert_eq!(columns.album, Some(1));
        assert_eq!(columns.artist, Some(2));
        assert_eq!(columns.spotify_url, Some(3));
    }

    #[test]
    fn refuses_to_replace_an_existing_output_without_overwrite() {
        let directory = TestDirectory::new();
        let input = directory.0.join("tracks.csv");
        let output = directory.0.join("out.csv");
        fs::write(&input, format!("{HEADER}\r\n")).unwrap();
        fs::write(&output, "keep me").unwrap();

        let error = run(Config {
            input,
            output: output.clone(),
            overwrite: false,
            python: "python3".to_owned(),
        })
        .unwrap_err();

        assert!(error.contains("already exists"), "{error}");
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");
    }

    /// A spreadsheet opens the file with a byte order mark, which would
    /// otherwise become part of the first header and hide that column.
    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_column() {
        let directory = TestDirectory::new();
        let input = directory.0.join("tracks.csv");
        fs::write(
            &input,
            format!("\u{feff}{HEADER}\r\nGet Lucky,RAM,Daft Punk,x\r\n"),
        )
        .unwrap();

        let table = read(&input).unwrap();

        assert_eq!(table.headers[0], "song name");
        assert_eq!(Columns::of(&table).unwrap().song, Some(0));
    }

    #[test]
    fn an_empty_file_is_refused_rather_than_answered() {
        let directory = TestDirectory::new();
        let input = directory.0.join("empty.csv");
        fs::write(&input, "").unwrap();

        assert!(read(&input).unwrap_err().contains("is empty"));
    }
}
