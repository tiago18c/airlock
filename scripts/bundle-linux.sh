#!/usr/bin/env bash
# Builds the Linux release files of one desktop app, for x86-64:
#
#   <package>-<version>-linux-amd64.deb        for Debian, Ubuntu and their kin
#   <package>-<version>-linux-x86_64.AppImage  one file that runs on most others
#
#   scripts/bundle-linux.sh airlock 0.1.0 out
#
# The online app (its Dioxus.toml names bridge-host in external_bin) gets the
# browser bridge's host in usr/bin, beside its own binary, where the Extension
# chip looks for it. Neither format has an install step that could ask: a .deb
# installs as root, and the registration is per user. So the chip is where it
# is offered. Run from the workspace root, on the oldest Linux the AppImage is
# meant to run on (its glibc is the floor). See docs/releasing.md.
set -euo pipefail

package="$1"
version="$2"
out="$3"

toml="apps/$package/Dioxus.toml"
mkdir -p "$out"

if grep -q '^external_bin = \["target/release/bridge-host"\]' "$toml"; then
  triple="$(rustc -vV | sed -n 's/^host: //p')"
  cargo build --locked --release -p bridge-host
  # dx looks for the file with the target triple appended, and drops it again.
  cp target/release/bridge-host "target/release/bridge-host-$triple"
fi

rm -rf "target/dx/$package/bundle/linux"
dx bundle -p "$package" --release --desktop --features bundle --package-types deb --package-types appimage

deb="$(find "target/dx/$package/bundle" -name '*.deb' | head -n 1)"
appimage="$(find "target/dx/$package/bundle" -name '*.AppImage' | head -n 1)"
if [ -z "$deb" ] || [ -z "$appimage" ]; then
  echo "::error::dx produced no .deb or no AppImage for $package."
  find target/dx -name '*.deb' -o -name '*.AppImage' 2>/dev/null || true
  exit 1
fi

if [ -n "${triple:-}" ]; then
  if ! dpkg-deb -c "$deb" | grep -Eq '(^|[ /])usr/bin/bridge-host$'; then
    echo "::error::$deb has no usr/bin/bridge-host."
    dpkg-deb -c "$deb"
    ls -l target/release/bridge-host*
    exit 1
  fi
fi

cp "$deb" "$out/$package-$version-linux-amd64.deb"
cp "$appimage" "$out/$package-$version-linux-x86_64.AppImage"
chmod +x "$out/$package-$version-linux-x86_64.AppImage"
