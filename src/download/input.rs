use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) line: usize,
    pub(super) url: String,
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

        match normalize_spotify_url(line) {
            Ok(url) => {
                if seen.insert(url.clone()) {
                    entries.push(Entry {
                        line: line_number,
                        url,
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
        return Err(format!("invalid input:\n{shown}{suffix}"));
    }
    if entries.is_empty() {
        return Err("the input file contains no Spotify links".into());
    }

    Ok(InputList {
        entries,
        duplicate_count,
    })
}

fn normalize_spotify_url(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.chars().any(char::is_whitespace) {
        return Err("URL contains whitespace".into());
    }

    if value
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("spotify:"))
    {
        return normalize_spotify_uri(value);
    }

    let scheme_end = value
        .find("://")
        .ok_or_else(|| "expected an https://open.spotify.com/... URL".to_owned())?;
    let scheme = &value[..scheme_end];
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err("Spotify URL must use http or https".into());
    }

    let after_scheme = &value[scheme_end + 3..];
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    let remainder = &after_scheme[host_end..];

    if host.eq_ignore_ascii_case("spotify.link") || host.eq_ignore_ascii_case("www.spotify.link") {
        let path = before_query_or_fragment(remainder).trim_matches('/');
        if path.is_empty() {
            return Err("spotify.link URL is missing its link code".into());
        }
        return Ok(format!("https://spotify.link/{path}"));
    }

    if !host.eq_ignore_ascii_case("open.spotify.com")
        && !host.eq_ignore_ascii_case("www.open.spotify.com")
    {
        return Err(format!("unsupported Spotify host: {host}"));
    }

    let path = before_query_or_fragment(remainder).trim_matches('/');
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.first().is_some_and(|part| {
        let lower = part.to_ascii_lowercase();
        lower == "embed" || lower.starts_with("intl-")
    }) {
        parts.remove(0);
    }
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("embed"))
    {
        parts.remove(0);
    }

    if parts.len() < 2 {
        return Err("Spotify URL must contain a content type and ID".into());
    }
    let kind = parts[0].to_ascii_lowercase();
    if !is_supported_kind(&kind) {
        return Err(format!("unsupported Spotify content type: {}", parts[0]));
    }
    let id = parts[1];
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("Spotify content ID is malformed".into());
    }

    Ok(format!("https://open.spotify.com/{kind}/{id}"))
}

fn normalize_spotify_uri(value: &str) -> Result<String, String> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 3 || !parts[0].eq_ignore_ascii_case("spotify") {
        return Err("Spotify URI must look like spotify:track:ID".into());
    }
    let kind = parts[1].to_ascii_lowercase();
    if !is_supported_kind(&kind) {
        return Err(format!("unsupported Spotify content type: {}", parts[1]));
    }
    if parts[2].is_empty()
        || !parts[2]
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("Spotify content ID is malformed".into());
    }
    Ok(format!("spotify:{kind}:{}", parts[2]))
}

fn before_query_or_fragment(value: &str) -> &str {
    let query = value.find('?').unwrap_or(value.len());
    let fragment = value.find('#').unwrap_or(value.len());
    &value[..query.min(fragment)]
}

fn is_supported_kind(kind: &str) -> bool {
    matches!(
        kind,
        "track" | "album" | "playlist" | "artist" | "episode" | "show"
    )
}

#[cfg(test)]
mod tests {
    use super::{normalize_spotify_url, parse};

    #[test]
    fn strips_tracking_parameters() {
        let normalized = normalize_spotify_url(
            "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW?utm_source=openai&go=1",
        )
        .unwrap();
        assert_eq!(
            normalized,
            "https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW"
        );
    }

    #[test]
    fn strips_locale_and_embed_prefixes() {
        assert_eq!(
            normalize_spotify_url("https://open.spotify.com/intl-fr/track/abc123?si=x").unwrap(),
            "https://open.spotify.com/track/abc123"
        );
        assert_eq!(
            normalize_spotify_url("https://open.spotify.com/embed/album/abc123").unwrap(),
            "https://open.spotify.com/album/abc123"
        );
    }

    #[test]
    fn accepts_spotify_uris_and_short_links() {
        assert_eq!(
            normalize_spotify_url("spotify:track:abc123").unwrap(),
            "spotify:track:abc123"
        );
        assert_eq!(
            normalize_spotify_url("https://spotify.link/AbCdEf?si=123").unwrap(),
            "https://spotify.link/AbCdEf"
        );
    }

    #[test]
    fn rejects_non_spotify_hosts() {
        assert!(normalize_spotify_url("https://example.com/track/abc123").is_err());
    }

    #[test]
    fn ignores_comments_blanks_and_duplicates() {
        let input = "\n# comment\nhttps://open.spotify.com/track/abc123?si=1\nhttps://open.spotify.com/track/abc123?si=2\n";
        let parsed = parse(input).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.duplicate_count, 1);
        assert_eq!(parsed.entries[0].line, 3);
    }
}
