#!/usr/bin/env bash
# Gives the APK dx just built Airlock's launcher icon:
#
#   dx build --android -p airlock-mobile --target x86_64-linux-android
#   scripts/android-icon.sh debug
#
#   dx bundle -p airlock-mobile --release --android --target aarch64-linux-android --package-types apk
#   scripts/android-icon.sh release
#
# dx 0.7.10 writes a placeholder launcher icon into the Gradle project it
# generates, target/dx/airlock-mobile/<profile>/android/app, on every build,
# and takes no Android icon from Dioxus.toml: neither [bundle] icon nor
# [android] icon reaches it. So this copies apps/airlock-mobile/android/res
# (made by scripts/make-android-icons.py) over dx's icon files, takes dx's
# placeholder drawables out, and runs the Gradle task dx ran again, in that
# project. Gradle rebuilds the APK where dx left it, signed as dx's was: with
# the debug key, or, when Dioxus.toml has [bundle.android], with the key dx
# wrote into app/build.gradle.kts. Run it from the workspace root, after
# every dx build of the app, with JAVA_HOME and ANDROID_HOME set as for dx.
#
# It stops, and changes nothing, if the project is not laid out as dx 0.7.10
# lays it out. A newer dx may supply icons itself: check before raising
# DX_VERSION.
set -euo pipefail

profile="${1:-}"
case "$profile" in
  debug | release) ;;
  *)
    echo "usage: $0 debug|release" >&2
    exit 2
    ;;
esac

ours="apps/airlock-mobile/android/res"
project="target/dx/airlock-mobile/$profile/android/app"
res="$project/app/src/main/res"
densities="mdpi hdpi xhdpi xxhdpi xxxhdpi"

fail() {
  echo "::error::$1" >&2
  echo "scripts/android-icon.sh expects the Gradle project dx 0.7.10 generates. Nothing was changed." >&2
  exit 1
}

[ -f "$ours/mipmap-anydpi-v26/ic_launcher.xml" ] || fail "$ours is missing: run from the workspace root."
[ -d "$project" ] || fail "No $project: build the app with dx first."
for file in gradlew app/build.gradle.kts app/src/main/AndroidManifest.xml; do
  [ -f "$project/$file" ] || fail "No $project/$file."
done
grep -q 'android:icon="@mipmap/ic_launcher"' "$project/app/src/main/AndroidManifest.xml" ||
  fail "The generated manifest does not use @mipmap/ic_launcher."

# Every launcher resource there is one dx writes or one of ours. Anything
# else, such as a round icon or an adaptive icon for a later Android, could
# be what a phone shows instead.
expected="drawable-v24/ic_launcher_foreground.xml drawable/ic_launcher_background.xml mipmap-anydpi-v26/ic_launcher.xml"
for density in $densities; do
  expected="$expected mipmap-$density/ic_launcher.webp"
done
found="$(cd "$res" && find . -type f \( -name 'ic_launcher*' -o -path './mipmap-anydpi*' \) | sed 's|^\./||' | sort)"
for file in $found; do
  case " $expected " in
    *" $file "*) ;;
    *) fail "Unexpected launcher resource $res/$file." ;;
  esac
done
for file in mipmap-anydpi-v26/ic_launcher.xml $(for d in $densities; do echo "mipmap-$d/ic_launcher.webp"; done); do
  [ -f "$res/$file" ] || fail "No $res/$file."
done
# dx's adaptive icon, or ours from an earlier run.
adaptive="$res/mipmap-anydpi-v26/ic_launcher.xml"
if ! cmp -s "$adaptive" "$ours/mipmap-anydpi-v26/ic_launcher.xml"; then
  grep -q '@drawable/ic_launcher_foreground' "$adaptive" && grep -q '@drawable/ic_launcher_background' "$adaptive" ||
    fail "$adaptive is not the adaptive icon dx writes."
fi

# dx runs assembleRelease only for a release build with [bundle.android], and
# then writes the release signing config; otherwise assembleDebug.
task=assembleDebug
apk="$project/app/build/outputs/apk/debug/app-debug.apk"
if [ "$profile" = release ] && grep -q 'signingConfig = signingConfigs.getByName("release")' "$project/app/build.gradle.kts"; then
  task=assembleRelease
  apk="$project/app/build/outputs/apk/release/app-release.apk"
fi
[ -f "$apk" ] || fail "No $apk: dx's build did not finish."

cp -R "$ours/." "$res/"
# Only dx's adaptive icon, now replaced, names these.
rm -f "$res/drawable/ic_launcher_background.xml" "$res/drawable-v24/ic_launcher_foreground.xml"

echo "Running Gradle's $task in $project"
(cd "$project" && ./gradlew "$task")
echo "Gave $apk Airlock's icon."
