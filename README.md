# music-organiser

A Rust CLI that downloads Spotify and YouTube Music links through spotDL,
cleans and embeds synced lyrics, standardises ID3 tags, and maintains a music
library on disk.

| Command | Purpose |
|---|---|
| [`download`](docs/download.md) | Download Spotify and/or YouTube Music links through spotDL and rewrite their ID3 metadata |
| [`resolve`](docs/resolve.md) | Pin Spotify links to their YouTube Music track through Odesli, so `download` does not have to search. **Needs an Odesli API key** — anonymous access has been withdrawn |
| [`delete`](docs/delete.md) | Remove selected ID3 frames recursively |
| [`export`](docs/export.md) | Write every ID3 frame found recursively into one CSV file |
| [`copyright`](docs/copyright.md) | Look the `TCOP` copyright message up again for music already on disk |

The Cargo package and executable are named `music-tag-transfer`. Downloaded
audio comes from spotDL's configured providers, not from Spotify. Only
download material you are permitted to keep.

## Requirements

A stable Rust toolchain with Rust 2024 edition support, plus:

| For | You need |
|---|---|
| `download` | spotDL 4.5.0 or newer, and FFmpeg |
| Some YouTube downloads | Deno, system-wide or via `spotdl --download-deno` |
| `copyright` with Discogs or Spotify | An access token for that source |
| Everything else | Nothing extra |

Check the external programs before downloading:

```bash
spotdl --version
ffmpeg -version
```

Deno is optional until spotDL reports that a particular download needs it;
`download` then offers to install it, or does so with `--auto-download-deno`.

## Installation

```bash
git clone https://github.com/Raina3266/music_organiser.git
cd music_organiser
cargo build --release
```

The binary is `target/release/music-tag-transfer`. Install it onto your `PATH`
with `cargo install --path .`, or run any example below with `cargo run --`
instead.

## Quick start

Paste the links you want into a text file. Spotify links, YouTube Music links,
and pairs of the two (in either order) can be mixed freely:

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

```bash
music-tag-transfer download links.txt --output ./music
```

Every line is downloaded as MP3 with synchronised lyrics, and each ID3v2.3 tag
is limited to the 15 supported metadata types. Synced lyrics come from LRCLIB,
matched against the length of the downloaded file so the timings belong to that
recording rather than to some other cut of the song; spotDL's own `.lrc` is the
fallback. No credentials are needed:
copyright comes from iTunes with a MusicBrainz fallback, and a missing ISRC
from MusicBrainz with a Deezer fallback. All three are open without an account.
Setting `DISCOGS_TOKEN` adds Discogs as a last copyright fallback, for the
albums neither iTunes nor MusicBrainz knows.

## Usage

```text
music-tag-transfer download  [OPTIONS] <INPUT_FILE>
music-tag-transfer resolve   <INPUT_FILE> [OUTPUT_FILE] [--api-key KEY] [--country XX]
music-tag-transfer delete    <FOLDER> "[Tag Name, Other Tag]" [--dry-run]
music-tag-transfer export    <FOLDER> [OUTPUT_CSV] [--overwrite]
music-tag-transfer copyright <FOLDER> [--source NAMES] [--only-missing] [--dry-run]
```

`-h`/`--help` prints the top-level help and `-V`/`--version` the version;
`download --help` prints command-specific help. Paths containing spaces must
be quoted. `download` expands `~` in its input, output, and token-file paths;
other commands receive paths exactly as the shell supplies them.

### download

Reads one link per line and downloads each through spotDL, then rewrites the
tag. See [docs/download.md](docs/download.md) for the input format, the
metadata whitelist, lyric handling, and the Spotify token modes.

For a bare Spotify link, automatic audio matching is restricted to verified
YouTube Music results. If no verified result exists, the line is kept in the
retry list rather than accepting an unverified live or user upload. An
exact-source pair still provides the strongest guarantee because it pins the
YouTube recording directly.

| Option | Meaning |
|---|---|
| `-o, --output <DIR>` | Download directory; default `~/Documents/Music` |
| `--spotdl <PROGRAM>` | spotDL executable or path; default `spotdl` |
| `--token-free` | Download without a Spotify token, skipping the startup question |
| `--official-api` | Use Spotify's official Web API |
| `--auth-token <TOKEN>` | Use this short-lived token, selecting official mode |
| `--token-file <FILE>` | Read a token from a file, selecting official mode |
| `--non-interactive` | Never prompt for Spotify mode, Deno, or a token |
| `--auto-download-deno` | Allow spotDL to install Deno when required |
| `--no-copyright` | Skip the iTunes, MusicBrainz, and Discogs copyright lookups |
| `--no-language-lookup` | Skip the MusicBrainz language lookup and read the lyrics instead |
| `--no-lyrics-lookup` | Skip LRCLIB and keep spotDL's own `.lrc` |
| `--language <LANGUAGE>` | Fallback language for `TLAN`; default `English` |
| `--max-attempts <N>` | Network attempts per line; default `3` |
| `--max-rate-limit-wait <SECS>` | Longest accepted Retry-After delay; default `300` |

`SPOTDL_PROGRAM` chooses a spotDL executable without repeating `--spotdl`.
`DISCOGS_TOKEN` enables the Discogs copyright fallback, and
`MUSICBRAINZ_CONTACT` identifies the run to MusicBrainz.

### resolve

Rewrites bare Spotify tracks as exact-source pairs by asking Odesli which
YouTube Music track each one is, so spotDL downloads that recording instead of
searching for it. Input and output are both `download` input files:

```bash
music-tag-transfer resolve links.txt          # writes links-resolved.txt
music-tag-transfer download links-resolved.txt --output ./music
```

A resolved track is written `SPOTIFY_TRACK_URL|YOUTUBE_MUSIC_URL`, the Spotify
link first, so the file lines up with the list of Spotify links it came from:

```text
https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW|https://music.youtube.com/watch?v=dQw4w9WgXcQ
```

| Option | Meaning |
|---|---|
| `OUTPUT_FILE` | Where to write; defaults to the input name with `-resolved` added |
| `--overwrite` | Allow the output file to be replaced |
| `--api-key KEY` | An Odesli API key; also read from `ODESLI_API_KEY` |
| `--api-key-file FILE` | Read the key from a file instead |
| `--country XX` | Storefront to ask about, as a two-letter code; default `US` |
| `--max-attempts N` | Attempts per request before giving up on it |
| `--max-wait SECONDS` | Longest rate-limit wait to sit through |
| `--max-throttle-retries N` | Times to wait out throttling for one track |

Odesli has withdrawn anonymous access to this API, so without a key the run
stops at the first track and leaves every line bare; `download` still handles
those, since spotDL searches for them as before.

Albums, playlists, YouTube links, and tracks Odesli cannot place are all copied
through unchanged, so the output is always a drop-in replacement for the input;
a pair the input already carried is copied through in that same Spotify-first
order. With a key the run starts at two requests a second and lets Odesli's
throttling correct it. See [docs/resolve.md](docs/resolve.md).

### delete

Removes the named ID3 frames from every music file under a folder. The
brackets are part of the argument, and the whole list should be quoted:

```bash
music-tag-transfer delete "/path/to/music" "[Encoded-by, Album Artist]" --dry-run
```

Tag names are case-sensitive. `--dry-run` reports the same counts without
writing. See [docs/delete.md](docs/delete.md) for the supported names.

### export

Writes every ID3 frame under a folder into one CSV:

```bash
music-tag-transfer export "/path/to/music"              # id3-frames.csv in that folder
music-tag-transfer export "/path/to/music" ~/frames.csv --overwrite
```

An existing destination is never replaced without `--overwrite`. See
[docs/export.md](docs/export.md) for the column layout.

### copyright

Looks the `TCOP` message up again for music already on disk — the way to fix a
library downloaded before the lookup existed, or one downloaded with
`--no-copyright`.

```bash
music-tag-transfer copyright "/path/to/music" --dry-run
music-tag-transfer copyright "/path/to/music" --source musicbrainz --only-missing
```

| Option | Meaning |
|---|---|
| `--source NAME[,NAME...]` | `itunes`, `musicbrainz`, `discogs`, or `spotify`. Several names build a fallback chain. Omit it and an interactive run asks |
| `--token-file PATH` | Read the source's token, or MusicBrainz's contact address, from a file |
| `--token VALUE` | The same inline; prefer `--token-file` or the environment |
| `--only-missing` | Leave files that already carry a copyright alone |
| `--dry-run` | Report the same counts without writing any file |
| `--csv PATH` | Write a before-and-after row for every file visited |
| `--overwrite` | Let `--csv` replace an existing report |
| `--max-attempts N` | Attempts per request before giving up on that album; default 5 |
| `--max-throttle-retries N` | Times to wait out throttling before skipping an album; default 30 |
| `--max-wait SECONDS` | Longest rate-limit pause before setting a source aside; default 60 |

See [docs/copyright.md](docs/copyright.md) for how a match is chosen, what
each catalogue needs, and how a run survives a rate limit.

## Exit status

| Status | Meaning |
|---|---|
| `0` | Help or version succeeded, or the operation completed without processing errors |
| `1` | Invalid arguments, a setup/input/output failure, download failures, or per-file metadata errors |

For `download`, any failed or unattempted line gives status 1 and is preserved
in `output.txt`. For `resolve`, a track Odesli has no link for is an answer
rather than a failure and leaves the status at `0`; a lookup that failed
outright makes it `1`.

## Documentation

| Page | Contents |
|---|---|
| [docs/download.md](docs/download.md) | Input format, the spotDL command, one folder per album, the 15-frame whitelist, LRCLIB synced lyrics, file naming, Spotify token modes |
| [docs/resolve.md](docs/resolve.md) | What Odesli resolves and what it leaves alone, rate limits, exit status |
| [docs/delete.md](docs/delete.md) | Every supported tag name and its frame ID |
| [docs/export.md](docs/export.md) | CSV layout |
| [docs/copyright.md](docs/copyright.md) | Choosing a catalogue, matching rules, rate limits, dry runs, change reports |
| [docs/troubleshooting.md](docs/troubleshooting.md) | Common failures and what to do about them |
| [docs/library.md](docs/library.md) | Using the crate as a Rust library |

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The GitHub Actions workflow runs the same three checks. Run all three before
merging behaviour or documentation changes.
