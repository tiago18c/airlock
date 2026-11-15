import assert from "node:assert/strict";
import { test } from "node:test";
import {
  BridgeError,
  INTENT_METHODS,
  METHODS,
  QUICK_TTL_MS,
  base64ToBytes,
  bytesToBase64,
  decodeBase58,
  encodeBase58,
  isBridgeMethod,
  isPushMethod,
  isRequestId,
  newRequestId,
  replyWindowMs,
  shortCode,
  waitsForUser,
} from "../src/protocol";

test("base58 matches known Solana addresses", () => {
  // The System program is 32 zero bytes.
  assert.equal(encodeBase58(new Uint8Array(32)), "11111111111111111111111111111111");
  // Leading zero bytes become leading 1s; the rest is big-endian base 58.
  assert.equal(encodeBase58(new Uint8Array([0, 0, 1])), "112");
  assert.equal(encodeBase58(new Uint8Array([255])), "5Q");
  assert.equal(encodeBase58(new TextEncoder().encode("hello world")), "StV1DL6CwTryKyV");
});

test("base64 round-trips bytes, including a payload larger than one chunk", () => {
  const bytes = new Uint8Array(100_000).map((_, i) => i % 251);
  assert.deepEqual(base64ToBytes(bytesToBase64(bytes)), bytes);
  assert.equal(bytesToBase64(new Uint8Array([0x81, 1, 2])), "gQEC");
});

test("a request id is short, and only letters, digits and dashes", () => {
  assert.ok(isRequestId("7f3ac0de-11aa-4bcd-8eef-001122334455"));
  assert.ok(!isRequestId(""));
  assert.ok(!isRequestId("has space"));
  assert.ok(!isRequestId("x".repeat(81)));
  assert.ok(!isRequestId(42));
  assert.equal(shortCode("7f3ac0de-11aa"), "7F3A");
});

test("newRequestId works when crypto.randomUUID is missing", () => {
  const original = globalThis.crypto;
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: {
      getRandomValues(bytes: Uint8Array) {
        for (let i = 0; i < bytes.length; i += 1) bytes[i] = (i * 17 + 3) & 0xff;
        return bytes;
      },
    },
  });
  try {
    const id = newRequestId();
    assert.ok(isRequestId(id));
    assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  } finally {
    Object.defineProperty(globalThis, "crypto", { configurable: true, value: original });
  }
});

test("only methods the desktop app answers are forwarded", () => {
  assert.deepEqual([...METHODS], ["status", "connect", "disconnect", "signTransaction", "signAndSendTransaction", "signAndSendAllTransactions", "signMessage", "signIn"]);
  assert.deepEqual([...INTENT_METHODS], ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution", "resolveMessage"]);
  for (const method of [...METHODS, ...INTENT_METHODS]) assert.ok(isBridgeMethod(method), method);
  for (const method of ["cancel", "pages", "resolutionChanged", "toString", 7]) assert.ok(!isBridgeMethod(method), String(method));
});

test("the intent methods are answered at once, except a message that may wait for a signature", () => {
  for (const method of ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution"]) {
    assert.ok(!waitsForUser(method, {}), method);
    assert.equal(replyWindowMs(method, {}), QUICK_TTL_MS, method);
  }
  assert.ok(waitsForUser("resolveMessage", { mode: "memo" }));
  assert.equal(replyWindowMs("resolveMessage", {}), replyWindowMs("signMessage", {}));
});

test("only the app's pushes are passed to pages", () => {
  assert.ok(isPushMethod("accountsChanged"));
  assert.ok(isPushMethod("resolutionChanged"));
  assert.ok(!isPushMethod("resolve"));
  assert.ok(!isPushMethod(undefined));
});

test("base58 decodes what it encodes, and refuses what is not base58", () => {
  for (const bytes of [new Uint8Array(32), new Uint8Array([0, 0, 1]), new Uint8Array(64).map((_, i) => i * 7), new Uint8Array()]) {
    assert.deepEqual(decodeBase58(encodeBase58(bytes)), bytes);
  }
  assert.deepEqual(decodeBase58("5Q"), new Uint8Array([255]));
  assert.equal(decodeBase58("0OIl"), null);
});

test("a rejection is named the way wallet adapters expect", () => {
  assert.equal(new BridgeError("rejected", "no").name, "WalletUserRejected");
  assert.equal(new BridgeError("expired", "late").name, "AirlockBridgeError");
});
