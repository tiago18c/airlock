/** Ed25519 through the browser's own Web Crypto, so the page signs a
 * transaction in any format and checks a signature. A browser whose Web
 * Crypto has no Ed25519 (Chrome on Android before 137, for one) gets
 * @noble/curves' instead, which web3.js already brings. */

import { ed25519 } from "@noble/curves/ed25519";

function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function subtle(): SubtleCrypto {
  if (!globalThis.crypto?.subtle) {
    throw new Error("This browser has no Web Crypto here. Open the page on http://127.0.0.1 or over https.");
  }
  return globalThis.crypto.subtle;
}

/** Whether Web Crypto refused the algorithm itself, as a browser without Ed25519 does. */
function unsupported(err: unknown): boolean {
  return err instanceof DOMException && (err.name === "NotSupportedError" || /Unrecognized name/i.test(err.message));
}

/** Signs `message` with a Solana secret key: the 32-byte seed, then the public key. */
export async function sign(secretKey: Uint8Array, message: Uint8Array): Promise<Uint8Array> {
  const jwk: JsonWebKey = {
    kty: "OKP",
    crv: "Ed25519",
    d: base64url(secretKey.subarray(0, 32)),
    x: base64url(secretKey.subarray(32, 64)),
  };
  let key: CryptoKey;
  try {
    key = await subtle().importKey("jwk", jwk, { name: "Ed25519" }, false, ["sign"]);
  } catch (err) {
    if (unsupported(err)) return ed25519.sign(message, secretKey.subarray(0, 32));
    throw err;
  }
  return new Uint8Array(await subtle().sign({ name: "Ed25519" }, key, message as BufferSource));
}

export async function verify(publicKey: Uint8Array, message: Uint8Array, signature: Uint8Array): Promise<boolean> {
  let key: CryptoKey;
  try {
    key = await subtle().importKey("raw", publicKey as BufferSource, { name: "Ed25519" }, false, ["verify"]);
  } catch (err) {
    if (unsupported(err)) return ed25519.verify(signature, message, publicKey);
    throw err;
  }
  return subtle().verify({ name: "Ed25519" }, key, signature as BufferSource, message as BufferSource);
}
