import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { test } from "node:test";
import { Keypair } from "@solana/web3.js";
import { sign, verify } from "../src/ed25519";

if (!globalThis.crypto) Object.defineProperty(globalThis, "crypto", { value: webcrypto });

/** Web Crypto as Chrome on Android before 137 has it: no Ed25519. */
async function withoutEd25519<T>(body: () => Promise<T>): Promise<T> {
  const subtle = globalThis.crypto.subtle;
  const importKey = subtle.importKey;
  subtle.importKey = (async () => {
    throw new DOMException("Algorithm: Unrecognized name", "NotSupportedError");
  }) as typeof subtle.importKey;
  try {
    return await body();
  } finally {
    subtle.importKey = importKey;
  }
}

test("without Ed25519 in Web Crypto, a signature is the same and still checks", async () => {
  const keypair = Keypair.generate();
  const message = new TextEncoder().encode("intents on a phone");
  const native = await sign(keypair.secretKey, message);
  const fallback = await withoutEd25519(() => sign(keypair.secretKey, message));
  assert.deepEqual(fallback, native);
  assert.equal(await withoutEd25519(() => verify(keypair.publicKey.toBytes(), message, native)), true);
  const other = new TextEncoder().encode("something else");
  assert.equal(await withoutEd25519(() => verify(keypair.publicKey.toBytes(), other, native)), false);
});

test("another Web Crypto failure is not taken for a missing algorithm", async () => {
  const subtle = globalThis.crypto.subtle;
  const importKey = subtle.importKey;
  subtle.importKey = (async () => {
    throw new DOMException("bad key data", "DataError");
  }) as typeof subtle.importKey;
  try {
    await assert.rejects(sign(Keypair.generate().secretKey, new Uint8Array([1])), /bad key data/);
  } finally {
    subtle.importKey = importKey;
  }
});
