#!/bin/sh
# Install bcp and bmv from the GitHub releases of Nexgear75/bcp.
#
#   curl -fsSL https://raw.githubusercontent.com/Nexgear75/bcp/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --to /usr/local/bin
#
# Options (or environment variables):
#   --version TAG   release to install (BCP_VERSION, default: latest)
#   --to DIR        install directory  (BCP_INSTALL_DIR, default: ~/.local/bin)
#
# The archive's SHA-256 is checked against the release's sha256sums.txt.

set -eu

REPO="Nexgear75/bcp"
VERSION="${BCP_VERSION:-latest}"
DEST="${BCP_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'bcp-install: %s\n' "$*" >&2; }
die() {
    say "error: $*"
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
    --version)
        [ $# -ge 2 ] || die "--version needs a value"
        VERSION="$2"
        shift 2
        ;;
    --to)
        [ $# -ge 2 ] || die "--to needs a value"
        DEST="$2"
        shift 2
        ;;
    -h | --help)
        printf 'usage: install.sh [--version TAG] [--to DIR]\n'
        exit 0
        ;;
    *) die "unknown option: $1" ;;
    esac
done

case "$(uname -s)" in
Darwin) os="apple-darwin" ;;
Linux) os="unknown-linux-musl" ;;
*) die "unsupported system: $(uname -s) (bcp supports macOS and Linux)" ;;
esac
case "$(uname -m)" in
arm64 | aarch64) arch="aarch64" ;;
x86_64 | amd64) arch="x86_64" ;;
*) die "unsupported architecture: $(uname -m)" ;;
esac
# A shell running under Rosetta reports x86_64: prefer the native binary.
if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch="aarch64"
fi
target="${arch}-${os}"
archive="bcp-${target}.tar.gz"

if [ "$VERSION" = latest ]; then
    base="https://github.com/${REPO}/releases/latest/download"
else
    case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac
    base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

if command -v curl >/dev/null 2>&1; then
    fetch() { curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --https-only "$1" -O "$2"; }
else
    die "curl or wget is required"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "sha256sum or shasum is required to verify the download"
fi

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t bcp-install)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading ${archive} (${VERSION})"
fetch "${base}/${archive}" "${tmp}/${archive}" || die "download failed: ${base}/${archive}"
fetch "${base}/sha256sums.txt" "${tmp}/sha256sums.txt" || die "download failed: ${base}/sha256sums.txt"

expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "${tmp}/sha256sums.txt")"
[ -n "$expected" ] || die "${archive} is not listed in sha256sums.txt"
actual="$(sha256 "${tmp}/${archive}")"
[ "$expected" = "$actual" ] || die "checksum mismatch for ${archive} (expected ${expected}, got ${actual})"

tar -xzf "${tmp}/${archive}" -C "$tmp"
src="${tmp}/bcp-${target}"
[ -x "${src}/bcp" ] && [ -x "${src}/bmv" ] || die "unexpected archive layout"

mkdir -p "$DEST" || die "cannot create ${DEST}"
for bin in bcp bmv; do
    # Write next to the target, then rename: never leaves a half-written binary.
    cp "${src}/${bin}" "${DEST}/.${bin}.new"
    chmod 755 "${DEST}/.${bin}.new"
    mv -f "${DEST}/.${bin}.new" "${DEST}/${bin}"
done

say "installed $("${DEST}/bcp" --version) and bmv into ${DEST}"
case ":${PATH}:" in
*":${DEST}:"*) ;;
*) say "note: ${DEST} is not in your PATH; add it to your shell profile" ;;
esac
say "man pages and shell completions are in the archive: https://github.com/${REPO}/releases"
