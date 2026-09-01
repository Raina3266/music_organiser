//! Turn bare Spotify links into exact-source pairs before downloading them.
//!
//! The input is a `download` input file and the output is another one, so the
//! two commands compose: `resolve` decides which recording each Spotify track
//! means, and `download` fetches it. Everything `resolve` cannot improve on is
//! copied through untouched, which is what makes the output a drop-in
//! replacement for the input rather than a subset of it.
//!
//! A track Odesli cannot place stays a bare Spotify link on purpose. Dropping
//! it would lose a song that spotDL can very likely still find by searching;
//! leaving it alone costs nothing and keeps the file complete.
//!
//! Pairs are written `SPOTIFY_TRACK_URL|YOUTUBE_MUSIC_URL`, the Spotify link
//! first, so a resolved file lines up with the file of Spotify links it came
//! from. `download` reads a pair in either order, so this costs nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::LookupError;
use crate::download::input::{self, Source, SpotifyKind};
use crate::download::odesli;
use crate::sources::Limits;

/// What one `resolve` run was asked to do.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Whether the output may replace an existing file.
    pub overwrite: bool,
    /// A key given on the command line. Every process on the machine can read
    /// this, so --api-key-file and the environment are preferred.
    pub api_key: Option<String>,
    pub api_key_file: Option<PathBuf>,
    /// The storefront to ask about, as a two-letter country code.
    pub country: String,
    /// How hard to try before giving up on a request and on the API.
    pub limits: Limits,
}

/// The environment variable an API key may arrive in instead of a flag.
pub const KEY_VARIABLE: &str = "ODESLI_API_KEY";

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Report {
    /// Bare Spotify tracks that came back with a YouTube Music link.
    pub resolved: usize,
    /// Bare Spotify tracks Odesli knew nothing about, or had no YouTube Music
    /// link for. They stay bare, and spotDL will search as before.
    pub unresolved: usize,
    /// Lines that were already pairs, and so had nothing left to resolve.
    pub already_paired: usize,
    /// Albums, playlists, and YouTube links: not one track, so not pairable.
    pub not_a_track: usize,
    /// Lookups that failed outright rather than answering "not found".
    pub failed: usize,
    /// Whether Odesli stopped answering and the rest of the file was copied
    /// through without being asked about.
    pub gave_up_early: bool,
}

impl Report {
    /// Whether the run got a real answer for everything it asked about.
    fn complete(&self) -> bool {
        self.failed == 0 && !self.gave_up_early
    }
}

pub fn run(config: Config) -> Result<i32, String> {
    if !config.overwrite && config.output.exists() {
        return Err(format!(
            "{} already exists; pass --overwrite to replace it",
            config.output.display()
        ));
    }

    let list = input::load(&config.input)?;
    let key = api_key(&config)?;
    let mut client = odesli::Client::new(key.as_deref(), &config.country, config.limits)?;

    let tracks = list
        .entries
        .iter()
        .filter(|entry| is_bare_track(&entry.source))
        .count();
    announce(tracks, client.interval(), key.is_some());

    let mut report = Report::default();
    let mut lines = Vec::with_capacity(list.entries.len());

    for entry in &list.entries {
        match &entry.source {
            Source::Pair { .. } => {
                report.already_paired += 1;
                lines.push(input::spotify_first(&entry.query));
            }
            Source::Spotify {
                kind: SpotifyKind::Track,
                ..
            } => {
                // Once the API has stopped answering, the rest of the file is
                // copied through rather than spending a doomed request, and a
                // long wait, on every line that is left.
                if report.gave_up_early {
                    report.unresolved += 1;
                    lines.push(entry.query.clone());
                    continue;
                }
                lines.push(resolve_track(&mut client, entry, &mut report));
            }
            _ => {
                report.not_a_track += 1;
                lines.push(entry.query.clone());
            }
        }
    }

    write_lines(&config.output, &config.input, &lines)?;
    summarize(&report, &config.output, lines.len());

    Ok(if report.complete() { 0 } else { 1 })
}

/// Ask about one track, and turn every way that can go into an output line.
fn resolve_track(client: &mut odesli::Client, entry: &input::Entry, report: &mut Report) -> String {
    match client.youtube_music_url(&entry.query) {
        Ok(Some(youtube)) => match input::pair_line(&entry.query, &youtube) {
            Ok(pair) => {
                report.resolved += 1;
                pair
            }
            // Odesli answered with something this crate would refuse to read
            // back. Saying so is more useful than writing a line that the next
            // command rejects.
            Err(reason) => {
                report.failed += 1;
                eprintln!(
                    "line {}: {LABEL} returned a link that is not a usable YouTube Music URL \
                     ({youtube}): {reason}",
                    entry.line,
                    LABEL = odesli::LABEL,
                );
                entry.query.clone()
            }
        },
        Ok(None) => {
            report.unresolved += 1;
            entry.query.clone()
        }
        Err(LookupError::Album(message)) => {
            report.failed += 1;
            eprintln!("line {}: {message}", entry.line);
            entry.query.clone()
        }
        Err(LookupError::Exhausted(message)) => {
            report.failed += 1;
            report.gave_up_early = true;
            eprintln!("line {}: {message}", entry.line);
            entry.query.clone()
        }
    }
}

/// Whether a line is a lone Spotify track, the one form there is anything to
/// resolve for.
fn is_bare_track(source: &Source) -> bool {
    matches!(
        source,
        Source::Spotify {
            kind: SpotifyKind::Track,
            ..
        }
    )
}

/// Where the API key comes from, in order of how well each place keeps it.
///
/// A file and the environment are both preferred to the command line, which
/// every other process on the machine can read out of the process list.
fn api_key(config: &Config) -> Result<Option<String>, String> {
    if let Some(path) = &config.api_key_file {
        return crate::download::token::read_token_file(path).map(Some);
    }
    if let Some(key) = &config.api_key {
        let key = key.trim();
        if key.is_empty() {
            return Err("the Odesli API key is empty".to_owned());
        }
        return Ok(Some(key.to_owned()));
    }
    match std::env::var(KEY_VARIABLE) {
        Ok(key) if !key.trim().is_empty() => Ok(Some(key.trim().to_owned())),
        _ => Ok(None),
    }
}

/// Say up front how long this will take, because on the free tier it is long
/// enough that a silent run looks like a hung one.
fn announce(tracks: usize, interval: Duration, keyed: bool) {
    if tracks == 0 {
        println!("No bare Spotify track to resolve; every line will be copied through.");
        return;
    }
    let seconds = interval.as_secs_f64() * tracks as f64;
    let rate = if keyed {
        "with an API key"
    } else {
        "on the free tier, at ten requests a minute"
    };
    println!(
        "Resolving {tracks} Spotify track(s) through {} {rate}; about {}.",
        odesli::LABEL,
        crate::sources::http::readable(seconds.ceil() as u64),
    );
}

fn summarize(report: &Report, output: &Path, lines: usize) {
    println!(
        "Wrote {lines} line(s) to {}. Pinned {} track(s) to a YouTube Music link; \
         {} could not be placed and stay bare for spotDL to search.",
        output.display(),
        report.resolved,
        report.unresolved,
    );
    if report.already_paired > 0 || report.not_a_track > 0 {
        println!(
            "  Copied through unchanged: {} pair(s) already resolved, {} album, playlist, \
             or YouTube link(s).",
            report.already_paired, report.not_a_track,
        );
    }
    if report.failed > 0 {
        println!(
            "  {} lookup(s) failed; those lines stay bare and can be resolved by \
             running the command again.",
            report.failed,
        );
    }
    if report.gave_up_early {
        println!(
            "  {} stopped answering partway through, so the rest of the file was copied \
             through without being asked about.",
            odesli::LABEL,
        );
    }
}

/// Write the resolved file, with a header saying where it came from.
fn write_lines(output: &Path, source: &Path, lines: &[String]) -> Result<(), String> {
    let mut contents = format!(
        "# Resolved from {} by {} {}.\n\
         # Each pair is SPOTIFY_TRACK_URL|YOUTUBE_MUSIC_URL; a bare link is one that\n\
         # could not be pinned, and spotDL will search for it as usual.\n",
        source.display(),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    );
    for line in lines {
        contents.push_str(line);
        contents.push('\n');
    }
    fs::write(output, contents)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::testing::{Server, ok, status};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-resolve-{}-{}-{}",
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

    const TRACK: &str = "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW";
    const OTHER: &str = "https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy";
    const VIDEO: &str = "https://music.youtube.com/watch?v=dQw4w9WgXcQ";
    const ALBUM: &str = "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy";

    fn links(youtube_music: &str) -> String {
        format!(r#"{{"linksByPlatform": {{"youtubeMusic": {{"url": "{youtube_music}"}}}}}}"#)
    }

    /// Run `resolve` against a scripted server, and hand back what was
    /// written.
    fn resolve(input_text: &str, responses: &[String]) -> (String, Report, i32) {
        let directory = TestDirectory::new();
        let input_path = directory.0.join("links.txt");
        let output_path = directory.0.join("resolved.txt");
        fs::write(&input_path, input_text).unwrap();

        let server = Server::replying(responses);
        let list = input::load(&input_path).unwrap();
        let mut client = odesli::Client::testing(&server.address);
        let mut report = Report::default();
        let mut lines = Vec::new();

        for entry in &list.entries {
            match &entry.source {
                Source::Pair { .. } => {
                    report.already_paired += 1;
                    lines.push(input::spotify_first(&entry.query));
                }
                Source::Spotify {
                    kind: SpotifyKind::Track,
                    ..
                } if !report.gave_up_early => {
                    lines.push(resolve_track(&mut client, entry, &mut report));
                }
                Source::Spotify {
                    kind: SpotifyKind::Track,
                    ..
                } => {
                    report.unresolved += 1;
                    lines.push(entry.query.clone());
                }
                _ => {
                    report.not_a_track += 1;
                    lines.push(entry.query.clone());
                }
            }
        }

        write_lines(&output_path, &input_path, &lines).unwrap();
        let written = fs::read_to_string(&output_path).unwrap();
        let code = if report.complete() { 0 } else { 1 };
        (written, report, code)
    }

    /// Only the lines that carry a link, so a test can ignore the header.
    fn links_of(written: &str) -> Vec<&str> {
        written
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect()
    }

    #[test]
    fn pins_a_bare_track_to_the_pair_spotdl_reads() {
        let (written, report, code) = resolve(TRACK, &[ok(&links(VIDEO))]);

        assert_eq!(links_of(&written), vec![format!("{TRACK}|{VIDEO}")]);
        assert_eq!(report.resolved, 1);
        assert_eq!(code, 0);
    }

    /// The point of the header is that the file explains itself weeks later.
    #[test]
    fn the_output_says_where_it_came_from_and_is_a_valid_input_file() {
        let (written, _, _) = resolve(TRACK, &[ok(&links(VIDEO))]);

        assert!(written.starts_with("# Resolved from "));
        assert!(written.contains("links.txt"));
        // The whole point: what comes out can go straight back in.
        let reparsed = input::parse_for_test(&written).unwrap();
        assert_eq!(reparsed.entries.len(), 1);
        assert_eq!(
            reparsed.entries[0].source.track_id(),
            Some("02Q0SXOsk74oV4hesiL6JW")
        );
    }

    #[test]
    fn a_track_odesli_does_not_know_stays_bare_rather_than_being_dropped() {
        let (written, report, code) = resolve(TRACK, &[status("404 Not Found", "{}")]);

        assert_eq!(links_of(&written), vec![TRACK]);
        assert_eq!(report.unresolved, 1);
        assert_eq!(report.resolved, 0);
        // Not finding a track is an answer, so the run still succeeded.
        assert_eq!(code, 0);
    }

    /// A pair the input already carried is copied through, but rewritten into
    /// the order the resolved lines use so the file reads as one thing.
    #[test]
    fn copies_pairs_albums_playlists_and_youtube_links_through_untouched() {
        let input_text =
            format!("{VIDEO}|{TRACK}\n{ALBUM}\nhttps://music.youtube.com/watch?v=9bZkp7q19f0\n");

        let (written, report, code) = resolve(&input_text, &[]);

        assert_eq!(
            links_of(&written),
            vec![
                format!("{TRACK}|{VIDEO}"),
                ALBUM.to_owned(),
                "https://music.youtube.com/watch?v=9bZkp7q19f0".to_owned(),
            ]
        );
        assert_eq!(report.already_paired, 1);
        assert_eq!(report.not_a_track, 2);
        assert_eq!(code, 0);
    }

    /// A pair written Spotify-first is still a pair: it must not be sent to
    /// Odesli a second time, and it comes back out as it went in.
    #[test]
    fn a_pair_written_spotify_first_is_not_looked_up_again() {
        let (written, report, _) = resolve(&format!("{TRACK}|{VIDEO}"), &[]);

        assert_eq!(links_of(&written), vec![format!("{TRACK}|{VIDEO}")]);
        assert_eq!(report.already_paired, 1);
        assert_eq!(report.resolved, 0);
    }

    #[test]
    fn a_failed_lookup_leaves_the_line_bare_and_fails_the_run() {
        // A 400 is not throttling and not a 404, so it is a failure.
        let (written, report, code) = resolve(TRACK, &[status("400 Bad Request", "{}")]);

        assert_eq!(links_of(&written), vec![TRACK]);
        assert_eq!(report.failed, 1);
        assert_eq!(code, 1);
    }

    /// Odesli is entitled to answer with a link this crate will not accept.
    /// The line stays bare rather than poisoning the output file.
    #[test]
    fn an_unusable_link_is_refused_rather_than_written_out() {
        let (written, report, code) =
            resolve(TRACK, &[ok(&links("https://example.com/not-youtube"))]);

        assert_eq!(links_of(&written), vec![TRACK]);
        assert_eq!(report.failed, 1);
        assert_eq!(code, 1);
    }

    #[test]
    fn resolves_every_track_in_a_file_and_keeps_their_order() {
        let input_text = format!("{TRACK}\n{ALBUM}\n{OTHER}\n");
        let other_video = "https://music.youtube.com/watch?v=9bZkp7q19f0";

        let (written, report, _) =
            resolve(&input_text, &[ok(&links(VIDEO)), ok(&links(other_video))]);

        assert_eq!(
            links_of(&written),
            vec![
                format!("{TRACK}|{VIDEO}"),
                ALBUM.to_owned(),
                format!("{OTHER}|{other_video}"),
            ]
        );
        assert_eq!(report.resolved, 2);
        assert_eq!(report.not_a_track, 1);
    }

    /// Once the API is spent, the rest of the file is copied through instead
    /// of spending a doomed request, and a wait, on every line left.
    #[test]
    fn stops_asking_once_the_api_gives_up_but_still_writes_every_line() {
        let input_text = format!("{TRACK}\n{OTHER}\n");
        // A rate limit far beyond what the run will wait sets the source aside.
        let refusal = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 86400\r\n\
             Content-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_owned();

        let (written, report, code) = resolve(&input_text, &[refusal]);

        assert_eq!(links_of(&written), vec![TRACK, OTHER]);
        assert!(report.gave_up_early);
        assert_eq!(report.unresolved, 1, "the line after the failure is copied");
        assert_eq!(code, 1);
    }

    #[test]
    fn refuses_to_replace_an_existing_output_without_overwrite() {
        let directory = TestDirectory::new();
        let input_path = directory.0.join("links.txt");
        let output_path = directory.0.join("resolved.txt");
        fs::write(&input_path, TRACK).unwrap();
        fs::write(&output_path, "keep me").unwrap();

        let error = run(Config {
            input: input_path,
            output: output_path.clone(),
            overwrite: false,
            api_key: None,
            api_key_file: None,
            country: odesli::DEFAULT_COUNTRY.to_owned(),
            limits: Limits::default(),
        })
        .unwrap_err();

        assert!(error.contains("already exists"));
        assert_eq!(fs::read_to_string(&output_path).unwrap(), "keep me");
    }

    #[test]
    fn a_key_on_the_command_line_is_used_and_an_empty_one_is_refused() {
        let base = Config {
            input: PathBuf::from("in.txt"),
            output: PathBuf::from("out.txt"),
            overwrite: false,
            api_key: Some("a-key".to_owned()),
            api_key_file: None,
            country: odesli::DEFAULT_COUNTRY.to_owned(),
            limits: Limits::default(),
        };

        assert_eq!(api_key(&base).unwrap().as_deref(), Some("a-key"));
        assert!(
            api_key(&Config {
                api_key: Some("   ".to_owned()),
                ..base.clone()
            })
            .is_err()
        );
    }

    #[test]
    fn a_key_file_is_preferred_to_the_command_line() {
        let directory = TestDirectory::new();
        let path = directory.0.join("key.txt");
        fs::write(&path, "  a-long-enough-odesli-key  \n").unwrap();

        let key = api_key(&Config {
            input: PathBuf::from("in.txt"),
            output: PathBuf::from("out.txt"),
            overwrite: false,
            api_key: Some("from-the-command-line".to_owned()),
            api_key_file: Some(path),
            country: odesli::DEFAULT_COUNTRY.to_owned(),
            limits: Limits::default(),
        })
        .unwrap();

        assert_eq!(key.as_deref(), Some("a-long-enough-odesli-key"));
    }
}
