# Export ID3 frames to CSV

[← Back to the README](../README.md)

The export command only reads music files. It walks a folder recursively and
writes every ID3 frame it finds into a single CSV file, which is the quickest
way to see what a library actually contains before deleting or rewriting
anything.

## Syntax

```text
music-tag-transfer export <FOLDER> [OUTPUT_CSV] [--overwrite]
music-tag-transfer copyright <FOLDER> [--only-missing] [--dry-run]
```

```bash
# writes /path/to/music/id3-frames.csv
music-tag-transfer export "/path/to/music"

# writes a named file, replacing it if it is already there
music-tag-transfer export "/path/to/music" ~/frames.csv --overwrite
```

Without `OUTPUT_CSV` the file is named `id3-frames.csv` and is created inside
the scanned folder. An existing destination is never replaced unless
`--overwrite` is passed; the command reports the conflict and writes nothing.

## CSV layout

- The first column, `File`, holds each file's path relative to the scanned
  folder.
- Every other column is one ID3 frame ID found anywhere under the folder,
  sorted by frame ID. Frames this program knows a name for are headed
  `Title (TIT2)`; anything else is headed with its bare frame ID.
- Every music file gets one row, including files without an ID3 tag, whose
  cells are all empty.
- A frame a file does not have stays an empty cell, and a frame that is
  present but empty stays empty too.
- Repeated frames sharing a frame ID, such as several `TXXX` or `APIC` frames,
  are joined with ` | ` in one cell.

The file is UTF-8 with `CRLF` record separators and RFC 4180 quoting, so
values containing commas, quotation marks, or newlines survive a round trip
through spreadsheet software.

Frame values are exported as text:

| Frame | Cell contents |
|---|---|
| Text and URL frames | The value itself |
| `TXXX`, `WXXX`, `COMM`, `USLT` | `description: value`, or just the value when the description is empty |
| `SYLT` | The timed lines rebuilt in `.lrc` form, `[mm:ss.xx] text`, one per line |
| `APIC` | `Front cover (image/jpeg, 40213 bytes)`, prefixed by the description when there is one |
| `PRIV` | `owner identifier: 32 bytes` |
| Other binary frames | A short summary rather than the raw bytes |

The scan matches the delete command: the same MP1, MP2, MP3, WAV, AIF, and
AIFF extensions, case-insensitively, with symlinks and non-music files
ignored. Files that cannot be read are reported on stderr, left out of the
CSV, and make the command exit with status 1; the rest of the library is still
exported.
