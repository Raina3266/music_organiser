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

The search found the song — the message names the YouTube video it picked — and
yt-dlp then could not fetch audio from it. This is not the YouTube Music
problem above and no audio provider works around it, because they all end at
the same yt-dlp.

Check these in order:

1. **yt-dlp's version.** YouTube breaks older releases constantly. spotDL 4.5.2
   asks for `yt-dlp>=2026.07.04`; a distribution package can lag well behind
   that. On Nix, `ls -d /nix/store/*yt-dlp*` shows which one your spotDL
   actually uses.
2. **Deno.** spotDL prints `Some YouTube downloads require Deno` alongside the
   failure when it is missing. No such line means Deno is fine.
3. **Reproduce it outside this program**, on the video the message named:

   ```bash
   yt-dlp -F 'https://www.youtube.com/watch?v=VIDEO_ID'
   ```

   If that errors or lists no audio formats, nothing in this program is
   involved.

If yt-dlp is current and still refused, YouTube is treating the request as
untrusted. Sign it in with cookies exported from a browser:

```bash
music-tag-transfer download links.txt --cookie-file ~/youtube-cookies.txt
```

Use the Netscape cookie format yt-dlp reads, and **protect the file as you
would a password** — it carries a live session. `--yt-dlp-args` passes anything
else yt-dlp needs, for example
`--yt-dlp-args '--extractor-args youtube:player_client=web'`.

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
