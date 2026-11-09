#!/usr/bin/env bash
# Builds the macOS release file of one desktop app, for Apple Silicon:
#
#   <package>-<version>-macos-arm64.dmg   the .app, ad-hoc signed, beside a link to /Applications
#
#   scripts/bundle-macos.sh airlock 0.1.0 out
#
# dx signs only with an identity, and then turns on the hardened runtime, which
# needs a camera entitlement. Without a Developer ID there is none, so dx
# bundles the .app alone, it is signed ad hoc here (Apple Silicon runs nothing
# unsigned), and hdiutil makes the disk image. Gatekeeper still asks the user
# to allow it once: see docs/releasing.md. Run from the workspace root, on a
# Mac.
#
# The online app (its Dioxus.toml lists bridge-host in its resources) gets the
# browser bridge's host in Contents/MacOS, beside its own binary, where the
# Extension chip looks for it. A disk image has no install step to ask in, so
# the chip is where it is offered.
set -euo pipefail

package="$1"
version="$2"
out="$3"

rm -rf "target/dx/$package/bundle/macos"
dx bundle -p "$package" --release --desktop --features bundle --package-types macos

app="$(find "target/dx/$package/bundle" -maxdepth 3 -name '*.app' -type d | head -n 1)"
if [ -z "$app" ]; then
  echo "::error::dx produced no .app for $package."
  find target/dx -name '*.app' -maxdepth 6 2>/dev/null || true
  exit 1
fi

# Without this key WKWebView hides the camera from the page (the Info.plist
# in apps/<package>/ adds it; dx's own plist leaves it out).
/usr/libexec/PlistBuddy -c 'Print :NSCameraUsageDescription' "$app/Contents/Info.plist" > /dev/null

if grep -q 'bridge-host' "apps/$package/Dioxus.toml"; then
  cargo build --locked --release -p bridge-host
  cp target/release/bridge-host "$app/Contents/MacOS/bridge-host"
  # Signed on its own first: the browser runs it directly, not as part of the app.
  codesign --force --sign - "$app/Contents/MacOS/bridge-host"
fi

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

stage="$(mktemp -d)"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$app/Contents/Info.plist")"
mkdir -p "$out"
hdiutil create -volname "$name" -srcfolder "$stage" -ov -format UDZO "$out/$package-$version-macos-arm64.dmg"
rm -rf "$stage"
