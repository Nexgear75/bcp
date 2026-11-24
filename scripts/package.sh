#!/bin/sh
# Package the release binaries of one target into dist/bcp-<target>.tar.gz.
#
#   scripts/package.sh <target> <assets-dir>
#
# <assets-dir> holds completions/ and man/ (from `cargo xtask assets`).
set -eu

target="$1"
assets="$2"
name="bcp-${target}"
stage="dist/${name}"

rm -rf "$stage"
mkdir -p "$stage"
cp "target/${target}/release/bcp" "target/${target}/release/bmv" "$stage/"
cp README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$stage/"
cp -R "${assets}/completions" "${assets}/man" "$stage/"

# Reproducible-ish archive: fixed owner, sorted entries.
(cd dist && tar --format=ustar -cf - "$name" | gzip -9n >"${name}.tar.gz")
rm -rf "$stage"
echo "dist/${name}.tar.gz"
