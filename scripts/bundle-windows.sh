#!/usr/bin/env bash
# Builds the Windows release files of one desktop app:
#
#   <package>-<version>-windows-x64-setup-full.exe   installer with WebView2's offline installer inside
#   <package>-<version>-windows-x64-setup.exe        installer without it (see <small-mode>)
#   <package>-<version>-windows-x64-portable.zip     the app and its assets, no installer
#
#   scripts/bundle-windows.sh airlock 0.1.0 DownloadBootstrapper out
#   scripts/bundle-windows.sh airlock-offline 0.1.0 Skip out
#
# <small-mode> is the dx `webview_install_mode` of the small installer:
# DownloadBootstrapper fetches WebView2 while installing when Windows lacks it,
# Skip assumes it is there. The full installer uses the mode in Dioxus.toml,
# which has to be OfflineInstaller. Run from the workspace root, in Git Bash
# on Windows. See docs/releasing.md.
#
# A Dioxus.toml that lists target/release/bridge-host.exe in its resources
# (the online app) gets the browser bridge's host built first, in its
# installers and in its portable zip.
set -euo pipefail

package="$1"
version="$2"
small_mode="$3"
out="$4"

toml="apps/$package/Dioxus.toml"
app_dir="target/dx/$package/release/windows/app"

if ! grep -q '^webview_install_mode = { OfflineInstaller' "$toml"; then
  echo "::error::$toml has to set webview_install_mode to OfflineInstaller."
  exit 1
fi
case "$small_mode" in
  Skip) small_value='"Skip"' ;;
  DownloadBootstrapper | EmbedBootstrapper) small_value="{ $small_mode = { silent = false } }" ;;
  *) echo "::error::Unknown small installer mode $small_mode."; exit 1 ;;
esac

# The small installer's build rewrites Dioxus.toml; put it back however this ends.
cp "$toml" "$toml.orig"
trap 'mv -f "$toml.orig" "$toml"' EXIT

mkdir -p "$out"

with_bridge=false
if grep -q 'target/release/bridge-host.exe' "$toml"; then
  with_bridge=true
  cargo build --locked --release -p bridge-host
fi

# Builds the installer and copies it to $out/$1.
bundle() {
  find "target/dx/$package" -path '*nsis*' -name '*-setup.exe' -delete 2>/dev/null || true
  dx bundle -p "$package" --release --desktop --features bundle --package-types nsis
  local installer
  installer="$(find "target/dx/$package" -path '*nsis*' -name '*-setup.exe' 2>/dev/null | head -n 1)"
  if [ -z "$installer" ]; then
    echo "::error::dx produced no installer for $package."
    find target/dx -name '*.exe' 2>/dev/null || true
    exit 1
  fi
  cp "$installer" "$out/$1"
}

bundle "$package-$version-windows-x64-setup-full.exe"

# The portable zip: the exe finds its assets in the folder beside it.
stage="$(mktemp -d)"
mkdir "$stage/$package-$version"
cp "$app_dir/$package.exe" "$stage/$package-$version/"
cp -r "$app_dir/assets" "$stage/$package-$version/"
if $with_bridge; then
  cp target/release/bridge-host.exe "$stage/$package-$version/"
fi
(cd "$stage" && 7z a -tzip -bso0 "$package-$version.zip" "$package-$version")
mv "$stage/$package-$version.zip" "$out/$package-$version-windows-x64-portable.zip"
rm -rf "$stage"

sed -i "s/^webview_install_mode = .*/webview_install_mode = $small_value/" "$toml"
grep -q "^webview_install_mode = $small_value" "$toml"
bundle "$package-$version-windows-x64-setup.exe"
