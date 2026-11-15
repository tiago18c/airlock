import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { test } from "node:test";
import { Keypair, SystemProgram, Transaction } from "@solana/web3.js";
import { bytesToBase64, encodeBase58 } from "../../../extension/src/protocol";
import { relayFromQuery } from "../src/dev-transport";
import { sign, verify } from "../src/ed25519";
import { memo } from "../src/instructions";
import { RpcRefusal, checkMemo, proveMemo, type FetchedTransaction, type Rpc } from "../src/memo-check";
import { isSigner, isWritable, missingSigners, parseTransaction, withSignature } from "../src/wire";
import { BLOCKHASH_A, BLOCKHASH_B, asV1, key, legacy } from "./helpers";

if (!globalThis.crypto) Object.defineProperty(globalThis, "crypto", { value: webcrypto });

test("a legacy transaction reads as web3.js wrote it", () => {
  const payer = key();
  const to = key();
  const wire = legacy(payer, [SystemProgram.transfer({ fromPubkey: payer, toPubkey: to, lamports: 5 })]);
  const tx = parseTransaction(wire);
  assert.equal(tx.format, "legacy");
  assert.equal(tx.blockhash, BLOCKHASH_A);
  assert.deepEqual(tx.staticKeys, [payer.toBase58(), to.toBase58(), SystemProgram.programId.toBase58()]);
  assert.ok(isSigner(tx, 0) && isWritable(tx, 0));
  assert.ok(!isSigner(tx, 1) && isWritable(tx, 1));
  assert.ok(!isWritable(tx, 2));
  assert.deepEqual(missingSigners(tx), [payer.toBase58()]);
});

test("a signature goes into its signer's slot, in legacy and in v1", async () => {
  const payer = Keypair.generate();
  const cosigner = Keypair.generate();
  const wire = legacy(payer.publicKey, [memo("hi", [payer.publicKey, cosigner.publicKey])]);
  for (const format of [wire, asV1(wire, BLOCKHASH_B)]) {
    const tx = parseTransaction(format);
    const signature = await sign(cosigner.secretKey, tx.message);
    assert.ok(await verify(cosigner.publicKey.toBytes(), tx.message, signature));
    const signed = withSignature(format, cosigner.publicKey.toBase58(), signature);
    assert.deepEqual(missingSigners(parseTransaction(signed)), [payer.publicKey.toBase58()]);
    assert.throws(() => withSignature(format, key().toBase58(), signature), /not a signer/);
  }
});

test("the page signs exactly what web3.js signs", async () => {
  const payer = Keypair.generate();
  const tx = new Transaction({ feePayer: payer.publicKey, recentBlockhash: BLOCKHASH_A }).add(memo("x", [payer.publicKey]));
  tx.sign(payer);
  const theirs = new Uint8Array(tx.signature!);
  const ours = await sign(payer.secretKey, parseTransaction(new Uint8Array(tx.serialize())).message);
  assert.deepEqual(ours, theirs);
});

test("a v1 transaction reads past its config to its instructions and signatures", () => {
  const payer = key();
  const wire = asV1(legacy(payer, [memo("hello", [payer])]));
  const tx = parseTransaction(wire);
  assert.equal(tx.format, "v1");
  assert.equal(tx.blockhash, BLOCKHASH_B);
  assert.equal(tx.signatures.length, 1);
  assert.equal(new TextDecoder().decode(tx.instructions[0].data), "hello");
  assert.equal(tx.message.length, wire.length - 64);
});

test("a memo proof holds only with exactly the account and the message, in a transaction that succeeded", () => {
  const account = key();
  const message = new TextEncoder().encode("site.test: nonce 1");
  const wire = legacy(account, [memo(message, [account])]);
  const fetched = (meta: FetchedTransaction["meta"], bytes = wire): FetchedTransaction => ({ transaction: [bytesToBase64(bytes), "base64"], meta });
  assert.ok(checkMemo(fetched({ err: null }), account.toBase58(), message).holds);
  assert.match(checkMemo(fetched({ err: { InstructionError: [0, "x"] } }), account.toBase58(), message).detail, /failed/);
  assert.match(checkMemo(fetched({ err: null }), account.toBase58(), new TextEncoder().encode("other")).detail, /other text/);

  const both = legacy(account, [memo(message, [account, key()])]);
  assert.match(checkMemo(fetched({ err: null }, both), account.toBase58(), message).detail, /not exactly the account/);
});

test("a memo run inside a program counts, read from the inner instructions", () => {
  const account = key();
  const carrier = key();
  const message = new TextEncoder().encode("from a vault");
  // The transaction a member signs: the vault's memo is not at its top.
  const wire = legacy(carrier, [memo("execute", [carrier])]);
  const keys = parseTransaction(wire).staticKeys;
  // The node adds the vault and the Memo program as loaded or known keys; here
  // they come from a lookup the node reports.
  const meta: FetchedTransaction["meta"] = {
    err: null,
    loadedAddresses: { writable: [], readonly: [account.toBase58()] },
    innerInstructions: [
      { index: 0, instructions: [{ programIdIndex: keys.indexOf("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"), accounts: [keys.length], data: encodeBase58(message) }] },
    ],
  };
  const verdict = checkMemo({ transaction: [bytesToBase64(wire), "base64"], meta }, account.toBase58(), message);
  assert.ok(verdict.holds, verdict.detail);
  assert.match(verdict.detail, /inner instruction 0 of instruction 0/);
});

test("a version 1 memo is read when the node is asked for version 1, and left unchecked when it shows none", async () => {
  const account = key();
  const message = new TextEncoder().encode("site.test: nonce 2");
  const wire = asV1(legacy(account, [memo(message, [account])]));
  const v1Refusal = new RpcRefusal("Internal error", "Transaction encoding error: Encoding does not support transaction version 1");
  const asked: unknown[] = [];
  // A node that knows version 1: refuses 0, shows it at 1.
  const knows: Rpc = async (method, params) => {
    asked.push(params[1]);
    if (method !== "getTransaction") throw new Error(method);
    if ((params[1] as { maxSupportedTransactionVersion: number }).maxSupportedTransactionVersion < 1) throw v1Refusal;
    return { version: 1, transaction: [bytesToBase64(wire), "base64"], meta: { err: null } };
  };
  const read = await proveMemo(knows, "sig", account.toBase58(), message);
  assert.equal(read?.verdict, "holds");
  assert.equal(asked.length, 2);

  // A node that shows no version 1 transaction: the status alone, unchecked.
  const blind: Rpc = async (method) => {
    if (method === "getTransaction") throw v1Refusal;
    return { value: [{ err: null, confirmationStatus: "confirmed" }] };
  };
  const unchecked = await proveMemo(blind, "sig", account.toBase58(), message);
  assert.equal(unchecked?.verdict, "unchecked");
  assert.ok(unchecked?.verdict === "unchecked" && unchecked.succeeded === true);
  assert.match(unchecked?.detail ?? "", /not checked/);

  // Not found yet: nothing to say.
  assert.equal(await proveMemo(async () => null, "sig", account.toBase58(), message), null);
});

test("the dev transport connects only to a websocket on this machine", () => {
  assert.equal(relayFromQuery(""), null);
  assert.equal(relayFromQuery("?other=1"), null);
  for (const good of ["ws://127.0.0.1:9000", "ws://localhost:9000/relay", "ws://[::1]:9000"]) {
    const url = relayFromQuery(`?wallet=${encodeURIComponent(good)}`);
    assert.ok(url instanceof URL, good);
  }
  for (const bad of ["ws://example.com:9000", "wss://127.0.0.1:9000", "http://127.0.0.1:9000", "ws://127.0.0.2:9000", "ws://127.0.0.1.evil.test", "nonsense"]) {
    assert.ok(relayFromQuery(`?wallet=${encodeURIComponent(bad)}`) instanceof Error, bad);
  }
});
