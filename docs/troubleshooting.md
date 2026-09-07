# Troubleshooting

[← Back to the README](../README.md)

## `cannot run 'spotdl'`

Confirm `spotdl --version` works in the same shell. Otherwise pass
`--spotdl /full/path/to/spotdl` or set `SPOTDL_PROGRAM`.

## spotDL configuration blocks token-free mode

Inspect `~/.config/spotdl/config.json` or `~/.spotdl/config.json`. Disable
`load_config`, or clear the official-API settings named in the error. Use
`--official-api` only when that mode is intentional.

## `JSONDecodeError: Expecting value: line 1 column 1 (char 0)`

YouTube Music served spotDL a page instead of results. spotDL reaches it
through ytmusicapi, which decodes the reply before checking the status code, so
a block, a rate limit, or a consent interstitial surfaces as a JSON error. It is
not a spotDL bug and no Spotify token affects it.

You may see it reported against one track, or as a traceback ending in
`check_ytmusic_connection` — spotDL's startup connectivity check, which runs
outside its own error handling and so takes the whole process down. Both mean
the same thing and are handled the same way.

The run handles it on its own: the line is retried up to `--max-attempts` and
then retried once more with `--audio youtube`, spotDL's yt-dlp provider, which
neither uses the YouTube Music API nor triggers that startup check. The batch is
not stopped, so only the lines that end with no audio at all reach `output.txt`.

If whole runs still come back empty, YouTube Music is refusing your address
rather than one track. Wait it out or move to another network. To take the
search out of the run altogether, pin the audio: an exact-source
`YOUTUBE_MUSIC_URL|SPOTIFY_TRACK_URL` pair is downloaded without any audio
search. [`resolve`](resolve.md) writes those pairs through Odesli, which does
not touch the YouTube Music API — though it now needs an Odesli key.
[`search-ytm-url`](search-ytm-url.md) finds the same links, but it searches
through ytmusicapi as well, so a block that stops a download stops it too.

One kind of line cannot fall back at all. A bare **YouTube Music link** has its
metadata read through that same API whatever `--audio` says, so changing the
audio provider does not help it. Pair those links with their Spotify track, or
download them once YouTube Music answers again. Pairs themselves are fine: they
fall back like any other line, because the only thing YouTube Music was doing
for them was a startup check the fallback skips.

## `AudioProviderError` / `Requested format is not available`

YouTube would not serve an audio format for a video. Which video, and at which
step, depends on the provider — and the two are worth telling apart, because
only one of them is really about the recording spotDL wanted:

| Provider | Searches through | The failure is |
|---|---|---|
| `youtube-music` | ytmusicapi, no yt-dlp | the **download** of the recording it chose |
| `youtube` | yt-dlp's own `ytsearch10:` | the **search** itself |

The search case is the surprising one. spotDL hands yt-dlp a logger whose
`error` method raises instead of logging, so a problem yt-dlp reports about
**one** of the ten search results aborts the whole search — including the nine
that were fine. One video nobody can download hides every alternative to it.

That one has a direct fix. yt-dlp raises only when
`ignore_no_formats_error` is unset, and spotDL's logger discards warnings, so
setting it turns the fatal entry into a skipped one:

```bash
music-tag-transfer download links.txt --yt-dlp-args '--ignore-no-formats-error'
```

Worth trying whenever *some* of a run downloads and the rest does not: that
pattern says YouTube is refusing particular videos rather than this machine,
and skipping them lets the search reach one that works.

For the download case, the run already retries the line and widens the search,
so a track refused on one recording can still succeed on another. Reaching the
failure report means every rung was refused.

If whole runs fail, check these in order:

1. **yt-dlp's version.** YouTube breaks older releases constantly. spotDL 4.5.2
   asks for `yt-dlp>=2026.07.04`. `ls -d /nix/store/*yt-dlp*` lists what a Nix
   system has; the version in a failure traceback is the one actually loaded.
2. **Deno.** spotDL prints `Some YouTube downloads require Deno` alongside the
   failure when it is missing. No such line means Deno is fine.
3. **The real error.** `AudioProviderError: YT-DLP download error` carries no
   diagnosis: spotDL sends the actual exception to `logger.debug` and re-raises
   that placeholder, so it is invisible at the default log level. Ask spotDL
   directly, with `--audio youtube` so its YouTube Music startup check cannot
   crash the run first:

   ```bash
   spotdl --log-level DEBUG --audio youtube --format mp3 \
     download 'https://open.spotify.com/track/TRACK_ID'
   ```

4. **Cookies, in both directions.** They can help, by making the requests
   signed-in — and they can hurt, because yt-dlp will not use some player
   clients alongside them, which may be the only clients YouTube is serving.
   Compare the two on a video that failed:

   ```bash
   yt-dlp -F 'https://www.youtube.com/watch?v=VIDEO_ID'
   yt-dlp --cookies ~/youtube-cookies.txt -F 'https://www.youtube.com/watch?v=VIDEO_ID'
   ```

   Whichever lists audio formats is the one to run with. To use cookies, export
   them for `youtube.com` in the Netscape format yt-dlp reads, and **protect
   the file as you would a password** — it carries a live session. Exporting
   from a private window and closing it without logging out keeps the copy
   valid; a session you keep browsing in rotates the cookies out from under it.

```bash
music-tag-transfer download links.txt --cookie-file ~/youtube-cookies.txt
```

`--yt-dlp-args` passes anything else yt-dlp needs, uninterpreted — for example
`--yt-dlp-args '--extractor-args youtube:player_client=tv'`.

## `cannot import ytmusicapi`

`search-ytm-url` searches through the ytmusicapi Python package. Install it into
the interpreter the command actually runs:

```bash
python3 -m pip install ytmusicapi
```

If Python 3 is not on the `PATH` as `python3`, or the package is installed into
a different interpreter or a virtual environment, name that one:
`--python /path/to/python3`. Nothing else is needed — searching is anonymous, so
there is no token or `ytmusicapi` setup file to configure.

## Every `search-ytm-url` cell came back empty

The song and artist columns are what a match is checked against, and the run
says which headers it read them from on its first line. If it named the wrong
columns, or reported none, rename the headers or check that the file really is
comma-separated — a tab- or semicolon-separated export reads as one giant
column.

If the columns were right, the rows genuinely did not match: a cell is filled
only when the result's title and one of its credited artists both match. A row
whose artist column holds something other than an artist — a genre, a playlist
name, a blank — can never match, however right the title is.

## No copyright was written

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

## A token was rejected or has expired

`Spotify rejected the token (HTTP 401)` and its Discogs equivalent mean the
token itself, not the album. A token does not repair itself, so the source is
retired immediately rather than spending the rest of the library discovering
the same thing. Spotify web-player tokens expire within the hour, so a long run
can outlive one; fetch a fresh token and run again with `--only-missing` to pick
up where it stopped.

## The run stopped part way through

Every source it had was spent — a rate limit, an expired token, or a catalogue
that would not stop throttling. This is deliberate: the alternative is several
hundred identical failures and, on a rate limit, a longer ban. Nothing was lost.
Re-run with `--only-missing`, and consider a fallback chain so the next run
survives one catalogue giving up.

## A download needs Deno

Run `spotdl --download-deno` manually, install Deno system-wide, or rerun the
download command with `--auto-download-deno`.

## A `.lrc` file was left behind

Its lyrics could not be embedded, and the reason is printed during the run and
recorded in `music-tag-transfer-download-failures.txt`. The usual cause is a
`.lrc` file that is empty, or an audio file whose tag could not be written.
Retry the line from `output.txt`, or delete the `.lrc` file yourself if the
track simply has no lyrics worth keeping.

## A player shows no lyrics

Confirm the frame is there, for example with mutagen:

```bash
python3 -c "from mutagen.id3 import ID3; print(ID3('track.mp3').getall('USLT'))"
```

The text should be the `.lrc` file as it was downloaded, timestamps included. If
it is there and the player still shows nothing, the player is not reading `USLT`
at all. If the lyrics show but do not scroll with the music, the player does not
parse LRC timestamps out of `USLT`; nothing in the tag can fix that.

## Metadata commands find no files

Confirm the path is a directory and the files use one of the supported
extensions: MP1, MP2, MP3, WAV, AIF, or AIFF. FLAC, M4A, and OGG files are not
processed.
