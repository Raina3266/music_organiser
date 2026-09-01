# Refresh the copyright message

[← Back to the README](../README.md)

The `download` command fills `TCOP` from the iTunes Search API as each file
arrives. This command does the same for music that is already on disk, which
is the way to fix a library downloaded before the lookup existed, or one whose
lookups were skipped with `--no-copyright`.

## Syntax

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
| `--max-attempts N` | How many times to try one request before giving up on that album (5 by default) |
| `--max-throttle-retries N` | How many times to wait out throttling before skipping that album (30 by default) |
| `--max-wait SECONDS` | Longest rate-limit pause to sit through before setting a source aside (60 by default) |

## Choosing where the copyright comes from

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

## Surviving a rate limit

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

## Tokens and contact addresses

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

## Seeing what a run would do, before it does it

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

## A file is only written when a copyright was found

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

## The rest of the tag is evidence too

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

## When a request fails

A request can fail for two quite different reasons, and they are answered
differently.

**The server says slow down.** A `429`, or a `503` — which is how MusicBrainz
signals a breached rate limit rather than the `429` you might expect — is
waited out, honouring `Retry-After`, and retried `--max-throttle-retries`
times (**30** by default). Running out of those skips that album and the scan
carries on: throttling passes, and the album after this one will very likely
go through, so one album's bad luck is never a reason to stop.

Waiting out the one refused request is not enough on its own, though. The next
request would go out at the same rate and be refused the same way, which is how
one throttled album turns into a screenful of them:

```text
MusicBrainz lookup failed (HTTP 503 Service Unavailable)
MusicBrainz lookup failed (HTTP 503 Service Unavailable)
MusicBrainz lookup failed (HTTP 503 Service Unavailable)
```

So the **pacing itself widens**. Every refusal doubles the gap the run leaves
between requests, up to ten seconds, and ten requests getting through cleanly
halve it again, down to the rate the catalogue publishes. A published limit is
an average rather than a promise — MusicBrainz allows roughly one request a
second, but a busy hour, or somebody else sharing your address, can see it
refuse a rate it accepted earlier in the same run — and this finds the rate
that is actually being accepted instead of insisting on the documented one:

```text
MusicBrainz is refusing requests at this rate; spacing them 2.2s apart from here on.
MusicBrainz is answering again; easing the spacing back to 1.1s.
```

The exception is a `Retry-After` measured in hours. No number of retries
shortens that, so the source is **set aside** — stopped being asked — while
the run itself continues to the end.

**Something went wrong in transit.** A timeout, a dropped connection, a `500`
or a `502` says nothing about the album, so the request is simply tried again
after a growing pause — 2s, 4s, 8s, up to 30s — `--max-attempts` times. After
that the album is given up on and the scan moves to the next one; nothing is
written and the file keeps whatever it had.

Both are visible while the run works:

```text
iTunes attempt 1 failed (operation timed out); retrying in 2s...
iTunes attempt 2 failed (operation timed out); retrying in 4s...
Daft Punk - Discovery: the lookup failed (iTunes request failed after 3 attempts: ...); left unchanged.
```

One album failing says nothing about the next. **Three in a row** — whether
they could not reach the catalogue at all or never got through its throttling
— says the network is down or the catalogue is refusing everyone, so the
source is set aside at that point — otherwise a whole library would grind through five
attempts each to discover the same thing hours later. A single success clears
the count, so an occasional blip never accumulates into a false verdict.

## The scan always finishes

Setting a source aside stops it being *asked*; it never stops the run. Every
file under the folder is still visited and still gets a row in `--csv`,
including the ones nobody could answer for — so the report is a complete
account of the library rather than however far the scan got before something
went wrong. With a fallback chain the remaining catalogues simply take over.

A reason is printed once, not once per album. A source that has stopped
answering would otherwise repeat itself for every remaining album in the
library, which buries the part of the output worth reading.

Nothing is lost either way: a file nobody could answer for is left exactly as
it was, so `--only-missing` picks it up on a later run.

All of this lives in one place and applies to every request each source makes,
searches and detail fetches alike.

## One request per album

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

## Seeing what changed

`export` pairs well with this: dump the library, look at the
`Copyright (TCOP)` column, run the refresh, and dump it again.

```bash
music-tag-transfer export "/path/to/music" before.csv
music-tag-transfer copyright "/path/to/music"
music-tag-transfer export "/path/to/music" after.csv
```
