# music-organiser

`music-organiser` is a Rust command-line project for preparing and maintaining
a local music library. One executable provides four related workflows:

| Command | Purpose |
|---|---|
| `resolve` | Search Spotify for free-text track descriptions and write Spotify URLs |
| `download` | Download a file of Spotify links through spotDL |
| `delete` | Remove selected ID3 frames recursively |
| `transfer` | Copy one ID3 frame between tracks with matching titles |

The Cargo package and executable are currently named `music-tag-transfer`.
Downloaded audio comes from spotDL's configured providers, not from Spotify.
Only download material you are permitted to keep.

## Contents

- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Global usage](#global-usage)
- [Resolve track descriptions](#resolve-track-descriptions)
- [Download Spotify links](#download-spotify-links)
- [Delete ID3 tags](#delete-id3-tags)
- [Transfer an ID3 frame](#transfer-an-id3-frame)
- [Exit status](#exit-status)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Requirements

All commands require a stable Rust toolchain with Rust 2024 edition support.
The individual workflows have these additional requirements:

| Workflow | Additional requirements |
|---|---|
| `resolve` | A Spotify developer app and its Client ID/Client Secret |
| `download` | spotDL 4.5.0 or newer and FFmpeg |
| Some YouTube downloads | Deno, installed system-wide or through `spotdl --download-deno` |
| `delete` / `transfer` | Read/write access to the relevant music folders |

Check the external programs before downloading:

```bash
spotdl --version
ffmpeg -version
deno --version
```

Deno is optional until spotDL reports that a particular YouTube download needs
it.

## Installation

Clone and build an optimized binary:

```bash
git clone https://github.com/Raina3266/music_organiser.git
cd music_organiser
cargo build --release
```

The result is `target/release/music-tag-transfer` on Linux and macOS, or
`target/release/music-tag-transfer.exe` on Windows.

Run it directly:

```bash
./target/release/music-tag-transfer --help
```

Or install it into Cargo's binary directory:

```bash
cargo install --path .
music-tag-transfer --version
```

During development, every example below can instead begin with
`cargo run --`. For example:

```bash
cargo run -- resolve tracks.txt spotify-links.txt
```

## Quick start

The most useful end-to-end workflow is a two-step pipeline.

1. Create `tracks.txt` with one search description per line:

   ```text
   Queen - Bohemian Rhapsody
   Daft Punk Get Lucky
   Adele, 21, Rolling in the Deep
   ```

2. Create an app in the
   [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and
   expose its credentials to the process.

   Linux/macOS:

   ```bash
   export SPOTIFY_CLIENT_ID='your-client-id'
   export SPOTIFY_CLIENT_SECRET='your-client-secret'
   ```

   PowerShell:

   ```powershell
   $env:SPOTIFY_CLIENT_ID = 'your-client-id'
   $env:SPOTIFY_CLIENT_SECRET = 'your-client-secret'
   ```

3. Resolve the descriptions, then download the resulting URLs:

   ```bash
   music-tag-transfer resolve tracks.txt spotify-links.txt
   music-tag-transfer download spotify-links.txt --output ./music
   ```

The resolver writes missing tracks and request failures as comments, and the
downloader ignores comments. The output can therefore pass directly from one
command to the next without manual cleanup.

## Global usage

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
music-tag-transfer resolve <INPUT_FILE> <OUTPUT_FILE>
music-tag-transfer delete <FOLDER> "[Tag Name, Other Tag]" [--dry-run]
music-tag-transfer transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>
music-tag-transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>
```

Global flags:

| Flag | Meaning |
|---|---|
| `-h`, `--help`, `help` | Print the top-level help |
| `-V`, `--version` | Print the package name and version |

`download --help` and `resolve --help` print command-specific help.

Paths containing spaces must be quoted. The download command expands `~` in
its input, output, and token-file paths. Other commands receive paths exactly
as the shell supplies them.

## Resolve track descriptions

### Syntax

```text
music-tag-transfer resolve <INPUT_FILE> <OUTPUT_FILE>
```

Required environment variables:

| Variable | Purpose |
|---|---|
| `SPOTIFY_CLIENT_ID` | Client ID of a Spotify developer app |
| `SPOTIFY_CLIENT_SECRET` | Client Secret of the same app |

Both variables must be non-empty. The command uses Spotify's Client
Credentials flow; it does not need a user access token.

### Input format

`INPUT_FILE` must be UTF-8 text. Each non-empty line that does not begin with
`#` is used as a Spotify track search:

```text
# Comments are preserved
artist:"Massive Attack" track:"Teardrop"
Nujabes Feather

Daft Punk - Get Lucky
```

The entire line is sent as the search query. Spotify's first track result is
selected, so more specific artist/title queries reduce ambiguity.

### Output and behavior

The output keeps the same line order:

- a resolved query becomes its `https://open.spotify.com/track/...` URL;
- an empty line remains empty;
- an input comment remains a comment;
- no result becomes `# NOT FOUND: original query`;
- a per-line request failure becomes `# ERROR (...): original query`.

The output file's parent directory must already exist. Existing output files
are replaced.

Requests use a 15-second timeout and pause briefly between searches. HTTP 429
responses are retried up to five times after the first request. A
`Retry-After` value above 300 seconds is rejected instead of causing a long
silent wait.

No-result lines are reported but do not make the command fail. Authentication,
input/output, or per-line request errors produce exit status 1.

## Download Spotify links

### Syntax

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
```

### Input format

`INPUT_FILE` must be UTF-8 text with one Spotify URL or URI per line:

```text
# Blank lines and comments are ignored
https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW
https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy?si=tracking
spotify:playlist:37i9dQZF1DXcBWIGoYBM5M
https://spotify.link/AbCdEf
```

Supported content types are `track`, `album`, `playlist`, `artist`,
`episode`, and `show`. The parser also accepts:

- `spotify:TYPE:ID` URIs;
- `spotify.link` short links;
- `open.spotify.com/intl-XX/...` locale paths;
- `open.spotify.com/embed/...` paths;
- HTTP links, which are normalized to HTTPS.

Query strings and fragments are removed. Duplicate normalized links are
downloaded only once. If any non-comment line is invalid, the command reports
up to ten invalid lines and stops before starting spotDL.

### Options

| Option | Meaning |
|---|---|
| `-o, --output <DIR>` | Download directory; default `~/Documents/Music` |
| `--spotdl <PROGRAM>` | spotDL executable or path; default `spotdl` |
| `--official-api` | Explicitly use Spotify's official Web API |
| `--auth-token <TOKEN>` | Official mode: use this short-lived access token |
| `--token-file <FILE>` | Official mode: read the access token from a file |
| `--non-interactive` | Never prompt for Deno or a replacement token |
| `--auto-download-deno` | Allow spotDL to install Deno when required |
| `--max-attempts <N>` | Network attempts per link; default `3`, minimum `1` |
| `--max-rate-limit-wait <SECS>` | Longest accepted Retry-After delay; default `300` |
| `-h, --help` | Print download help |
| `-V, --version` | Print the application version |

`--output=...`, `--spotdl=...`, `--auth-token=...`, and
`--token-file=...` are also accepted. Use `--` before an input filename
that starts with a hyphen.

Set `SPOTDL_PROGRAM=/full/path/to/spotdl` to choose a spotDL executable
without repeating `--spotdl`. An explicit `--spotdl` option takes
precedence.

### Token-free and official API modes

The default mode is token-free. It deliberately does not pass
`--use-official-api`, `--auth-token`, or `--use-cache-file` to spotDL.
Before downloading, it checks spotDL's active `config.json` and stops if
`use_official_api`, `use_cache_file`, `user_auth`, or a saved
`auth_token` would silently change that behavior. Disable config loading or
clear those settings before retrying.

Use the official API only as an explicit fallback:

```bash
SPOTIFY_AUTH_TOKEN='short-lived-access-token' \
  music-tag-transfer download --official-api spotify-links.txt
```

Official-mode token precedence is:

1. `--auth-token`;
2. `--token-file`;
3. `SPOTIFY_AUTH_TOKEN`.

`--auth-token` and `--token-file` are rejected unless `--official-api`
is present. Token files may contain a plain token, a `Bearer ...` value, or a
quoted JavaScript-style token assignment. Protect these files and never commit
tokens, cookies, Client Secrets, or other credentials.

When an official token expires, an interactive terminal can request a
replacement. `--non-interactive` disables this prompt.

### Download execution and output files

Each unique link is sent to spotDL separately so failures can be attributed to
the original input line. Files use spotDL's template:

```text
{artists} - {title} [{track-id}].{output-ext}
```

The command always uses `--overwrite skip`, so rerunning the same input skips
files spotDL considers complete.

The output directory receives:

| File | Contents |
|---|---|
| `output.txt` | Failed and unattempted URLs, ready to retry |
| `music-tag-transfer-download-failures.txt` | Detailed attempted failures and the stop reason |

`output.txt` is always written; it is empty after a completely successful
run. The detailed failure report is written only when at least one attempted
URL fails or causes the batch to stop.

Ordinary network/service failures use exponential backoff up to
`--max-attempts`. A short Spotify `Retry-After` delay is respected once.
Application quota errors, repeated rate limits, or delays above
`--max-rate-limit-wait` stop the batch and preserve the current and remaining
URLs rather than rotating tokens or sleeping for a long time.

If spotDL reports that Deno is required:

- an interactive run asks before invoking `spotdl --download-deno`;
- `--auto-download-deno` approves that setup automatically;
- `--non-interactive` without automatic setup stops and preserves the URLs.

## Delete ID3 tags

The delete command changes metadata in place. Back up valuable files and run a
dry run first.

### Syntax

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

### Supported delete tag names

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

## Transfer an ID3 frame

### Syntax

```text
music-tag-transfer transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>
```

The original three-positional-argument form remains supported:

```text
music-tag-transfer <SOURCE_FOLDER> <DESTINATION_FOLDER> <FRAME_ID>
```

Example—copy unsynchronised lyrics:

```bash
music-tag-transfer transfer "/library/with-lyrics" "/library/without-lyrics" USLT
```

`FRAME_ID` must contain exactly four uppercase ASCII letters or digits, such
as `USLT`, `APIC`, or `TENC`.

The command scans both folder trees using the same supported extensions and
symlink rules as `delete`. Source frames are indexed by the source file's
`TIT2` title, and destination files are matched by the exact same title.
Files without an ID3 tag or title are skipped.

If several source files have the same title and requested frame, the first
path in sorted traversal order supplies the frame data. Matching frames are
added to each destination tag and the result is written as ID3v2.3. The
command does not rename, copy, or delete audio files, and it has no dry-run
mode.

## Exit status

| Status | Meaning |
|---|---|
| `0` | Help/version succeeded, or the requested operation completed without processing errors |
| `1` | Invalid arguments, setup/input/output failure, download failures, resolver request errors, or per-file metadata errors |

For `resolve`, a valid query with no Spotify result is not an error. For
`download`, any failed or unattempted link results in status 1 and is
preserved in `output.txt`.

## Troubleshooting

### `cannot run 'spotdl'`

Confirm `spotdl --version` works in the same shell. Otherwise pass
`--spotdl /full/path/to/spotdl` or set `SPOTDL_PROGRAM`.

### spotDL configuration blocks token-free mode

Inspect `~/.config/spotdl/config.json` or `~/.spotdl/config.json`. Disable
`load_config`, or clear the official-API settings named in the error. Use
`--official-api` only when that mode is intentional.

### Spotify resolver authentication fails

Confirm `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` belong to the same
Spotify developer app. These are app credentials and are different from
`SPOTIFY_AUTH_TOKEN`, which is used only by the downloader's optional
official mode.

### A download needs Deno

Run `spotdl --download-deno` manually, install Deno system-wide, or rerun the
download command with `--auto-download-deno`.

### Metadata commands find no files

Confirm the path is a directory and the files use one of the supported
extensions: MP1, MP2, MP3, WAV, AIF, or AIFF. FLAC, M4A, and OGG files are not
processed.

## Using the Rust library

The package also exposes the recursive metadata operations:

```rust
use std::path::Path;
use music_tag_transfer::{SUPPORTED_TAGS, delete_tags_recursively};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let encoded_by = SUPPORTED_TAGS
        .iter()
        .copied()
        .find(|tag| tag.name == "Encoded-by")
        .expect("supported tag");

    let report = delete_tags_recursively(
        Path::new("/path/to/music"),
        &[encoded_by],
        true,
    )?;
    println!("Would update {} file(s)", report.files_changed);
    Ok(())
}
```

`transfer_frame_recursively`, `TransferReport`, `DeleteReport`,
`FileError`, and `TagSpec` are also exported. The `cli`, `download`,
and `resolve` modules expose the executable's command configuration and
entry points.

## Development

The GitHub Actions workflow runs the same three checks used locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Run all three before merging behavior or documentation changes.
