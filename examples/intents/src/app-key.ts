/** The app's own key: a throwaway keypair that stands for an app's signer
 * and fee sponsor. It lives in this browser's storage, in plain text. */

import { Keypair } from "@solana/web3.js";
import { load, save } from "./storage";

const KEY = "appKey";

export function loadAppKey(): Keypair {
  const stored = load<number[]>(KEY);
  if (Array.isArray(stored) && stored.length === 64) {
    try {
      return Keypair.fromSecretKey(Uint8Array.from(stored));
    } catch {
      // A damaged key is replaced below.
    }
  }
  return newAppKey();
}

export function newAppKey(): Keypair {
  const key = Keypair.generate();
  save(KEY, [...key.secretKey]);
  return key;
}
