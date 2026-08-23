# music-organiser

`music-organiser` is a Rust command-line project for preparing and maintaining
a local music library. One executable provides two related workflows:

| Command | Purpose |
|---|---|
| `download` | Download `YOUTUBE_MUSIC_URL\|SPOTIFY_TRACK_URL` pairs through spotDL and rewrite their ID3 metadata |
| `delete` | Remove selected ID3 frames recursively |

The Cargo package and executable are currently named `music-tag-transfer`.
Downloaded audio comes from spotDL's configured providers, not from Spotify.
Only download material you are permitted to keep.

## Contents

- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Global usage](#global-usage)
- [Download exact-source pairs](#download-exact-source-pairs)
- [Delete ID3 tags](#delete-id3-tags)
- [Exit status](#exit-status)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Requirements

All commands require a stable Rust toolchain with Rust 2024 edition support.
The individual workflows have these additional requirements:

| Workflow | Additional requirements |
|---|---|
| `download` | spotDL 4.5.0 or newer and FFmpeg |
| Copyright and ISRC | A Spotify developer app and its Client ID/Client Secret |
| Synced lyrics and language | Nothing extra; both are handled by this program |
| Some YouTube downloads | Deno, installed system-wide or through `spotdl --download-deno` |
| `delete` | Read/write access to the relevant music folder |

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
cargo run -- download pairs.txt
```

## Quick start

`download` reads one exact-source pair per line, so the fastest path is to
write those pairs yourself.

1. Create `pairs.txt` with one `YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` pair per
   line:

   ```text
   https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW
   https://music.youtube.com/watch?v=9bZkp7q19f0|https://open.spotify.com/track/03UrZgTINDqvnUMbbIMhql
   ```

2. Download them:

   ```bash
   music-tag-transfer download pairs.txt --output ./music
   ```

Every pair is downloaded as MP3 with synchronised lyrics. Each file's ID3v2.3
tag is then rewritten: the `POPM` rating frame is removed, `TCOP`, `TSRC`, and
`TLAN` are filled in, and the generated `.lrc` file becomes a `SYLT` frame and
is deleted.

The copyright and ISRC lookups need a Spotify app's Client ID and Client
Secret. Export them, and the run will not stop to ask:

```bash
export SPOTIFY_CLIENT_ID='your-client-id'
export SPOTIFY_CLIENT_SECRET='your-client-secret'
```

Otherwise the command prompts for both before downloading anything. Pass
`--no-spotify-metadata` to skip the lookup entirely.

## Global usage

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
music-tag-transfer delete <FOLDER> "[Tag Name, Other Tag]" [--dry-run]
```

Global flags:

| Flag | Meaning |
|---|---|
| `-h`, `--help`, `help` | Print the top-level help |
| `-V`, `--version` | Print the package name and version |

`download --help` prints command-specific help.

Paths containing spaces must be quoted. The download command expands `~` in
its input, output, and token-file paths. Other commands receive paths exactly
as the shell supplies them.

## Download exact-source pairs

### Syntax

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
```

### Input format

`INPUT_FILE` must be UTF-8 text. Every non-empty line that does not begin with
`#` must be one exact-source pair:

```text
# Blank lines and comments are ignored
https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW
https://youtu.be/9bZkp7q19f0|spotify:track:03UrZgTINDqvnUMbbIMhql
```

This is spotDL's exact-source syntax. Nothing is left to a search: the URL
before `|` pins the audio that is downloaded, and the URL after `|` pins the
Spotify track whose metadata is written into the file.

The left-hand side accepts `music.youtube.com`, `www.youtube.com`,
`youtube.com`, `m.youtube.com`, and `youtu.be` links. `/watch?v=ID` and
`/playlist?list=ID` paths are supported; short `youtu.be/ID` links are expanded
to `https://www.youtube.com/watch?v=ID`.

The right-hand side must be a Spotify **track**, either as an
`open.spotify.com/track/ID` URL or a `spotify:track:ID` URI. `intl-XX` and
`embed` path prefixes are removed and HTTP links are normalized to HTTPS.
Album, playlist, artist, episode, show, and `spotify.link` URLs are rejected,
because a pair describes exactly one track.

Query strings and fragments are removed, so tracking parameters such as `si=`
do not create duplicates. Duplicate normalized pairs are downloaded only once.
If any non-comment line is invalid, the command reports up to ten invalid lines
and stops before starting spotDL.

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
| `--no-spotify-metadata` | Skip the Spotify copyright/ISRC lookup instead of asking for credentials |
| `--language <CODE>` | Fallback ISO-639-2 language for `TLAN`; default `eng` |
| `--max-attempts <N>` | Network attempts per pair; default `3`, minimum `1` |
| `--max-rate-limit-wait <SECS>` | Longest accepted Retry-After delay; default `300` |
| `-h, --help` | Print download help |
| `-V, --version` | Print the application version |

`--output=...`, `--spotdl=...`, `--auth-token=...`, `--token-file=...`, and
`--language=...` are also accepted. Use `--` before an input filename
that starts with a hyphen.

Set `SPOTDL_PROGRAM=/full/path/to/spotdl` to choose a spotDL executable
without repeating `--spotdl`. An explicit `--spotdl` option takes
precedence.

### The spotDL command

Each pair is passed to spotDL on its own, as a single quoted argument, so a
failure can be attributed to the input line it came from:

```bash
spotdl download "YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL" \
    --overwrite force --format mp3 --lyrics synced --generate-lrc
```

Those four options are fixed and are not configurable:

| Option | Effect |
|---|---|
| `--overwrite force` | Every listed pair is downloaded again, replacing any existing file |
| `--format mp3` | Output is always MP3, so an ID3 tag can always be written |
| `--lyrics synced` | spotDL prefers a provider that has time-synced lyrics |
| `--generate-lrc` | spotDL writes the timed lyrics next to the audio as `.lrc` |

The command also passes `--print-errors`, `--max-retries 0` (retries are
handled here instead), and spotDL's output template:

```text
{album-artist} || {album}/{artists} - {title} [{track-id}].{output-ext}
```

Because `--overwrite force` is used, rerunning an input file re-downloads
every pair in it rather than skipping finished files.

### One folder per album

The leading `{album-artist} || {album}` in that template groups every song into
a folder named after its album, so tracks that share an album land together:

```text
music/
├── Daft Punk || Discovery/
│   ├── Daft Punk - One More Time [0DiWol3AO6WpXZgp0goxAV].mp3
│   └── Daft Punk - Aerodynamic [2VEZx7NWsZ1D0eJ4uv5Fym].mp3
├── Daft Punk || Random Access Memories/
│   └── Daft Punk - Get Lucky [69kOkLUCkxIZYexIgSG8rq].mp3
├── output.txt
└── music-tag-transfer-download-failures.txt
```

spotDL creates the folders and sanitizes both values for the filesystem, so no
files are moved after the fact. `output.txt` and the failure report stay in the
output directory itself, and a `.lrc` file that had to be kept stays beside its
audio file inside the album folder.

Two caveats:

- Windows does not allow `|` in a path, so spotDL sanitizes the separator away
  there; the folder is still one per album, but it will not read `||`.
- Existing downloads are not moved. Rerunning the same input file re-downloads
  them into the new layout, because `--overwrite force` is used.

### Rating, copyright, ISRC, and language frames

spotDL writes a `POPM` popularimeter frame from Spotify's popularity score —
the frame most taggers display as the track's rating — and leaves `TCOP` empty,
because a single-track fetch does not carry the album's copyright.

After each download this program rewrites four frames:

| Frame | What happens |
|---|---|
| `POPM` | Removed outright |
| `TCOP` | Set to the copyright message fetched from Spotify |
| `TSRC` | Set to the ISRC fetched from Spotify |
| `TLAN` | Set to the language detected from the lyrics, or `--language` |

When Spotify lists no value for `TCOP` or `TSRC`, that frame is left alone and
the run says so rather than storing something invented.

#### Where the values come from

The ISRC rides along on the track request, so it costs nothing extra.
Copyright exists only on Spotify's album object, so a lookup is a track request
followed by an album request. Albums are cached, so a whole album's worth of
pairs costs one album request. When an album lists both a `C` copyright and a
`P` phonogram entry, the `C` entry wins.

The lookup uses the client-credentials flow and needs a Spotify developer app:

| Variable | Purpose |
|---|---|
| `SPOTIFY_CLIENT_ID` | Client ID of a Spotify developer app |
| `SPOTIFY_CLIENT_SECRET` | Client Secret of the same app |

If either is missing, the command prompts for it before downloading anything,
so a long run never stops halfway to ask. The values are used only for that run
and are never written to disk. With `--non-interactive` there is nothing to
prompt, so the command stops and names the two variables.
`--no-spotify-metadata` skips the lookup, and with it `TCOP` and `TSRC`.

#### Language

Spotify does not expose a language for tracks — only shows and episodes carry
one — so `TLAN` cannot be fetched the way copyright and ISRC are. The synced
lyrics are the only evidence available, and the language is detected from their
text.

- A confident detection sets `TLAN`, and the `SYLT` frame's own language field
  is set to match.
- Too little text, or an unreliable guess, falls back to `--language`
  (default `eng`) rather than recording something invented. Two or more
  non-blank lyric lines are required before a guess is even attempted.
- A track with no `.lrc` file has nothing to detect from, so it gets
  `--language` too.

`TLAN` is written even with `--no-spotify-metadata`, because it never came
from Spotify. The detector reports ISO-639-3, which for every individual
language it knows is the same as the ISO-639-2/T code ID3v2.3 asks for; the two
macrolanguage cases are mapped (`cmn` becomes `zho`, `pes` becomes `fas`).

### Synchronised lyrics as an ID3 `SYLT` frame

spotDL does not write synchronised lyrics into the tag. `--lyrics synced` only
embeds the plain text in a `USLT` frame, which has no timestamps, and
`--generate-lrc` leaves the timed lyrics in a separate `.lrc` file.

After each successful download this program closes that gap, in the same tag
write as the rating and copyright changes above:

1. Every `.lrc` file below the output directory is parsed. `[mm:ss.xx]`,
   `[mm:ss]`, and `[hh:mm:ss.xxx]` timestamps are converted to milliseconds,
   metadata tags such as `[ar:...]` are ignored, and a line carrying several
   timestamps is repeated at each one.
2. The lines are written to the matching audio file as an ID3v2.3 `SYLT`
   frame: UTF-16 text, millisecond timestamp format, content type "lyrics",
   language `eng`. Any previous `SYLT` frame is replaced.
3. The now-redundant `USLT` frame is deleted.
4. The file is read back from disk and the stored frame is compared with the
   `.lrc` file. Only if it matches — and the `USLT` frame is really gone — is
   the `.lrc` file deleted.

The frame payload is written directly rather than through the `id3` crate's
encoder, which appends a trailing NUL byte that is not in the ID3v2.3
specification and makes strict parsers such as mutagen discard the whole
frame.

If any of those steps fails, the `.lrc` file is **kept** so its lyrics are
never the thing that gets lost, nothing is written to the audio file, and the
pair it belongs to is reported as failed so it appears in the retry list. The
usual cause is a `.lrc` file with no timestamped line at all. A pair for which
spotDL generated no `.lrc` file is not a failure: the rating and copyright
changes are still applied, and there were simply no synced lyrics to embed.

Downloaded files are matched back to their input pair through the Spotify track
ID in the forced `[{track-id}]` output template, which is also what the
copyright lookup uses.

### Download execution and output files

The output directory receives:

| File | Contents |
|---|---|
| `output.txt` | Failed and unattempted pairs, ready to retry |
| `music-tag-transfer-download-failures.txt` | Detailed attempted failures and the stop reason |

`output.txt` is always written; it is empty after a completely successful
run. Because it holds the same pair syntax as the input, it can be passed
straight back to `download`. The detailed failure report is written only when
at least one attempted pair fails or causes the batch to stop.

Ordinary network/service failures use exponential backoff up to
`--max-attempts`. A short Spotify `Retry-After` delay is respected once.
Application quota errors, repeated rate limits, or delays above
`--max-rate-limit-wait` stop the batch and preserve the current and remaining
pairs rather than rotating tokens or sleeping for a long time.

If spotDL reports that Deno is required:

- an interactive run asks before invoking `spotdl --download-deno`;
- `--auto-download-deno` approves that setup automatically;
- `--non-interactive` without automatic setup stops and preserves the pairs.

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
  music-tag-transfer download --official-api pairs.txt
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

## Exit status

| Status | Meaning |
|---|---|
| `0` | Help/version succeeded, or the requested operation completed without processing errors |
| `1` | Invalid arguments, setup/input/output failure, download failures, or per-file metadata errors |

Any failed or unattempted pair results in status 1 and is preserved in
`output.txt`. A pair whose audio downloaded but whose metadata could not be
finished counts as failed, and its `.lrc` file is kept.

## Troubleshooting

### `cannot run 'spotdl'`

Confirm `spotdl --version` works in the same shell. Otherwise pass
`--spotdl /full/path/to/spotdl` or set `SPOTDL_PROGRAM`.

### spotDL configuration blocks token-free mode

Inspect `~/.config/spotdl/config.json` or `~/.spotdl/config.json`. Disable
`load_config`, or clear the official-API settings named in the error. Use
`--official-api` only when that mode is intentional.

### Spotify authentication for copyright/ISRC fails

Confirm `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` belong to the same
Spotify developer app. These are app credentials and are different from
`SPOTIFY_AUTH_TOKEN`, which is used only by the downloader's optional
official mode. Run with `--no-spotify-metadata` to download without them.

### A download needs Deno

Run `spotdl --download-deno` manually, install Deno system-wide, or rerun the
download command with `--auto-download-deno`.

### A `.lrc` file was left behind

Its lyrics could not be embedded, and the reason is printed during the run and
recorded in `music-tag-transfer-download-failures.txt`. The usual cause is a
`.lrc` file that contains no timestamped line. Retry the pair from
`output.txt`, or delete the `.lrc` file yourself if the track simply has no
synced lyrics.

### A player shows no synced lyrics

Confirm the frame is there, for example with mutagen:

```bash
python3 -c "from mutagen.id3 import ID3; print(ID3('track.mp3').getall('SYLT'))"
```

If the frame is present, the player does not support `SYLT`; many players read
only the untimed `USLT` frame, which this program removes.

### Metadata commands find no files

Confirm the path is a directory and the files use one of the supported
extensions: MP1, MP2, MP3, WAV, AIF, or AIFF. FLAC, M4A, and OGG files are not
processed.

## Using the Rust library

The package also exposes the recursive metadata deletion operation:

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

The same metadata rules can be applied to a single file that already has a
`.lrc` sidecar next to it:

```rust
use std::path::Path;
use music_tag_transfer::{finalize, spotify::TrackMetadata};

fn main() {
    let remote = TrackMetadata {
        copyright: Some("2013 Some Label".to_owned()),
        isrc: Some("GBAYE0601498".to_owned()),
    };
    let report = finalize(
        Path::new("/path/to/Artist - Song [id].mp3"),
        &remote,
        "eng",
    );
    println!(
        "Removed {} rating frame(s), embedded {} line(s)",
        report.ratings_removed, report.lines_embedded
    );
    for failure in &report.failures {
        eprintln!("{}: {}", failure.path.display(), failure.message);
    }
}
```

`DeleteReport`, `MetadataReport`, `FileError`, and `TagSpec` are also exported.
The `cli`, `download`, `metadata`, and `spotify` modules expose the
executable's command configuration and entry points.

## Development

The GitHub Actions workflow runs the same three checks used locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Run all three before merging behavior or documentation changes.
