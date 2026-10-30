#!/bin/sh
# Installs the stow binary from a GitHub Release.
#
# Environment:
#   STOW_INSTALL_DIR  where to put the binary (default: $HOME/.local/bin)
#   STOW_VERSION      version to install, with or without a leading "v"
#                     (default: the latest published release)
#   STOW_BASE_URL     where to download the archive and SHA256SUMS from
#                     (default: the GitHub Release for the chosen version).
#                     Useful for testing downloaded assets.
set -eu

REPO="karldreher/stow"

fail() {
    echo "error: $*" >&2
    exit 1
}

case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-musl" ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
esac

target="$arch-$os"

command -v curl >/dev/null 2>&1 || fail "curl is required"

if [ -n "${STOW_VERSION:-}" ]; then
    tag="v${STOW_VERSION#v}"
else
    # The /releases/latest page redirects to the newest published tag.
    tag="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" ||
        fail "could not look up the latest release"
    tag="${tag##*/}"
    case "$tag" in
        v[0-9]*) ;;
        *) fail "no published release found; set STOW_VERSION or publish a release first" ;;
    esac
fi
version="${tag#v}"

archive="stow-$version-$target.tar.gz"
base_url="${STOW_BASE_URL:-https://github.com/$REPO/releases/download/$tag}"
install_dir="${STOW_INSTALL_DIR:-$HOME/.local/bin}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading $archive"
curl -fsSL -o "$tmp/$archive" "$base_url/$archive" || fail "could not download $base_url/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base_url/SHA256SUMS" || fail "could not download $base_url/SHA256SUMS"

expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || fail "$archive is not listed in SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$archive" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')"
else
    fail "sha256sum or shasum is required to verify the download"
fi
[ "$actual" = "$expected" ] || fail "checksum mismatch for $archive (expected $expected, got $actual)"

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$install_dir"
cp "$tmp/stow-$version-$target/stow" "$install_dir/stow"
chmod 755 "$install_dir/stow"

echo "installed stow $version to $install_dir/stow"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "note: $install_dir is not on your PATH" >&2 ;;
esac
