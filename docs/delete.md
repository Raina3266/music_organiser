# Delete ID3 tags

[← Back to the README](../README.md)

The delete command changes metadata in place. Back up valuable files and run a
dry run first.

## Syntax

```text
music-tag-transfer delete <FOLDER> "[Tag Name, Other Tag]" [--dry-run]
```

The brackets are part of the argument, and the entire list should be quoted:

```bash
music-tag-transfer delete "/path/to/music" "[Encoded-by, Album Artist]" --dry-run
music-tag-transfer delete "/path/to/music" "[Encoded-by, Album Artist]"
```

Tag names are case-sensitive. Whitespace around comma-separated names is
ignored, and duplicate frame IDs are removed from the request.

The command:

- searches the folder and its subdirectories;
- processes MP1, MP2, MP3, WAV, AIF, and AIFF extensions case-insensitively;
- ignores symlinks and non-music files;
- skips files without an ID3 tag;
- skips tagged files that contain none of the requested frames;
- removes every frame with a requested ID;
- writes changed tags as ID3v2.3;
- reports per-file errors and continues with other files.

Without `--dry-run`, each changed file is copied to a temporary file in the
same directory, updated there, and then replaced. `--dry-run` performs the
same scan and reports the expected counts without writing files.

## Supported delete tag names

| Tag name | ID3v2.3 frame |
|---|---|
| `Encoded-by` | `TENC` |
| `Album Artist` | `TPE2` |
| `Album` | `TALB` |
| `Artist` | `TPE1` |
| `BPM` | `TBPM` |
| `Comment` | `COMM` |
| `Composer` | `TCOM` |
| `Conductor` | `TPE3` |
| `Copyright` | `TCOP` |
| `Date` | `TDAT` |
| `Disc Number` | `TPOS` |
| `Encoding Settings` | `TSSE` |
| `File Owner` | `TOWN` |
| `File Type` | `TFLT` |
| `Genre` | `TCON` |
| `Grouping` | `TIT1` |
| `Initial Key` | `TKEY` |
| `ISRC` | `TSRC` |
| `Language` | `TLAN` |
| `Length` | `TLEN` |
| `Lyrics` | `USLT` |
| `Synced Lyrics` | `SYLT` |
| `Lyricist` | `TEXT` |
| `Media Type` | `TMED` |
| `Original Album` | `TOAL` |
| `Original Artist` | `TOPE` |
| `Original Filename` | `TOFN` |
| `Original Lyricist` | `TOLY` |
| `Original Release Year` | `TORY` |
| `Picture` | `APIC` |
| `Playlist Delay` | `TDLY` |
| `Publisher` | `TPUB` |
| `Recording Dates` | `TRDA` |
| `Remixed By` | `TPE4` |
| `Subtitle` | `TIT3` |
| `Time` | `TIME` |
| `Title` | `TIT2` |
| `Track Number` | `TRCK` |
| `User Text` | `TXXX` |
| `User URL` | `WXXX` |
| `Year` | `TYER` |
