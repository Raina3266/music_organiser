use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(super) const PAIR_SEPARATOR: char = '|';

/// One `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` pair from the input file.
///
/// The pair is spotDL's exact-source syntax: the YouTube Music URL pins the
/// audio that is downloaded and the Spotify track URL supplies the metadata,
/// so neither side is left to a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) line: usize,
    pub(super) query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputList {
    pub(super) entries: Vec<Entry>,
    pub(super) duplicate_count: usize,
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

        match normalize_pair(line) {
            Ok(query) => {
                if seen.insert(query.clone()) {
                    entries.push(Entry {
                        line: line_number,
                        query,
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
            "invalid input:\n{shown}{suffix}\n\nEach line must be YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL."
        ));
    }
    if entries.is_empty() {
        return Err("the input file contains no YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL pairs".into());
    }

    Ok(InputList {
        entries,
        duplicate_count,
    })
}

/// Validate one `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` line.
fn normalize_pair(raw: &str) -> Result<String, String> {
    let separators = raw.matches(PAIR_SEPARATOR).count();
    if separators == 0 {
        return Err(
            "missing the '|' separator; expected YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL".into(),
        );
    }
    if separators > 1 {
        return Err(format!(
            "found {separators} '|' separators; expected exactly one"
        ));
    }

    let (youtube, spotify) = raw
        .split_once(PAIR_SEPARATOR)
        .expect("a single separator was just counted");
    let youtube = normalize_youtube_url(youtube.trim())?;
    let spotify = normalize_spotify_track_url(spotify.trim())?;
    Ok(format!("{youtube}{PAIR_SEPARATOR}{spotify}"))
}

fn normalize_youtube_url(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("the YouTube Music URL is missing before '|'".into());
    }
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

fn normalize_spotify_track_url(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("the Spotify track URL is missing after '|'".into());
    }

    if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("spotify:"))
    {
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() != 3 || !parts[1].eq_ignore_ascii_case("track") {
            return Err("Spotify URI must look like spotify:track:ID".into());
        }
        return Ok(format!(
            "https://open.spotify.com/track/{}",
            spotify_id(parts[2])?
        ));
    }

    let (host, remainder) = split_url(raw, "expected a https://open.spotify.com/track/... URL")?;
    if !host.eq_ignore_ascii_case("open.spotify.com")
        && !host.eq_ignore_ascii_case("www.open.spotify.com")
    {
        return Err(format!(
            "unsupported Spotify host: {host}; the pair needs an open.spotify.com track URL"
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
    if !parts[0].eq_ignore_ascii_case("track") {
        return Err(format!(
            "the right-hand side must be a Spotify track URL, not a {} URL",
            parts[0].to_ascii_lowercase()
        ));
    }
    Ok(format!(
        "https://open.spotify.com/track/{}",
        spotify_id(parts[1])?
    ))
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
        return Err("Spotify track ID is malformed".into());
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
    use super::{normalize_pair, parse};

    const PAIR: &str = "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW";

    #[test]
    fn keeps_the_exact_source_pair_and_strips_tracking_parameters() {
        let normalized = normalize_pair(
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVM&si=x|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW?utm_source=openai",
        )
        .unwrap();
        assert_eq!(normalized, PAIR);
    }

    #[test]
    fn accepts_the_other_youtube_hosts_and_spotify_uris() {
        assert_eq!(
            normalize_pair(
                "https://youtu.be/dQw4w9WgXcQ?t=30|spotify:track:02Q0SXOsk74oV4hesiL6JW"
            )
            .unwrap(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
        assert_eq!(
            normalize_pair(
                "http://m.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/intl-fr/track/02Q0SXOsk74oV4hesiL6JW"
            )
            .unwrap(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
    }

    #[test]
    fn requires_exactly_one_separator() {
        assert!(normalize_pair("https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW").is_err());
        assert!(normalize_pair(&format!("{PAIR}|extra")).is_err());
    }

    #[test]
    fn rejects_a_non_track_or_foreign_right_hand_side() {
        assert!(
            normalize_pair(
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy"
            )
            .is_err()
        );
        assert!(
            normalize_pair("https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://example.com/x")
                .is_err()
        );
        assert!(
            normalize_pair(
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://spotify.link/AbCdEf"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_a_missing_or_foreign_left_hand_side() {
        assert!(normalize_pair("|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW").is_err());
        assert!(
            normalize_pair(
                "https://vimeo.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
            )
            .is_err()
        );
        assert!(
            normalize_pair(
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
    }
}
