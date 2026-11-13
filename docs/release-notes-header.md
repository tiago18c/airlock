<!-- The top of every release page. release.yml and nightly.yml put it above the changelog, with the version placeholder replaced by the version. Links are absolute: a release page resolves no relative link. -->
> [!WARNING]
> **Not ready for production, and not formally audited.** This is a prototype that handles private keys and signs transactions. Do not use it to hold or move real funds. Read the [disclaimer](https://github.com/tiago18c/airlock#disclaimer) first.

## Which file to download

**Airlock** is the online app: it holds a vault, builds and submits transactions, and answers the browser extension. **Airlock Offline** is for a second device kept off the network: it reviews and signs, and opens no connection of its own. Transactions go between the two as QR codes or pasted text.

| For | Download |
| --- | --- |
| Windows, Airlock | `airlock-{{version}}-windows-x64-setup.exe`: the installer. It downloads WebView2 while installing if Windows has none |
| Windows, Airlock, on a machine that cannot download WebView2 | `airlock-{{version}}-windows-x64-setup-full.exe`: the same, with WebView2 inside (about 190 MB more) |
| Windows, Airlock, without installing | `airlock-{{version}}-windows-x64-portable.zip`: unzip and run `airlock.exe` |
| Windows, Airlock Offline | `airlock-offline-{{version}}-windows-x64-setup.exe` needs WebView2 already, as Windows 11 has it. `airlock-offline-{{version}}-windows-x64-setup-full.exe` has it inside, for a machine that has never been online. `airlock-offline-{{version}}-windows-x64-portable.zip` runs without installing |
| Mac with Apple Silicon | `airlock-{{version}}-macos-arm64.dmg`, `airlock-offline-{{version}}-macos-arm64.dmg`. Intel Macs are not built |
| Debian, Ubuntu and their kin, x86-64 | `airlock-{{version}}-linux-amd64.deb`, `airlock-offline-{{version}}-linux-amd64.deb` |
| Other x86-64 Linux | `airlock-{{version}}-linux-x86_64.AppImage`, `airlock-offline-{{version}}-linux-x86_64.AppImage`. The system needs WebKitGTK (`libwebkit2gtk-4.1`), as Ubuntu and Fedora desktops have |
| Android 14 and later, arm64 phones | `airlock-mobile-{{version}}-android-arm64.apk`: online or offline, chosen in the app |
| Signing for websites in Chrome, Edge or Brave | `airlock-extension-{{version}}.zip`, with Airlock on the same computer. See [the browser extension](#the-browser-extension) |
| The browser bridge on its own | `bridge-host-{{version}}-windows-x64.exe`, `bridge-host-{{version}}-macos-arm64.tar.gz`. Only if you need it apart from the app: Airlock's installers, portable zip, Mac app and Linux packages already have it |
| Checking the files | `SHA256SUMS.txt` and `airlock-{{version}}.sigstore.json`. See [checking a download](#checking-a-download) |
| Licences of the IDL files the apps ship | `THIRD-PARTY-NOTICES.md` |

## Opening it the first time

None of the apps is signed by a publisher Windows, macOS or Android knows, so each of them warns the first time.

- **Windows** shows "Windows protected your PC" (SmartScreen). Choose **More info**, then **Run anyway**. If Airlock is already running, quit it from its tray icon before installing a new version: with **Close to tray** on, closing the window leaves it running.
- **macOS 15 and later** says Apple could not verify the app is free of malware, and offers only **Done** and **Move to Trash**. Choose Done, then open **System Settings → Privacy & Security**, choose **Open Anyway** beside the app's name, and confirm with your password. Or, in Terminal: `xattr -dr com.apple.quarantine "/Applications/Airlock.app"`. macOS asks for the camera again after each update. For the standalone `bridge-host` tarball, run `xattr -d com.apple.quarantine bridge-host` after unpacking it.
- **Android** installs an APK from outside a store only once you allow it: when you open the file, Android asks to let the app you opened it from (your browser or file manager) **install unknown apps**. Allow it, go back, and install. A new version installs over the old one and keeps its data.
- **Linux:** install the `.deb` with `sudo apt install ./airlock-{{version}}-linux-amd64.deb`. Make the AppImage executable (`chmod +x`) and run it. For a Ledger or Trezor, install the udev rules first: see [hardware wallets on Linux](https://github.com/tiago18c/airlock#hardware-wallets-on-linux).

## The browser extension

The extension shows websites a Solana wallet and forwards their requests to Airlock on the same computer, which holds the keys and asks you. It is not in a store yet, so it is loaded from the zip:

1. Install Airlock (the online app) on the computer. The Windows installer registers the browser bridge unless you untick it. Otherwise, or on a Mac or Linux, open Airlock and press **Install the browser bridge** on its **Extension** chip, beside the network chip in the sidebar.
2. Unzip `airlock-extension-{{version}}.zip` into a folder you will keep: the browser loads the extension from that folder every time it starts.
3. Open the browser's extensions page: `chrome://extensions` in Chrome, `edge://extensions` in Edge, `brave://extensions` in Brave.
4. Turn on **Developer mode**.
5. Choose **Load unpacked**, and select the folder you unzipped (the one with `manifest.json` in it).
6. Restart the browser. Then start Airlock, unlock it and set it to Online.

To update it, unzip the new version over the same folder and press the extension's reload button on the extensions page. Firefox is not supported yet: the bridge registers itself for Firefox, but the zip is made for Chromium browsers, and Firefox keeps an unsigned extension only until it restarts.

## Checking a download

`SHA256SUMS.txt` has the SHA-256 of every file. Compare it with the file you downloaded:

```bash
sha256sum airlock-{{version}}-linux-amd64.deb          # Linux
shasum -a 256 airlock-{{version}}-macos-arm64.dmg      # macOS
Get-FileHash airlock-{{version}}-windows-x64-setup.exe # Windows PowerShell
```

Each file also has a provenance attestation: a signed statement that it was built by this repository's workflow from the tagged commit. With [GitHub's CLI](https://cli.github.com):

```bash
gh attestation verify airlock-{{version}}-windows-x64-setup.exe --repo tiago18c/airlock
```

To check it with cosign instead, without a GitHub account, see [releasing](https://github.com/tiago18c/airlock/blob/main/docs/releasing.md#what-a-release-contains). Both prove where a file was built. Neither is what Windows, macOS or Android check before running it.
