#!/usr/bin/env bash
set -euo pipefail

# WEF 03448978's matching CEF distribution. The archive is intentionally not
# committed: the Linux package is roughly a gigabyte once unpacked.
version='137.0.10+g7e14fe1+chromium-137.0.7151.69'
archive="cef_binary_${version}_linux64.tar.bz2"
url="https://cef-builds.spotifycdn.com/${archive//+/%2B}"
expected_sha1='0af5cc3840af1cfcccd4a602f55f41d3df22bdca'
destination="${ARTIST_CEF_DIR:-$PWD/.cache/cef}"
download="$destination/$archive"

mkdir -p "$destination"
if [[ ! -f "$download" ]] || ! printf '%s  %s\n' "$expected_sha1" "$download" | sha1sum --check --status; then
  curl --fail --location --output "$download.part" "$url"
  printf '%s  %s\n' "$expected_sha1" "$download.part" | sha1sum --check
  mv "$download.part" "$download"
fi

root="$destination/cef_binary_${version}_linux64"
if [[ ! -d "$root" ]]; then
  tar -xjf "$download" -C "$destination"
fi

printf 'CEF_ROOT=%s\n' "$root"
