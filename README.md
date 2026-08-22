# music-tag-transfer

A Rust CLI that keeps three related music-library workflows in one executable:

- download a list of Spotify links through spotDL;
- delete selected ID3 tags recursively;
- transfer one ID3 frame between matching source and destination tracks.

Downloaded audio comes from spotDL's configured audio providers, not from Spotify.
Only download content you are permitted to keep.

## Requirements

- a stable Rust toolchain;
- spotDL 4.5.0 or newer for the `download` command;
- FFmpeg, as required by spotDL;
- Deno for YouTube downloads that require a JavaScript runtime.

Build the unified executable with:

```bash
cargo build --release
```

The binary is `target/release/music-tag-transfer` on Linux/macOS and
`target\release\music-tag-transfer.exe` on Windows.

## Download a list of Spotify links

Create a UTF-8 text file containing one Spotify URL or URI per line:

```text
# Blank lines and comments are ignored
https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW
https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy
spotify:playlist:37i9dQZF1DXcBWIGoYBM5M
```

Track, album, playlist, artist, episode, and show links are accepted.
`spotify.link` short links are also accepted. Tracking parameters are removed
and duplicate canonical links are skipped.

Run the downloader with:

```bash
cargo run -- download spotify-links.txt
```

Downloads go to `~/Documents/Music` by default. Choose another directory with:

```bash
cargo run -- download spotify-links.txt --output "/path/to/music"
```

The default mode uses spotDL's token-free metadata client. It deliberately does
not pass `--auth-token` or `--use-official-api`. Before downloading, the
command checks spotDL's active configuration and stops if official-only settings
would silently override token-free mode.

The official Spotify Web API remains an explicit fallback:

```bash
SPOTIFY_AUTH_TOKEN='short-lived-access-token' \
  cargo run -- download --official-api spotify-links.txt
```

A protected token file can be supplied with `--token-file`. Do not commit
tokens, cookies, or other credentials.

### Download options

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>

-o, --output <DIR>                Output directory [default: ~/Documents/Music]
    --spotdl <PROGRAM>            spotDL executable [default: spotdl]
    --official-api                Use Spotify's official Web API intentionally
    --auth-token <TOKEN>          Official mode: short-lived access token
    --token-file <FILE>           Official mode: read an access token from a file
    --non-interactive             Never prompt for Deno or a replacement token
    --auto-download-deno          Let spotDL install Deno when required
    --max-attempts <N>            Network-failure attempts [default: 3]
    --max-rate-limit-wait <SECS>  Longest Retry-After wait [default: 300]
-h, --help                        Print download help
-V, --version                     Print the application version
```

Set `SPOTDL_PROGRAM=/full/path/to/spotdl` instead of passing `--spotdl`
on every run.

Each URL is sent to spotDL separately so failures can be attributed precisely.
The selected output directory always receives `output.txt`, containing failed
and unattempted URLs in input order. A successful run leaves that file empty.
Detailed attempted failures are written to
`music-tag-transfer-download-failures.txt`.

Short ordinary `Retry-After` delays are respected once. Application quota
errors and delays longer than `--max-rate-limit-wait` stop the batch without
rotating tokens; the remaining URLs are preserved for a later retry. spotDL is
called with `--overwrite skip`, so rerunning the original list or
`output.txt` keeps completed files.

If spotDL reports that Deno is required, an interactive run can approve
`spotdl --download-deno`. For unattended setup, use
`--auto-download-deno`; otherwise install Deno before rerunning.

## Delete tags recursively

Quote the bracketed list so the shell passes it as one argument:

```bash
cargo run -- delete "/path/to/music" "[Encoded-by, Album Artist]"
```

Tag names are case-sensitive. A file without any requested tag is left
untouched. Files without an ID3 tag are skipped. Subdirectories are searched
recursively, symlinks are ignored, and MP3/MP2/MP1, WAV, AIFF, and AIF
containers are supported. Changed tags are written as ID3v2.3.

Preview the operation without changing files:

```bash
cargo run -- delete "/path/to/music" "[Encoded-by, Album Artist]" --dry-run
```

Common supported names include:

| Command name | ID3v2.3 frame |
|---|---|
| `Encoded-by` | `TENC` |
| `Album Artist` | `TPE2` |
| `Album` | `TALB` |
| `Artist` | `TPE1` |
| `Comment` | `COMM` |
| `Composer` | `TCOM` |
| `Genre` | `TCON` |
| `Lyrics` | `USLT` |
| `Picture` | `APIC` |
| `Title` | `TIT2` |
| `Track Number` | `TRCK` |
| `Year` | `TYER` |

Run `cargo run -- --help` and see `src/frames.rs` for the complete
case-sensitive list.

## Transfer a frame

Source and destination tracks are matched by title, then the requested frame is
copied recursively:

```bash
cargo run -- transfer "/source/music" "/destination/music" USLT
```

The previous three-positional-argument form remains supported:

```bash
cargo run -- "/source/music" "/destination/music" USLT
```

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
