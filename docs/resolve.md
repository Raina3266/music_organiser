# Pin Spotify links to YouTube Music

[← Back to the README](../README.md)

`download` hands a bare Spotify link to spotDL, which searches YouTube Music
for the title and artist and scores what comes back. That is a good guess, and
an exact-source pair exists to replace a guess — so a pair is only worth
writing down when it came from somewhere that did not guess.

[Odesli](https://odesli.co) (the service behind `song.link`) is that somewhere.
It cross-references the streaming catalogues by their own identifiers rather
than by matching text, so it disagrees with a search precisely where a search
goes wrong. The `resolve` command asks it which YouTube Music track each
Spotify track is, and rewrites those lines as pairs.

A resolved line is written `SPOTIFY_TRACK_URL|YOUTUBE_MUSIC_URL` — the Spotify
link first, so the resolved file reads in the same column order as the list of
Spotify links it came from:

```text
https://open.spotify.com/track/02Q0SXOsk74oV4hesiL6JW|https://music.youtube.com/watch?v=dQw4w9WgXcQ
```

spotDL itself reads a pair the other way round, but `download` accepts either
order and swaps it back before handing it over, so nothing is lost by writing
the readable one. A pair the input file already carried is rewritten into the
same order, so a resolved file never mixes the two.

## Syntax

```text
music-tag-transfer resolve <INPUT_FILE> [OUTPUT_FILE] [OPTIONS]
```

```bash
# writes links-resolved.txt beside links.txt
music-tag-transfer resolve links.txt

# then download the pinned file
music-tag-transfer download links-resolved.txt --output ./music
```

The input is an ordinary `download` input file and so is the output, so the two
commands compose. Only bare Spotify **track** lines are looked up. Everything
else is copied through untouched:

| Input line | What `resolve` does |
|---|---|
| Spotify track | Asks Odesli, and writes a pair when it answers |
| Spotify album or playlist | Copied through — an album is not one track |
| YouTube Music link | Copied through — the audio is already pinned |
| An existing pair | Copied through, in Spotify-first order — there is nothing left to resolve |

A track Odesli cannot place stays a bare Spotify link on purpose. Dropping it
would lose a song spotDL can very likely still find by searching; leaving it
alone costs nothing and keeps the file complete. That means a resolved file is
always a drop-in replacement for the file it came from, never a subset of it.

Only Odesli's `youtubeMusic` link is used. It often also knows a plain
`youtube` link for the same track, but that is as likely to be the music video —
wrong length, spoken intro, live take — and pinning the audio to the wrong
recording is worse than letting spotDL search for the right one.

## Options

| Option | Meaning |
|---|---|
| `OUTPUT_FILE` | Where to write. Defaults to the input name with `-resolved` before the extension |
| `--overwrite` | Allow the output file to be replaced |
| `--api-key KEY` | An Odesli API key. Readable by every process on the machine |
| `--api-key-file FILE` | Read the key from a file instead |
| `--country XX` | Storefront to ask about, as a two-letter code. Defaults to `US` |
| `--max-attempts N` | How many times to try one request before giving up on it |
| `--max-wait SECONDS` | Longest rate-limit wait to sit through before giving up |
| `--max-throttle-retries N` | How many times to wait out throttling for one track |

The key may also arrive in `ODESLI_API_KEY`. A file and the environment are
both preferred to `--api-key`, which every other process on the machine can
read out of the process list.

## Rate limits and how long a run takes

Odesli allows ten requests a minute without a key, so `resolve` spaces its
requests six seconds apart and says up front how long the file will take. A
hundred tracks is ten minutes. With a key it starts at two requests a second
and lets the throttling correct it: a `429` widens the spacing, and it eases
back once requests are getting through again.

Each distinct track is asked about once however often the file repeats it, and
a run can be repeated cheaply — re-resolving an already-resolved file only asks
about the lines that stayed bare.

## Exit status

`0` when every track got a real answer, whether or not that answer was a link.
`1` when a lookup failed outright, or Odesli stopped answering partway through.
Either way the output file is written in full, so a failed run can simply be
run again.
