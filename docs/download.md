# Download music

[← Back to the README](../README.md)

## Syntax

```text
music-tag-transfer download [OPTIONS] <INPUT_FILE>
```

## Input format

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

# 3. an exact-source pair, in either order
https://music.youtube.com/watch?v=dQw4w9WgXcQ|https://open.spotify.com/track/03UrZgTINDqvnUMbbIMhql
https://youtu.be/9bZkp7q19f0|spotify:track:02Q0SXOsk74oV4hesiL6JW
https://open.spotify.com/track/03UrZgTINDqvnUMbbIMhql|https://music.youtube.com/watch?v=dQw4w9WgXcQ
```

Each form leaves a different part of the job to spotDL's own search:

| Form | Audio | Metadata |
|---|---|---|
| Spotify link | searched on YouTube by spotDL | pinned by the link |
| YouTube link | pinned by the link | searched on Spotify by spotDL |
| `YOUTUBE_MUSIC_URL\|SPOTIFY_TRACK_URL` | pinned by the YouTube URL | pinned by the Spotify URL |

The pair is spotDL's exact-source syntax, and it is the only form in which
nothing at all is left to a search. The two halves may be written in either
order — `SPOTIFY_TRACK_URL\|YOUTUBE_MUSIC_URL` is accepted too, and is
rewritten into the order spotDL expects.

Pairs can be written by hand, or generated from a file of Spotify links with
[`resolve`](resolve.md).

**Spotify links** may be `open.spotify.com` URLs or `spotify:` URIs, and may
name a **track**, an **album**, or a **playlist**; an album or playlist link
downloads every song on it. `intl-XX` and `embed` path prefixes are removed and
HTTP links are normalized to HTTPS. Artist, episode, show, and `spotify.link`
URLs are rejected — the last because a shortened link is never resolved.

**YouTube links** may be `music.youtube.com`, `www.youtube.com`, `youtube.com`,
`m.youtube.com`, or `youtu.be`. `/watch?v=ID` and `/playlist?list=ID` paths are
supported; short `youtu.be/ID` links are expanded to
`https://www.youtube.com/watch?v=ID`.

The Spotify half of a **pair** must be a track, because a pair describes
exactly one track, and the two halves must name different services: two
Spotify URLs or two YouTube URLs are rejected.

Query strings and fragments are removed, so tracking parameters such as `si=`
do not create duplicates. Duplicate normalized lines are downloaded only once;
a pair and a bare Spotify track link are different downloads even when they
name the same track, because they resolve the audio differently; the same pair
written in both orders is one download. If any
non-comment line is invalid, the command reports up to ten invalid lines and
stops before starting spotDL.

## Options

| Option | Meaning |
|---|---|
| `-o, --output <DIR>` | Download directory; default `~/Documents/Music` |
| `--spotdl <PROGRAM>` | spotDL executable or path; default `spotdl` |
| `--token-free` | Explicitly download without a Spotify token and skip the startup question |
| `--official-api` | Explicitly use Spotify's official Web API |
| `--auth-token <TOKEN>` | Use this short-lived token and automatically select official mode |
| `--token-file <FILE>` | Read a token from a file and automatically select official mode |
| `--non-interactive` | Never prompt for Spotify mode, Deno, or a token |
| `--auto-download-deno` | Allow spotDL to install Deno when required |
| `--no-copyright` | Skip the iTunes, MusicBrainz, and Discogs copyright lookups |
| `--no-language-lookup` | Skip the MusicBrainz language lookup and read the lyrics instead |
| `--no-lyrics-lookup` | Skip LRCLIB and keep spotDL's own `.lrc` |
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

## The spotDL command

Each line is passed to spotDL on its own, as a single quoted argument, so a
failure can be attributed to the input line it came from:

```bash
spotdl download "LINE" \
    --overwrite force --format mp3 --lyrics synced --generate-lrc
```

When `LINE` is a bare Spotify track, album, or playlist, the command also
passes:

```text
--audio youtube-music --only-verified-results
```

That confines automatic audio matching to verified YouTube Music results. If
there is no verified match, the line fails and is preserved in `output.txt`
instead of falling through to an unverified upload such as a random live
version. An exact-source pair does not need these options because its YouTube
URL already pins the recording.

Those four options are fixed and are not configurable:

| Option | Effect |
|---|---|
| `--overwrite force` | Every listed line is downloaded again, replacing any existing file |
| `--format mp3` | Output is always MP3, so an ID3 tag can always be written |
| `--lyrics synced` | spotDL prefers a provider that has time-synced lyrics |
| `--generate-lrc` | spotDL writes the timed lyrics next to the audio as `.lrc` |

For Spotify-only lines, `--audio youtube-music` and
`--only-verified-results` are fixed as well. Verification makes automatic
matching safer, but the only absolute choice is an exact-source pair: an
official artist can publish both studio and live recordings through verified
channels.

The command also passes `--print-errors`, `--max-retries 0` (retries are
handled here instead), and spotDL's output template:

```text
{album-artist} || {album}/{artists} - {title} [{track-id}].{output-ext}
```

Because `--overwrite force` is used, rerunning an input file re-downloads
every line in it rather than skipping finished files.

## One folder per album

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

## The 15-frame metadata whitelist

After each download, every frame outside this list is deleted. This catches
extra metadata written by both spotDL and FFmpeg.

| Tag | ID3 frame |
|---|---|
| Title | `TIT2` |
| Artist | `TPE1` |
| Album | `TALB` |
| Comment | `COMM` |
| Date | `TDRC` |
| Track Number | `TRCK` |
| Genre | `TCON` |
| Album Artist | `TPE2` |
| Copyright | `TCOP` |
| Disc Number | `TPOS` |
| ISRC | `TSRC` |
| Language | `TLAN` |
| Lyrics | `USLT` |
| Picture: Cover (front) | `APIC` with type `CoverFront` |
| WWW Audio Source | `WOAS` |

Other `APIC` picture types, such as back covers and artist images, are also
deleted. A tag on this list is kept when the source provides it. When a
token-free download has no `TSRC`, MusicBrainz is searched by recording title
and track artist and its ISRC is written when found; [Deezer](#isrc) is asked
next when MusicBrainz has none. A run says how many files ended with no ISRC at
all, since a silently missing frame looks the same as a lookup that never ran.
`TCOP` and `TLAN` are also filled by this program, and the cleaned synced
lyrics are written to `USLT`.

### Copyright

Copyright is requested from the [iTunes Search
API](https://performance-partners.apple.com/search-api) first. When iTunes has
no confident match or fails, MusicBrainz is tried next and constructs a line
from the release's phonographic-copyright or copyright label relationship.
Neither source needs an account, key, or token.

Discogs is asked third, and only when `DISCOGS_TOKEN` names a personal access
token; without one the run says so at startup and stops after MusicBrainz. It
is last because it needs that token and because its copyright lines are
community-entered, so it is the widest net rather than the most consistent
wording — worth having for the releases the other two do not list at all.

| Source | Asked | Needs |
|---|---|---|
| iTunes | Always | Nothing |
| MusicBrainz | When iTunes has no confident match | Nothing; `MUSICBRAINZ_CONTACT` is polite |
| Discogs | When neither of the first two answered | `DISCOGS_TOKEN` |

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

If every lookup fails outright — API throttling or no network — that is
reported as a warning and **the rest of the tag is still written**. The rating
still goes, the lyrics are still embedded, and only `TCOP` is left untouched.
`--no-copyright` skips all three sources altogether.

Two things to know about the API: it is throttled per IP, so requests are
spaced out and a throttled response is retried a few times before giving up;
and it is queried against the US storefront, whose catalogue is the most
complete. The `℗` line is the label's and rarely differs between storefronts.

### ISRC

`TSRC` is the one frame spotDL fills only in official-API mode, so a token-free
download arrives without it. Two catalogues are asked, in order:

| Source | Asked | Needs |
|---|---|---|
| MusicBrainz | Always, when the tag has no ISRC | Nothing; `MUSICBRAINZ_CONTACT` is polite |
| Deezer | Only when MusicBrainz found none | Nothing |

Both are searched by **track artist** and title, and a candidate is used only
when its title *and* its artist match, because a wrong ISRC is worse than none
— every later lookup would follow it confidently to the wrong recording. A
malformed value is discarded rather than written.

Deezer is second rather than first because MusicBrainz's catalogue is fuller,
but it is a volunteer database and a track nobody has entered is simply absent;
Deezer is commercial and strongest on recent and mainstream releases, which is
where MusicBrainz is thinnest. They miss different tracks, which is the point
of asking both. Deezer costs nothing on tracks MusicBrainz already knows,
because it is only asked when MusicBrainz came back empty.

Only the ISRC is taken from Deezer. It publishes no copyright line at all — its
album object carries a `label`, which is the marketing imprint rather than the
phonographic copyright holder, and the two are routinely different companies.
Assembling a `℗` line from it would read right and name the wrong entity, so
`TCOP` is left to iTunes, MusicBrainz, and Discogs.

### Language

`TLAN` is looked up on MusicBrainz, per track, and falls back to reading the
lyrics when MusicBrainz has no answer.

The language comes from the **work** — the song as written — reached through
the recording, rather than from the release. That distinction matters: a
release's own language field describes the text on its track list, so an
English song on a Korean album would be labelled Korean. The work is where
MusicBrainz records what the song is actually sung in.

The recording is found by ISRC where the tag has one, which is exact, and by
**track artist** and title otherwise, ranked the same way releases are so that
a search for a common title cannot return somebody else's song of that name.

The track artist matters: MusicBrainz credits a recording to whoever performed
it, so a compilation searched by its album artist — "Various Artists" — matches
nothing at all. `TPE1` is used when it differs from the album artist, and the
album artist still identifies the *release* for the copyright lookup, where it
is the right name.

Work data is patchy — plenty of recordings have no work linked, and plenty of
works have no language recorded — so a miss is ordinary and costs nothing: the
synced lyrics are then detected from as before, with the timestamps and any
`[ar:...]` metadata stripped first so they cannot mislead the detector. A
catalogue that names the language is preferred over the detector, since it
describes the song rather than guessing at whatever text the `.lrc` happens to
hold.

This costs two MusicBrainz requests per track, spaced at the published one per
second, and returns the recording's ISRC in the same lookup. Against the time
spent fetching and transcoding the audio that is not the bottleneck.
`--no-language-lookup` skips the work-language part but still looks up a
missing ISRC and any copyright fallback. Set `MUSICBRAINZ_CONTACT` so
MusicBrainz can reach you.

`TLAN` is written as a readable **name** — `English`, `Chinese`, `Korean`,
`Spanish` — because that is what a tagger displays. ID3v2.3 specifies an
ISO-639-2 code in this frame, so a strict reader will see a value it does not
recognise; the three-byte language field inside the `USLT` frame, whose width
the frame format fixes, still carries the code.

- A language from MusicBrainz sets `TLAN`, and the lyrics frame's own language
  field is set to the matching code.
- Failing that, a confident detection from the lyrics does the same.
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

## Synced lyrics in the ordinary `USLT` frame

Every download asks spotDL for time-synced lyrics and for the `.lrc` file that
carries them — `--lyrics synced --generate-lrc`, on every line of the input
file, with no way to turn it off.

### Why LRCLIB is asked first

spotDL's `synced` provider searches on the track's name and artist and nothing
else. No step in it checks how long the recording is, so an anniversary
remaster, a radio edit, or a live take answers as readily as the cut actually on
disk: the words come back right and the timings drift.

[LRCLIB](https://lrclib.net) is asked first because it refuses that. A lookup
carries the artist, title, album, and **the length of the file spotDL just
wrote**, read back off its MPEG frames, and LRCLIB answers only when its record
is within about two seconds of that. The wrong version of a song is therefore a
miss rather than a plausible-looking answer — the same bargain the copyright
lookups strike, where a wrong value is worse than none.

| Source | Asked | Matched on |
|---|---|---|
| LRCLIB | Always, unless `--no-lyrics-lookup` | Artist, title, album, **and duration** |
| spotDL's `.lrc` | When LRCLIB has nothing of that length | Artist and title only |

It needs no account, key, or token. Only timed text is taken: LRCLIB also
publishes plain lyrics, but replacing spotDL's timed text with untimed text
would be a loss even when the words are better. A track LRCLIB knows to be
instrumental is left alone rather than fallen back on.

A file whose duration cannot be read is not asked about at all — with nothing to
check a candidate against, the lookup would be exactly the loose search it
exists to avoid.

### Pasting it into the tag

After each successful download the winning text is pasted into the tag during
the same metadata rewrite that applies the whitelist:

1. The lyrics are taken from LRCLIB, or from the `.lrc` file beside the audio
   when LRCLIB had none.
2. Empty lines and lines containing only timestamps, such as `[00:00:00]` or
   `[00:12.50][00:42.50]`, are removed.
3. The cleaned text is written to the ID3v2.3 `USLT` lyrics frame. Timestamps
   attached to real lyric lines remain intact, and any previous `USLT` is
   replaced.
4. Every `SYLT` synchronised-lyrics frame is removed, including spotDL's.
5. The file is read back from disk. Only if the stored `USLT` frame matches the
   cleaned text, and no `SYLT` frame survives, is the `.lrc` file deleted — it
   is deleted whether its own text won or LRCLIB's superseded it.

`USLT` is the frame players actually read; `SYLT` is the one most of them
ignore. Players that do understand timed lyrics parse the timestamps out of the
`USLT` text, so retaining the timestamps attached to lyric lines makes the
lyrics show up in both kinds of player.

Only the language detector sees the lyrics without their timestamps — the
`[mm:ss.xx]` prefixes and any `[ar:...]` metadata header would only mislead it.

If any of those steps fails, the `.lrc` file is **kept** so its lyrics are
never the thing that gets lost, nothing is written to the audio file, and the
line it belongs to is reported as failed so it appears in the retry list. An
`.lrc` file with no content left after cleaning counts as a failure when it was
the only candidate; when LRCLIB has already answered it is merely the losing
one, and the file finishes normally. An untimed `.lrc` is pasted like any
other. A line for which spotDL generated no `.lrc` file
is not a failure either: the metadata whitelist is still applied, and there
were simply no synced lyrics to paste.

## How downloaded files are found again

Before each line runs, the output directory's music files are recorded; when
spotDL finishes, whatever is new or was rewritten is what that line produced.
That works for every form, including the lines that name no single Spotify
track, so an album or playlist line gets all of its songs tagged. For a line
that does name a single track, the Spotify track ID in the forced
`[{track-id}]` output template is kept as a fallback.

## The track ID is removed from the file name

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

## Download execution and output files

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

## Optional Spotify token

An interactive run now makes the choice explicit:

```text
Choose how this download should access Spotify metadata:
  1) Without a Spotify token (token-free)
  2) With a Spotify token (official Web API)
Select 1 or 2 [1]:
```

Press Enter or choose `1` for token-free mode. Choose `2` to paste a token.
For a script, use `--token-free` to select the first mode without a prompt, or
provide exactly one of these to select official mode:

```bash
music-tag-transfer download links.txt --auth-token 'short-lived-access-token'
music-tag-transfer download links.txt --token-file ./spotify-token.txt
SPOTIFY_AUTH_TOKEN='short-lived-access-token' music-tag-transfer download links.txt
```

Any of these automatically enables `--use-official-api`; `--official-api` does
not have to be supplied separately. The token may be a bare value, `Bearer
...`, or a quoted `const token = '...'` assignment.

Which token you can use depends on where it came from. A token issued to a
developer-mode Spotify app requires the **app owner to hold an active Premium
subscription** — since Spotify's February 2026 developer changes, such an app
owned by a free account answers every request with `403 Active premium
subscription required for the owner of the app`, which stops the batch. A token
copied from the `open.spotify.com` web player carries no such requirement, but
expires within the hour; when it does, an interactive run asks for a
replacement.

## Token-free and official API modes

The interactive default and non-interactive fallback are token-free. It
deliberately does not pass
`--use-official-api`, `--auth-token`, or `--use-cache-file` to spotDL.
Before downloading, it checks spotDL's active `config.json` and stops if
`use_official_api`, `use_cache_file`, `user_auth`, or a saved
`auth_token` would silently change that behavior. Disable config loading or
clear those settings before retrying.

Use the official API only as an explicit fallback. Supplying a token selects
it automatically:

```bash
SPOTIFY_AUTH_TOKEN='short-lived-access-token' \
  music-tag-transfer download links.txt
```

Official-mode token precedence is:

1. `--auth-token`;
2. `--token-file`;
3. `SPOTIFY_AUTH_TOKEN`.

`--auth-token` and `--token-file` are mutually exclusive. Token files may
contain a plain token, a `Bearer ...` value, or a quoted JavaScript-style token
assignment. Protect these files and never commit tokens, cookies, Client
Secrets, or other credentials.

When an official token expires, an interactive terminal can request a
replacement. `--non-interactive` disables this prompt.
