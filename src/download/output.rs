use std::fs;
use std::path::{Path, PathBuf};

pub const FAILED_URLS_FILENAME: &str = "output.txt";

pub fn write_failed_urls<'a, I>(output_dir: &Path, urls: I) -> Result<PathBuf, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let path = output_dir.join(FAILED_URLS_FILENAME);
    let contents = format_failed_urls(urls);
    fs::write(&path, contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

pub fn format_failed_urls<'a, I>(urls: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let mut contents = String::new();
    for url in urls {
        contents.push_str(url);
        contents.push('\n');
    }
    contents
}

#[cfg(test)]
mod tests {
    use super::format_failed_urls;

    #[test]
    fn formats_one_url_per_line_in_order() {
        let urls = [
            "https://open.spotify.com/track/first",
            "https://open.spotify.com/track/second",
        ];
        assert_eq!(
            format_failed_urls(urls),
            "https://open.spotify.com/track/first\nhttps://open.spotify.com/track/second\n"
        );
    }

    #[test]
    fn formats_no_failures_as_an_empty_file() {
        assert_eq!(format_failed_urls(std::iter::empty::<&str>()), "");
    }
}
