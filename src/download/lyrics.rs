use id3::frame::{SynchronisedLyrics, SynchronisedLyricsType, TimestampFormat};
use id3::{ErrorKind, Tag, TagLike, Version};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn embed_generated_lrc(
    output_dir: &Path,
    spotify_track_id: &str,
) -> Result<usize, String> {
    let mp3_path = find_downloaded_mp3(output_dir, spotify_track_id)?;
    let lrc_path = mp3_path.with_extension("lrc");
    if !lrc_path.is_file() {
        return Err(format!(
            "spotDL did not generate synchronized lyrics at {}; the MP3 was kept",
            lrc_path.display()
        ));
    }

    let lrc = fs::read_to_string(&lrc_path)
        .map_err(|error| format!("cannot read {}: {error}", lrc_path.display()))?;
    let content = parse_lrc(&lrc)?;
    let line_count = content.len();

    let mut tag = match Tag::read_from_path(&mp3_path) {
        Ok(tag) => tag,
        Err(error) if matches!(error.kind, ErrorKind::NoTag) => Tag::new(),
        Err(error) => {
            return Err(format!(
                "cannot read ID3 metadata from {}: {error}",
                mp3_path.display()
            ));
        }
    };
    tag.remove("SYLT");
    tag.add_frame(SynchronisedLyrics {
        lang: "und".into(),
        timestamp_format: TimestampFormat::Ms,
        content_type: SynchronisedLyricsType::Lyrics,
        description: String::new(),
        content,
    });
    tag.write_to_path(&mp3_path, Version::Id3v23)
        .map_err(|error| format!("cannot write ID3 metadata to {}: {error}", mp3_path.display()))?;

    let verified = Tag::read_from_path(&mp3_path).map_err(|error| {
        format!(
            "cannot verify ID3 metadata in {} after writing: {error}",
            mp3_path.display()
        )
    })?;
    if verified.get("SYLT").is_none() {
        return Err(format!(
            "ID3 verification found no SYLT frame in {}; the LRC file was kept",
            mp3_path.display()
        ));
    }

    fs::remove_file(&lrc_path).map_err(|error| {
        format!(
            "embedded synchronized lyrics, but cannot delete {}: {error}",
            lrc_path.display()
        )
    })?;
    Ok(line_count)
}

fn find_downloaded_mp3(output_dir: &Path, spotify_track_id: &str) -> Result<PathBuf, String> {
    let expected_suffix = format!(" [{spotify_track_id}].mp3").to_ascii_lowercase();
    let mut matches = fs::read_dir(output_dir)
        .map_err(|error| format!("cannot inspect {}: {error}", output_dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.ends_with(&expected_suffix).then(|| entry.path())
        })
        .collect::<Vec<_>>();
    matches.sort();

    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(format!(
            "cannot find the downloaded MP3 ending in [{}.mp3] in {}",
            spotify_track_id,
            output_dir.display()
        )),
        _ => Err(format!(
            "found multiple downloaded MP3 files for Spotify track {} in {}",
            spotify_track_id,
            output_dir.display()
        )),
    }
}

fn parse_lrc(contents: &str) -> Result<Vec<(u32, String)>, String> {
    let offset = contents
        .lines()
        .find_map(parse_offset)
        .transpose()?
        .unwrap_or(0);
    let mut synchronized = Vec::new();

    for (index, raw_line) in contents.lines().enumerate() {
        let mut remainder = raw_line.trim_start_matches('\u{feff}').trim_start();
        let mut timestamps = Vec::new();

        while let Some(after_open) = remainder.strip_prefix('[') {
            let Some(close) = after_open.find(']') else {
                break;
            };
            let marker = &after_open[..close];
            let Some(timestamp) = parse_timestamp(marker)? else {
                break;
            };
            timestamps.push(timestamp);
            remainder = &after_open[close + 1..];
        }

        for timestamp in timestamps {
            let adjusted = i64::from(timestamp) + offset;
            let adjusted = adjusted.clamp(0, i64::from(u32::MAX)) as u32;
            synchronized.push((adjusted, remainder.trim().to_owned()));
        }

        if synchronized.len() > 100_000 {
            return Err(format!(
                "LRC contains too many synchronized entries near line {}",
                index + 1
            ));
        }
    }

    synchronized.sort_by_key(|(timestamp, _)| *timestamp);
    synchronized.dedup();
    if synchronized.is_empty() {
        return Err("the generated LRC contains no synchronized lyric lines; the LRC file was kept"
            .into());
    }
    Ok(synchronized)
}

fn parse_offset(line: &str) -> Option<Result<i64, String>> {
    let line = line.trim().trim_start_matches('\u{feff}');
    let value = line
        .strip_prefix("[offset:")
        .or_else(|| line.strip_prefix("[OFFSET:"))?
        .strip_suffix(']')?;
    Some(
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("invalid LRC offset: {value}")),
    )
}

fn parse_timestamp(marker: &str) -> Result<Option<u32>, String> {
    let Some((minutes, rest)) = marker.split_once(':') else {
        return Ok(None);
    };
    if minutes.is_empty() || !minutes.chars().all(|character| character.is_ascii_digit()) {
        return Ok(None);
    }

    let (seconds, fraction) = rest
        .split_once('.')
        .or_else(|| rest.split_once(':'))
        .map_or((rest, None), |(seconds, fraction)| {
            (seconds, Some(fraction))
        });
    if seconds.len() != 2 || !seconds.chars().all(|character| character.is_ascii_digit()) {
        return Err(format!("invalid LRC timestamp: [{marker}]"));
    }
    let minutes = minutes
        .parse::<u64>()
        .map_err(|_| format!("invalid LRC timestamp: [{marker}]"))?;
    let seconds = seconds
        .parse::<u64>()
        .map_err(|_| format!("invalid LRC timestamp: [{marker}]"))?;
    if seconds >= 60 {
        return Err(format!("invalid LRC timestamp: [{marker}]"));
    }

    let fraction_ms = match fraction {
        None | Some("") => 0,
        Some(value)
            if value.len() <= 3
                && value.chars().all(|character| character.is_ascii_digit()) =>
        {
            let parsed = value
                .parse::<u64>()
                .map_err(|_| format!("invalid LRC timestamp: [{marker}]"))?;
            parsed * 10u64.pow(3 - value.len() as u32)
        }
        Some(_) => return Err(format!("invalid LRC timestamp: [{marker}]")),
    };
    let timestamp = minutes
        .checked_mul(60_000)
        .and_then(|value| value.checked_add(seconds * 1_000))
        .and_then(|value| value.checked_add(fraction_ms))
        .filter(|value| *value <= u64::from(u32::MAX))
        .ok_or_else(|| format!("LRC timestamp is too large: [{marker}]"))?;
    Ok(Some(timestamp as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use id3::frame::Content;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = format!(
                "music-tag-transfer-lyrics-{}-{}-{}",
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

    #[test]
    fn parses_multiple_timestamps_fractions_and_offset() {
        let parsed = parse_lrc(
            "\u{feff}[ar:Artist]\n[offset:-50]\n[00:01.2][00:02.345]First\n[01:03]Second\n",
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                (1_150, "First".into()),
                (2_295, "First".into()),
                (62_950, "Second".into())
            ]
        );
    }

    #[test]
    fn embeds_sylt_and_deletes_lrc_after_verification() {
        let directory = TestDirectory::new();
        let mp3 = directory.0.join("Artist - Song [abc123].mp3");
        let lrc = mp3.with_extension("lrc");
        fs::write(&mp3, b"audio payload").unwrap();
        Tag::new().write_to_path(&mp3, Version::Id3v23).unwrap();
        fs::write(&lrc, "[00:01.00]First\n[00:02.50]Second\n").unwrap();

        assert_eq!(embed_generated_lrc(&directory.0, "abc123").unwrap(), 2);
        assert!(!lrc.exists());

        let tag = Tag::read_from_path(&mp3).unwrap();
        let frame = tag.get("SYLT").unwrap();
        let Content::SynchronisedLyrics(lyrics) = frame.content() else {
            panic!("expected SYLT content");
        };
        assert_eq!(
            lyrics.content,
            vec![(1_000, "First".into()), (2_500, "Second".into())]
        );
    }

    #[test]
    fn keeps_lrc_when_it_has_no_timestamps() {
        let directory = TestDirectory::new();
        let mp3 = directory.0.join("Artist - Song [abc123].mp3");
        let lrc = mp3.with_extension("lrc");
        fs::write(&mp3, b"audio payload").unwrap();
        fs::write(&lrc, "plain lyrics only\n").unwrap();

        assert!(embed_generated_lrc(&directory.0, "abc123").is_err());
        assert!(lrc.exists());
    }
}
