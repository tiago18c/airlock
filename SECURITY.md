# Security

Airlock is a prototype. It handles private keys and signs transactions, it has not been formally audited, and it should not be used to hold or move real funds. Reports of how it could lose, leak or misuse a key, or get someone to sign what they did not mean to, are still very welcome.

## Reporting a vulnerability

Report it privately: on this repository's **Security** tab, choose **Report a vulnerability**. That opens a private advisory that only you and the maintainers can see. Please do not open a public issue or pull request for a vulnerability, or discuss it in public, until a fix is released.

## In scope

- The apps: Airlock (desktop and web), Airlock Offline and Airlock for Android, as built from this repository or downloaded from its releases.
- The browser extension and the browser bridge: `bridge-host`, the desktop listener, and the requests and grants that pass through them. On Android, the requests other apps send through Mobile Wallet Adapter.
- The vault, the QR protocol (QRS1) and anything that moves between the two devices.
- The wording of the IDL pack and the core IDLs: a sentence, name, amount or account label that misstates what a transaction does, so that what is shown before signing differs from what is signed.
- The release files and how they are built: the workflows, the scripts, and the provenance attestations.

Out of scope: the third-party programs whose IDLs ship with the app (report those to their authors), a node that misreports what the chain holds (the app trusts its node for that, though never for what is signed: see the [threat model](docs/browser-bridge.md#threat-model)), and attacks that need control of the computer or phone the app runs on.

## What to include

- What an attacker can do, and what they need first (a site the user visits, a crafted transaction or IDL, a QR code, access to the device).
- The version or commit, the app, and the platform.
- Steps to reproduce: a transaction (base64 or base58), an IDL, a QR code or a site, as small as it can be.
- What you expected, and what happened.

## Fixes

A fix is released as a new version and noted under **Security** in [CHANGELOG.md](CHANGELOG.md). Releases are listed at <https://github.com/tiago18c/airlock/releases>.
