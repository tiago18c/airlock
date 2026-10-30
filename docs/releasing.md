# Releasing

A release is a tag. Pushing `v1.2.3` builds the files and opens a **draft** release on GitHub. A person reads the draft, checks the files, and publishes it.

## What a release contains

| File | What it is |
| --- | --- |
| `airlock-<version>-windows-x64-setup.exe` | Online desktop installer |
| `airlock-offline-<version>-windows-x64-setup.exe` | Airgap desktop installer (no network stacks) |
| `bridge-host-<version>-windows-x64.exe` | Native messaging host for the browser extension |
| `airlock-extension-<version>.zip` | Browser extension, ready to load or to upload to a store |
| `SHA256SUMS.txt` | Checksum of every file above |

The online app is `apps/airlock`, bundled with `dx bundle -p airlock`. The airgap build is a separate package (`apps/airlock-offline`) bundled with `dx bundle -p airlock-offline`.

Each file also has a build provenance attestation: a signed statement that it was built by the release workflow from the tagged commit. Check one with:

```bash
gh attestation verify airlock-1.2.3-windows-x64-setup.exe --repo <owner>/airlock
```

## Versions

One version number for everything. It is written in three places, and the release refuses to build unless they agree:

- `Cargo.toml`, under `[workspace.package]`. Every crate inherits it with `version.workspace = true`; `release-check.mjs` also checks that the apps, `wallet-*`, `qr-protocol` and `bridge-host` still do.
- `extension/package.json`, `extension/manifest.json`

Versions follow semantic versioning. While the version is below 1.0, a change that breaks a stored file, the QR protocol or the bridge protocol raises the minor number.

## Changelog and release notes

`CHANGELOG.md` is written by hand, for users. A pull request that changes behavior adds a line under **Unreleased**, in one of Added, Changed, Fixed, Removed, Security. The Changelog workflow fails a pull request that does not touch the changelog unless it carries the `no-changelog` label, and checks again when the label is added or removed, which is right for refactors, tests and dependency updates.

The release notes are that version's section of the changelog, copied as is. There is nothing separate to write.

Commit messages are not parsed. They can stay as full sentences.

## Steps

1. **Prepare.** On a branch:
   - Set the new version in the three files above, then run `cargo check` so `Cargo.lock` follows.
   - In `CHANGELOG.md`, rename `## [Unreleased]` to `## [1.2.3] - YYYY-MM-DD` and add a new empty `## [Unreleased]` above it.
   - Run `node scripts/release-check.mjs 1.2.3`. It prints the release notes, or says what is inconsistent.
2. **Merge** that pull request once CI and the security checks pass.
3. **Tag** the merge commit and push the tag:
   ```bash
   git tag -a v1.2.3 -m "Airlock 1.2.3"
   git push origin v1.2.3
   ```
4. **Wait** for the Release workflow. It re-runs every check, builds the files, and opens the draft.
5. **Check the draft.** Install the online app from its installer on a clean machine, load the extension zip, and confirm one QR transfer and one browser connect. Optionally install the airgap build and confirm a durable-nonce transfer can be built, signed and shown as QR. Compare a checksum.
6. **Publish** the draft.

A version with a suffix, such as `1.2.3-rc.1`, is opened as a pre-release.

If a build is wrong, delete the draft and the tag, fix it on the default branch, and tag again. A published release is never replaced: fix forward with a new version.

## Not automated yet

- **Code signing.** The Windows installer is not signed, so Windows shows a SmartScreen warning. Signing needs a certificate and a decision on where its key lives.
- **Extension stores.** The zip is built but not uploaded to the Chrome Web Store or Firefox Add-ons. The extension id is pinned by the key in `extension/manifest.json`; a store listing will have its own.
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
