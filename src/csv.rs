//! Reading and writing RFC 4180 CSV, shared by the commands that handle one.
//!
//! The frame export and the copyright report write a table of text that has to
//! survive a spreadsheet: values carrying commas, quotes, and — in the case of
//! lyrics — whole paragraphs of newlines. `search-ytm-url` reads a table back,
//! which is the same rules read in the other direction.

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

/// A table read back off disk: one header row, then the rows under it.
///
/// Rows are kept exactly as long as the header, short ones padded and long
/// ones left as they are, so that a column index found in the header is always
/// safe to index a row with.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// The index of the first header matching one of `names`, compared with
    /// punctuation, case, and spacing set aside.
    ///
    /// A person writes `Song Name`, a spreadsheet exports `song_name`, and an
    /// API writes `songName`; all three mean the column, so all three find it.
    /// The names are tried in order, so a caller lists its preferred spelling
    /// first and its fallbacks after.
    pub fn column(&self, names: &[&str]) -> Option<usize> {
        let headers: Vec<String> = self.headers.iter().map(|header| key(header)).collect();
        names
            .iter()
            .find_map(|name| headers.iter().position(|header| *header == key(name)))
    }
}

/// One cell of a row, trimmed, or `""` when the row is short or the column is
/// not in the file at all.
///
/// Taking the column as an `Option` is what lets a caller read a row the same
/// way whether or not the file had that column, instead of branching on it at
/// every use.
pub fn cell(row: &[String], column: Option<usize>) -> &str {
    column
        .and_then(|index| row.get(index))
        .map_or("", |value| value.trim())
}

/// A header reduced to what identifies it, so that the several spellings of
/// one column all collapse onto the same key.
///
/// Everything but letters and digits goes, which folds `song_name`,
/// `Song Name`, and `song-name` together. A camel-cased `songName` is split on
/// its capitals first, so it lands on the same key rather than on `songname`.
fn key(header: &str) -> String {
    let mut split = String::with_capacity(header.len() + 4);
    let mut previous_lowercase = false;
    for character in header.chars() {
        if previous_lowercase && character.is_uppercase() {
            split.push(' ');
        }
        previous_lowercase = character.is_lowercase() || character.is_numeric();
        split.push(character);
    }
    split
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Read a whole CSV document.
///
/// The separator may be `\r\n`, `\n`, or a lone `\r`, because a file that has
/// been through a spreadsheet, a shell, and a text editor can carry any of
/// them -- and inside a quoted cell all three are content rather than the end
/// of a row.
pub fn parse(contents: &str) -> Table {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut characters = contents.chars().peekable();

    while let Some(character) = characters.next() {
        if quoted {
            match character {
                // Inside quotes, a doubled quote is one literal quote and a
                // single one ends the quoting.
                '"' if characters.peek() == Some(&'"') => {
                    characters.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                _ => cell.push(character),
            }
            continue;
        }
        match character {
            // Only a quote opening the cell quotes it; one appearing mid-cell
            // is a stray that a spreadsheet would show, so it is kept.
            '"' if cell.is_empty() => quoted = true,
            ',' => record.push(std::mem::take(&mut cell)),
            '\r' | '\n' => {
                if character == '\r' && characters.peek() == Some(&'\n') {
                    characters.next();
                }
                record.push(std::mem::take(&mut cell));
                records.push(std::mem::take(&mut record));
            }
            _ => cell.push(character),
        }
    }
    // A file that does not end in a newline still ends in a row.
    if !cell.is_empty() || !record.is_empty() {
        record.push(cell);
        records.push(record);
    }

    // A blank line carries no cells worth keeping, wherever it falls: the one
    // a trailing newline leaves behind, and any left in the middle of a file
    // that has been edited by hand.
    records.retain(|record| record.iter().any(|cell| !cell.trim().is_empty()));

    let mut records = records.into_iter();
    let Some(headers) = records.next() else {
        return Table::default();
    };
    let headers: Vec<String> = headers
        .into_iter()
        .map(|header| header.trim().to_owned())
        .collect();
    let rows = records
        .map(|mut row| {
            row.resize(row.len().max(headers.len()), String::new());
            row
        })
        .collect();

    Table { headers, rows }
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
    fn reads_a_header_and_its_rows() {
        let table = parse("song name,artist name\r\nGet Lucky,Daft Punk\r\n");

        assert_eq!(table.headers, ["song name", "artist name"]);
        assert_eq!(table.rows, [["Get Lucky", "Daft Punk"]]);
    }

    /// Everything the writer escapes has to survive being read back, or a file
    /// this crate produced could not be fed to a command that reads one.
    #[test]
    fn round_trips_commas_quotes_and_newlines() {
        let cells = ["Earth, Wind & Fire", "say \"hi\"", "two\nlines", "plain"];
        let mut written = Vec::new();
        write_record(&mut written, ["a", "b", "c", "d"].into_iter()).unwrap();
        write_record(&mut written, cells.into_iter()).unwrap();

        let table = parse(&String::from_utf8(written).unwrap());

        assert_eq!(table.rows, [cells]);
    }

    #[test]
    fn accepts_a_file_separated_by_bare_line_feeds_and_ending_without_one() {
        let table = parse("a,b\nc,d");

        assert_eq!(table.headers, ["a", "b"]);
        assert_eq!(table.rows, [["c", "d"]]);
    }

    /// A short row is padded so that a column index taken from the header can
    /// always be used on a row without checking its length first.
    #[test]
    fn pads_short_rows_and_drops_blank_ones() {
        let table = parse("a,b,c\r\nonly\r\n\r\n   \r\nx,y,z\r\n");

        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0], ["only", "", ""]);
        assert_eq!(table.rows[1], ["x", "y", "z"]);
    }

    /// A cell holding nothing but a comma is one value, not two.
    #[test]
    fn a_quoted_cell_hides_its_separators() {
        let table = parse("a,b\r\n\"x,y\",\"line\r\nbreak\"\r\n");

        assert_eq!(table.rows, [["x,y", "line\r\nbreak"]]);
    }

    #[test]
    fn an_empty_document_has_no_headers_and_no_rows() {
        assert_eq!(parse(""), Table::default());
        assert_eq!(parse("\r\n"), Table::default());
    }

    /// The four columns the caller asks for are spelled differently by every
    /// tool that exports them, and all the spellings mean the same column.
    #[test]
    fn finds_a_column_however_its_header_is_spelled() {
        let table = parse("Song_Name,albumName,ARTIST NAME,spotify-url\r\n");

        assert_eq!(table.column(&["song name"]), Some(0));
        assert_eq!(table.column(&["album name"]), Some(1));
        assert_eq!(table.column(&["artist name"]), Some(2));
        assert_eq!(table.column(&["spotify_url"]), Some(3));
        assert_eq!(table.column(&["nothing here"]), None);
    }

    /// Names are tried in order, so a caller's preferred spelling wins over a
    /// fallback that also appears in the file.
    #[test]
    fn prefers_the_first_name_it_is_given() {
        let table = parse("title,song name\r\n");

        assert_eq!(table.column(&["song name", "title"]), Some(1));
        assert_eq!(table.column(&["title", "song name"]), Some(0));
    }

    #[test]
    fn a_missing_column_and_a_short_row_both_read_as_empty() {
        let table = parse("a,b\r\nx\r\n");
        let row = &table.rows[0];

        assert_eq!(cell(row, Some(0)), "x");
        assert_eq!(cell(row, Some(1)), "");
        assert_eq!(cell(row, None), "");
        assert_eq!(cell(row, Some(9)), "");
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
