# Releasing

A release is a tag. Pushing `v1.2.3` builds the files and opens a **draft** release on GitHub. A person reads the draft, checks the files, and publishes it.

## What a release contains

| File | What it is |
| --- | --- |
| `airlock-<version>-windows-x64-setup.exe` | Online desktop installer. Downloads WebView2 while installing if Windows has none |
| `airlock-<version>-windows-x64-setup-full.exe` | The same, with WebView2's offline installer inside (about 190 MB more) |
| `airlock-<version>-windows-x64-portable.zip` | The online app without an installer: unzip and run `airlock.exe` |
| `airlock-offline-<version>-windows-x64-setup.exe` | Airgap desktop installer (opens no connection of its own). Needs WebView2 already installed, as Windows 11 has it |
| `airlock-offline-<version>-windows-x64-setup-full.exe` | The same, with WebView2's offline installer inside, for a machine that has never been online |
| `airlock-offline-<version>-windows-x64-portable.zip` | The airgap app without an installer |
| `airlock-<version>-macos-arm64.dmg` | Online app for Macs with Apple Silicon, ad-hoc signed |
| `airlock-offline-<version>-macos-arm64.dmg` | Airgap app for Macs with Apple Silicon, ad-hoc signed |
| `airlock-<version>-linux-amd64.deb` | Online app for Debian, Ubuntu and their kin, x86-64 |
| `airlock-<version>-linux-x86_64.AppImage` | Online app as one file, for other x86-64 Linux |
| `airlock-offline-<version>-linux-amd64.deb` | Airgap app, `.deb` |
| `airlock-offline-<version>-linux-x86_64.AppImage` | Airgap app, AppImage |
| `bridge-host-<version>-windows-x64.exe` | Native messaging host for the browser extension. The online installers and portable zip have it too |
| `bridge-host-<version>-macos-arm64.tar.gz` | The same, for macOS. The online app has it inside |
| `airlock-mobile-<version>-android-arm64.apk` | Android app, for Android 14 and later on arm64 phones |
| `airlock-extension-<version>.zip` | Browser extension, ready to load or to upload to a store |
| `THIRD-PARTY-NOTICES.md` | Where each IDL file the apps ship was taken from, and the licence of that source. Copied from the repository as it is at the tag |
| `SHA256SUMS.txt` | Checksum of every file above |
| `airlock-<version>.sigstore.json` | The provenance attestation of every file above, as a Sigstore bundle |

The online app is `apps/airlock`, bundled with `dx bundle -p airlock`. The airgap build is a separate package (`apps/airlock-offline`) bundled with `dx bundle -p airlock-offline`. `scripts/bundle-windows.sh` makes the three Windows files of each: it bundles once with the `webview_install_mode` in `Dioxus.toml` (`OfflineInstaller`), zips the app folder dx leaves (the exe and its `assets`), and bundles again with the small installer's mode. For the online app it first builds `bridge-host`, which `Dioxus.toml` lists in its `resources` (a path dx reads from the workspace root), and puts it in the zip too. The online installer is dx's NSIS template with a page that asks whether to register the browser bridge, `apps/airlock/installer.nsi`; compare it with dx's own (`src/bundler/windows.rs` in dioxus-cli) when `DX_VERSION` changes. `scripts/bundle-macos.sh` makes each macOS disk image: see [macOS](#macos). Every Windows variant needs WebView2 at run time; none ships a fixed copy of it, which Windows Update would not patch. The portable app keeps its data where the installed one does, under `%LOCALAPPDATA%`, so the two share a vault, and only one copy runs on it at a time. The Android app is `apps/airlock-mobile`, bundled with `dx bundle -p airlock-mobile --android`; see [Signing the Android app](#signing-the-android-app).

Each file also has a build provenance attestation: a signed statement that it was built by the release workflow from the tagged commit. Check one with:

```bash
gh attestation verify airlock-1.2.3-windows-x64-setup.exe --repo tiago18c/airlock
```

The same attestation is published as `airlock-<version>.sigstore.json`, so [cosign](https://docs.sigstore.dev/cosign/) checks it without GitHub's CLI or an account:

```bash
cosign verify-blob-attestation --bundle airlock-1.2.3.sigstore.json --type slsaprovenance1 \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/tiago18c/airlock/.github/workflows/release.yml@refs/tags/v' \
  airlock-1.2.3-windows-x64-setup.exe
```

Both prove where a file was built, to whoever checks. Neither is what Windows or Android check before running it: see [Signing the Android app](#signing-the-android-app) and code signing under [Not automated yet](#not-automated-yet).

## Versions

One version number for everything. It is written in six files. The release refuses to build unless the first five agree:

- `Cargo.toml`, under `[workspace.package]`. Every crate inherits it with `version.workspace = true`; `release-check.mjs` also checks that the apps, `wallet-*`, `qr-protocol` and `bridge-host` still do.
- `extension/package.json`, `extension/manifest.json`
- `apps/airlock/Info.plist`, `apps/airlock-offline/Info.plist`, as `CFBundleShortVersionString` and `CFBundleVersion`. `dx bundle` copies them into the macOS apps: the plist dx writes itself leaves out the camera description.
- `extension/package-lock.json`, in its two `version` fields (the top one and the one under `packages[""]`). `release-check.mjs` does not read it, so nothing fails when it is left behind: set it with the others.

Versions follow semantic versioning. While the version is below 1.0, a change that breaks a stored file, the QR protocol or the bridge protocol raises the minor number.

## Changelog and release notes

`CHANGELOG.md` is written by hand, for users. A pull request that changes behavior adds a line under **Unreleased**, in one of Added, Changed, Fixed, Removed, Security. The Changelog workflow fails a pull request that does not touch the changelog unless it carries the `no-changelog` label, and checks again when the label is added or removed, which is right for refactors, tests and dependency updates.

The release notes are [`docs/release-notes-header.md`](release-notes-header.md), with the version filled in, then that version's section of the changelog, copied as is. The header is the same for every release: the prototype warning, which file is for whom, how to load the extension, what Windows, macOS and Android say the first time, and how to check a download. Change it there, not on the draft. There is nothing else to write.

Commit messages are not parsed. They can stay as full sentences.

## Steps

1. **Prepare.** On a branch:
   - Set the new version in the files above, then run `cargo check` so `Cargo.lock` follows.
   - In `CHANGELOG.md`, rename `## [Unreleased]` to `## [1.2.3] - YYYY-MM-DD` and add a new empty `## [Unreleased]` above it.
   - Run `node scripts/release-check.mjs 1.2.3`. It prints the release notes, or says what is inconsistent.
2. **Merge** that pull request once CI, Security, Changelog and Dependency age pass.
3. **Tag** the merge commit and push the tag, on the day, once two things are green: Security for that commit (`gh run list --workflow Security --branch main --limit 1`), since the release does not run it; and a Program watch run on `main` (`gh run list --workflow "Program watch" --branch main --limit 1`), scheduled at 06:41 UTC or started by hand from the Actions tab. If Program watch reports something, deal with what its issue lists first.
   ```bash
   git tag -a v1.2.3 -m "Airlock 1.2.3"
   git push origin v1.2.3
   ```
4. **Wait** for the Release workflow. It runs `release-check.mjs` and CI (`ci.yml`: format, Clippy, the tests, the on-chain flows and the extension), builds the files, and opens the draft. It does not run Security (cargo-deny, npm audit, CodeQL) or Fuzz. A release takes 35 minutes with warm caches and about an hour cold. No job sets a time limit (`timeout-minutes`), so a stuck step, such as `apt-get` waiting on an unreachable mirror, runs until GitHub stops it after six hours: cancel the run and re-run its failed jobs.
5. **Check the draft.** Its notes already start with the header, so paste nothing into them by hand that the header covers. Install the online app from its installer on a clean machine, load the extension zip, and confirm one QR transfer and one browser connect. Optionally install the airgap build and confirm a durable-nonce transfer can be built, signed and shown as QR. Start each portable zip once. On a Mac with Apple Silicon, open each disk image, get the app past Gatekeeper, and confirm the scanner gets the camera. Compare a checksum, and verify one file's attestation.
6. **Publish** the draft.

A version with a suffix, such as `1.2.3-rc.1`, cannot be released yet, though the workflow would open it as a pre-release. `release-check.mjs` requires every file it reads to hold the whole version, suffix included, and `extension/build.mjs` copies `manifest.json` into the zip as it is. Chrome and Firefox load only a manifest `version` of one to four numbers separated by dots, so the extension would not load. The Windows installer writes `VIProductVersion "{{version}}.0"` (`apps/airlock/installer.nsi`), which NSIS requires to be four numbers, and macOS expects `CFBundleVersion` to be numbers separated by dots too. Until `release-check.mjs` compares those files with the version's numeric part, tag plain versions only: the nightly is the release candidate.

If a build is wrong, delete the draft and the tag, fix it on the default branch, and tag again. A published release is never replaced: fix forward with a new version.

## Nightly

`.github/workflows/build.yml` builds every file above from the commit it runs on. `release.yml` calls it for a tag; `nightly.yml` calls it once a day when `main` has moved since the last nightly, and publishes the files as a **prerelease** tagged `nightly`. Its schedule says 03:17 UTC, but GitHub starts a scheduled run when it has room: in November 2026 they started at about 11:00 UTC. A nightly takes the version in `Cargo.toml` and adds the date and commit, so its files are named like `airlock-0.1.0-nightly.20261113.1a2a81d-windows-x64-setup.exe`. That sorts below the release of the same number, as a pre-release version should.

- **There is only one.** Each nightly deletes the one before, its tag included, before it publishes, so old builds do not accumulate. Its workflow artifacts are kept for 3 days, not the repository's 90.
- **It is not "latest".** GitHub leaves prereleases out of `/releases/latest`, which the app's Extension chip and the README link to. Its notes are a warning that nobody has tried the build, the same header as a release's, and the changelog's Unreleased section.
- **It checks what a release checks**, except the changelog and version checks (`release-check.mjs`) and CI. CI runs on every push to `main`, but the nightly neither waits for it nor asks whether it passed: the commit it builds may still be in CI, may have failed it, or may never have had its own run, since a CI run still queued when the next push arrives is replaced by that push's. Neither the nightly nor the release runs Security or Fuzz. Both build alike: the Linux job installs the `.deb`, starts both formats and registers the bridge, and the Android APK is signed with the release key and refused if debuggable. A failed nightly mails whoever last changed the schedule, and leaves the previous nightly in place unless the publish step itself failed.
- **The same provenance.** Its files have attestations too; with cosign, check the identity `^https://github.com/tiago18c/airlock/.github/workflows/nightly.yml@refs/heads/main$`.
- **Run it by hand** from the Actions tab (Nightly, Run workflow) to build now, even when `main` has not moved. From another branch it builds and uploads the workflow artifacts without publishing, which is how to try a packaging change before merging it. Pull requests never build the bundles: CI only builds and tests.

## Signing the Android app

Android installs an update only when it is signed with the same key as the app already on the phone, so the key that signs the first release signs every one after it. Losing it means users have to uninstall, and lose the app's data, to move to a build signed with another key.

dx builds a release APK (Gradle's `assembleRelease`: R8, no debuggable flag, WebView debugging off) only when `Dioxus.toml` has a `[bundle.android]` section naming a keystore. Without one it builds a debug APK, even with `--release`. The release workflow writes that section and the keystore for its run only, from four repository secrets:

| Secret | What it holds |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | The keystore (`.jks`), base64 encoded |
| `ANDROID_KEYSTORE_PASSWORD` | The keystore's password |
| `ANDROID_KEY_ALIAS` | The alias of the key in it |
| `ANDROID_KEY_PASSWORD` | The key's password |

dx writes the passwords into Kotlin and TOML strings unescaped, so they cannot hold `"`, `\` or `$`. The key is made with `keytool`, from the JDK; no outside service is involved, and the certificate is self-signed. To make it once:

```bash
keytool -genkeypair -keystore airlock-release.jks -alias airlock -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Airlock"
base64 -w0 airlock-release.jks | gh secret set ANDROID_KEYSTORE_BASE64
gh secret set ANDROID_KEYSTORE_PASSWORD
gh secret set ANDROID_KEY_ALIAS --body airlock
gh secret set ANDROID_KEY_PASSWORD
```

In PowerShell, which has no `base64`:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("$PWD\airlock-release.jks")) | gh secret set ANDROID_KEYSTORE_BASE64 --repo tiago18c/airlock
```

The two password commands ask for the value. A PKCS12 keystore, which `keytool` makes since JDK 9, has one password for the store and the key.

Keep the `.jks` and its passwords somewhere safe outside GitHub as well: a secret cannot be read back.

The workflow refuses an APK that is debuggable, and prints its certificate. R8 drops Kotlin that nothing in Kotlin calls, so the bridges in `MainActivity.kt` that Rust finds by name are marked `@Keep`; a new one needs it too.

## macOS

The macOS apps are built for Apple Silicon only. There is no Developer ID, so they are not notarized. `scripts/bundle-macos.sh` has dx bundle the `.app` alone, signs it ad hoc (`codesign --sign -`; Apple Silicon runs nothing unsigned), checks that its `Info.plist` has the camera description, and makes the disk image with `hdiutil`. It leaves dx's own signing alone: given an identity, dx turns on the hardened runtime, under which the camera needs the `com.apple.security.device.camera` entitlement.

What a user sees:

- **The first launch is refused.** macOS 15 and later say Apple could not verify the app is free of malware, and offer only Done and Move to Trash; Control-click → Open no longer gets past it. Open it once, then **System Settings → Privacy & Security → Open Anyway**, and confirm with the password. Or remove the quarantine: `xattr -dr com.apple.quarantine "/Applications/Airlock.app"`.
- **The camera is asked for again after each update.** macOS ties the permission to the app's signature, and an ad-hoc signature changes with every build.
- **The browser bridge** is inside the online app, in `Contents/MacOS/bridge-host`, signed ad hoc on its own before the app. A disk image has no install step to ask in, so the app's Extension chip offers **Install the browser bridge**. That removes the host's quarantine first: a browser starts it directly, not through Finder, so macOS would otherwise refuse it even after the app was allowed. The chip refuses while the app runs from the mounted image or from App Translocation's temporary copy, whose paths do not last. Dragging the app to the Trash leaves the registration naming a missing binary, which the browser reports as a missing host; reinstalling and pressing Install again fixes it.
- **The standalone host** also comes as a tarball, which keeps its executable bit. A downloaded tarball is quarantined too, so after unpacking it run `xattr -d com.apple.quarantine bridge-host`, then `./bridge-host install`.

A Developer ID (the Apple Developer Program) would remove the first point: sign with the hardened runtime and the camera entitlement, then notarize with `notarytool`. dx does both when `APPLE_CERTIFICATE` and an App Store Connect API key are set.

## Linux

`.github/workflows/linux-bundle.yml` runs `scripts/bundle-linux.sh` for each desktop app on Ubuntu 22.04 with a `dx` built there (the prebuilt one needs glibc 2.39), the oldest Ubuntu still supported, because an AppImage runs only on a glibc at least as new as the one it was built on. The release calls it. Before uploading the files it installs the `.deb`, starts both formats under Xvfb for 20 seconds, and registers the bridge from each, so a broken package fails the release build. Pull requests do not run it: CI only builds and tests.

- **The `.deb`** is repacked by the script with `dpkg-deb --root-owner-group`: dx writes its files with the build user's uid, and dpkg would install them owned by whatever account has that uid. The script fails if any file is not root's. It puts the app in `/usr/bin` and declares the libraries it needs (`[bundle.deb] depends` in `Dioxus.toml`, by their Ubuntu 22.04 and Debian 12 names). It installs as root, and the browser bridge's registration is per user, so the package registers nothing: the online app's `/usr/bin/bridge-host` is offered by the Extension chip. Removing the package leaves a registration naming a missing binary, which the chip then shows as not installed.
- **The AppImage** carries the libraries `linuxdeploy` finds the app linked against, except those every system has (glibc, GL). WebKitGTK starts its helper processes from the system's own WebKitGTK, so the system needs `libwebkit2gtk-4.1` too, as Ubuntu and Fedora desktops have. An AppImage runs from a mount that goes away when it quits, so the chip registers a copy of its host, in the data folder, and refreshes that copy whenever the AppImage that registered it starts with a different one.
- dx finds the host through `external_bin` in `Dioxus.toml`: the script copies `target/release/bridge-host` to `bridge-host-<target triple>`, which dx puts in `usr/bin` under its own name.
- Neither is signed. The udev rules for hardware wallets (`resources/linux`) are not installed by either.

## Not automated yet

- **Code signing.** The Windows installer is not signed, so Windows shows a SmartScreen warning. Signing needs a certificate and a decision on where its key lives. The macOS apps are signed ad hoc and not notarized: see [macOS](#macos).
- **Extension stores.** The zip is built but not uploaded to the Chrome Web Store or Firefox Add-ons. The extension id is pinned by the key in `extension/manifest.json`; a store listing will have its own.
- **Android.** Only arm64 is built, as an APK; no store listing (Google Play, Solana dApp Store). dx sets the APK's `versionCode` to 1 in every build, so Android cannot tell two releases apart by it: an update installs over the old one, but a store would refuse it.
- **Linux arm64, and Intel Macs.** The Linux apps are built for x86-64 only, and the macOS apps for Apple Silicon only.
- **The web app.** It is not published anywhere by the release.

## Updating the compiler

`rust-toolchain.toml` pins the Rust version for everyone and for CI. Raise it in its own pull request, and fix whatever new lints it reports there.

## Updating dependencies

A new version waits 7 days, so a compromised release has time to be noticed and pulled before it reaches a build.

- Dependabot's version updates hold back for 7 days through `cooldown` in `.github/dependabot.yml`.
- The Dependency age workflow runs `scripts/check-dep-age.mjs` on every pull request. It fails if `Cargo.lock` or `extension/package-lock.json` takes a version published less than 7 days ago, whoever made the change. Git dependencies are not checked: `deny.toml` allows only listed repositories, pinned to a commit.
- GitHub Actions are pinned to commit SHAs, so a moved tag changes nothing until a Dependabot pull request says so. The check does not read them; the cooldown is what holds them back. Tools installed in a workflow, such as `cargo-fuzz`, are pinned to a version and raised by hand.

To take a version sooner because it fixes a vulnerability:

1. Dependabot security updates ignore the cooldown and open a pull request on their own. Otherwise, make the update by hand.
2. Check the advisory and what changed in the new version.
3. Label the pull request `security-override`. The check passes for that pull request and the label stays on it as the record.
