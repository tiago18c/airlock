// Airlock's activity: wry's, plus the bridge to the Seed Vault of a Solana
// Mobile phone (Saga, Seeker), fingerprint unlock (KeyGuardBridge), and
// Ledgers and Trezors over USB (UsbBridge). dx writes this file in place of its default
// MainActivity.kt (`android_main_activity` in Dioxus.toml).
//
// Rust (`app-mobile`, `seed_vault.rs`) calls SeedVaultBridge's methods over
// JNI. Each request gets an id; the Seed Vault's screen opens over the app,
// and its result is kept under that id until Rust takes it with take(id).
// A result is "ok:<text>" or "err:<code>:<text>".
package dev.dioxus.main

import android.app.PendingIntent
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
import java.util.concurrent.ConcurrentHashMap

typealias BuildConfig = dev.airlock.mobile.BuildConfig

class MainActivity : WryActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        // Before super: wry starts the app's Rust from it, which may ask the
        // bridges for the activity at once.
        SeedVaultBridge.activity = this
        super.onCreate(savedInstanceState)
        fitSystemBars()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        followTheme()
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
