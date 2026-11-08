// Airlock's activity: wry's, plus the bridge to the Seed Vault of a Solana
// Mobile phone (Saga, Seeker). dx writes this file in place of its default
// MainActivity.kt (`android_main_activity` in Dioxus.toml).
//
// Rust (`app-mobile`, `seed_vault.rs`) calls SeedVaultBridge's methods over
// JNI. Each request gets an id; the Seed Vault's screen opens over the app,
// and its result is kept under that id until Rust takes it with take(id).
// A result is "ok:<text>" or "err:<code>:<text>".
package dev.dioxus.main

import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
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
        super.onCreate(savedInstanceState)
        SeedVaultBridge.activity = this
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
                else -> Wallet.onAuthorizeSeedResult(code, data)
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
