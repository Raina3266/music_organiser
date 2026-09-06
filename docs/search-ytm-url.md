# Add a YouTube Music URL column to a CSV

[← Back to the README](../README.md)

`resolve` answers "which YouTube Music track is this Spotify track?" for a file
of links. `search-ytm-url` answers the same question for a table: give it a CSV
naming each track's song, album, artist, and Spotify URL, and it writes the
file back out with a `youtube_music_url` column added.

It exists because that table is what you already have. A Spotify playlist
export, a spreadsheet somebody keeps by hand, the output of another tool — all
of them are rows of names, and none of them are the link file `download` reads.

Unlike `resolve`, this needs no API key. It searches YouTube Music through the
[ytmusicapi](https://github.com/sigma67/ytmusicapi) Python package, which
speaks the same private API the web player uses.

## Requirements

```bash
pip install ytmusicapi
```

Python 3 must be on your `PATH` as `python3`, or named with `--python`. No
account, token, or `ytmusicapi` setup file is needed: searching is anonymous.

## Syntax

```text
music-tag-transfer search-ytm-url <INPUT_CSV> [OUTPUT_CSV] [OPTIONS]
```

```bash
# writes tracks-with-ytm.csv beside tracks.csv
music-tag-transfer search-ytm-url tracks.csv

# or name the destination yourself
music-tag-transfer search-ytm-url tracks.csv answered.csv --overwrite
```

| Option | Meaning |
|---|---|
| `--overwrite` | Replace the output file if it already exists |
| `--python PATH` | The interpreter that runs ytmusicapi (default `python3`) |

`--search-ytm-url` is accepted as another spelling of the subcommand.

Without `OUTPUT_CSV` the answered table is written beside the input as
`<name>-with-ytm.<extension>`. The command refuses to write over its own input,
and refuses to replace an existing output unless `--overwrite` is given.

## The input

Four columns, in any order, among as many others as you like:

```csv
song name,album name,artist name,spotify_url
Get Lucky,Random Access Memories,Daft Punk,https://open.spotify.com/track/69kOkLUCkxIZYexIgSG8rq
Instant Crush,Random Access Memories,Daft Punk,https://open.spotify.com/track/2cGxRwrMyEAp8dEbuZaVv6
```

Headers are matched with case, spacing, and punctuation set aside, so
`song name`, `Song Name`, `song_name`, and `songName` all find the same column.
These spellings are recognised:

| Column | Also accepted as |
|---|---|
| `song name` | `song`, `track name`, `track`, `title`, `name` |
| `album name` | `album`, `album title` |
| `artist name` | `artist`, `artists`, `album artist` |
| `spotify_url` | `spotify url`, `spotify link`, `spotify track url`, `url` |

Only the song and the artist have to be there — they are what a match is
checked against, and a file without them is refused up front rather than
answered with a column of empty cells. The album is optional and only ever
separates two results that match equally well.

The Spotify URL is never looked up. It is the row's identity: two rows carrying
the same one are the same recording, so it is searched for once however many
times a playlist export repeats it.

## The output

Every column and every row of the input, in the order they arrived, plus one:

```csv
song name,album name,artist name,spotify_url,youtube_music_url
Get Lucky,Random Access Memories,Daft Punk,https://open.spotify.com/track/69kOkLUCkxIZYexIgSG8rq,https://music.youtube.com/watch?v=5NV6Rdv1a3I
Instant Crush,Random Access Memories,Daft Punk,https://open.spotify.com/track/2cGxRwrMyEAp8dEbuZaVv6,
```

**An empty cell means nothing matched.** That is the whole contract. The search
is a name search and names are ambiguous: YouTube Music will happily return a
cover, a karaoke version, a sped-up edit, or a different artist's song of the
same title. A link to the wrong recording is worse than no link, because it
would be downloaded as though it were right — so a cell is filled only when the
result's title **and** one of its credited artists both match the row.

Matching is the same comparison the `copyright` command uses, so it survives
what the two catalogues genuinely disagree about — case, accents, punctuation,
`&` against `and`, and an edition suffix one of them prints — without ever
conceding that two different recordings are the same one:

| Row says | YouTube Music says | Matched |
|---|---|---|
| `Deja Vu` / `Beyonce` | `DÉJÀ VU` / `Beyoncé` | yes |
| `Get Lucky` / `Daft Punk` | `Get Lucky` / `Daft Punk, Pharrell Williams` | yes |
| `Get Lucky` / `Daft Punk` | `Get Lucky (Remix)` / `Daft Punk` | no |
| `Get Lucky` / `Daft Punk` | `Get Lucky` / `Some Cover Band` | no |

Any one of the credited artists is enough, because YouTube Music lists everyone
on the recording where a Spotify export usually names only the first.

The album corroborates but never decides. A recording sits on its album, its
single, and any number of compilations, and the two services routinely name
different ones — so a result on the album you named beats an equally good one
that is not, and a result on a different album is still written when it is the
only match there is.

## Running it twice

Running the command over its own output only searches for the rows that are
still empty; the links already found are kept, in the same column rather than
in a second one. That is how a run that half failed is finished, and it makes a
second pass over a long file cheap.

```bash
music-tag-transfer search-ytm-url tracks.csv answered.csv
# some rows failed; fill them in without redoing the rest
music-tag-transfer search-ytm-url answered.csv answered-2.csv
```

## How long a run takes

ytmusicapi is started once and kept open for the whole file, because importing
it and handshaking with YouTube Music costs far more than a search does. After
that it is one search per distinct track — rows that repeat a track, rows that
already carry a link, and rows with no song or artist to search on cost
nothing.

## What the run tells you

```text
Read 250 row(s): song from "song name", artist from "artist name", album from "album name".
Wrote 250 row(s) to answered.csv. 237 row(s) carry a YouTube Music link and 13 are empty.
  Searched YouTube Music for 243 row(s): 237 matched, 6 matched nothing.
  6 row(s) named a track an earlier row had already asked about, and were answered without searching again.
  1 row(s) had no song name or no artist to search on, so nothing was asked about them.
```

The headline counts the cells that ended up filled, not the searches that
filled them, so a rerun that only had a handful left to do still reports the
whole table rather than reading like a failed run.

Rows are counted apart from each other on purpose. A row that *matched nothing*
was asked about and got a real answer; a row that was *never asked about* —
because the bridge stopped answering partway through — is not evidence of
anything, and a rerun may well place it.

## Exit status

`0` when every row got a real answer, whether or not that answer was a link.
`1` when a search failed outright, or the bridge stopped answering partway
through. Either way the output is written in full, with every row present, so a
failed run can simply be run again over its own output.

## Feeding the result to `download`

The two link columns side by side are an exact-source pair, so a spreadsheet
formula or a one-liner turns the answered table into a `download` input file:

```bash
python3 -c '
import csv, sys
for row in csv.DictReader(open("answered.csv")):
    spotify, youtube = row["spotify_url"], row["youtube_music_url"]
    if spotify and youtube:
        print(f"{spotify}|{youtube}")
' > links.txt

music-tag-transfer download links.txt --output ./music
```

Python rather than `awk -F,` because a title like `Earth, Wind & Fire` is one
quoted cell holding a comma, and splitting on commas would tear the row in two.

Rows whose link came back empty are left out of the pair file; add them as bare
Spotify links if you would rather spotDL searched for them.
