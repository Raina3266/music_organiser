# Troubleshooting

[← Back to the README](../README.md)

## `cannot run 'spotdl'`

Confirm `spotdl --version` works in the same shell. Otherwise pass
`--spotdl /full/path/to/spotdl` or set `SPOTDL_PROGRAM`.

## spotDL configuration blocks token-free mode

Inspect `~/.config/spotdl/config.json` or `~/.spotdl/config.json`. Disable
`load_config`, or clear the official-API settings named in the error. Use
`--official-api` only when that mode is intentional.

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
