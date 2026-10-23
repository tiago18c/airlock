import assert from "node:assert/strict";
import { test } from "node:test";
import {
  BridgeError,
  METHODS,
  base64ToBytes,
  bytesToBase64,
  encodeBase58,
  isRequestId,
  newRequestId,
  shortCode,
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
  assert.deepEqual([...METHODS], ["status", "connect", "disconnect", "signTransaction"]);
});

test("a rejection is named the way wallet adapters expect", () => {
  assert.equal(new BridgeError("rejected", "no").name, "WalletUserRejected");
  assert.equal(new BridgeError("expired", "late").name, "AirlockBridgeError");
});
