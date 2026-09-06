"""Search YouTube Music through ytmusicapi, one JSON request per line.

Started once and kept open for the whole run, because importing ytmusicapi and
handshaking with YouTube Music costs far more than a search does: paying that
per row would dominate the time a long CSV takes.

Requests arrive on stdin as one JSON object per line, `{"query": ..., "limit":
...}`, and each is answered on stdout by exactly one JSON object per line, so
neither side ever has to guess where a message ends. Only the fields the
matching in Rust actually reads are passed back; ytmusicapi returns a great
deal more per result, and forwarding it would just be noise on the pipe.

Every error is reported in-band as `{"ok": false, ...}` rather than by dying:
one unanswerable query must not take the rest of the CSV down with it.
"""

import json
import sys


def fail(error, fatal=False):
    """Report a failure without stopping, unless nothing can continue."""
    return {"ok": False, "error": str(error), "fatal": fatal}


def reply(message):
    # Flushed every time: the reader is blocked on this line, so a reply left
    # sitting in the buffer would deadlock both sides.
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def names(values):
    """The names out of ytmusicapi's list of credited artists."""
    found = []
    for value in values or []:
        name = (value or {}).get("name") if isinstance(value, dict) else value
        if isinstance(name, str) and name.strip():
            found.append(name)
    return found


def result_of(item):
    """One search result, reduced to what deciding on a match needs."""
    if not isinstance(item, dict):
        return None
    video = item.get("videoId")
    if not isinstance(video, str) or not video:
        # An album or an artist rather than a recording. There is nothing to
        # link to, so it is dropped here rather than travelling to Rust to be
        # dropped there.
        return None
    album = item.get("album")
    return {
        "videoId": video,
        "title": item.get("title") or "",
        "artists": names(item.get("artists")),
        "album": (album or {}).get("name") or "" if isinstance(album, dict) else "",
        "resultType": item.get("resultType") or "",
        "duration_seconds": item.get("duration_seconds"),
    }


def main():
    try:
        from ytmusicapi import YTMusic
    except ImportError as error:
        reply(fail(f"cannot import ytmusicapi: {error}", fatal=True))
        return 1

    try:
        client = YTMusic()
    except Exception as error:  # noqa: BLE001 - any failure here is fatal
        reply(fail(f"cannot start ytmusicapi: {error}", fatal=True))
        return 1

    reply({"ok": True, "ready": True})

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except ValueError as error:
            reply(fail(f"unreadable request: {error}"))
            continue

        query = request.get("query") or ""
        limit = request.get("limit") or 10
        # "songs" keeps podcasts, playlists, and user uploads out of the
        # results; a music video of the same song is a different recording and
        # is not what a Spotify track means.
        search_filter = request.get("filter") or "songs"

        try:
            found = client.search(query, filter=search_filter, limit=limit)
        except Exception as error:  # noqa: BLE001 - reported, never fatal
            reply(fail(error))
            continue

        results = [result_of(item) for item in found or []]
        reply({"ok": True, "results": [item for item in results if item]})

    return 0


if __name__ == "__main__":
    sys.exit(main())
