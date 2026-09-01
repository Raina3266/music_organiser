use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(super) const PAIR_SEPARATOR: char = '|';

/// What one input line asks spotDL to download.
///
/// A line is one of three forms, and each one leaves a different part of the
/// job to spotDL's own search:
///
/// * `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` is spotDL's exact-source syntax:
///   the YouTube Music URL pins the audio and the Spotify track URL supplies
///   the metadata, so neither side is searched. The two URLs may be written in
///   either order; the pair is normalized to the order spotDL expects.
/// * a Spotify URL on its own pins the metadata; spotDL searches YouTube for
///   the audio.
/// * a YouTube Music URL on its own pins the audio; spotDL searches Spotify
///   for the metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    Pair { track_id: String },
    Spotify { kind: SpotifyKind, id: String },
    YouTube,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpotifyKind {
    Track,
    Album,
    Playlist,
}

impl SpotifyKind {
    fn path(self) -> &'static str {
        match self {
            SpotifyKind::Track => "track",
            SpotifyKind::Album => "album",
            SpotifyKind::Playlist => "playlist",
        }
    }
}

impl Source {
    /// The Spotify track ID when the line names exactly one track.
    ///
    /// It ties a downloaded file back to this entry through spotDL's
    /// `[{track-id}]` file name and drives the copyright lookup. A bare
    /// YouTube link, an album, and a playlist have no single track ID, so they
    /// return `None` and are matched by what the download wrote instead.
    pub(super) fn track_id(&self) -> Option<&str> {
        match self {
            Source::Pair { track_id } => Some(track_id),
            Source::Spotify {
                kind: SpotifyKind::Track,
                id,
            } => Some(id),
            _ => None,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Source::Pair { .. } => "exact-source pair(s)",
            Source::Spotify {
                kind: SpotifyKind::Track,
                ..
            } => "Spotify track(s)",
            Source::Spotify {
                kind: SpotifyKind::Album,
                ..
            } => "Spotify album(s)",
            Source::Spotify {
                kind: SpotifyKind::Playlist,
                ..
            } => "Spotify playlist(s)",
            Source::YouTube => "YouTube link(s)",
        }
    }
}

/// One download the input file asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) line: usize,
    pub(super) query: String,
    pub(super) source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputList {
    pub(super) entries: Vec<Entry>,
    pub(super) duplicate_count: usize,
}

impl InputList {
    /// A one-line breakdown of the accepted entries by line form.
    pub(super) fn summary(&self) -> String {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for entry in &self.entries {
            let label = entry.source.describe();
            match counts.iter_mut().find(|(name, _)| *name == label) {
                Some((_, count)) => *count += 1,
                None => counts.push((label, 1)),
            }
        }
        counts
            .into_iter()
            .map(|(label, count)| format!("{count} {label}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Parse an input file's text, for tests that build one rather than read it.
#[cfg(test)]
pub(super) fn parse_for_test(contents: &str) -> Result<InputList, String> {
    parse(contents)
}

pub(super) fn load(path: &Path) -> Result<InputList, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&contents)
}

fn parse(contents: &str) -> Result<InputList, String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicate_count = 0;
    let mut invalid = Vec::new();

    for (index, raw_line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match normalize_line(line) {
            Ok(normalized) => {
                if seen.insert(normalized.query.clone()) {
                    entries.push(Entry {
                        line: line_number,
                        query: normalized.query,
                        source: normalized.source,
                    });
                } else {
                    duplicate_count += 1;
                }
            }
            Err(reason) => invalid.push(format!("line {line_number}: {reason}")),
        }
    }

    if !invalid.is_empty() {
        let shown = invalid
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let remainder = invalid.len().saturating_sub(10);
        let suffix = if remainder == 0 {
            String::new()
        } else {
            format!("\n... and {remainder} more invalid line(s)")
        };
        return Err(format!(
            "invalid input:\n{shown}{suffix}\n\nEach line must be a SPOTIFY_URL, a YOUTUBE_MUSIC_URL, or a YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL (or SPOTIFY_TRACK_URL|YOUTUBE_MUSIC_URL) pair."
        ));
    }
    if entries.is_empty() {
        return Err("the input file contains no Spotify or YouTube Music links".into());
    }

    Ok(InputList {
        entries,
        duplicate_count,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Normalized {
    query: String,
    source: Source,
}

/// Build the pair line the resolver writes: the Spotify track it started from,
/// then the YouTube Music URL that pins that track's audio.
///
/// Spotify first because a resolved file is read next to the file of Spotify
/// links it came from, and the eye wants the two in the same column. `download`
/// accepts a pair in either order and puts it back into spotDL's order before
/// handing it over, so the order here is only about who reads the file.
///
/// The resolver hands over one URL it read from an input file and one it was
/// given by Odesli, so both go through the same validation an input file gets.
/// That way a resolved file cannot contain a line `download` would later
/// reject.
pub(super) fn pair_line(spotify: &str, youtube: &str) -> Result<String, String> {
    if youtube.contains(PAIR_SEPARATOR) || spotify.contains(PAIR_SEPARATOR) {
        return Err(format!("a URL contains the {PAIR_SEPARATOR:?} separator"));
    }
    let youtube = normalize_youtube_url(youtube)?;
    let track_id = spotify_track_id(spotify)?;
    Ok(format!(
        "{}{PAIR_SEPARATOR}{youtube}",
        spotify_track_url(&track_id)
    ))
}

/// Rewrite an already-normalized pair query into the order [`pair_line`]
/// writes, so a pair copied through a resolved file reads like the pairs
/// resolved into it rather than betraying which of the two it was.
pub(super) fn spotify_first(pair_query: &str) -> String {
    match pair_query.split_once(PAIR_SEPARATOR) {
        Some((youtube, spotify)) => format!("{spotify}{PAIR_SEPARATOR}{youtube}"),
        None => pair_query.to_owned(),
    }
}

/// The canonical URL for a Spotify track ID.
fn spotify_track_url(track_id: &str) -> String {
    format!("https://open.spotify.com/track/{track_id}")
}

/// Validate one input line, whichever of the three forms it uses.
fn normalize_line(raw: &str) -> Result<Normalized, String> {
    if raw.contains(PAIR_SEPARATOR) {
        normalize_pair(raw)
    } else {
        normalize_single(raw)
    }
}

/// Validate one pair line, written in either order.
///
/// spotDL only reads `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL`, so a line that
/// names the Spotify track first is accepted and swapped back into that order.
fn normalize_pair(raw: &str) -> Result<Normalized, String> {
    let separators = raw.matches(PAIR_SEPARATOR).count();
    if separators > 1 {
        return Err(format!(
            "found {separators} '|' separators; expected exactly one"
        ));
    }

    let (left, right) = raw
        .split_once(PAIR_SEPARATOR)
        .expect("a single separator was just counted");
    let left = left.trim();
    if left.is_empty() {
        return Err("the URL before '|' is missing".into());
    }
    let right = right.trim();
    if right.is_empty() {
        return Err("the URL after '|' is missing".into());
    }

    let (youtube, spotify) = match (service_of(left)?, service_of(right)?) {
        (Service::YouTube, Service::Spotify) => (left, right),
        (Service::Spotify, Service::YouTube) => (right, left),
        (Service::YouTube, Service::YouTube) => {
            return Err("both sides of the pair are YouTube URLs; a pair needs one YouTube Music URL and one Spotify track URL".into());
        }
        (Service::Spotify, Service::Spotify) => {
            return Err("both sides of the pair are Spotify URLs; a pair needs one YouTube Music URL and one Spotify track URL".into());
        }
    };

    let youtube = normalize_youtube_url(youtube)?;
    let track_id = spotify_track_id(spotify)?;
    Ok(Normalized {
        query: format!("{youtube}{PAIR_SEPARATOR}{}", spotify_track_url(&track_id)),
        source: Source::Pair { track_id },
    })
}

/// Validate a line that names a single Spotify or YouTube Music link.
fn normalize_single(raw: &str) -> Result<Normalized, String> {
    match service_of(raw)? {
        Service::Spotify => {
            let link = spotify_link(raw)?;
            Ok(Normalized {
                query: link.url(),
                source: Source::Spotify {
                    kind: link.kind,
                    id: link.id,
                },
            })
        }
        Service::YouTube => Ok(Normalized {
            query: normalize_youtube_url(raw)?,
            source: Source::YouTube,
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    Spotify,
    YouTube,
}

/// Decide which service a lone URL belongs to before validating it in detail.
fn service_of(raw: &str) -> Result<Service, String> {
    if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("spotify:"))
    {
        return Ok(Service::Spotify);
    }

    let (host, _) = split_url(
        raw,
        "expected a https://open.spotify.com/... or https://music.youtube.com/... URL",
    )?;
    let host = host.to_ascii_lowercase();
    if host == "spotify.com" || host.ends_with(".spotify.com") || host == "spotify.link" {
        return Ok(Service::Spotify);
    }
    if host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com") {
        return Ok(Service::YouTube);
    }
    Err(format!(
        "unsupported host: {host}; a line must name a Spotify or YouTube Music URL"
    ))
}

fn normalize_youtube_url(raw: &str) -> Result<String, String> {
    let (host, remainder) = split_url(raw, "expected a https://music.youtube.com/... URL")?;
    let host = host.to_ascii_lowercase();
    let path = before_query_or_fragment(remainder).trim_matches('/');

    if host == "youtu.be" || host == "www.youtu.be" {
        return Ok(format!(
            "https://www.youtube.com/watch?v={}",
            youtube_id(path, "youtu.be URL is missing its video ID")?
        ));
    }

    let host = match host.as_str() {
        "music.youtube.com" => "music.youtube.com",
        "youtube.com" | "www.youtube.com" | "m.youtube.com" => "www.youtube.com",
        _ => return Err(format!("unsupported YouTube host: {host}")),
    };

    let query = query_of(remainder);
    match path.to_ascii_lowercase().as_str() {
        "watch" => {
            let id = query_parameter(query, "v")
                .ok_or_else(|| "YouTube watch URL is missing its v= video ID".to_owned())?;
            Ok(format!(
                "https://{host}/watch?v={}",
                youtube_id(id, "YouTube video ID is malformed")?
            ))
        }
        "playlist" => {
            let id = query_parameter(query, "list")
                .ok_or_else(|| "YouTube playlist URL is missing its list= ID".to_owned())?;
            Ok(format!(
                "https://{host}/playlist?list={}",
                youtube_id(id, "YouTube playlist ID is malformed")?
            ))
        }
        _ => Err("YouTube URL must be a /watch or /playlist link".into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpotifyLink {
    kind: SpotifyKind,
    id: String,
}

impl SpotifyLink {
    fn url(&self) -> String {
        format!("https://open.spotify.com/{}/{}", self.kind.path(), self.id)
    }
}

/// Extract the content type and ID from a Spotify URL or `spotify:` URI.
fn spotify_link(raw: &str) -> Result<SpotifyLink, String> {
    if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("spotify:"))
    {
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() != 3 {
            return Err("Spotify URI must look like spotify:track:ID".into());
        }
        return Ok(SpotifyLink {
            kind: spotify_kind(parts[1])?,
            id: spotify_id(parts[2])?.to_owned(),
        });
    }

    let (host, remainder) = split_url(raw, "expected a https://open.spotify.com/... URL")?;
    if !host.eq_ignore_ascii_case("open.spotify.com")
        && !host.eq_ignore_ascii_case("www.open.spotify.com")
    {
        return Err(format!(
            "unsupported Spotify host: {host}; an open.spotify.com link is needed because shortened links are not resolved"
        ));
    }

    let path = before_query_or_fragment(remainder).trim_matches('/');
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    while parts.first().is_some_and(|part| {
        let lower = part.to_ascii_lowercase();
        lower == "embed" || lower.starts_with("intl-")
    }) {
        parts.remove(0);
    }

    if parts.len() < 2 {
        return Err("Spotify URL must contain a content type and ID".into());
    }
    Ok(SpotifyLink {
        kind: spotify_kind(parts[0])?,
        id: spotify_id(parts[1])?.to_owned(),
    })
}

fn spotify_kind(value: &str) -> Result<SpotifyKind, String> {
    match value.to_ascii_lowercase().as_str() {
        "track" => Ok(SpotifyKind::Track),
        "album" => Ok(SpotifyKind::Album),
        "playlist" => Ok(SpotifyKind::Playlist),
        other => Err(format!(
            "unsupported Spotify link type {other:?}; use a track, album, or playlist URL"
        )),
    }
}

/// Extract the track ID from the Spotify half of a pair.
fn spotify_track_id(raw: &str) -> Result<String, String> {
    let link = spotify_link(raw)?;
    if link.kind != SpotifyKind::Track {
        return Err(format!(
            "the Spotify half of a pair must be a track URL, not a {} URL",
            link.kind.path()
        ));
    }
    Ok(link.id)
}

fn split_url<'a>(value: &'a str, expectation: &str) -> Result<(&'a str, &'a str), String> {
    if value.chars().any(char::is_whitespace) {
        return Err("URL contains whitespace".into());
    }
    let scheme_end = value.find("://").ok_or_else(|| expectation.to_owned())?;
    let scheme = &value[..scheme_end];
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err("URL must use http or https".into());
    }

    let after_scheme = &value[scheme_end + 3..];
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    Ok((&after_scheme[..host_end], &after_scheme[host_end..]))
}

fn spotify_id(id: &str) -> Result<&str, String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("Spotify ID is malformed".into());
    }
    Ok(id)
}

fn youtube_id<'a>(id: &'a str, reason: &str) -> Result<&'a str, String> {
    let valid = !id.is_empty()
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        });
    if !valid {
        return Err(reason.to_owned());
    }
    Ok(id)
}

fn query_of(remainder: &str) -> &str {
    let without_fragment = remainder.split('#').next().unwrap_or_default();
    without_fragment
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
}

fn query_parameter<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        key.eq_ignore_ascii_case(name).then_some(value)
    })
}

fn before_query_or_fragment(value: &str) -> &str {
    let query = value.find('?').unwrap_or(value.len());
    let fragment = value.find('#').unwrap_or(value.len());
    &value[..query.min(fragment)]
}

#[cfg(test)]
mod tests {
    use super::{Source, SpotifyKind, normalize_line, pair_line, parse, spotify_first};

    const TRACK: &str = "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW";
    const VIDEO: &str = "https://music.youtube.com/watch?v=dQw4w9WgXcQ";
    const PAIR: &str = "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW";

    fn query_of_line(raw: &str) -> String {
        normalize_line(raw).unwrap().query
    }

    #[test]
    fn keeps_the_exact_source_pair_and_strips_tracking_parameters() {
        let pair = normalize_line(
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVM&si=x|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW?utm_source=openai",
        )
        .unwrap();
        assert_eq!(pair.query, PAIR);
        assert_eq!(pair.source.track_id(), Some("02Q0SXOsk74oV4hesiL6JW"));
    }

    #[test]
    fn builds_a_resolved_pair_line_spotify_first() {
        let line = pair_line(
            "https://open.spotify.com/intl-fr/track/02Q0SXOsk74oV4hesiL6JW?si=1",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD",
        )
        .unwrap();

        // Spotify first, both halves normalized, and still a line `download`
        // reads back to the same download as the canonical pair.
        assert_eq!(line, format!("{TRACK}|{VIDEO}"));
        assert_eq!(normalize_line(&line).unwrap().query, PAIR);
    }

    #[test]
    fn a_resolved_pair_line_refuses_what_an_input_file_would_refuse() {
        // Not a YouTube URL, an album rather than a track, and a URL that
        // already carries the separator.
        assert!(pair_line(TRACK, "https://example.com/watch?v=x").is_err());
        assert!(
            pair_line(
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy",
                VIDEO
            )
            .is_err()
        );
        assert!(pair_line(TRACK, &format!("{VIDEO}|extra")).is_err());
    }

    #[test]
    fn copies_a_pair_through_in_the_order_resolved_lines_use() {
        assert_eq!(spotify_first(PAIR), format!("{TRACK}|{VIDEO}"));
        // A line that is not a pair is left exactly as it is.
        assert_eq!(spotify_first(TRACK), TRACK);
    }

    #[test]
    fn accepts_a_pair_written_spotify_first() {
        let pair = normalize_line(
            "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW?si=1|https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        )
        .unwrap();
        assert_eq!(pair.query, PAIR);
        assert_eq!(pair.source.track_id(), Some("02Q0SXOsk74oV4hesiL6JW"));

        assert_eq!(
            query_of_line("spotify:track:02Q0SXOsk74oV4hesiL6JW|https://youtu.be/dQw4w9WgXcQ?t=30"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
    }

    #[test]
    fn a_pair_names_the_same_download_in_either_order() {
        let reversed = "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW|https://music.youtube.com/watch?v=dQw4w9WgXcQ";
        let parsed = parse(&format!("{PAIR}\n{reversed}\n")).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.duplicate_count, 1);
    }

    #[test]
    fn rejects_a_pair_whose_sides_name_the_same_service() {
        assert!(normalize_line(&format!("{TRACK}|spotify:track:4aawyAB9vmqN3uQ7FjRGTy")).is_err());
        assert!(normalize_line(&format!("{VIDEO}|https://youtu.be/dQw4w9WgXcQ")).is_err());
    }

    #[test]
    fn rejects_a_non_track_spotify_side_written_first() {
        assert!(
            normalize_line(&format!(
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy|{VIDEO}"
            ))
            .is_err()
        );
        assert!(normalize_line(&format!("https://spotify.link/AbCdEf|{VIDEO}")).is_err());
        assert!(
            normalize_line(&format!("{TRACK}|https://music.youtube.com/channel/UC123")).is_err()
        );
    }

    #[test]
    fn accepts_the_other_youtube_hosts_and_spotify_uris() {
        assert_eq!(
            query_of_line("https://youtu.be/dQw4w9WgXcQ?t=30|spotify:track:02Q0SXOsk74oV4hesiL6JW"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
        assert_eq!(
            query_of_line(
                "http://m.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/intl-fr/track/02Q0SXOsk74oV4hesiL6JW"
            ),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
    }

    #[test]
    fn accepts_a_spotify_link_on_its_own() {
        let track = normalize_line(&format!("{TRACK}?si=abc")).unwrap();
        assert_eq!(track.query, TRACK);
        assert_eq!(
            track.source,
            Source::Spotify {
                kind: SpotifyKind::Track,
                id: "02Q0SXOsk74oV4hesiL6JW".into()
            }
        );
        assert_eq!(track.source.track_id(), Some("02Q0SXOsk74oV4hesiL6JW"));

        let album =
            normalize_line("https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy").unwrap();
        assert_eq!(
            album.query,
            "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy"
        );
        assert_eq!(album.source.track_id(), None);

        assert_eq!(
            query_of_line("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"),
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
        );
    }

    #[test]
    fn accepts_a_youtube_link_on_its_own() {
        let video =
            normalize_line("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD").unwrap();
        assert_eq!(video.query, VIDEO);
        assert_eq!(video.source, Source::YouTube);
        assert_eq!(video.source.track_id(), None);

        assert_eq!(
            query_of_line("https://youtu.be/dQw4w9WgXcQ?t=30"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn rejects_more_than_one_separator_and_foreign_links() {
        assert!(normalize_line(&format!("{PAIR}|extra")).is_err());
        assert!(normalize_line("https://example.com/track/1").is_err());
        assert!(normalize_line("https://spotify.link/AbCdEf").is_err());
        assert!(normalize_line("not a url").is_err());
        assert!(normalize_line("https://open.spotify.com/artist/0TnOYISbd1XYRBk9myaseg").is_err());
        assert!(normalize_line("https://music.youtube.com/channel/UC123").is_err());
    }

    #[test]
    fn rejects_a_non_track_or_foreign_right_hand_side() {
        assert!(
            normalize_line(
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy"
            )
            .is_err()
        );
        assert!(
            normalize_line("https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://example.com/x")
                .is_err()
        );
        assert!(
            normalize_line(
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://spotify.link/AbCdEf"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_a_missing_or_foreign_left_hand_side() {
        assert!(normalize_line("|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW").is_err());
        assert!(normalize_line("https://music.youtube.com/watch?v=dQw4w9WgXcQ|").is_err());
        assert!(
            normalize_line(
                "https://vimeo.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
            )
            .is_err()
        );
        assert!(
            normalize_line(
                "https://music.youtube.com/channel/UC123|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
            )
            .is_err()
        );
    }

    #[test]
    fn ignores_comments_blanks_and_duplicates() {
        let input = format!("\n# comment\n{PAIR}?si=1\n{PAIR}\n");
        let parsed = parse(&input).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.duplicate_count, 1);
        assert_eq!(parsed.entries[0].line, 3);
        assert_eq!(parsed.entries[0].query, PAIR);
        assert_eq!(
            parsed.entries[0].source.track_id(),
            Some("02Q0SXOsk74oV4hesiL6JW")
        );
    }

    #[test]
    fn reads_a_file_that_mixes_all_three_forms() {
        let input = format!("{TRACK}\n{PAIR}\n{VIDEO}\nspotify:album:4aawyAB9vmqN3uQ7FjRGTy\n");
        let parsed = parse(&input).unwrap();

        let queries: Vec<&str> = parsed
            .entries
            .iter()
            .map(|entry| entry.query.as_str())
            .collect();
        assert_eq!(
            queries,
            vec![
                TRACK,
                PAIR,
                VIDEO,
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy",
            ]
        );
        assert_eq!(
            parsed.summary(),
            "1 Spotify track(s), 1 exact-source pair(s), 1 YouTube link(s), 1 Spotify album(s)"
        );
    }

    #[test]
    fn a_pair_and_its_bare_spotify_track_are_different_downloads() {
        let parsed = parse(&format!("{PAIR}\n{TRACK}\n")).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.duplicate_count, 0);
    }

    #[test]
    fn reports_every_invalid_line_at_once() {
        let error = parse("https://example.com/a\nhttps://example.com/b\n").unwrap_err();
        assert!(error.contains("line 1"));
        assert!(error.contains("line 2"));
        assert!(error.contains("YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL"));
    }
}
