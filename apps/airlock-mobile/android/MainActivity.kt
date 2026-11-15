// Airlock's activity: wry's, plus the bridge to the Seed Vault of a Solana
// Mobile phone (Saga, Seeker), fingerprint unlock (KeyGuardBridge), Ledgers
// and Trezors over USB (UsbBridge), Mobile Wallet Adapter requests from
// other apps (MwaBridge, MwaActivity), and starting the app again after a
// change of mode (RestartActivity). dx writes this file in place of its
// default MainActivity.kt (`android_main_activity` in Dioxus.toml).
//
// Rust (`app-mobile`, `seed_vault.rs`) calls SeedVaultBridge's methods over
// JNI. Each request gets an id; the Seed Vault's screen opens over the app,
// and its result is kept under that id until Rust takes it with take(id).
// A result is "ok:<text>" or "err:<code>:<text>".
package dev.dioxus.main

import android.app.Activity
import android.app.PendingIntent
import android.content.ComponentName
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.annotation.Keep
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import android.net.Uri
import android.os.Bundle
import android.util.Base64
import android.util.Log
import com.solanamobile.seedvault.Bip32DerivationPath
import com.solanamobile.seedvault.BipLevel
import com.solanamobile.seedvault.SeedVault
import com.solanamobile.seedvault.SigningRequest
import com.solanamobile.seedvault.Wallet
import com.solanamobile.seedvault.WalletContractV1
import com.solana.mobilewalletadapter.common.ProtocolContract
import com.solana.mobilewalletadapter.common.protocol.MessageReceiver
import com.solana.mobilewalletadapter.common.protocol.MessageSender
import com.solana.mobilewalletadapter.common.protocol.MobileWalletAdapterSessionCommon
import com.solana.mobilewalletadapter.common.signin.SignInWithSolana
import com.solana.mobilewalletadapter.common.util.Base58
import com.solana.mobilewalletadapter.walletlib.association.AssociationUri
import com.solana.mobilewalletadapter.walletlib.association.LocalAssociationUri
import com.solana.mobilewalletadapter.walletlib.authorization.AuthIssuerConfig
import com.solana.mobilewalletadapter.walletlib.protocol.MobileWalletAdapterConfig
import com.solana.mobilewalletadapter.walletlib.protocol.MobileWalletAdapterServer
import com.solana.mobilewalletadapter.walletlib.protocol.MobileWalletAdapterSession
import com.solana.mobilewalletadapter.walletlib.scenario.AuthorizeRequest
import com.solana.mobilewalletadapter.walletlib.scenario.AuthorizedAccount
import com.solana.mobilewalletadapter.walletlib.scenario.DeauthorizedEvent
import com.solana.mobilewalletadapter.walletlib.scenario.LocalScenario
import com.solana.mobilewalletadapter.walletlib.scenario.LocalWebSocketServerScenario
import com.solana.mobilewalletadapter.walletlib.scenario.ReauthorizeRequest
import com.solana.mobilewalletadapter.walletlib.scenario.Scenario
import com.solana.mobilewalletadapter.walletlib.scenario.SignAndSendTransactionsRequest
import com.solana.mobilewalletadapter.walletlib.scenario.SignInResult
import com.solana.mobilewalletadapter.walletlib.scenario.SignMessagesRequest
import com.solana.mobilewalletadapter.walletlib.scenario.SignPayloadsRequest
import com.solana.mobilewalletadapter.walletlib.scenario.SignTransactionsRequest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

typealias BuildConfig = dev.airlock.mobile.BuildConfig

class MainActivity : WryActivity() {
    /// The page's WebView, once wry has made it.
    private var webView: WebView? = null
    /// Whether the keyboard is up, as the page was last told.
    private var keyboardShown = false

    override fun onWebViewCreate(webView: WebView) {
        this.webView = webView
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before super: wry starts the app's Rust from it, which may ask the
        // bridges for the activity at once.
        SeedVaultBridge.activity = this
        super.onCreate(savedInstanceState)
        fitSystemBars()
        // Back, the key or the gesture. Without this, wry's activity finishes,
        // which ends the app: a half-filled form is lost, and the next start
        // is cold, on the lock screen.
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = back()
        })
    }

    /// Back closes what is open on the page first, then goes back to the list
    /// a pane came from, and only then leaves: to the background, as Home
    /// does, so the app keeps its state and auto-lock counts the time away.
    private fun back() {
        val view = webView
        if (view == null) {
            moveTaskToBack(true)
            return
        }
        view.evaluateJavascript(BACK_SCRIPT) { handled ->
            if (handled != "true") moveTaskToBack(true)
        }
    }

    private companion object {
        /// What Back does on the page, answering whether it did anything:
        /// 1. The topmost dialog, sheet or menu: its own control marked
        ///    `data-back` (the template browser's list, the inspector's
        ///    Back), else what a tap outside it does. A dialog that a tap
        ///    outside does not close (a signature underway) stays.
        /// 2. An open list of address suggestions closes, as Escape does.
        /// 3. A pane's control marked `data-back`, to the list it came from.
        const val BACK_SCRIPT = """(function () {
  var shown = function (e) { return !!e && e.getClientRects().length > 0; };
  var all = function (root, sel) { return Array.prototype.filter.call(root.querySelectorAll(sel), shown); };
  var layers = all(document, '.modal-backdrop, .phone-scrim');
  if (layers.length) {
    var z = function (e) { return parseInt(getComputedStyle(e).zIndex, 10) || 0; };
    var top = layers.reduce(function (a, b) { return z(b) >= z(a) ? b : a; });
    var layer = top.classList.contains('modal-backdrop') ? top : top.nextElementSibling;
    var own = layer ? all(layer, '[data-back]').pop() : null;
    (own || top).click();
    return true;
  }
  var picker = document.querySelector('.picker.open input');
  if (picker) {
    picker.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    return true;
  }
  var pane = all(document, '.content [data-back]').pop();
  if (pane) { pane.click(); return true; }
  return false;
})()"""
    }

    /// The manifest takes every configuration change in place (configChanges),
    /// since the activity cannot be made again. The page lays itself out for
    /// a new size or density by itself; its text follows the font size here,
    /// as the WebView set it from the font size when it was made.
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        followTheme()
        webView?.settings?.textZoom = (newConfig.fontScale * 100).roundToInt()
    }

    /// From Android 15 the app is drawn under the system bars, and the
    /// window no longer shrinks for the keyboard. The WebView does not report
    /// the navigation bar or the keyboard as safe-area insets, so the page
    /// is kept clear of every bar here: between the status bar (and any
    /// camera cutout) and the navigation bar or keyboard. On Android 14 the
    /// window already stops at the bars, and these insets are zero.
    private fun fitSystemBars() {
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val top = insets.getInsets(
                WindowInsetsCompat.Type.statusBars() or WindowInsetsCompat.Type.displayCutout()
            ).top
            val bottom = insets.getInsets(
                WindowInsetsCompat.Type.navigationBars() or WindowInsetsCompat.Type.ime()
            ).bottom
            view.setPadding(0, top, 0, bottom)
            // The page cannot tell the keyboard is up (the WebView only
            // shrinks), so it is told: its tab bar stands aside meanwhile.
            val keyboard = insets.isVisible(WindowInsetsCompat.Type.ime())
            if (keyboard != keyboardShown) {
                keyboardShown = keyboard
                webView?.evaluateJavascript(
                    "document.documentElement.classList.toggle('keyboard', $keyboard)", null
                )
            }
            WindowInsetsCompat.CONSUMED
        }
        followTheme()
    }

    /// The bars take the page's surface color, with dark icons on the light
    /// theme and light ones on the dark theme. They follow the phone's theme.
    private fun followTheme() {
        val night = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES
        // --surface in tokens.css, light and dark.
        findViewById<View>(android.R.id.content)
            .setBackgroundColor(if (night) 0xFF141922.toInt() else 0xFFFFFFFF.toInt())
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = !night
            isAppearanceLightNavigationBars = !night
        }
    }

    override fun onDestroy() {
        if (SeedVaultBridge.activity === this) SeedVaultBridge.activity = null
        super.onDestroy()
    }

    @Deprecated("Seed Vault answers through startActivityForResult")
    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (!SeedVaultBridge.onActivityResult(requestCode, resultCode, data)) {
            super.onActivityResult(requestCode, resultCode, data)
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        if (!SeedVaultBridge.onPermissionResult(requestCode, grantResults)) {
            super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        }
    }
}

@Keep
object SeedVaultBridge {
    private const val TAG = "AirlockSeedVault"
    /// The bridge's request codes: AppCompat takes only 16 bits.
    private const val PERMISSION_CODES = 0x5E00
    private const val RESULT_CODES = 0x6E00
    private const val CODE_SPAN = 0x100
    private const val PERMISSION = WalletContractV1.PERMISSION_ACCESS_SEED_VAULT

    @Volatile
    var activity: MainActivity? = null

    private val results = ConcurrentHashMap<Long, String>()
    /// What each request turns its activity result into, by request code.
    private val pending = ConcurrentHashMap<Int, Pair<Long, (Int, Intent?) -> String>>()
    /// Requests waiting for the permission, run once it is granted.
    private val afterPermission = ConcurrentHashMap<Long, () -> Unit>()

    // ---- Called from Rust ----

    /// 0: no Seed Vault. 1: Solana Mobile's simulator. 2: the real one.
    @JvmStatic
    fun availability(): Int {
        val a = activity ?: return 0
        return when {
            SeedVault.isAvailable(a, false) -> 2
            SeedVault.isAvailable(a, true) -> 1
            else -> 0
        }
    }

    /// The result of request `id`, once there is one. Taken once.
    @JvmStatic
    fun take(id: Long): String? = results.remove(id)

    /// how: 0 pick an existing seed, 1 import a phrase, 2 create one.
    /// Picking reuses a seed Airlock may already use (authorizations outlive
    /// a reinstall) when the Seed Vault has no other seed to grant it.
    @JvmStatic
    fun authorize(id: Long, how: Int) = withPermission(id) {
        val purpose = WalletContractV1.PURPOSE_SIGN_SOLANA_TRANSACTION
        val a = activity
        if (how == 0 && a != null) {
            val granted = authorizedToken(a, purpose)
            val more = try {
                Wallet.hasUnauthorizedSeedsForPurpose(a, purpose)
            } catch (e: Exception) {
                true
            }
            Log.i(TAG, "choose seed: authorized=${granted != null} more=$more")
            if (granted != null && !more) {
                results[id] = "ok:$granted"
                return@withPermission
            }
        }
        launch(id, {
            when (how) {
                1 -> Wallet.importSeed(it, purpose)
                2 -> Wallet.createSeed(it, purpose)
                else -> Wallet.authorizeSeed(it, purpose)
            }
        }) { code, data ->
            val token = when (how) {
                1 -> Wallet.onImportSeedResult(code, data)
                2 -> Wallet.onCreateSeedResult(code, data)
                else -> try {
                    Wallet.onAuthorizeSeedResult(code, data)
                } catch (e: Wallet.ActionFailedException) {
                    // Nothing new to grant: the seed may already be this app's,
                    // which the provider can fail to say right after a cold start.
                    val granted = if (code == WalletContractV1.RESULT_NO_AVAILABLE_SEEDS) {
                        activity?.let { authorizedToken(it, purpose) }
                    } else {
                        null
                    }
                    granted ?: throw e
                }
            }
            "ok:$token"
        }
    }

    /// paths: derivation paths such as m/44'/501'/0'/0', one per line.
    /// Answers the base64 of each 32-byte key, one per line.
    @JvmStatic
    fun publicKeys(id: Long, token: Long, paths: String) = withPermission(id) {
        val uris = ArrayList(paths.lines().filter { it.isNotBlank() }.map { toUri(it) })
        launch(id, { Wallet.requestPublicKeys(it, token, uris) }) { code, data ->
            val keys = Wallet.onRequestPublicKeysResult(code, data)
            "ok:" + keys.joinToString("\n") { Base64.encodeToString(it.publicKey, Base64.NO_WRAP) }
        }
    }

    /// Sign the base64 `payload` with the key at `path`. message: an
    /// off-chain message rather than a transaction's message. Answers the
    /// base64 of the 64-byte signature.
    @JvmStatic
    fun sign(id: Long, token: Long, path: String, payload: String, message: Boolean) =
        withPermission(id) {
            val bytes = Base64.decode(payload, Base64.NO_WRAP)
            val request = arrayListOf(SigningRequest(bytes, arrayListOf(toUri(path))))
            launch(id, {
                if (message) Wallet.signMessages(it, token, request)
                else Wallet.signTransactions(it, token, request)
            }) { code, data ->
                val responses = if (message) Wallet.onSignMessagesResult(code, data)
                else Wallet.onSignTransactionsResult(code, data)
                val signature = responses.firstOrNull()?.signatures?.firstOrNull()
                    ?: throw IllegalStateException("no signature came back")
                "ok:" + Base64.encodeToString(signature, Base64.NO_WRAP)
            }
        }

    // ---- Called from the activity ----

    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        val (id, finish) = pending.remove(requestCode) ?: return false
        results[id] = try {
            finish(resultCode, data)
        } catch (e: Wallet.ActionFailedException) {
            // RESULT_CANCELED is the user backing out; anything else is a refusal
            // the Seed Vault explains in its own screen.
            if (resultCode == android.app.Activity.RESULT_CANCELED) "err:declined:${e.message}"
            else if (resultCode == WalletContractV1.RESULT_INVALID_AUTH_TOKEN) "err:auth:${e.message}"
            else if (resultCode == WalletContractV1.RESULT_NO_AVAILABLE_SEEDS) "err:noseeds:${e.message}"
            else "err:failed:${e.message}"
        } catch (e: Exception) {
            Log.w(TAG, "request failed", e)
            "err:failed:${e.message}"
        }
        return true
    }

    fun onPermissionResult(requestCode: Int, grantResults: IntArray): Boolean {
        if (requestCode < PERMISSION_CODES || requestCode >= PERMISSION_CODES + CODE_SPAN) return false
        val slot = requestCode - PERMISSION_CODES
        val id = afterPermission.keys.firstOrNull { (it % CODE_SPAN).toInt() == slot } ?: return true
        val run = afterPermission.remove(id) ?: return true
        if (grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
            run()
        } else {
            results[id] = "err:permission:the Seed Vault permission was refused"
        }
        return true
    }

    // ---- Inside ----

    private fun withPermission(id: Long, run: () -> Unit) {
        val a = activity ?: run { results[id] = "err:failed:the app is not in front"; return }
        a.runOnUiThread {
            if (a.checkSelfPermission(PERMISSION) == PackageManager.PERMISSION_GRANTED) {
                run()
            } else {
                afterPermission[id] = run
                a.requestPermissions(arrayOf(PERMISSION), PERMISSION_CODES + (id % CODE_SPAN).toInt())
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun launch(
        id: Long,
        intent: (MainActivity) -> Intent,
        finish: (Int, Intent?) -> String
    ) {
        val a = activity ?: run { results[id] = "err:failed:the app is not in front"; return }
        try {
            val code = RESULT_CODES + (id % CODE_SPAN).toInt()
            pending[code] = Pair(id, finish)
            a.startActivityForResult(intent(a), code)
        } catch (e: Exception) {
            Log.w(TAG, "could not open the Seed Vault", e)
            results[id] = "err:failed:${e.message}"
        }
    }

    /// The auth token of a seed this app may already use for `purpose`.
    private fun authorizedToken(a: MainActivity, purpose: Int): Long? = try {
        Wallet.getAuthorizedSeeds(
            a,
            arrayOf(
                WalletContractV1.AUTHORIZED_SEEDS_AUTH_TOKEN,
                WalletContractV1.AUTHORIZED_SEEDS_AUTH_PURPOSE
            )
        )?.use { rows ->
            var token: Long? = null
            while (token == null && rows.moveToNext()) {
                if (rows.getInt(1) == purpose) token = rows.getLong(0)
            }
            token
        }
    } catch (e: Exception) {
        Log.w(TAG, "could not read authorized seeds", e)
        null
    }

    /// m/44'/501'/0'/0' as the bip32: URI the Seed Vault takes.
    private fun toUri(path: String): Uri {
        val builder = Bip32DerivationPath.newBuilder()
        path.trim().removePrefix("m").split('/').filter { it.isNotEmpty() }.forEach { level ->
            val hardened = level.endsWith("'") || level.endsWith("h")
            val index = level.trimEnd('\'', 'h').toInt()
            builder.appendLevel(BipLevel(index, hardened))
        }
        return builder.build().toUri()
    }
}

/// Fingerprint unlock. The vault's key (what `SealKey::to_bytes` writes) is
/// sealed with an AES-256-GCM key that Android's Keystore makes and keeps:
/// in StrongBox where the phone has one (Knox Vault, Titan M2), in the TEE
/// otherwise. The Keystore key works only right after a strong biometric
/// check (per use), never leaves the hardware, and is destroyed when the
/// phone's fingerprints change. Rust keeps what it sealed (IV and
/// ciphertext), which opens nothing without that key.
///
/// Requests work as SeedVaultBridge's: an id, then take(id). A result is
/// "ok:<text>" or "err:<code>:<text>"; an opened key is taken once with
/// takeSecret(id), and Rust wipes the array after copying it.
@Keep
object KeyGuardBridge {
    private const val TAG = "AirlockKeyGuard"
    private const val STORE = "AndroidKeyStore"
    private const val TRANSFORM = "AES/GCM/NoPadding"

    private val results = ConcurrentHashMap<Long, String>()
    private val secrets = ConcurrentHashMap<Long, ByteArray>()

    /// 0: no strong biometric enrolled. 1: the TEE holds the key. 2: StrongBox.
    @JvmStatic
    fun availability(): Int {
        val a = SeedVaultBridge.activity ?: return 0
        val biometrics = a.getSystemService(BiometricManager::class.java) ?: return 0
        if (biometrics.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) !=
            BiometricManager.BIOMETRIC_SUCCESS
        ) return 0
        return if (a.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)) 2 else 1
    }

    @JvmStatic
    fun take(id: Long): String? = results.remove(id)

    @JvmStatic
    fun takeSecret(id: Long): ByteArray? = secrets.remove(id)

    /// Make a new Keystore key under `alias`, ask for the fingerprint, and seal
    /// `secret` with it. Answers "ok:<iv>:<sealed>:<StrongBox|TEE>". `secret`
    /// is wiped either way.
    @JvmStatic
    fun enroll(id: Long, alias: String, secret: ByteArray) {
        val a = SeedVaultBridge.activity
        if (a == null) {
            secret.fill(0)
            results[id] = "err:failed:the app is not in front"
            return
        }
        a.runOnUiThread {
            try {
                val (key, where) = newKey(alias)
                val cipher = Cipher.getInstance(TRANSFORM)
                cipher.init(Cipher.ENCRYPT_MODE, key)
                prompt(a, id, "Turn on fingerprint unlock", cipher, onEnd = { secret.fill(0) }) { done ->
                    val sealed = done.doFinal(secret)
                    "ok:" + b64(done.iv) + ":" + b64(sealed) + ":" + where
                }
            } catch (e: Exception) {
                secret.fill(0)
                Log.w(TAG, "could not turn on", e)
                results[id] = "err:failed:${e.message}"
            }
        }
    }

    /// Ask for the fingerprint and open what enroll sealed. The key is then
    /// taken with takeSecret(id).
    @JvmStatic
    fun open(id: Long, alias: String, iv: String, sealed: String) {
        val a = SeedVaultBridge.activity
        if (a == null) {
            results[id] = "err:failed:the app is not in front"
            return
        }
        a.runOnUiThread {
            try {
                val store = KeyStore.getInstance(STORE).apply { load(null) }
                val key = store.getKey(alias, null) as? SecretKey
                if (key == null) {
                    results[id] = "err:invalidated:no key"
                } else {
                    val cipher = Cipher.getInstance(TRANSFORM)
                    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, unb64(iv)))
                    prompt(a, id, "Unlock the vault", cipher) { done ->
                        secrets[id] = done.doFinal(unb64(sealed))
                        "ok:"
                    }
                }
            } catch (e: KeyPermanentlyInvalidatedException) {
                forget(alias)
                results[id] = "err:invalidated:${e.message}"
            } catch (e: Exception) {
                Log.w(TAG, "could not open", e)
                results[id] = "err:failed:${e.message}"
            }
        }
    }

    @JvmStatic
    fun forget(alias: String) {
        try {
            KeyStore.getInstance(STORE).apply { load(null) }.deleteEntry(alias)
        } catch (e: Exception) {
            Log.w(TAG, "could not delete the key", e)
        }
    }

    private fun newKey(alias: String): Pair<SecretKey, String> {
        fun make(strongBox: Boolean): SecretKey {
            val spec = KeyGenParameterSpec.Builder(
                alias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(true)
                .setIsStrongBoxBacked(strongBox)
                .build()
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE)
            generator.init(spec)
            return generator.generateKey()
        }
        return try {
            Pair(make(true), "StrongBox")
        } catch (e: Exception) {
            // No StrongBox, or it refused these settings: the TEE keeps it.
            Pair(make(false), "TEE")
        }
    }

    private fun prompt(
        a: MainActivity,
        id: Long,
        title: String,
        cipher: Cipher,
        onEnd: () -> Unit = {},
        finish: (Cipher) -> String
    ) {
        val prompt = BiometricPrompt.Builder(a)
            .setTitle(title)
            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
            .setNegativeButton("Use password", a.mainExecutor) { _, _ ->
                onEnd()
                results[id] = "err:declined:"
            }
            .build()
        prompt.authenticate(
            BiometricPrompt.CryptoObject(cipher),
            CancellationSignal(),
            a.mainExecutor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    results[id] = try {
                        val done = result.cryptoObject?.cipher ?: throw IllegalStateException("no cipher")
                        finish(done)
                    } catch (e: Exception) {
                        Log.w(TAG, "sealing failed", e)
                        "err:failed:${e.message}"
                    } finally {
                        onEnd()
                    }
                }

                override fun onAuthenticationError(code: Int, message: CharSequence) {
                    onEnd()
                    results[id] = when (code) {
                        BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED,
                        BiometricPrompt.BIOMETRIC_ERROR_CANCELED -> "err:declined:$message"
                        else -> "err:failed:$message"
                    }
                }
            }
        )
    }

    private fun b64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)
    private fun unb64(text: String): ByteArray = Base64.decode(text, Base64.NO_WRAP)
}

/// Ledgers and Trezors on the phone's USB-C port. An app cannot list or open
/// USB devices itself: Rust lists them with list(), and open(id, name) asks
/// the user to let Airlock use one when it may not yet, opens it, and answers
/// "ok:<fd>" with a file descriptor that Rust owns from then on, or
/// "err:denied:", "err:gone:" or "err:failed:<text>".
@Keep
object UsbBridge {
    private const val TAG = "AirlockUsb"
    private const val ACTION = "dev.airlock.mobile.USB_PERMISSION"
    private const val LEDGER = 0x2c97
    private const val TREZOR = 0x1209
    private const val TREZOR_PRODUCT = 0x53c1

    private val results = ConcurrentHashMap<Long, String>()

    private fun wallet(device: UsbDevice): Boolean =
        device.vendorId == LEDGER || (device.vendorId == TREZOR && device.productId == TREZOR_PRODUCT)

    /// One line per hardware wallet plugged in: "<name>|<vendor id>|<product id>".
    @JvmStatic
    fun list(): String {
        val a = SeedVaultBridge.activity ?: return ""
        val manager = a.getSystemService(UsbManager::class.java) ?: return ""
        return manager.deviceList.values
            .filter(::wallet)
            .joinToString("\n") { "${it.deviceName}|${it.vendorId}|${it.productId}" }
    }

    @JvmStatic
    fun take(id: Long): String? = results.remove(id)

    @JvmStatic
    fun open(id: Long, name: String) {
        val a = SeedVaultBridge.activity
        if (a == null) {
            results[id] = "err:failed:the app is not in front"
            return
        }
        a.runOnUiThread {
            try {
                val manager = a.getSystemService(UsbManager::class.java)
                if (manager == null) {
                    results[id] = "err:failed:this phone has no USB host"
                    return@runOnUiThread
                }
                val device = manager.deviceList[name]
                if (device == null) {
                    results[id] = "err:gone:"
                    return@runOnUiThread
                }
                if (manager.hasPermission(device)) {
                    results[id] = connect(manager, device)
                    return@runOnUiThread
                }
                // Android asks the user, and answers with a broadcast. A
                // Ledger that opens an app comes back as a new device, and
                // is asked about again.
                val receiver = object : BroadcastReceiver() {
                    override fun onReceive(context: Context, intent: Intent) {
                        context.unregisterReceiver(this)
                        val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                        results[id] = try {
                            if (granted && manager.hasPermission(device)) connect(manager, device) else "err:denied:"
                        } catch (e: Exception) {
                            Log.w(TAG, "open after permission", e)
                            "err:failed:" + (e.message ?: e.javaClass.simpleName)
                        }
                    }
                }
                a.registerReceiver(receiver, IntentFilter(ACTION), Context.RECEIVER_NOT_EXPORTED)
                // Mutable, so UsbManager can add whether it was granted; aimed
                // at this app, as Android 14 wants of a mutable PendingIntent.
                val pending = PendingIntent.getBroadcast(
                    a,
                    (id % 0x10000).toInt(),
                    Intent(ACTION).setPackage(a.packageName),
                    PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
                )
                manager.requestPermission(device, pending)
            } catch (e: Exception) {
                Log.w(TAG, "open", e)
                results[id] = "err:failed:" + (e.message ?: e.javaClass.simpleName)
            }
        }
    }

    /// Open `device` and hand over a copy of its file descriptor. The
    /// connection's own descriptor is closed at once: the copy keeps the
    /// device open until Rust closes it.
    private fun connect(manager: UsbManager, device: UsbDevice): String {
        val connection = manager.openDevice(device) ?: return "err:failed:Android could not open it"
        return try {
            val fd = ParcelFileDescriptor.fromFd(connection.fileDescriptor).detachFd()
            "ok:$fd"
        } finally {
            connection.close()
        }
    }
}

/// Mobile Wallet Adapter: other apps on the phone, and web pages in Chrome,
/// ask Airlock to connect and sign through a `solana-wallet:` link, which
/// opens MwaActivity. Solana Mobile's walletlib holds the session: the local
/// WebSocket, its encryption, and the auth tokens it issues. Each request is
/// handed to Rust (`app-mobile`, `mwa.rs`) as JSON through next(); Rust puts
/// it through the same checks and request modal as a browser request, and
/// answers with answer(id, json).
///
/// MwaActivity is off in the manifest. Rust turns it on only in online mode
/// with "Apps on this phone" on in Settings, so an offline Airlock is not a
/// wallet any app can find.
///
/// A session also answers the intents prototype's methods (`airlock_resolve`
/// and the rest, docs/intents.md), which walletlib does not know: its server
/// is replaced with one that takes them (AirlockScenario, AirlockServer) and
/// hands them to Rust like any request, once the session is authorized.
@Keep
object MwaBridge {
    private const val TAG = "AirlockMwa"

    /// The intents prototype's methods, by their MWA names, and whether each
    /// waits for the person. One that waits brings Airlock forward, and
    /// Airlock steps back after its answer, as for a signature. One that
    /// does not (a profile, a resolution, signatures given, a cancel) does
    /// neither.
    private val AIRLOCK_METHODS = mapOf(
        "airlock_get_signing_profile" to false,
        "airlock_resolve" to true,
        "airlock_get_resolution" to false,
        "airlock_provide_signatures" to false,
        "airlock_cancel_resolution" to false,
        "airlock_sign_message" to true,
    )

    /// What get_capabilities lists for them.
    private val AIRLOCK_FEATURES = arrayOf(
        "airlock:getSigningProfile",
        "airlock:resolve",
        "airlock:getResolution",
        "airlock:provideSignatures",
        "airlock:cancelResolution",
        "airlock:signMessage",
    )

    /// The JSON-RPC error code of every refusal of those methods. Which
    /// refusal it is is in `data.code`, the bridge's code. Outside MWA's own
    /// codes (-1 to -7, -100) and JSON-RPC's (-32768 to -32000).
    private const val AIRLOCK_ERROR = -1000

    /// `activity` is the MwaActivity an app waits on, or null for a browser's
    /// session, which no activity holds (see MwaActivity).
    private class Session(val id: Long, val activity: MwaActivity?, val pkg: String?) {
        var scenario: Scenario? = null
        /// A request of it got its answer: the user saw it through.
        @Volatile
        var answered = false
        /// Ended from here, with the setting turned off, not by the app or page.
        @Volatile
        var stopped = false
    }

    /// `waits`: the request may wait for the person.
    private class Pending(val session: Long, val request: Any, val waits: Boolean) {
        val asked = android.os.SystemClock.elapsedRealtime()
    }

    /// One of the intents prototype's methods, waiting for Rust's answer.
    private class AirlockRequest(val server: AirlockServer, val id: Any) {
        fun refuse(code: String, message: String) {
            server.error(id, code, message, null)
        }
    }

    private val events = LinkedBlockingQueue<String>()
    private val sessions = ConcurrentHashMap<Long, Session>()
    private val pending = ConcurrentHashMap<Long, Pending>()
    private val ids = AtomicLong(1)

    /// Whether other apps find Airlock as a wallet.
    @JvmStatic
    fun setEnabled(on: Boolean) {
        val a = SeedVaultBridge.activity ?: return
        val state = if (on) PackageManager.COMPONENT_ENABLED_STATE_ENABLED
        else PackageManager.COMPONENT_ENABLED_STATE_DISABLED
        a.packageManager.setComponentEnabledSetting(
            ComponentName(a, MwaActivity::class.java),
            state,
            PackageManager.DONT_KILL_APP
        )
        if (!on) sessions.values.forEach {
            it.stopped = true
            it.scenario?.close()
        }
    }

    /// The next event for Rust, waiting up to `timeoutMs` for one.
    @JvmStatic
    fun next(timeoutMs: Long): String? = events.poll(timeoutMs, TimeUnit.MILLISECONDS)

    /// Rust's answer to request `id`.
    @JvmStatic
    fun answer(id: Long, json: String) {
        val waiting = pending.remove(id) ?: return
        val session = sessions[waiting.session]
        if (waiting.waits) session?.answered = true
        try {
            complete(waiting.request, JSONObject(json))
        } catch (e: Exception) {
            Log.w(TAG, "could not answer", e)
        }
        if (session != null && waiting.waits) {
            answered(session, android.os.SystemClock.elapsedRealtime() - waiting.asked)
        }
    }

    /// Start a session for `activity`'s link. An app (`pkg` set) waits on the
    /// activity until the session ends; a browser's session is not tied to it.
    fun start(activity: MwaActivity, association: LocalAssociationUri, pkg: String?): Long {
        val id = ids.getAndIncrement()
        val session = Session(id, if (pkg != null) activity else null, pkg)
        sessions[id] = session
        // Without the members AirlockScenario reads, walletlib's own
        // scenario, which answers none of the intent methods.
        val intents = WalletlibMembers.found
        val features = arrayOf(
            ProtocolContract.FEATURE_ID_SIGN_TRANSACTIONS,
            ProtocolContract.FEATURE_ID_SIGN_IN_WITH_SOLANA
        ) + if (intents) AIRLOCK_FEATURES else emptyArray()
        val config = MobileWalletAdapterConfig(
            16, // transactions per request, as for a browser
            1, // messages per request
            arrayOf<Any>(MobileWalletAdapterConfig.LEGACY_TRANSACTION_VERSION, 0),
            3000L,
            features
        )
        val context = activity.applicationContext
        val issuer = AuthIssuerConfig("Airlock")
        val callbacks = Callbacks(session)
        val scenario = if (intents) {
            AirlockScenario(context, config, issuer, callbacks, association, session)
        } else {
            association.createScenario(context, config, issuer, callbacks)
        }
        session.scenario = scenario
        events.put(JSONObject().put("kind", "open").put("session", id).toString())
        scenario.startAsync()
        Log.i(TAG, "session $id from ${pkg ?: "a browser"}")
        return id
    }

    /// The page of a browser's session connected, or asked something: Airlock
    /// comes forward for it. It is in front already as a session starts,
    /// unless the browser first asked its user whether the site may reach
    /// apps on this phone (Brave does, for a site whose wallet library did
    /// not ask beforehand): that question was behind Airlock, and whoever
    /// went back to answer it left Airlock behind. And it stepped back after
    /// an answer (see answered), which a page may follow with another
    /// request. Android may refuse: an app with nothing on screen may put
    /// itself in front only for some seconds after it last had a screen
    /// there. Then the request waits in Airlock.
    private fun comeForward(session: Session) {
        if (session.activity != null) return
        val main = SeedVaultBridge.activity ?: return
        main.runOnUiThread {
            if (main.hasWindowFocus()) return@runOnUiThread
            try {
                main.getSystemService(android.app.ActivityManager::class.java)
                    ?.appTasks
                    ?.firstOrNull { it.taskInfo.baseActivity?.className == MainActivity::class.java.name }
                    ?.moveToFront()
            } catch (e: Exception) {
                Log.w(TAG, "could not bring Airlock forward", e)
            }
        }
    }

    /// How long after an answer Airlock stays in front for what the page asks
    /// next in the same session, before stepping back to the browser: a
    /// moment after an answer the user gave, longer after one that needed
    /// nobody (a site connected before, asking to go on), which a page
    /// follows with what it came for.
    private const val STEP_BACK_AFTER_MS = 300L
    private const val STEP_BACK_UNASKED_AFTER_MS = 1500L

    /// Under this long from request to answer, nobody was asked.
    private const val UNASKED_UNDER_MS = 500L

    /// A request of a browser's session got its answer: back to the page.
    /// Not only once the page ends the session: a phone may keep a browser
    /// that is behind from hearing anything (a Galaxy drops its packets, the
    /// ones from this phone too, within seconds; pages are frozen), so the
    /// answer reaches the page only with the browser in front, and waiting
    /// here for the page to end the session would wait for ever. A moment is
    /// left for a page that asks again at once, which keeps Airlock in front.
    /// `took` is how long the answer took, in milliseconds.
    private fun answered(session: Session, took: Long) {
        if (session.activity != null) return
        val wait = if (took < UNASKED_UNDER_MS) STEP_BACK_UNASKED_AFTER_MS else STEP_BACK_AFTER_MS
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            // An ended session stepped back already (see ended).
            if (sessions[session.id] === session) stepBack(session)
        }, wait)
    }

    /// Airlock steps back, and the task under it, the browser's, shows: no
    /// activity is left in the browser's task to finish instead. Only with
    /// an answer given to the page, Airlock in front still, and nothing else
    /// waiting on the user here. A request that never waits for the user,
    /// such as a page asking how its resolution is going, does not count.
    private fun stepBack(session: Session) {
        val main = SeedVaultBridge.activity ?: return
        main.runOnUiThread {
            val nobodyAsked = pending.values.none { it.waits }
            if (main.hasWindowFocus() && session.answered && !session.stopped && nobodyAsked) {
                main.moveTaskToBack(true)
            }
        }
    }

    /// An app's MwaActivity went away before its session ended: the app gave
    /// up. A browser's session does not end with the activity.
    fun activityGone(id: Long) {
        val session = sessions[id] ?: return
        if (session.activity != null) session.scenario?.close()
    }

    private fun ended(session: Session) {
        if (sessions.remove(session.id) == null) return
        // A resolve still waiting for the user outlives its session, as it
        // outlives its page on desktop: the user may still decide, and the
        // page asks for the resolution later. Airlock stays in front for it.
        val deciding = pending.values.any {
            it.session == session.id && it.waits && it.request is AirlockRequest
        }
        pending.entries.removeIf { it.value.session == session.id }
        events.put(JSONObject().put("kind", "closed").put("session", session.id).toString())
        Log.i(TAG, "session ${session.id} ended")
        val front = SeedVaultBridge.activity?.hasWindowFocus() == true && !deciding
        val activity = session.activity
        if (activity == null) {
            // A browser, whose page has its answer: back to it now.
            if (!deciding) stepBack(session)
            return
        }
        // Back to the app that asked, when Airlock is in front for it: its
        // task, which MwaActivity is in, comes forward, and MwaActivity
        // finishes there, which is the app's sign that the session is over.
        activity.runOnUiThread {
            if (front) {
                activity.getSystemService(android.app.ActivityManager::class.java)
                    ?.moveTaskToFront(activity.taskId, 0)
            }
            activity.done()
        }
    }

    private fun b64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)
    private fun unb64(text: String): ByteArray = Base64.decode(text, Base64.NO_WRAP)

    private fun strings(items: Iterable<String>) = JSONArray().apply { items.forEach { put(it) } }

    private fun accounts(list: Array<AuthorizedAccount>) =
        strings(list.map { Base58.encode(it.publicKey) })

    private fun payloads(list: Array<ByteArray>) = strings(list.map { b64(it) })

    /// Hand a request to Rust with who it comes from. One that `waits` for
    /// the user brings Airlock forward.
    private fun ask(
        session: Session,
        request: Any,
        name: String?,
        uri: Uri?,
        call: JSONObject,
        waits: Boolean = true
    ) {
        val id = ids.getAndIncrement()
        pending[id] = Pending(session.id, request, waits)
        if (waits) comeForward(session)
        val identity = JSONObject()
            .put("name", name ?: JSONObject.NULL)
            .put("uri", uri?.toString() ?: JSONObject.NULL)
            .put("package", session.pkg ?: JSONObject.NULL)
        events.put(
            JSONObject()
                .put("kind", "request")
                .put("id", id)
                .put("session", session.id)
                .put("identity", identity)
                .put("call", call)
                .toString()
        )
    }

    /// The Wallet Standard's SolanaSignInInput for MWA's sign-in payload.
    private fun signInInput(p: SignInWithSolana.Payload): JSONObject {
        val o = JSONObject()
        p.domain?.let { o.put("domain", it) }
        p.addressRaw?.let { o.put("address", Base58.encode(it)) }
        p.statement?.let { o.put("statement", it) }
        p.uri?.let { o.put("uri", it.toString()) }
        p.version?.let { o.put("version", it) }
        p.chainId?.let { o.put("chainId", it) }
        p.nonce?.let { o.put("nonce", it) }
        p.issuedAt?.let { o.put("issuedAt", it) }
        p.expirationTime?.let { o.put("expirationTime", it) }
        p.notBefore?.let { o.put("notBefore", it) }
        p.requestId?.let { o.put("requestId", it) }
        p.resources?.let { list -> o.put("resources", strings(list.map { it.toString() })) }
        return o
    }

    private fun complete(request: Any, answer: JSONObject) {
        val kind = answer.optString("kind")
        fun array(key: String): Array<ByteArray> {
            val list = answer.getJSONArray(key)
            return Array(list.length()) { unb64(list.getString(it)) }
        }
        fun flags(): BooleanArray {
            val list = answer.getJSONArray("valid")
            return BooleanArray(list.length()) { list.getBoolean(it) }
        }
        when (request) {
            is AuthorizeRequest -> {
                if (kind != "authorized") return request.completeWithDecline()
                val list = answer.getJSONArray("accounts")
                val accounts = Array(list.length()) {
                    val one = list.getJSONObject(it)
                    AuthorizedAccount(
                        unb64(one.getString("publicKey")),
                        one.optString("label").ifEmpty { null },
                        null,
                        arrayOf(request.chain),
                        null
                    )
                }
                val signIn = answer.optJSONObject("signIn")?.let {
                    SignInResult(
                        unb64(it.getString("publicKey")),
                        unb64(it.getString("signedMessage")),
                        unb64(it.getString("signature")),
                        "ed25519"
                    )
                }
                request.completeWithAuthorize(accounts, null, null, signIn)
            }
            is ReauthorizeRequest ->
                if (kind == "reauthorized") request.completeWithReauthorize()
                else request.completeWithDecline()
            is SignPayloadsRequest -> when (kind) {
                "signed" -> request.completeWithSignedPayloads(array("payloads"))
                "invalid" -> request.completeWithInvalidPayloads(flags())
                "tooMany" -> request.completeWithTooManyPayloads()
                "notAuthorized" -> request.completeWithAuthorizationNotValid()
                else -> request.completeWithDecline()
            }
            is SignAndSendTransactionsRequest -> when (kind) {
                "sent" -> request.completeWithSignatures(array("signatures"))
                "notSubmitted" -> request.completeWithNotSubmitted(array("signatures"))
                "invalid" -> request.completeWithInvalidSignatures(flags())
                "tooMany" -> request.completeWithTooManyPayloads()
                "notAuthorized" -> request.completeWithAuthorizationNotValid()
                else -> request.completeWithDecline()
            }
            is DeauthorizedEvent -> request.complete()
            is AirlockRequest -> when (kind) {
                "result" -> request.server.result(request.id, answer.opt("result") ?: JSONObject())
                "error" -> request.server.error(
                    request.id,
                    answer.optString("code").ifEmpty { "internal" },
                    answer.optString("message"),
                    answer.optString("requirement").ifEmpty { null }
                )
                else -> request.refuse("rejected", "Airlock declined the request.")
            }
        }
    }

    /// One of the intents prototype's methods, from `server`'s session. It
    /// goes to Rust with the site of the session's authorization, as
    /// walletlib checked it, and is refused without one.
    private fun airlock(
        session: Session,
        scenario: AirlockScenario,
        server: AirlockServer,
        id: Any,
        method: String,
        waits: Boolean,
        params: Any?
    ) {
        val request = AirlockRequest(server, id)
        if (params != null && params !is JSONObject) {
            return request.refuse("bad_request", "The params of $method are an object.")
        }
        val site = scenario.authorization()
            ?: return request.refuse("rejected", "This session is not authorized. Call authorize first.")
        val call = JSONObject()
            .put("method", "intent")
            .put("name", method)
            .put("params", params ?: JSONObject())
        ask(session, request, site.first, site.second, call, waits)
    }

    /// walletlib builds a session's server from two members of its scenario
    /// that a subclass cannot reach: the method handlers (package-private,
    /// in BaseScenario) and the session's state callbacks (private, in
    /// LocalScenario). They are found by their types, which R8's renaming
    /// leaves to match, and read once a page connects.
    private object WalletlibMembers {
        private val fields: Pair<java.lang.reflect.Field, java.lang.reflect.Field>? by lazy {
            try {
                val all = generateSequence<Class<*>>(LocalWebSocketServerScenario::class.java) { it.superclass }
                    .flatMap { it.declaredFields.asSequence() }
                    .filter { !java.lang.reflect.Modifier.isStatic(it.modifiers) }
                    .toList()
                fun of(type: Class<*>) = all.single { type.isAssignableFrom(it.type) }.apply { isAccessible = true }
                Pair(
                    of(MobileWalletAdapterServer.MethodHandlers::class.java),
                    of(MobileWalletAdapterSessionCommon.StateCallbacks::class.java)
                )
            } catch (e: Exception) {
                Log.w(TAG, "walletlib is not laid out as expected: no intent methods", e)
                null
            }
        }

        val found: Boolean get() = fields != null

        fun read(scenario: LocalWebSocketServerScenario):
            Pair<MobileWalletAdapterServer.MethodHandlers, MobileWalletAdapterSessionCommon.StateCallbacks>? {
            val (handlers, states) = fields ?: return null
            return try {
                Pair(
                    handlers.get(scenario) as MobileWalletAdapterServer.MethodHandlers,
                    states.get(scenario) as MobileWalletAdapterSessionCommon.StateCallbacks
                )
            } catch (e: Exception) {
                Log.w(TAG, "could not read walletlib's scenario", e)
                null
            }
        }
    }

    /// walletlib's local scenario, built as `LocalAssociationUri.createScenario`
    /// builds it, whose sessions' server is an AirlockServer.
    private class AirlockScenario(
        context: Context,
        config: MobileWalletAdapterConfig,
        issuer: AuthIssuerConfig,
        callbacks: LocalScenario.Callbacks,
        association: LocalAssociationUri,
        private val session: Session
    ) : LocalWebSocketServerScenario(
        context,
        config,
        issuer,
        callbacks,
        association.associationPublicKey,
        association.associationProtocolVersions,
        association.port
    ) {
        override fun createMessageReceiver(): MessageReceiver {
            val (handlers, states) = WalletlibMembers.read(this) ?: return super.createMessageReceiver()
            val server = AirlockServer(mMobileWalletAdapterConfig, mIoLooper, handlers, this)
            return MobileWalletAdapterSession(this, server, states)
        }

        /// The name and site of the authorization the session holds, as
        /// walletlib issued or checked it, or null with none. walletlib's own
        /// signing requests go by the same record.
        fun authorization(): Pair<String?, Uri?>? {
            val record = synchronized(mLock) { mActiveAuthorization } ?: return null
            if (record.isRevoked) return null
            val uri = record.identity.uri?.takeIf { it.toString().isNotEmpty() }
            val name = record.identity.name?.takeIf { it.isNotEmpty() }
            return Pair(name, uri)
        }

        fun request(server: AirlockServer, id: Any, method: String, waits: Boolean, params: Any?) =
            airlock(session, this, server, id, method, waits, params)
    }

    /// walletlib's server, which also takes the intent methods. Everything
    /// else is walletlib's. An error goes out with its `data` as an object,
    /// which walletlib's own error path writes only as a string.
    private class AirlockServer(
        config: MobileWalletAdapterConfig,
        looper: android.os.Looper,
        handlers: MobileWalletAdapterServer.MethodHandlers,
        private val scenario: AirlockScenario
    ) : MobileWalletAdapterServer(config, looper, handlers) {
        private val io = android.os.Handler(looper)

        /// The session's sender, which walletlib's server keeps to itself.
        /// Sent on under the server's own lock, as walletlib sends.
        private var sender: MessageSender? = null

        override fun receiverConnected(messageSender: MessageSender) {
            super.receiverConnected(messageSender)
            synchronized(this) { sender = messageSender }
        }

        override fun receiverDisconnected() {
            synchronized(this) { sender = null }
            super.receiverDisconnected()
        }

        override fun dispatchRpc(id: Any?, method: String, params: Any?) {
            val waits = AIRLOCK_METHODS[method] ?: return super.dispatchRpc(id, method, params)
            // A notification gets no answer, so nobody would hear one.
            if (id == null || id == JSONObject.NULL) return
            scenario.request(this, id, method, waits, params)
        }

        /// Answer request `id` with `result`, on walletlib's thread.
        fun result(id: Any, result: Any) {
            io.post {
                try {
                    handleRpcResult(id, result)
                } catch (e: java.io.IOException) {
                    Log.w(TAG, "could not answer $id", e)
                }
            }
        }

        /// Refuse request `id`: the bridge's `code`, and the requirement
        /// it names, in `data`.
        fun error(id: Any, code: String, message: String, requirement: String?) {
            val data = JSONObject().put("code", code)
            requirement?.let { data.put("requirement", it) }
            val error = JSONObject().put("code", AIRLOCK_ERROR).put("message", message).put("data", data)
            val bytes = JSONObject()
                .put("jsonrpc", "2.0")
                .put("error", error)
                .put("id", id)
                .toString()
                .toByteArray(Charsets.UTF_8)
            io.post {
                try {
                    synchronized(this) {
                        val to = sender
                        if (to == null) Log.w(TAG, "no session to answer $id") else to.send(bytes)
                    }
                } catch (e: java.io.IOException) {
                    Log.w(TAG, "could not answer $id", e)
                }
            }
        }
    }

    private class Callbacks(private val session: Session) : LocalScenario.Callbacks {
        override fun onScenarioReady() {}
        override fun onScenarioServingClients() = comeForward(session)
        override fun onScenarioServingComplete() {
            session.scenario?.close()
        }
        override fun onScenarioComplete() {}
        override fun onScenarioError() = ended(session)
        override fun onScenarioTeardownComplete() = ended(session)
        override fun onLowPowerAndNoConnection() {
            Log.w(TAG, "low power mode, and no connection from the app")
        }

        override fun onAuthorizeRequest(request: AuthorizeRequest) {
            val call = JSONObject().put("method", "authorize")
            request.signInPayload?.let { call.put("signIn", signInInput(it)) }
            ask(session, request, request.identityName, request.identityUri, call)
        }

        override fun onReauthorizeRequest(request: ReauthorizeRequest) {
            // walletlib checked the token; Rust checks the site still has a grant.
            val call = JSONObject().put("method", "reauthorize")
            ask(session, request, request.identityName, request.identityUri, call)
        }

        override fun onSignTransactionsRequest(request: SignTransactionsRequest) {
            val call = JSONObject()
                .put("method", "signTransactions")
                .put("accounts", accounts(request.authorizedAccounts))
                .put("chain", request.chain)
                .put("payloads", payloads(request.payloads))
            ask(session, request, request.identityName, request.identityUri, call)
        }

        override fun onSignMessagesRequest(request: SignMessagesRequest) {
            val call = JSONObject()
                .put("method", "signMessages")
                .put("addresses", strings(request.addresses.map { Base58.encode(it) }))
                .put("payloads", payloads(request.payloads))
            ask(session, request, request.identityName, request.identityUri, call)
        }

        override fun onSignAndSendTransactionsRequest(request: SignAndSendTransactionsRequest) {
            val call = JSONObject()
                .put("method", "signAndSend")
                .put("accounts", accounts(request.authorizedAccounts))
                .put("chain", request.chain)
                .put("payloads", payloads(request.payloads))
                .put("commitment", request.commitment ?: JSONObject.NULL)
                .put("serial", request.waitForCommitmentToSendNextTransaction == true)
            ask(session, request, request.identityName, request.identityUri, call)
        }

        override fun onDeauthorizedEvent(event: DeauthorizedEvent) {
            val call = JSONObject().put("method", "deauthorize")
            ask(session, event, event.identityName, event.identityUri, call)
        }
    }
}

/// What a `solana-wallet:` link opens. It has no window of its own: it
/// starts the MWA session and brings Airlock forward for the request modal.
///
/// An app opens it for a result, in the app's own task, and waits on it: it
/// stays behind until the session ends and then finishes, which is the app's
/// sign that the session is over.
///
/// A browser opens it with NEW_TASK, in a task of its own, and waits on the
/// session's WebSocket alone. It finishes at once: kept, that task would be a
/// blank card in recents, or, excluded from recents, one Android trims as
/// soon as Airlock is in front ("recent-task-trimmed"), which used to end the
/// session before the user could answer.
class MwaActivity : Activity() {
    private var session = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Restored after Airlock's process went away, or opened again from
        // recents: the link is an old one that no app is waiting on any more.
        val fromHistory = (intent?.flags ?: 0) and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0
        if (savedInstanceState != null || fromHistory) {
            finishAndRemoveTask()
            return
        }
        val association = intent?.data?.let {
            try {
                AssociationUri.parse(it)
            } catch (e: Exception) {
                null
            }
        }
        if (association !is LocalAssociationUri) {
            // Sessions through a reflector server are not offered.
            setResult(RESULT_CANCELED)
            finish()
            return
        }
        val pkg = callingPackage
        session = MwaBridge.start(this, association, pkg)
        showAirlock()
        if (pkg == null) {
            // A browser: the session goes on without this activity.
            session = 0L
            finish()
        }
    }

    /// Bring Airlock's own task forward. A running Airlock is moved to the
    /// front as it is: an intent could start a second MainActivity, and with
    /// it a second app over the same activity reference. Otherwise it is
    /// started from the launcher.
    private fun showAirlock() {
        val tasks = getSystemService(android.app.ActivityManager::class.java)?.appTasks.orEmpty()
        val running = tasks.firstOrNull {
            it.taskInfo.baseActivity?.className == MainActivity::class.java.name
        }
        if (running != null) {
            running.moveToFront()
        } else {
            packageManager.getLaunchIntentForPackage(packageName)?.let { startActivity(it) }
        }
    }

    fun done() {
        if (!isFinishing) {
            setResult(RESULT_OK)
            finish()
        }
    }

    override fun onDestroy() {
        if (session != 0L) MwaBridge.activityGone(session)
        super.onDestroy()
    }
}

/// Starts Airlock again in a new process, as ProcessPhoenix does: Settings →
/// Mode switches folders, which only a new process can. It runs in a process
/// of its own (":restart"), ends Airlock's, and opens the launcher activity.
@Keep
class RestartActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pid = intent.getIntExtra(PID, -1)
        if (pid > 0 && pid != android.os.Process.myPid()) android.os.Process.killProcess(pid)
        packageManager.getLaunchIntentForPackage(packageName)?.let {
            it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
            startActivity(it)
        }
        finish()
        Runtime.getRuntime().exit(0)
    }

    companion object {
        private const val PID = "pid"

        @JvmStatic
        fun restart() {
            val a = SeedVaultBridge.activity ?: return
            a.startActivity(
                Intent(a, RestartActivity::class.java)
                    .putExtra(PID, android.os.Process.myPid())
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        }
    }
}
