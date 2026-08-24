//! Writing RFC 4180 CSV, shared by the commands that produce one.
//!
//! Both the frame export and the copyright report write a table of text that
//! has to survive a spreadsheet: values carrying commas, quotes, and — in the
//! case of lyrics — whole paragraphs of newlines.

use std::{
    io::{self, Write},
    path::Path,
};

/// The record separator RFC 4180 asks for. Newlines inside a value -- lyrics,
/// mostly -- stay as line feeds inside the quoted cell.
pub const RECORD_SEPARATOR: &str = "\r\n";

/// Write one row, escaping each cell.
pub fn write_record<'a>(
    writer: &mut impl Write,
    cells: impl Iterator<Item = &'a str>,
) -> io::Result<()> {
    for (index, cell) in cells.enumerate() {
        if index > 0 {
            writer.write_all(b",")?;
        }
        writer.write_all(escape(cell).as_bytes())?;
    }
    writer.write_all(RECORD_SEPARATOR.as_bytes())
}

/// One cell, quoted if it holds anything that would otherwise break the row.
///
/// A carriage return is folded to a line feed first, so that the only `\r` in
/// the file is the one ending each record.
pub fn escape(value: &str) -> String {
    let value = if value.contains('\r') {
        value.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        value.to_owned()
    };

    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

/// A path shown relative to the folder that was scanned, so the column stays
/// readable however deep the library is nested.
pub fn relative_label(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_what_would_break_a_row() {
        assert_eq!(escape("Discovery"), "Discovery");
        assert_eq!(escape("Earth, Wind & Fire"), "\"Earth, Wind & Fire\"");
        assert_eq!(escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn folds_carriage_returns_so_only_the_record_separator_has_one() {
        assert_eq!(escape("two\r\nlines"), "\"two\nlines\"");
        assert_eq!(escape("two\rlines"), "\"two\nlines\"");
    }

    #[test]
    fn writes_a_row_of_escaped_cells() {
        let mut written = Vec::new();
        write_record(&mut written, ["a", "b,c", "d"].into_iter()).unwrap();
        assert_eq!(String::from_utf8(written).unwrap(), "a,\"b,c\",d\r\n");
    }

    #[test]
    fn shows_a_path_relative_to_the_scanned_folder() {
        assert_eq!(
            relative_label(Path::new("/music/Daft Punk/one.mp3"), Path::new("/music")),
            "Daft Punk/one.mp3"
        );
        // A path outside the root is shown whole rather than mangled.
        assert_eq!(
            relative_label(Path::new("/other/one.mp3"), Path::new("/music")),
            "/other/one.mp3"
        );
    }
}
