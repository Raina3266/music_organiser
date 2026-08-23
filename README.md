# music-organiser

`music-organiser` is a Rust command-line project for preparing and maintaining
a local music library. One executable provides three related workflows:

| Command | Purpose |
|---|---|
| `download` | Download Spotify and/or YouTube Music links through spotDL and rewrite their ID3 metadata |
| `delete` | Remove selected ID3 frames recursively |
| `export` | Write every ID3 frame found recursively into one CSV file |

The Cargo package and executable are currently named `music-tag-transfer`.
Downloaded audio comes from spotDL's configured providers, not from Spotify.
Only download material you are permitted to keep.

## Contents

- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Global usage](#global-usage)
- [Download music](#download-music)
- [Delete ID3 tags](#delete-id3-tags)
- [Export ID3 frames to CSV](#export-id3-frames-to-csv)
- [Exit status](#exit-status)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Requirements

All commands require a stable Rust toolchain with Rust 2024 edition support.
The individual workflows have these additional requirements:

| Workflow | Additional requirements |
|---|---|
| `download` | spotDL 4.5.0 or newer and FFmpeg |
| Copyright | Nothing; the iTunes Search API needs no account, key, or token |
| Synced lyrics and language | Nothing extra; both are handled by this program |
| Some YouTube downloads | Deno, installed system-wide or through `spotdl --download-deno` |
| `delete` | Read/write access to the relevant music folder |
| `export` | Read access to the music folder and write access to the CSV destination |

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
cargo run -- download links.txt
```

## Quick start

`download` reads one link per line, so the fastest path is to paste the links
you want into a text file. Spotify links, YouTube Music links, and
`YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` exact-source pairs can be mixed freely in
the same file.

1. Create `links.txt`:

   ```text
   # a Spotify link: spotDL searches YouTube for the audio
   https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW

   # a YouTube Music link: spotDL searches Spotify for the metadata
   https://music.youtube.com/watch?v=9bZkp7q19f0

   # a pair: neither side is searched
   https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/03UrZgTINDqvnUMbbIMhql

   # an album or playlist link downloads every song on it
   https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy
   ```

2. Download them:

   ```bash
   music-tag-transfer download links.txt --output ./music
   ```

Every line is downloaded as MP3 with synchronised lyrics. Each file's ID3v2.3
tag is then rewritten: the `POPM` rating and `TSSE` encoder-settings frames are
removed, `TCOP` and `TLAN` are filled in, and the generated `.lrc` file is
pasted into the ordinary `USLT` lyrics frame and deleted.

No credentials are needed for any of it. The copyright comes from the iTunes
Search API, which is open to anyone.

## Global usage

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
music-tag-transfer delete <FOLDER> "[Tag Name, Other Tag]" [--dry-run]
music-tag-transfer export <FOLDER> [OUTPUT_CSV] [--overwrite]
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

## Download music

### Syntax

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
```

### Input format

`INPUT_FILE` must be UTF-8 text. Every non-empty line that does not begin with
`#` is one of three forms, and a single file may mix them freely:

```text
# Blank lines and comments are ignored

# 1. a Spotify link
https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW
https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy
spotify:playlist:37i9dQZF1DXcBWIGoYBM5M

# 2. a YouTube Music link
https://music.youtube.com/watch?v=9bZkp7q19f0
https://youtu.be/dQw4w9WgXcQ

# 3. an exact-source pair
https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/03UrZgTINDqvnUMbbIMhql
https://youtu.be/9bZkp7q19f0|spotify:track:02Q0SXOsk74oV4hesiL6JW
```

Each form leaves a different part of the job to spotDL's own search:

| Form | Audio | Metadata |
|---|---|---|
| Spotify link | searched on YouTube by spotDL | pinned by the link |
| YouTube link | pinned by the link | searched on Spotify by spotDL |
| `YOUTUBE_MUSIC_URL\|SPOTIFY_TRACK_URL` | pinned by the left URL | pinned by the right URL |

The pair is spotDL's exact-source syntax, and it is the only form in which
nothing at all is left to a search.

**Spotify links** may be `open.spotify.com` URLs or `spotify:` URIs, and may
name a **track**, an **album**, or a **playlist**; an album or playlist link
downloads every song on it. `intl-XX` and `embed` path prefixes are removed and
HTTP links are normalized to HTTPS. Artist, episode, show, and `spotify.link`
URLs are rejected — the last because a shortened link is never resolved.

**YouTube links** may be `music.youtube.com`, `www.youtube.com`, `youtube.com`,
`m.youtube.com`, or `youtu.be`. `/watch?v=ID` and `/playlist?list=ID` paths are
supported; short `youtu.be/ID` links are expanded to
`https://www.youtube.com/watch?v=ID`.

The right-hand side of a **pair** must be a Spotify track, because a pair
describes exactly one track.

Query strings and fragments are removed, so tracking parameters such as `si=`
do not create duplicates. Duplicate normalized lines are downloaded only once;
a pair and a bare Spotify track link are different downloads even when they
name the same track, because they resolve the audio differently. If any
non-comment line is invalid, the command reports up to ten invalid lines and
stops before starting spotDL.

### Options

| Option | Meaning |
|---|---|
| `-o, --output <DIR>` | Download directory; default `~/Documents/Music` |
| `--spotdl <PROGRAM>` | spotDL executable or path; default `spotdl` |
| `--official-api` | Explicitly use Spotify's official Web API |
| `--auth-token <TOKEN>` | Official mode: use this short-lived access token |
| `--token-file <FILE>` | Official mode: read the access token from a file |
| `--non-interactive` | Never prompt for a token, Deno, or a replacement token |
| `--auto-download-deno` | Allow spotDL to install Deno when required |
| `--no-copyright` | Skip the iTunes copyright lookup |
| `--language <LANGUAGE>` | Fallback language for `TLAN`, by name or code; default `English` |
| `--max-attempts <N>` | Network attempts per line; default `3`, minimum `1` |
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

Each line is passed to spotDL on its own, as a single quoted argument, so a
failure can be attributed to the input line it came from:

```bash
spotdl download "LINE" \
    --overwrite force --format mp3 --lyrics synced --generate-lrc
```

Those four options are fixed and are not configurable:

| Option | Effect |
|---|---|
| `--overwrite force` | Every listed line is downloaded again, replacing any existing file |
| `--format mp3` | Output is always MP3, so an ID3 tag can always be written |
| `--lyrics synced` | spotDL prefers a provider that has time-synced lyrics |
| `--generate-lrc` | spotDL writes the timed lyrics next to the audio as `.lrc` |

The command also passes `--print-errors`, `--max-retries 0` (retries are
handled here instead), and spotDL's output template:

```text
{album-artist} || {album}/{artists} - {title} [{track-id}].{output-ext}
```

Because `--overwrite force` is used, rerunning an input file re-downloads
every line in it rather than skipping finished files.

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

### Rating, encoder, copyright, and language frames

A freshly downloaded file carries two frames that describe the download rather
than the music: `POPM`, the popularimeter spotDL fills from Spotify's
popularity score — the frame most taggers display as a rating — and `TSSE`, the
encoder-settings string FFmpeg leaves behind (something like `Lavf58.76.100`).
spotDL also leaves `TCOP` empty, because a single-track fetch does not carry
the album's copyright.

After each download this program rewrites four frames:

| Frame | What happens |
|---|---|
| `POPM` | Removed outright |
| `TSSE` | Removed outright |
| `TCOP` | Set to the copyright message looked up on iTunes |
| `TLAN` | Set to the name of the language detected from the lyrics, or `--language` |

Removal happens after spotDL is finished with the file, so it catches whatever
FFmpeg wrote during the transcode. Files downloaded before this existed can be
cleaned up with the `delete` command:

```bash
music-tag-transfer delete "/path/to/music" "[Encoding Settings]"
```

`TSRC` is deliberately **not** touched. spotDL already fills the ISRC from its
own metadata, and iTunes publishes no ISRCs, so whatever spotDL wrote is left
in place.

#### Copyright

The copyright comes from the [iTunes Search
API](https://performance-partners.apple.com/search-api), which needs no
account, key, or token — nothing has to be registered or exported to use it.

Lookups are by album rather than by track, because the copyright belongs to the
album and because the downloads are already grouped that way. The album artist
and album name in the tag spotDL just wrote are the search terms, and results
are cached, so an album's worth of tracks costs one request.

A copyright is stored only when a result matches **both** the album artist and
the album name, because a wrong copyright is worse than none. The comparison
ignores case and punctuation and allows one name to extend the other, so
`Random Access Memories` still matches a listing called `Random Access Memories
(Deluxe Edition)`. Anything less similar is treated as a different release and
the frame is left alone.

If the lookup fails outright — the API throttling, or no network — that is
reported as a warning and **the rest of the tag is still written**. The rating
still goes, the lyrics are still embedded, and only `TCOP` is left untouched.
`--no-copyright` skips the lookup altogether.

Two things to know about the API: it is throttled per IP, so requests are
spaced out and a throttled response is retried a few times before giving up;
and it is queried against the US storefront, whose catalogue is the most
complete. The `℗` line is the label's and rarely differs between storefronts.

#### Language

Neither Spotify nor iTunes exposes a language for tracks, so `TLAN` cannot be
looked up. The synced lyrics are the only evidence available, and the language
is detected from their text, with the timestamps and any `[ar:...]` metadata
stripped first so they cannot mislead the detector.

`TLAN` is written as a readable **name** — `English`, `Chinese`, `Korean`,
`Spanish` — because that is what a tagger displays. ID3v2.3 specifies an
ISO-639-2 code in this frame, so a strict reader will see a value it does not
recognise; the three-byte language field inside the `USLT` frame, whose width
the frame format fixes, still carries the code.

- A confident detection sets `TLAN`, and the lyrics frame's own language field
  is set to the matching code.
- Too little text, or an unreliable guess, falls back to `--language`
  (default `English`) rather than recording something invented. Two or more
  non-blank lyric lines are required before a guess is even attempted.
- A track with no `.lrc` file has nothing to detect from, so it gets
  `--language` too.

`--language` takes either form, so `--language Korean`, `--language korean`,
and `--language kor` are the same request. An unrecognised value is refused
before anything is downloaded.

The detector reports ISO-639-3, which for every individual language it knows is
the same as the ISO-639-2/T code ID3v2.3 asks for; the two macrolanguage cases
are mapped to their collective code and name (`cmn` becomes `zho` and
`Chinese`, `pes` becomes `fas` and `Persian`).

### Synced lyrics in the ordinary `USLT` frame

Every download asks spotDL for time-synced lyrics and for the `.lrc` file that
carries them — `--lyrics synced --generate-lrc`, on every line of the input
file, with no way to turn it off.

After each successful download that `.lrc` file is pasted into the tag, in the
same write as the rating and copyright changes above:

1. The `.lrc` file sitting beside the audio is read and its text is trimmed.
2. That text is written to the ID3v2.3 `USLT` lyrics frame **verbatim**,
   `[mm:ss.xx]` timestamps and all. Any previous `USLT` frame is replaced.
3. Every `SYLT` synchronised-lyrics frame is removed, including the one spotDL
   writes itself whenever its lyrics arrive in LRC format.
4. The file is read back from disk. Only if the stored `USLT` frame matches the
   `.lrc` file, and no `SYLT` frame survives, is the `.lrc` file deleted.

`USLT` is the frame players actually read; `SYLT` is the one most of them
ignore. Players that do understand timed lyrics parse the timestamps out of the
`USLT` text, so keeping the `.lrc` text as it came is what makes the lyrics show
up in both kinds of player.

Only the language detector sees the lyrics without their timestamps — the
`[mm:ss.xx]` prefixes and any `[ar:...]` metadata header would only mislead it.

If any of those steps fails, the `.lrc` file is **kept** so its lyrics are
never the thing that gets lost, nothing is written to the audio file, and the
line it belongs to is reported as failed so it appears in the retry list. An
empty `.lrc` file is the only content that counts as a failure; an untimed one
is pasted like any other. A line for which spotDL generated no `.lrc` file is
not a failure either: the rating and copyright changes are still applied, and
there were simply no synced lyrics to paste.

### How downloaded files are found again

Before each line runs, the output directory's music files are recorded; when
spotDL finishes, whatever is new or was rewritten is what that line produced.
That works for every form, including the lines that name no single Spotify
track, so an album or playlist line gets all of its songs tagged. For a line
that does name a single track, the Spotify track ID in the forced
`[{track-id}]` output template is kept as a fallback.

### Download execution and output files

The output directory receives:

| File | Contents |
|---|---|
| `output.txt` | Failed and unattempted lines, ready to retry |
| `music-tag-transfer-download-failures.txt` | Detailed attempted failures and the stop reason |

`output.txt` is always written; it is empty after a completely successful
run. Because it holds the same line syntax as the input, it can be passed
straight back to `download`. The detailed failure report is written only when
at least one attempted line fails or causes the batch to stop.

Ordinary network/service failures use exponential backoff up to
`--max-attempts`. A short Spotify `Retry-After` delay is respected once.
Application quota errors, repeated rate limits, or delays above
`--max-rate-limit-wait` stop the batch and preserve the current and remaining
lines rather than rotating tokens or sleeping for a long time.

If spotDL reports that Deno is required:

- an interactive run asks before invoking `spotdl --download-deno`;
- `--auto-download-deno` approves that setup automatically;
- `--non-interactive` without automatic setup stops and preserves the lines.

### The token prompt

Before the first download, an interactive run asks for a Spotify access token:

```text
Spotify metadata source
  A token switches this run to Spotify's official Web API, the only source for
  the ISRC (TSRC) frame; spotDL's token-free client always reports an empty one.
  A developer-app token needs the app owner to have Spotify Premium; a token
  copied from the open.spotify.com web player does not, but expires within the
  hour, and this command will ask again when it does.
  Never enter your password or Client Secret.
Paste an access token, or press Enter to download token-free:
```

Pressing Enter is a valid answer and keeps the default token-free mode, so the
prompt is an offer rather than a requirement. Pasting a token switches the run
to Spotify's official Web API, because a token is only ever passed to spotDL
alongside `--use-official-api`. A malformed token is reported and asked for
again, up to three times.

The token may be pasted in any of the forms devtools hands you — a bare token,
`Bearer ...`, or a quoted `const token = '...'` assignment.

The prompt is skipped, leaving behaviour exactly as it was, when:

- `--auth-token`, `--token-file`, or `SPOTIFY_AUTH_TOKEN` already supplied one;
- `--non-interactive` was passed;
- stdin is not a terminal, as in a cron job or a pipeline.

Which token you can use depends on where it came from. A token issued to a
developer-mode Spotify app requires the **app owner to hold an active Premium
subscription** — since Spotify's February 2026 developer changes, such an app
owned by a free account answers every request with `403 Active premium
subscription required for the owner of the app`, which stops the batch. A token
copied from the `open.spotify.com` web player carries no such requirement, but
expires within the hour; when it does, an interactive run asks for a
replacement.

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
  music-tag-transfer download --official-api links.txt
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

## Export ID3 frames to CSV

The export command only reads music files. It walks a folder recursively and
writes every ID3 frame it finds into a single CSV file, which is the quickest
way to see what a library actually contains before deleting or rewriting
anything.

### Syntax

```text
music-tag-transfer export <FOLDER> [OUTPUT_CSV] [--overwrite]
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

### CSV layout

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

## Exit status

| Status | Meaning |
|---|---|
| `0` | Help/version succeeded, or the requested operation completed without processing errors |
| `1` | Invalid arguments, setup/input/output failure, download failures, or per-file metadata errors |

Any failed or unattempted line results in status 1 and is preserved in
`output.txt`. A line whose audio downloaded but whose metadata could not be
finished counts as failed, and its `.lrc` file is kept.

## Troubleshooting

### `cannot run 'spotdl'`

Confirm `spotdl --version` works in the same shell. Otherwise pass
`--spotdl /full/path/to/spotdl` or set `SPOTDL_PROGRAM`.

### spotDL configuration blocks token-free mode

Inspect `~/.config/spotdl/config.json` or `~/.spotdl/config.json`. Disable
`load_config`, or clear the official-API settings named in the error. Use
`--official-api` only when that mode is intentional.

### No copyright was written

Either iTunes had no album matching both the album artist and the album name in
the tag, or the lookup failed and said so. Both are reported during the run,
and neither stops anything else in the tag from being written. Reissues and
regional editions are the usual cause of a miss; `--no-copyright` turns the
lookup off if you would rather not see it.

### A download needs Deno

Run `spotdl --download-deno` manually, install Deno system-wide, or rerun the
download command with `--auto-download-deno`.

### A `.lrc` file was left behind

Its lyrics could not be embedded, and the reason is printed during the run and
recorded in `music-tag-transfer-download-failures.txt`. The usual cause is a
`.lrc` file that is empty, or an audio file whose tag could not be written.
Retry the line from `output.txt`, or delete the `.lrc` file yourself if the
track simply has no lyrics worth keeping.

### A player shows no lyrics

Confirm the frame is there, for example with mutagen:

```bash
python3 -c "from mutagen.id3 import ID3; print(ID3('track.mp3').getall('USLT'))"
```

The text should be the `.lrc` file as it was downloaded, timestamps included. If
it is there and the player still shows nothing, the player is not reading `USLT`
at all. If the lyrics show but do not scroll with the music, the player does not
parse LRC timestamps out of `USLT`; nothing in the tag can fix that.

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
use music_tag_transfer::finalize;

fn main() {
    let report = finalize(
        Path::new("/path/to/Artist - Song [id].mp3"),
        Some("\u{2117} 2001 Daft Life Limited"),
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

The recursive frame export is available too:

```rust
use std::path::Path;
use music_tag_transfer::{default_csv_path, export_frames_to_csv};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = Path::new("/path/to/music");
    let report = export_frames_to_csv(folder, &default_csv_path(folder), true)?;
    println!(
        "Exported {} frame(s) across {} column(s)",
        report.frames_exported, report.frame_columns
    );
    Ok(())
}
```

`DeleteReport`, `ExportReport`, `ExportError`, `MetadataReport`, `FileError`, and `TagSpec` are also exported,
along with `album_of` for reading the album key a lookup searches with. The
`cli`, `download`, `itunes`, and `metadata` modules expose the executable's
command configuration and entry points.

## Development

The GitHub Actions workflow runs the same three checks used locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Run all three before merging behavior or documentation changes.
