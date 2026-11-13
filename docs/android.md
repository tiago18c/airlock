# Airlock on Android

`apps/airlock-mobile` is Airlock for Android phones: the online app's shell and panes, laid out for a phone, run online or offline as chosen on the first launch and in Settings. It supports Android 14 and later (API 34), and targets API 36.

## How it is put together

```
apps/airlock-mobile   main.rs: the tabs, the mode, the hub   android/: MainActivity.kt, AndroidManifest.xml
  └─ app-ui, app-online        the same shell and panes as the desktop online app
  └─ app-mobile                Android services, over JNI:
       private folder · links · online/offline · auto-lock lifecycle · the Seed Vault
```

It is a third app over the same crates, not a branch inside them. The rules in [architecture.md](architecture.md) hold: the shared crates gained no `cfg(target_os = "android")` beyond leaving out what Android cannot do (desktop file dialogs, opening a folder, `xdg-open`).

- **Layout.** `AppShell { more }` lays the shell out for a phone: four sections along a bottom tab bar, the rest under **More**, short tab labels, and the page drawn under the system bars with the safe-area insets. Panes read `use_phone_layout()` to leave out what a phone has no room for or no use for (empty wallet groups, the tray, USB hardware wallets, Open log folder). Text a person can work out from the screen carries the `wordy` class, which a phone hides; what changes with the screen's state stays. The top bar has no brand mark: the wallet picker fills it, as a button that opens the **Act as** sheet and, at its end, one that copies the current wallet's address. What the top bar and the tab bar open (Act as, notifications, More) are `PhoneSheet`s: bottom sheets over a scrim, More docked on the tab bar. The shell measures both bars, into `--top-h` and `--dock-h`, for what sticks under the one (the Programs tabs) or sits on the other. A pane of a list and a detail shows one at a time on a phone, the detail under a Back control (Programs). The network's status line shows only while the node does not answer or a sync failed (`NetworkAlert`); Settings names the cluster.
- **Online or offline.** Asked on the first launch, before the vault is created (`app_mobile::Setup`, in the shell's `setup` slot), and changed in Settings → Mode. Each mode has its own data folder, so its own vault (`airlock` and `airlock-offline` under the app's folder). Offline lists only what works without a node and provides no `Network`, as Airlock Offline does. Switching restarts the app. Unlike Airlock Offline on desktop, the offline mode is a promise the app keeps, not something Android enforces: the APK still has the network code and the `INTERNET` permission.
- **Storage.** Everything is under `Context.getNoBackupFilesDir()`: no other app can read it, and neither Android's cloud backup nor a device-to-device transfer copies it (`allowBackup="false"` in the manifest as well). Moving to a new phone goes through Airlock's export or the seed phrase.
- **The WebView.** The page policy is the desktop one (`app_ui_kit::platform::page_policy`), and nothing navigates away from the app. wry asks for the camera permission the first time the scanner starts, and grants the page its camera.
- **Links** open in the phone's browser through an `ACTION_VIEW` intent, `https` and `http` only.
- **Auto-lock** counts the time the app spends in the background, as it counts a hidden window on desktop.

## The Seed Vault (Saga, Seeker)

A Solana Mobile phone keeps seeds in its Seed Vault, in secure hardware, and signs once the user confirms with its own screen and fingerprint. Airlock never sees the seed. To Airlock, a Seed Vault account is a hardware wallet (`HardwareVendor::SeedVault`) whose device is the phone:

| Layer | What it does |
| --- | --- |
| `hw-protocol::seed_vault` | The `SeedVault` trait: authorize a seed (pick, import a phrase, create one), read public keys at paths, sign |
| `DeviceHub::seed_vault()` | How the core reaches it, with the rest of the devices (the Ledgers and Trezors on the USB-C port, below) |
| `app-mobile::seed_vault` | The implementation, over JNI to `SeedVaultBridge` in `apps/airlock-mobile/android/MainActivity.kt` |
| `MainActivity.kt` | Solana Mobile's `seedvault-wallet-sdk`: asks for the `ACCESS_SEED_VAULT` permission, starts the Seed Vault's screens and keeps each result under its request id until Rust takes it |
| `app-ui-kit::actions::device` | Add wallet reads keys (`read_seed_vault_keys_for`), Sign signs (`sign_seed_vault`). Each signature is checked against the account before it is used (`wallet_flows::device::seed_vault_sign`) |

Add wallet → Seed Vault, a tab of its own, offers **Choose seed**, **Import phrase** and **Create seed**. The wallet keeps the auth token the Seed Vault gave Airlock for that seed (`device`), and the path. Revoking Airlock's access in the Seed Vault's settings makes signing say so, and the wallet has to be added again.

The Seed Vault signs the bytes it is given: a transaction's message, an authorization, or an off-chain message. It shows no address of its own, so **Show on device** says the address was read from it.

### Testing it

- **Emulator.** Build Solana Mobile's simulator from [seed-vault-sdk](https://github.com/solana-mobile/seed-vault-sdk) (`./gradlew :SeedVaultSimulator:assembleDebug`) and install it. The app reports it as `seed_vault="simulator"` in the log, and the keys it reads show the model "Seed Vault simulator". It does not sign securely: development only.
- **Saga or Seeker.** The real Seed Vault (`com.solanamobile.seedvaultimpl`) is reported as `seed_vault="secure"`. Authorizing a seed and confirming a signature take the owner's fingerprint or PIN on the phone, so that part of a test is done by hand.

## Ledger and Trezor over USB

With **Pre-release hardware wallets** on (Settings), Add wallet → Hardware offers a Ledger or a Trezor on the phone's USB-C port, and Sign signs on it, as on desktop. Keystone needs no cable: its codes go through the camera.

An app cannot list or open the phone's USB devices itself, so the pieces differ from desktop:

| Piece | What it does |
| --- | --- |
| `UsbBridge` in `MainActivity.kt` | Lists the Ledgers and Trezors `UsbManager` sees. Opening one asks the user to let Airlock use it (Android's own dialog), opens it, and hands Rust a copy of its file descriptor |
| `app-mobile::usb` | The phone hub's `list` and `open`, over JNI to `UsbBridge`. A device's id is its kind and Android's name for it (`ledger:/dev/bus/usb/…`) |
| `hw_transport::android::open_fd` | `nusb` from the file descriptor. A Trezor is opened as on desktop. A Ledger's APDU interface is HID, which Android gives an app no driver for, so it is read as plain USB: the first HID interface with an interrupt endpoint each way, in the same 64-byte packets, without a report id. `async-hid` is not built for Android |

A Ledger that opens its Solana app restarts its USB connection, and Android sees a new device, so it asks again. Bluetooth (Ledger Nano X, Stax and Flex; Trezor Safe 7) is not supported.

## Requests from other apps (Mobile Wallet Adapter)

Online, other apps on the phone, and sites open in Chrome, can ask Airlock to connect and sign, as the extension does on desktop. They find it through [Mobile Wallet Adapter](https://docs.solanamobile.com/mobile-wallet-adapter/overview) (MWA): a `solana-wallet:` link opens Airlock, and they talk over an encrypted local WebSocket. The first launch asks whether to allow this (on by default), and Settings → **Apps on this phone** changes it. Offline, Airlock is never listed as a wallet.

| Piece | What it does |
| --- | --- |
| `MwaActivity` in `MainActivity.kt` | What a `solana-wallet:` link opens. It has no window: it starts the session in the asking app's task, brings Airlock forward, and finishes when the session ends, which is what that app waits for. It is off in the manifest, and `MwaBridge.setEnabled` turns it on only in online mode with the setting on, so otherwise no app is offered Airlock |
| `MwaBridge` in `MainActivity.kt` | Holds each session with Solana Mobile's `walletlib` (2.1.1): the association, its encryption, and the auth tokens it issues and checks. Hands each request to Rust as JSON (`next`), and completes it with Rust's answer (`answer`) |
| `app-mobile::wallet_adapter` | A thread that takes those events, keeps an app session for each MWA session, and answers each request on a thread of its own. Also the setting |
| `app_desktop::bridge::AppSession` | Reads each request into the frame the extension would have sent, and puts it through the bridge's own checks, queue and request modal. `app-desktop` is linked with `bridge` only: no socket, tray or OS toasts |

| MWA | What Airlock does |
| --- | --- |
| `authorize` | Always asks, in the connect modal, even from a site with a grant: any app can give that site's address. With a sign-in payload, the sign-in modal |
| `authorize` with an auth token | `walletlib` checks the token. Airlock goes on without asking only while the site still holds a grant it did not end |
| `sign_transactions` | `signTransaction`, each transaction asked of the granted account that signs it, at most 16 |
| `sign_and_send_transactions` | `signAndSendAllTransactions`, serial when the app asks each to wait for the one before |
| `sign_messages` | `signMessage`, one message at a time |
| `deauthorize` | Ends the site's connection, as a site's `disconnect` does |

Android does not vouch for who an app says it is. The request is filed under the address the app gave (`https://…`), or its package (`android-app://…`) when it gave none, and the modal says the app only says it is that site, with the package Android reported, when it did (an app that started the session; Chrome does not say). A sign-in is still checked against that address. Remote sessions through a reflector server are not offered.

## Building and running

Install Android Studio's SDK with Platform-Tools, Build-Tools, NDK (r28 or later, for 16 KB pages), and a platform of API 36. Set `ANDROID_HOME`, `ANDROID_NDK_HOME` and `JAVA_HOME` (JDK 17), and `rustup target add aarch64-linux-android x86_64-linux-android`.

```bash
dx build --android -p airlock-mobile --target aarch64-linux-android   # phones; the emulator runs it too
dx build --android -p airlock-mobile --target x86_64-linux-android    # faster on an x86_64 emulator
scripts/android-icon.sh debug                                         # after every dx build: Airlock's icon
adb install -r target/dx/airlock-mobile/debug/android/app/app/build/outputs/apk/debug/app-debug.apk
```

Each `dx build` replaces only its own target's library in the APK, so after a change rebuild every target you install: an x86_64 emulator loads `lib/x86_64` even when `lib/arm64-v8a` is newer.

**The launcher icon.** dx 0.7.10 writes a placeholder launcher icon into the Gradle project it generates (`target/dx/airlock-mobile/<profile>/android/app`) on every build, and reads no Android icon from `Dioxus.toml`. `scripts/android-icon.sh debug` (or `release`, after `dx bundle --release`) copies Airlock's over it from `apps/airlock-mobile/android/res`, and runs the Gradle task dx ran again in that project, so the APK dx left has the icon and is signed with the same key. The release workflow runs it after `dx bundle`. The icon is the desktop app's vault door: an adaptive icon with the door inside the safe zone on the brand colour, the door's padlock as the monochrome layer for themed icons, and the door alone for launchers that draw no adaptive icons. `python scripts/make-android-icons.py` makes those files again from `apps/airlock-mobile/icons/icon.png` (Pillow with WebP). The script stops if the generated project is not laid out as dx 0.7.10 lays it out: check it, and whether dx can now take the icon itself, when `DX_VERSION` changes.

A debug build turns on WebView debugging, so a test drives the page over the Chrome DevTools Protocol, as on desktop:

```bash
adb forward tcp:9333 localabstract:webview_devtools_remote_$(adb shell pidof dev.airlock.mobile)
curl http://127.0.0.1:9333/json     # the page target
```

Native screens (permission prompts, the Seed Vault) are driven with `adb shell uiautomator dump` and `adb shell input tap`. The app's log is in logcat under `RustStdoutStderr`.

Emulators worth having: Android 14 (`google_apis`, API 34) and Android 16 and 17 with 16 KB pages (`google_apis_ps16k`, API 36 and 37).

## Secrets, and what could hold them better

What the app keeps, and where:

| What | Where now | Could be |
| --- | --- | --- |
| Keys and seed phrases of hot wallets | `vault.json`, sealed with XChaCha20-Poly1305 under an Argon2id key from the password. With fingerprint unlock on, that key is also kept sealed by a Keystore key behind the fingerprint (below) | Unchanged |
| Seed Vault accounts | The phone's Seed Vault. The vault keeps the auth token and path | Already the strongest place the phone has |
| An RPC URL with an API key, the Tokens API key | `workspace.json`, in plain text, so prices refresh while locked | Encrypted under an Android Keystore key that needs no user, so a copy of the folder does not hold them |
| Hash-based key state (XMSS leaves) | The vault and its record file | Unchanged: it has to survive the app, and a lost record risks reusing a leaf |
| The password while typed | The WebView, then zeroized in Rust | Unchanged |

**Fingerprint unlock** (Settings → Vault). Every recent Android phone has a Keystore backed by its TEE, and many have StrongBox, a separate secure chip: Samsung's Knox Vault on the Galaxy S series (S26 included), Google's Titan M2 on Pixels.

| Step | What happens |
| --- | --- |
| Turn it on | With the vault unlocked, `Vault::seal_key_bytes` gives the key the vault is sealed under (the Argon2id output with its salt and cost, never the password). `KeyGuardBridge` makes an AES-256-GCM key in StrongBox (the TEE where there is none) with `setUserAuthenticationRequired(true)`, per-use `AUTH_BIOMETRIC_STRONG` and `setInvalidatedByBiometricEnrollment(true)`, asks for the fingerprint (`BiometricPrompt` with a `CryptoObject`), and seals the bytes. The IV and ciphertext are kept in the mode's folder (`fingerprint-unlock.txt`); on their own they open nothing. |
| Unlock | The lock screen asks for the fingerprint as it opens, and has **Use fingerprint**. The Keystore key opens what was sealed, the bytes cross JNI as a byte array that is wiped once copied, and `Unlocking::open_with_key` opens the vault with no derivation. |
| The key changes | A new password, or the vault raised to a higher cost, seals under a new key: the kept one stops opening it (`open_with_key` refuses another salt or cost), fingerprint unlock turns itself off, and the lock screen says to use the password. Changing the password turns it off at once. |
| The fingerprints change | Android destroys the Keystore key (`KeyPermanentlyInvalidatedException`); fingerprint unlock turns itself off and says why. |

A copied `vault.json`, or the whole folder, still needs the password: the Keystore key never leaves the phone's hardware. Each mode has its own Keystore key (`airlock-vault-<folder>`), as it has its own vault. `adb` cannot screenshot the system's fingerprint prompt.

**Signing in the hardware.** Android's Keystore signs Ed25519 from KeyMint 200 (Android 13 and later), but only in the TEE: StrongBox chips are not expected to support Curve25519, and Keystore keys cannot be imported from or exported to a seed phrase. A Keystore key could be a fresh, phone-bound signer (for a programmatic signer's authority, say), not a home for existing wallets. On a Solana Mobile phone, the Seed Vault is that home.

**Samsung Blockchain Keystore.** Samsung's own wallet keystore, in its TrustZone, on some Galaxy phones. Its SDK (last released December 2023) lists no Solana support, so it is no option for now.

**Signing on a device without the vault.** When every signature a transaction still needs is a device's (the Seed Vault, or a hardware wallet), Sign does not ask for the vault's password: none of its keys would sign, and the device asks the user itself (`SigningStatus::needs_vault`).

**Screens that show secrets.** `FLAG_SECURE` on the activity would keep a seed phrase or private key out of screenshots, screen recordings and the recents thumbnail. It applies to the whole window, so it would be set while such a screen is open (Android 15 also tells an app when it is being recorded, `addScreenRecordingCallback`). Not done yet.
