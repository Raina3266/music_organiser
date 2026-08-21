# music-tag-transfer

A small, dependency-light CLI for recursively editing ID3 metadata. Changed
files are written as ID3v2.3.

## Delete tags recursively

Quote the bracketed list so the shell passes it as one argument:

```bash
cargo run -- delete "/path/to/music" "[Encoded-by, Album Artist]"
```

Tag names are **case-sensitive**. A file without any requested tag is left
untouched. Files without an ID3 tag are skipped. Subdirectories are searched
recursively, symlinks are ignored, and MP3/MP2/MP1, WAV, AIFF, and AIF
containers are supported.

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

The original transfer behavior remains available, now recursively and without
machine-specific paths. Source and destination tracks are matched by title:

```bash
cargo run -- transfer "/source/music" "/destination/music" USLT
```

The previous three-positional-argument form remains supported for
compatibility.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
