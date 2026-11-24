# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-24

First public release.

### Added
- `bcp`: parallel copy with progress bars (bytes, files, directories, throughput, ETA).
- `bmv`: atomic rename on the same filesystem; copy, flush, verify, then delete across filesystems.
  The source is kept if anything was not transferred or changed during the move.
- Atomic writes: temporary entry next to the destination, then a no-clobber rename
  (`renameat2(RENAME_NOREPLACE)` on Linux, `renamex_np(RENAME_EXCL)` on macOS).
- APFS clones on macOS, `copy_file_range` on Linux, sparse files, hard links, FIFOs, symlinks.
- Metadata preservation: mode, ownership, timestamps, extended attributes (`-p`, `-a`, `--preserve=`).
- `cp`/`mv`-style options: `-r/-R -a -f -n -i -u -b -S -t -T -L -P -H`, plus `-j`, `--buffer-size`,
  `--sync=none|batch|full`, `--strict`, `--dry-run`, `-q`, `--no-progress`.
- Exit codes: 0 success, 1 error, 2 usage, 130 interrupted. Ctrl+C stops cleanly; a second Ctrl+C
  removes temporary files and quits.
- Shell completions (bash, zsh, fish) and man pages.
- Prebuilt binaries for macOS (arm64, x86_64) and Linux (static musl, x86_64, aarch64),
  Homebrew tap and install script.

[Unreleased]: https://github.com/Nexgear75/bcp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Nexgear75/bcp/releases/tag/v0.1.0
