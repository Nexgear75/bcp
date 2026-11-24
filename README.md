# bcp / bmv

[![CI](https://github.com/Nexgear75/bcp/actions/workflows/ci.yml/badge.svg)](https://github.com/Nexgear75/bcp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Nexgear75/bcp)](https://github.com/Nexgear75/bcp/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

`cp` and `mv` with progress bars, atomic writes and data-loss protection.
For macOS and Linux.

```
$ bcp -r ~/Photos /Volumes/Backup/
Bytes ┃█████████████████▓░░░░░░░░░░░░┃  57% │ 212.40 GB/371.02 GB │ 812.31 MB/s │ ETA 3m
Files ┃█████████████████▓░░░░░░░░░░░░┃ 38112/64870
```

- **Progress you can trust**: a scan first, then live totals for bytes, files and
  directories, with throughput and ETA.
- **Never a half-written file**: every file is written to a temporary entry
  next to its destination and atomically renamed into place. An existing
  destination is never replaced unless you ask for it (`-f`, `-u`, `-i`).
- **Safe moves**: on the same filesystem `bmv` is a single atomic rename.
  Across filesystems it copies, flushes to disk, verifies, and only then
  deletes the source. If anything fails, the source is left untouched.
- **Fast**: parallel copies, APFS clones on macOS, `copy_file_range` on
  Linux, sparse files kept sparse, hard links preserved.
- **Familiar**: the flags you know from `cp` and `mv` (`-r -a -f -n -i -u -b
  -t -T -L -P -H -p --preserve`).

## Install

### Homebrew (macOS and Linux)

```sh
brew install nexgear75/tap/bcp
```

This installs `bcp`, `bmv`, their man pages and shell completions.

### Install script (macOS and Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/Nexgear75/bcp/main/install.sh | sh
```

It downloads the right binary for your system from the latest release,
checks its SHA-256 and installs `bcp` and `bmv` into `~/.local/bin`.
Options are passed after `sh -s --`:

```sh
# a specific version, into /usr/local/bin
curl -fsSL https://raw.githubusercontent.com/Nexgear75/bcp/main/install.sh | sh -s -- --version v0.1.0 --to /usr/local/bin
```

Linux binaries are static (musl), so they run on any distribution.

### Prebuilt archives

Every [release](https://github.com/Nexgear75/bcp/releases) has archives for
`aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, with
`sha256sums.txt`. Each archive contains the two binaries, man pages and
completions.

### From source

Requires Rust 1.88 or newer.

```sh
cargo install --locked --git https://github.com/Nexgear75/bcp bcp bmv
```

## Usage

```sh
bcp file.txt backup/                 # copy into a directory
bcp -r photos/ /Volumes/Disk/        # copy a tree
bcp -a project/ /mnt/backup/project  # archive: recursive + all metadata
bcp -u -r src/ dst/                  # only replace older files
bmv big-folder/ /Volumes/Disk/       # move (rename or copy + verify + delete)
bmv --dry-run a b                    # show what would happen
```

Run `bcp --help`, `bmv --help`, `man bcp` or `man bmv` for every option.

### Existing destinations

| Flag | Behaviour |
|---|---|
| *(none)* | refuse, with an error (exit 1) |
| `-f` | replace |
| `-n` | skip silently |
| `-i` | ask |
| `-u` | replace only if the source is newer |
| `-b` / `-S SUFFIX` | keep a backup of the replaced file (`NAME~`) |

When several of `-f`, `-n` and `-i` are given, the last one wins.

### Durability (`--sync`)

| Mode | What it does |
|---|---|
| `batch` *(default)* | flushes everything once at the end (one `syncfs` on Linux, `F_FULLFSYNC` on macOS) |
| `full` | flushes each file before installing it: slowest, safest |
| `none` | no explicit flush, like `cp` (renames stay atomic) |

`bmv` always flushes before deleting a source.

### Exit status

`0` success, `1` at least one error, `2` usage error, `130` interrupted.

The first Ctrl+C finishes the current chunk and stops cleanly. A second
one quits immediately and removes temporary files.

## Platform support

| | macOS (Apple Silicon, Intel) | Linux (x86_64, aarch64) |
|---|---|---|
| Fast path | APFS `clonefile` | `copy_file_range` |
| Atomic no-clobber rename | `renamex_np(RENAME_EXCL)` | `renameat2(RENAME_NOREPLACE)` |
| Metadata | mode, owner, timestamps, xattrs | mode, owner, timestamps, xattrs |

Windows is not supported.

## Known limitations

- An interrupted cross-filesystem `bmv` cannot be resumed yet: the source
  is intact, but the partial destination must be removed before running it
  again.
- `bmv` checks that every entry was transferred and that the sources did
  not change, but does not re-read the copied data to compare checksums.
- When started with `nohup` or in the background of a non-interactive
  shell, signals are not intercepted, so a kill can leave `*.bcptmp`
  temporary files behind (sources are never affected).
- Sockets and device files are reported as errors and not copied.

## Development

```sh
cargo test --workspace                        # unit + integration tests
BCP_REQUIRE_XDEV=1 cargo test --workspace     # fail instead of skipping cross-filesystem tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo xtask assets target/assets              # completions + man pages
```

Cross-filesystem tests use `/dev/shm` on Linux and a temporary APFS disk
image on macOS, or any directory given in `BCP_XDEV_DIR`.

### Releasing

1. Bump `version` in the root `Cargo.toml` and add a section to `CHANGELOG.md`.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.

The release workflow builds the four targets, publishes the GitHub release
with checksums and updates the formula in
[Nexgear75/homebrew-tap](https://github.com/Nexgear75/homebrew-tap).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
