use std::fs;
use std::path::{Path, PathBuf};

const RETRY_FILENAME: &str = "output.txt";

/// Write the pairs that still need attention, ready to be used as input again.
pub(super) fn write_retry_queries<'a, I>(output_dir: &Path, queries: I) -> Result<PathBuf, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let path = output_dir.join(RETRY_FILENAME);
    let contents = format_retry_queries(queries);
    fs::write(&path, contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

fn format_retry_queries<'a, I>(queries: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let mut contents = String::new();
    for query in queries {
        contents.push_str(query);
        contents.push('\n');
    }
    contents
}

#[cfg(test)]
mod tests {
    use super::format_retry_queries;

    #[test]
    fn formats_one_pair_per_line_in_order() {
        let queries = [
            "https://music.youtube.com/watch?v=first|https://open.spotify.com/track/first",
            "https://music.youtube.com/watch?v=second|https://open.spotify.com/track/second",
        ];
        assert_eq!(
            format_retry_queries(queries),
            "https://music.youtube.com/watch?v=first|https://open.spotify.com/track/first\nhttps://music.youtube.com/watch?v=second|https://open.spotify.com/track/second\n"
        );
    }

    #[test]
    fn formats_no_failures_as_an_empty_file() {
        assert_eq!(format_retry_queries(std::iter::empty::<&str>()), "");
    }
}
