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
| `bridge-host-<version>-windows-x64.exe` | Native messaging host for the browser extension |
| `airlock-mobile-<version>-android-arm64.apk` | Android app, for Android 14 and later on arm64 phones |
| `airlock-extension-<version>.zip` | Browser extension, ready to load or to upload to a store |
| `SHA256SUMS.txt` | Checksum of every file above |
| `airlock-<version>.sigstore.json` | The provenance attestation of every file above, as a Sigstore bundle |

The online app is `apps/airlock`, bundled with `dx bundle -p airlock`. The airgap build is a separate package (`apps/airlock-offline`) bundled with `dx bundle -p airlock-offline`. `scripts/bundle-windows.sh` makes the three Windows files of each: it bundles once with the `webview_install_mode` in `Dioxus.toml` (`OfflineInstaller`), zips the app folder dx leaves (the exe and its `assets`), and bundles again with the small installer's mode. Every variant needs WebView2 at run time; none ships a fixed copy of it, which Windows Update would not patch. The portable app keeps its data where the installed one does, under `%LOCALAPPDATA%`, so the two share a vault, and only one copy runs on it at a time. The Android app is `apps/airlock-mobile`, bundled with `dx bundle -p airlock-mobile --android`; see [Signing the Android app](#signing-the-android-app).

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

One version number for everything. It is written in five files, and the release refuses to build unless they agree:

- `Cargo.toml`, under `[workspace.package]`. Every crate inherits it with `version.workspace = true`; `release-check.mjs` also checks that the apps, `wallet-*`, `qr-protocol` and `bridge-host` still do.
- `extension/package.json`, `extension/manifest.json`
- `apps/airlock/Info.plist`, `apps/airlock-offline/Info.plist`, as `CFBundleShortVersionString` and `CFBundleVersion`. `dx bundle` copies them into the macOS apps: the plist dx writes itself leaves out the camera description.

Versions follow semantic versioning. While the version is below 1.0, a change that breaks a stored file, the QR protocol or the bridge protocol raises the minor number.

## Changelog and release notes

`CHANGELOG.md` is written by hand, for users. A pull request that changes behavior adds a line under **Unreleased**, in one of Added, Changed, Fixed, Removed, Security. The Changelog workflow fails a pull request that does not touch the changelog unless it carries the `no-changelog` label, and checks again when the label is added or removed, which is right for refactors, tests and dependency updates.

The release notes are that version's section of the changelog, copied as is. There is nothing separate to write.

Commit messages are not parsed. They can stay as full sentences.

## Steps

1. **Prepare.** On a branch:
   - Set the new version in the files above, then run `cargo check` so `Cargo.lock` follows.
   - In `CHANGELOG.md`, rename `## [Unreleased]` to `## [1.2.3] - YYYY-MM-DD` and add a new empty `## [Unreleased]` above it.
   - Run `node scripts/release-check.mjs 1.2.3`. It prints the release notes, or says what is inconsistent.
2. **Merge** that pull request once CI and the security checks pass.
3. **Tag** the merge commit and push the tag:
   ```bash
   git tag -a v1.2.3 -m "Airlock 1.2.3"
   git push origin v1.2.3
   ```
4. **Wait** for the Release workflow. It re-runs every check, builds the files, and opens the draft.
5. **Check the draft.** Install the online app from its installer on a clean machine, load the extension zip, and confirm one QR transfer and one browser connect. Optionally install the airgap build and confirm a durable-nonce transfer can be built, signed and shown as QR. Start each portable zip once. Compare a checksum, and verify one file's attestation.
6. **Publish** the draft.

A version with a suffix, such as `1.2.3-rc.1`, is opened as a pre-release.

If a build is wrong, delete the draft and the tag, fix it on the default branch, and tag again. A published release is never replaced: fix forward with a new version.

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

## Not automated yet

- **Code signing.** The Windows installer is not signed, so Windows shows a SmartScreen warning. Signing needs a certificate and a decision on where its key lives.
- **Extension stores.** The zip is built but not uploaded to the Chrome Web Store or Firefox Add-ons. The extension id is pinned by the key in `extension/manifest.json`; a store listing will have its own.
- **Android.** Only arm64 is built, as an APK; no store listing (Google Play, Solana dApp Store). dx sets the APK's `versionCode` to 1 in every build, so Android cannot tell two releases apart by it: an update installs over the old one, but a store would refuse it.
- **macOS and Linux desktop builds.** CI builds the core crates on Linux, but no desktop bundle is produced for either.
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
