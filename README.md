# music-organiser

`music-organiser` is a Rust command-line project for preparing and maintaining
a local music library. One executable provides four related workflows:

| Command | Purpose |
|---|---|
| `download` | Download Spotify and/or YouTube Music links through spotDL and rewrite their ID3 metadata |
| `delete` | Remove selected ID3 frames recursively |
| `export` | Write every ID3 frame found recursively into one CSV file |
| `copyright` | Look the `TCOP` copyright message up again for music already on disk, in the catalogue of your choice |

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
- [Refresh the copyright message](#refresh-the-copyright-message)
- [Exit status](#exit-status)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Requirements

All commands require a stable Rust toolchain with Rust 2024 edition support.
The individual workflows have these additional requirements:

| Workflow | Additional requirements |
|---|---|
| `download` | spotDL 4.5.0 or newer and FFmpeg |
| Copyright | Nothing for iTunes or MusicBrainz; a personal access token for Discogs, an OAuth access token for Spotify |
| Synced lyrics and language | Nothing extra; both are handled by this program |
| Some YouTube downloads | Deno, installed system-wide or through `spotdl --download-deno` |
| `delete` | Read/write access to the relevant music folder |
| `copyright` | Read/write access to the music folder, and internet access to the chosen catalogue |
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
music-tag-transfer copyright <FOLDER> [--source NAMES] [--only-missing] [--dry-run]
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

### Rating, encoder, date, copyright, and language frames

A freshly downloaded file carries two frames that describe the download rather
than the music: `POPM`, the popularimeter spotDL fills from Spotify's
popularity score — the frame most taggers display as a rating — and `TSSE`, the
encoder-settings string FFmpeg leaves behind (something like `Lavf58.76.100`).
spotDL also leaves `TCOP` empty, because a single-track fetch does not carry
the album's copyright.

spotDL writes the release date **twice**: the whole date in `TDRC`
(`2020-05-20`) and the year again in `TYER` (`2020`). `TDRC` is ID3v2.4's
single timestamp frame; `TYER` is the ID3v2.3 frame it replaced, and spotDL
writes both so that readers of either version find a date. Taggers show the
pair as two competing date fields.

After each download this program rewrites five frames:

| Frame | What happens |
|---|---|
| `POPM` | Removed outright |
| `TSSE` | Removed outright |
| `TYER` | Removed outright, leaving the complete `TDRC` date |
| `TCOP` | Set to the copyright message looked up on iTunes |
| `TLAN` | Set to the name of the language detected from the lyrics, or `--language` |

Dropping `TYER` leaves the date in a v2.4 frame inside a v2.3 tag, which
Mp3tag, foobar2000, MusicBee, VLC, Plex, and Jellyfin all read. Strict v2.3
readers — Windows Explorer's Year column, Windows Media Player, and some older
car head units and portable players — look only at `TYER` and will show no
year. Keep it by removing `"TYER"` from `STRIPPED_FRAMES` in `src/metadata.rs`
if your players need it.

Removal happens after spotDL is finished with the file, so it catches whatever
FFmpeg wrote during the transcode. Files downloaded before this existed can be
cleaned up with the `delete` command:

```bash
music-tag-transfer delete "/path/to/music" "[Encoding Settings, Year]"
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

### The track ID is removed from the file name

spotDL is asked for the `[{track-id}]` suffix only so that a file can be tied
back to the line that asked for it. That job is done once the tag has been
rewritten, so each file is then renamed without it:

```text
i-dle - Luv U [2Mvdcda3pVMDASD7oZWPr4].mp3   →   i-dle - Luv U.mp3
```

Only a trailing bracketed run of 16 to 32 letters and digits is removed, so a
bracket that belongs to the title survives: `Artist - Song [Live]
[2Mvdcda3pVMDASD7oZWPr4].mp3` becomes `Artist - Song [Live].mp3`. A `.lrc`
file still sitting next to the audio — which happens only when the metadata
step could not finish — is renamed with it.

Because `--overwrite force` means a rerun downloads every line again, a file
already using the trimmed name is an earlier download of the same track in the
same album folder, and it is replaced. The run prints a line naming any file
replaced this way, and the closing summary counts them:

```text
Names: dropped the track ID from 12 file name(s); 1 replaced an earlier download.
```

A rename that fails is reported and nothing else is affected: the audio and its
tag are already finished, so only the name is left untidy.

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

## Refresh the copyright message

The `download` command fills `TCOP` from the iTunes Search API as each file
arrives. This command does the same for music that is already on disk, which
is the way to fix a library downloaded before the lookup existed, or one whose
lookups were skipped with `--no-copyright`.

### Syntax

```text
music-tag-transfer copyright <FOLDER> [--source NAME] [--token-file PATH] [--only-missing] [--dry-run]
```

```bash
music-tag-transfer copyright "/path/to/music" --dry-run
music-tag-transfer copyright "/path/to/music"
music-tag-transfer copyright "/path/to/music" --source musicbrainz --only-missing
```

| Flag | Effect |
|---|---|
| `--source NAME[,NAME...]` | Which catalogue to ask: `itunes`, `musicbrainz`, `discogs`, or `spotify`. Several names build a fallback chain, tried in order. Omit it and an interactive run asks |
| `--max-wait SECONDS` | Longest rate-limit pause to sit through before giving up on a source. Default 60 |
| `--token-file PATH` | Read the chosen source's token, or MusicBrainz's contact address, from a file |
| `--token VALUE` | The same, given inline. Every process on the machine can read a command line, so prefer `--token-file` or the environment |
| `--only-missing` | Leave files that already carry a copyright message alone, and never look their album up |
| `--dry-run` | Report the same counts without writing any file |
| `--csv PATH` | Write a before-and-after row for every file visited |
| `--overwrite` | Let `--csv` replace an existing report |

### Choosing where the copyright comes from

The four catalogues disagree, on wording and on coverage alike, so the source
is a question the command asks rather than a setting buried in a config file.
Run it without `--source` at a terminal and it offers the choice:

```text
Where should the copyright message come from?
  1) iTunes       no account needed; the ℗ line as Apple's store publishes it
  2) MusicBrainz  no account needed; built from the release's copyright and
                  phonographic-copyright label relationships
  3) Discogs      needs a personal access token; built from the release's
                  Copyright (c) and Phonographic Copyright (p) credits
  4) Spotify      needs an access token; the album's own copyright lines,
                  taken as written

Choose 1-4 or a name, Enter for iTunes:
```

Answer with the number or the name. `--source` answers it in advance, and a
run with no terminal — a script, a cron job, a pipe — uses iTunes rather than
stopping for an answer nobody is there to give.

| Source | Account | Where the message comes from |
|---|---|---|
| `itunes` | None | The `copyright` field of the matching album in the iTunes Search API, as Apple publishes it |
| `musicbrainz` | None | Assembled from the release's `phonographic copyright` label relationship, or its `copyright` one, plus the year that relationship began |
| `discogs` | Personal access token | Assembled from the release's `Phonographic Copyright (p)` credit, or its `Copyright (c)` one, plus the release year |
| `spotify` | OAuth access token | The album's own `copyrights` entry, `P` for preference and `C` otherwise, gaining the ℗ or © symbol only if Spotify left it off |

Because two of the four assemble the line rather than quote it, the same album
can come out worded differently depending on who was asked. Pick one source for
a library and stay with it, or use `--only-missing` so an established message is
never rewritten in another catalogue's style.

### Surviving a rate limit

Catalogues run out of patience, and a library is hundreds of albums. Spotify in
particular measures its limit over a rolling window and answers a breach with a
`Retry-After` of **hours**, not seconds.

Two things follow from that, and both are handled:

**A spent source is dropped, not retried.** A wait longer than `--max-wait`, a
run of throttled responses, or a rejected token all mean the source has stopped
answering — asking it again does not help and, with a rate limit, makes it
worse. That source is retired for the rest of the run and named in the summary.

**A fallback chain keeps the run alive.** Give `--source` several names and each
album is offered to them in turn, so one catalogue running dry costs you that
catalogue, not the run:

```bash
music-tag-transfer copyright "/path/to/music" --source spotify,musicbrainz,itunes
```

Spotify answers until its limit is spent, then MusicBrainz takes every
remaining album, with iTunes behind it. The summary shows who supplied what:

```text
Looking copyrights up in Spotify, then MusicBrainz, then iTunes.
Spotify is rate limited for another 5h 21m, far past the 60-second limit this run will wait.
  Dropping Spotify for the rest of this run.
  Spotify: 47 album(s), then stopped answering
  MusicBrainz: 231 album(s)
  iTunes: 12 album(s)
```

If every source is spent the scan **stops** rather than turning each remaining
album into an identical failure. Files already written stay written, and
`--only-missing` resumes from there:

```bash
music-tag-transfer copyright "/path/to/music" --source musicbrainz --only-missing
```

`--max-wait 900` will sit through a fifteen-minute pause if you would rather
wait than switch source. It will not sit through five hours, and neither
should you.

### Tokens and contact addresses

Discogs and Spotify will not answer without a token. Each is read from
`--token`, then `--token-file`, then the environment, and only then by asking:

| Source | Variable | Where to get one |
|---|---|---|
| `discogs` | `DISCOGS_TOKEN` | <https://www.discogs.com/settings/developers> → "Generate new token". A personal access token, not your password |
| `spotify` | `SPOTIFY_ACCESS_TOKEN` | The same kind of token `download` asks for. One copied from the open.spotify.com web player works but expires within the hour |
| `musicbrainz` | `MUSICBRAINZ_CONTACT` | Not a token: an email address or URL. MusicBrainz asks every application to identify itself so it can contact whoever runs one that misbehaves rather than blocking it |

```bash
export DISCOGS_TOKEN="..."
music-tag-transfer copyright "/path/to/music" --source discogs
```

A run with no terminal and no token fails immediately, naming the variable to
set, rather than scanning the whole library only to find it cannot ask anything.

### Seeing what a run would do, before it does it

`--dry-run` prints counts, which tells you how much would change but not
*what*. Pair it with `--csv` and you get the detail, one row per file:

```bash
music-tag-transfer copyright "/path/to/music" --dry-run --csv changes.csv
```

| Column | What it holds |
|---|---|
| `File` | The path, relative to the folder scanned |
| `Album Artist`, `Album` | What the lookup searched with |
| `Copyright Before` | The message the file carries now |
| `Copyright After` | What would be written, or empty when nothing was found |
| `Outcome` | `written`, `unchanged`, `skipped`, `no match`, `lookup failed`, `no album in tag`, `no ID3 tag`, or `error` |
| `Source` | Which catalogue supplied the message |
| `Note` | Why, for the outcomes that have a reason |

Every visited file gets a row, including the ones nothing happened to — a
`no match` row still shows the message it kept, so the report is a complete
account rather than a list of changes. `Copyright After` is left empty
whenever nothing would be written, which is what makes a kept message
obvious at a glance.

The same flag works without `--dry-run`, where the file becomes a record of
what was written rather than a preview. It refuses to replace an existing
report unless `--overwrite` is given, the same rule `export` follows.

With a fallback chain the `Source` column is worth reading on its own: it
shows which catalogue actually answered for each album.

### A file is only written when a copyright was found

Nothing is ever cleared — an existing `TCOP` survives every kind of miss,
including a rate limit that ends the run. A file is left exactly as it was
when:

- the chosen source has no confident match for its album;
- the lookup fails, for instance with no network;
- its tag names no album artist and album to search with;
- it carries no ID3 tag at all;
- the message found is the one it already has.

Only a release that matches **both** the album artist and the album name in the
tag is used, whichever source answered, so a wrong copyright is never written.

The comparison has to absorb the ways catalogues disagree without ever merging
two different records. It ignores case, punctuation, and accents, so a tag
reading `Beyonce` finds Spotify's `Beyoncé`; it reads `&` as `and`, because
one catalogue prints `Earth, Wind & Fire` and the next spells it out; and it
ignores a leading `The`.

An edition suffix is ignored **only when it says it is one** — bracketed, as in
`(Deluxe Edition)`, `[Platinum Edition]`, or the `(2)` Discogs appends to a
disambiguated artist; or spelled out of a small vocabulary of edition words, as
in Spotify's `B'Day Deluxe Edition`. Anything else is a different record:
`Vol. 1` and `Pt. 2` distinguish releases rather than editions, and a suffix
naming a different recording — `(Live)`, `(Acoustic Version)`, `(Sped Up)`,
`(Radio Edit)` — is never ignorable, because those carry their own copyright.

Where several candidates match, the closest wins rather than the first listed,
so a search for `Discovery` prefers the plain release over a deluxe edition
that happened to come back ahead of it.

### The rest of the tag is evidence too

A name is a weak key on its own, so the lookup uses everything else the tag
knows about the release:

| Frame | What it settles | Used by |
|---|---|---|
| `TSRC` | The ISRC, a globally unique identifier for the **recording** | Spotify and MusicBrainz search by it directly |
| `TRCK` | The album's track total, from the `12` in `5/12` | all four, to rank candidates |
| `TDRC` | The release year | all four, to rank candidates |

The ISRC is the strong one: where a source can search by it, the release is
found by identity instead of by name, and a whole class of error — the wrong
artist's album entirely — becomes impossible. It identifies a *recording*
rather than a release, though, since the same recording sits on the album, the
single and any number of compilations, so the album name still chooses between
the releases it turns up. A malformed ISRC is ignored rather than searched
with, and an ISRC that finds nothing falls back to the name.

spotDL fills `TSRC` **only when it used the official Spotify API**, so a
library downloaded token-free has none. Check yours with `export` and look at
the `ISRC (TSRC)` column.

The track count is the underrated one, and needs no account at all:
`Discovery` has fourteen tracks and `Discovery (Deluxe Edition)` has twenty,
which separates them far more reliably than any comparison of their names.

This evidence **ranks** candidates; it does not veto them. Agreement beats
missing evidence, which beats conflict, so a release whose year and track count
both match wins — but the only candidate there is still supplies its copyright
even when the year disagrees, because catalogues genuinely disagree about
release dates. The one exception is Discogs, which opens candidates in the
order its search returned them and so cannot reorder them; there a
contradicting track count skips the release and moves to the next.

None of it costs an extra request. The evidence is taken from the first file
seen for an album and reused for the rest, so a lookup is still one search per
album however many tracks it has.

Misses and failures are printed as they happen and counted in the summary, and
the summary names the other sources, since an album missing from one catalogue
is often complete in another.

### One request per album

The album artist and album name in each tag are the search key, and the answer
is remembered for the rest of the run, so a 12-track album costs one request
rather than twelve. Throttling responses are retried with a backoff that honours
`Retry-After`, and requests are spaced to stay inside each catalogue's published
limit:

| Source | Spacing | Requests per album |
|---|---|---|
| `itunes` | 200 ms | One |
| `spotify` | 500 ms — deliberately slower than the API demands, because two requests per album at 200 ms earned a five-hour ban on a real library | Two: the search answers with a simplified album that carries no copyrights, so the match is fetched in full |
| `musicbrainz` | 1.1 s, its published one-per-second limit | Up to four: the search, then up to three matching releases, since only some pressings carry the relationships |
| `discogs` | 1.1 s, within its 60-per-minute limit | Up to four: a search result carries neither the credits nor a reliably split artist and title, so candidates have to be opened to be judged |

A library of 500 albums therefore takes roughly two minutes on iTunes, about
ten on Spotify, and up to half an hour on MusicBrainz or Discogs. For bulk
work prefer MusicBrainz or iTunes and keep Spotify for filling gaps with
`--only-missing`.

Every changed file is written as ID3v2.3 through the same copy-edit-rename that
the other commands use.

### Seeing what changed

`export` pairs well with this: dump the library, look at the
`Copyright (TCOP)` column, run the refresh, and dump it again.

```bash
music-tag-transfer export "/path/to/music" before.csv
music-tag-transfer copyright "/path/to/music"
music-tag-transfer export "/path/to/music" after.csv
```

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

Either the source had no release matching both the album artist and the album
name in the tag, or the lookup failed and said so. Both are reported during the run,
and neither stops anything else in the tag from being written. Reissues and
regional editions are the usual cause of a miss; `--no-copyright` turns the
lookup off if you would rather not see it.

Run `music-tag-transfer copyright <FOLDER>` later to try those albums again;
files whose lookup missed or failed are left untouched, so nothing is lost by
retrying. Coverage differs between the four catalogues, so trying another
source is usually more productive than retrying the same one:

```bash
music-tag-transfer copyright "/path/to/music" --source musicbrainz --only-missing
```

`--only-missing` makes this safe to chain: each run only visits the files the
previous ones could not fill in.

### A token was rejected or has expired

`Spotify rejected the token (HTTP 401)` and its Discogs equivalent mean the
token itself, not the album. A token does not repair itself, so the source is
retired immediately rather than spending the rest of the library discovering
the same thing. Spotify web-player tokens expire within the hour, so a long run
can outlive one; fetch a fresh token and run again with `--only-missing` to pick
up where it stopped.

### The run stopped part way through

Every source it had was spent — a rate limit, an expired token, or a catalogue
that would not stop throttling. This is deliberate: the alternative is several
hundred identical failures and, on a rate limit, a longer ban. Nothing was lost.
Re-run with `--only-missing`, and consider a fallback chain so the next run
survives one catalogue giving up.

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

The copyright refresh is available as a library call, taking anything that
implements `CopyrightLookup` so a caller can supply its own source:

```rust
use std::path::Path;
use music_tag_transfer::{
    refresh_copyrights,
    sources::{DEFAULT_MAX_WAIT, Source},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lookup = Source::MusicBrainz.open(None, DEFAULT_MAX_WAIT)?;
    let report = refresh_copyrights(Path::new("/path/to/music"), lookup.as_mut(), false, true)?;
    println!("Would write {} file(s)", report.files_updated);
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

`DeleteReport`, `ExportReport`, `ExportError`, `CopyrightReport`, `CopyrightError`,
`MetadataReport`, `FileError`, and `TagSpec` are also exported,
along with `album_of` for reading the album key a lookup searches with. The
`cli`, `download`, `sources`, and `metadata` modules expose the executable's
command configuration and entry points.

## Development

The GitHub Actions workflow runs the same three checks used locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Run all three before merging behavior or documentation changes.
