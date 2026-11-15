import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { Keypair } from "@solana/web3.js";
import type { WalletAccount } from "@wallet-standard/base";
import {
  CANCEL_RESOLUTION,
  GET_RESOLUTION,
  GET_SIGNING_PROFILE,
  INTENT_FEATURES,
  PROVIDE_SIGNATURES,
  RESOLVE,
  SIGN_MESSAGE,
  type IntentFeatures,
  type Resolution,
} from "../../../extension/src/intents";
import { decodeBase58, encodeBase58 } from "../../../extension/src/protocol";
import * as patch from "../mwa-patch.mjs";
import { asRefusal, refusalMatches } from "../src/actions";
import { GESTURE_NEEDED, MwaTransport, createMwaWallet, isAndroid, mwaError, rpcName, type MwaAuth, type MwaTiming } from "../src/mwa";
import { ResolutionStore } from "../src/resolutions";
import { FakeMwa, base64, refusal, resolution, rpcError, sessionClosed } from "./fake-mwa";

const KEY = Keypair.fromSeed(new Uint8Array(32).fill(7));
const ADDRESS = KEY.publicKey.toBase58();
const IDENTITY = { name: "Airlock intents test page", uri: "http://localhost:8124", icon: "icon.png" };
const TX = new Uint8Array([1, 2, 3, 4]);

/** Fast clocks: ask every 5 ms, for up to half a second. */
const FAST: Partial<MwaTiming> = { pollMs: 5, pollWindowMs: 500, lingerMs: 30, quickMs: 300, waitMs: 1000 };

function setup(options: { timing?: Partial<MwaTiming>; saved?: MwaAuth | null; canOpen?: () => boolean; chain?: string } = {}) {
  const fake = new FakeMwa([KEY.publicKey.toBytes()]);
  const logs: string[] = [];
  const notFound: string[] = [];
  let kept: MwaAuth | null = options.saved ?? null;
  const transport = new MwaTransport({
    transact: fake.transact,
    identity: IDENTITY,
    chain: () => options.chain ?? "solana:devnet",
    events: {
      log: (kind, text) => logs.push(`${kind} ${text}`),
      notFound: (id) => notFound.push(id),
      changed() {},
    },
    saved: { load: () => kept, save: (auth) => (kept = auth) },
    timing: { ...FAST, ...options.timing },
    canOpen: options.canOpen,
  });
  const wallet = createMwaWallet(transport);
  const intents = () => wallet.features as unknown as IntentFeatures;
  const connect = (silent = false) =>
    (wallet.features["standard:connect"] as { connect(input?: { silent?: boolean }): Promise<{ accounts: readonly WalletAccount[] }> }).connect({ silent });
  const account = () => wallet.accounts[0];
  const heard: Resolution[] = [];
  return { fake, transport, wallet, logs, notFound, saved: () => kept, intents, connect, account, heard };
}

/** Waits for `condition`, a few milliseconds at a time. */
async function until(condition: () => boolean, what: string, ms = 2000): Promise<void> {
  const start = Date.now();
  while (!condition()) {
    if (Date.now() - start > ms) throw new Error(`Timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

/** Connects, and waits for Connect's session to end. */
async function connected(t: ReturnType<typeof setup>): Promise<void> {
  await t.connect();
  await until(() => t.fake.open.size === 0, "Connect's session to end");
}

/** Answers get_resolution for `id` with each state in turn, then the last one for good. */
function states(fake: FakeMwa, id: string, list: Resolution["state"][], extra: Partial<Resolution> = {}): void {
  let turn = 0;
  fake.on.airlock_get_resolution = (params) => {
    assert.equal(params.id, id);
    const state = list[Math.min(turn, list.length - 1)];
    turn += 1;
    return { resolution: resolution(id, state, extra) };
  };
}

test("the library's camelCase names reach the six airlock_ methods", () => {
  assert.equal(rpcName("airlockGetSigningProfile"), "airlock_get_signing_profile");
  assert.equal(rpcName("airlockResolve"), "airlock_resolve");
  assert.equal(rpcName("airlockGetResolution"), "airlock_get_resolution");
  assert.equal(rpcName("airlockProvideSignatures"), "airlock_provide_signatures");
  assert.equal(rpcName("airlockCancelResolution"), "airlock_cancel_resolution");
  assert.equal(rpcName("airlockSignMessage"), "airlock_sign_message");
  assert.equal(isAndroid("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/131.0 Mobile Safari/537.36"), true);
  assert.equal(isAndroid("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0"), false);
});

test("Connect authorizes for the page's chain with its identity, checks the features and keeps the token", async () => {
  const t = setup();
  const { accounts } = await t.connect();
  assert.deepEqual(accounts.map((a) => a.address), [ADDRESS]);
  assert.deepEqual(t.fake.methods(1), ["authorize", "get_capabilities"]);
  const authorize = t.fake.callsOf("authorize")[0];
  assert.deepEqual(authorize.params, { identity: IDENTITY, chain: "solana:devnet" });
  assert.equal(t.saved()?.token, "token-1");
  assert.equal(t.saved()?.chain, "solana:devnet");
  assert.equal(t.transport.speaksIntents(), true);
  for (const feature of INTENT_FEATURES) assert.ok(feature in t.wallet.features, feature);
  // What the page asks right after Connect (the profile) goes in Connect's session.
  t.fake.on.airlock_get_signing_profile = (params) => ({ profile: { account: params.account, timing: "sync" } });
  const profile = await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() });
  assert.equal(profile.account, ADDRESS);
  assert.deepEqual(t.fake.methods(1), ["authorize", "get_capabilities", "airlock_get_signing_profile"]);
  await until(() => t.fake.open.size === 0, "the session to end once nothing more came");
  assert.equal(t.fake.sessions, 1);
});

test("every later call opens a new session, reauthorizes with the kept token for its chain, and keeps a new token", async () => {
  const t = setup();
  await connected(t);
  t.fake.rotateTokens = true;
  t.fake.on.airlock_get_signing_profile = (params) => ({ profile: { account: params.account } });
  await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() });
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_get_signing_profile"]);
  assert.deepEqual(t.fake.calls.filter((c) => c.session === 2)[0].params, { identity: IDENTITY, chain: "solana:devnet", auth_token: "token-1" });
  assert.equal(t.saved()?.token, "token-2");
  // It ends at once: nothing waits on it.
  await until(() => t.fake.open.size === 0, "session 2 to end");
  await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() });
  assert.equal(t.fake.calls.filter((c) => c.session === 3)[0].params.auth_token, "token-2");
});

test("a reload keeps the connection: a silent connect takes up the kept authorization and opens no session", async () => {
  const first = setup();
  await connected(first);
  const t = setup({ saved: first.saved() });
  const { accounts } = await t.connect(true);
  assert.deepEqual(accounts.map((a) => a.address), [ADDRESS]);
  assert.equal(t.fake.sessions, 0);
  // With nothing kept, a silent connect is no accounts, still with no session.
  const none = setup();
  assert.deepEqual((await none.connect(true)).accounts, []);
  assert.equal(none.fake.sessions, 0);
});

test("a wallet that lists no airlock: features is offered nothing but the fallback", async () => {
  const t = setup();
  t.fake.features = ["solana:signTransactions", "solana:signAndSendTransaction"];
  await connected(t);
  assert.equal(t.transport.speaksIntents(), false);
  assert.deepEqual(t.transport.missingFeatures(), [...INTENT_FEATURES]);
  assert.ok(!(RESOLVE in t.wallet.features));
  assert.ok("solana:signAndSendTransaction" in t.wallet.features);
  assert.ok("standard:connect" in t.wallet.features);
  assert.ok(t.logs.some((line) => line.includes("does not speak intents")));
});

test("a refusal's data.code and requirement reach the rows as the extension's do, whatever the JSON-RPC number", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_resolve = () => {
    throw refusal("unsupported", "This account is not offered cosign.", "delivery", -32099);
  };
  const err = await t.intents()[RESOLVE].resolve({ account: t.account(), chain: "solana:devnet", transactions: [TX], delivery: "cosign" }).catch((e) => e);
  const seen = asRefusal(err);
  assert.deepEqual(seen, { code: "unsupported", message: "This account is not offered cosign.", requirement: "delivery" });
  assert.equal(refusalMatches({ reason: "", code: "unsupported", requirement: "delivery" }, seen!), true);
  // The same refusal under another number reads the same.
  t.fake.on.airlock_resolve = () => {
    throw refusal("bad_notes", "The derivation gives another address.", undefined, 7);
  };
  const lying = asRefusal(await t.intents()[RESOLVE].resolve({ account: t.account(), chain: "solana:devnet", transactions: [TX] }).catch((e) => e));
  assert.deepEqual(lying, { code: "bad_notes", message: "The derivation gives another address.", requirement: undefined });
  // Declined, expired and cancelled come back as errors too.
  for (const code of ["rejected", "expired", "cancelled"]) {
    t.fake.on.airlock_resolve = () => {
      throw refusal(code, code);
    };
    assert.equal(asRefusal(await t.intents()[RESOLVE].resolve({ account: t.account(), chain: "solana:devnet", transactions: [TX] }).catch((e) => e))?.code, code);
  }
});

test("errors without Airlock's data: MWA's own codes by name, and a broken session is not a refusal", () => {
  assert.equal((mwaError(rpcError(-3, "not signed")) as { code: string }).code, "rejected");
  assert.equal((mwaError(rpcError(-1, "auth")) as { code: string }).code, "rejected");
  assert.equal((mwaError(rpcError(-32601, "Method not found")) as { code: string }).code, "method_not_found");
  assert.equal((mwaError(rpcError(12, "odd")) as { code: string }).code, "mwa_12");
  const closed = mwaError(sessionClosed());
  assert.equal(asRefusal(closed), null);
  assert.match(closed.message, /session with Airlock closed/);
  const missing = mwaError(Object.assign(new Error("x"), { code: "ERROR_WALLET_NOT_FOUND" }));
  assert.equal(asRefusal(missing), null);
  assert.match(missing.message, /Is Airlock installed/);
  assert.match(mwaError(Object.assign(new Error("x"), { code: "ERROR_SECURE_CONTEXT_REQUIRED" })).message, /localhost/);
});

test("a token Airlock no longer takes is dropped, and the page shows no accounts", async () => {
  const t = setup();
  await connected(t);
  t.fake.tokens.clear();
  const err = await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() }).catch((e) => e);
  assert.equal(asRefusal(err)?.code, "rejected");
  assert.match(err.message, /Press Connect again/);
  assert.equal(t.saved(), null);
  assert.deepEqual(t.wallet.accounts, []);
  assert.equal(t.fake.callsOf("airlock_get_signing_profile").length, 0);
});

test("after a sync resolve the session stays open, asks until it ends, and every answer reaches onResolution and the store", async () => {
  const t = setup();
  await connected(t);
  const store = new ResolutionStore();
  t.intents()[RESOLVE].onResolution((r) => {
    t.heard.push(r);
    store.update(r);
  });
  t.fake.on.airlock_resolve = (params) => {
    assert.equal(params.account, ADDRESS);
    assert.deepEqual(params.transactions, [base64(TX)]);
    return { resolution: resolution(params.id as string, "sending") };
  };
  states(t.fake, "memo-1", ["sending", "sending", "done"]);
  store.start("memo-1", "Send a memo", ADDRESS);
  const first = await t.intents()[RESOLVE].resolve({ id: "memo-1", account: t.account(), chain: "solana:devnet", transactions: [TX] });
  assert.equal(first.state, "sending");
  store.update(first);
  const ended = await store.waitFor("memo-1", []);
  assert.equal(ended.state, "done");
  assert.deepEqual(t.heard.map((r) => r.state), ["sending", "sending", "done"]);
  await until(() => t.fake.open.size === 0, "the session to end once it was done");
  assert.equal(t.fake.sessions, 2);
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_resolve", "airlock_get_resolution", "airlock_get_resolution", "airlock_get_resolution"]);
});

test("asking stops when the window is over, with the resolution still open, and the session ends", async () => {
  const t = setup({ timing: { pollWindowMs: 40 } });
  await connected(t);
  t.fake.on.airlock_resolve = (params) => ({ resolution: resolution(params.id as string, "sending") });
  states(t.fake, "slow", ["sending"]);
  await t.intents()[RESOLVE].resolve({ id: "slow", account: t.account(), chain: "solana:devnet", transactions: [TX] });
  await until(() => t.fake.open.size === 0, "the session to end after the window");
  assert.ok(t.fake.callsOf("airlock_get_resolution").length >= 1);
  assert.ok(t.logs.some((line) => line.includes("Press Check")), "says to press Check");
  const asked = t.fake.callsOf("airlock_get_resolution").length;
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(t.fake.callsOf("airlock_get_resolution").length, asked, "no more asking");
});

test("a not_found while asking stops asking for it and tells the page", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_resolve = (params) => ({ resolution: resolution(params.id as string, "sending") });
  t.fake.on.airlock_get_resolution = () => {
    throw refusal("not_found", "No resolution of this site has this id.");
  };
  await t.intents()[RESOLVE].resolve({ id: "gone", account: t.account(), chain: "solana:devnet", transactions: [TX] });
  await until(() => t.fake.open.size === 0, "the session to end");
  assert.deepEqual(t.notFound, ["gone"]);
  assert.equal(t.fake.callsOf("airlock_get_resolution").length, 1);
});

test("a session that drops while asking is ended, and the next call opens a new one", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_resolve = (params) => ({ resolution: resolution(params.id as string, "sending") });
  t.fake.on.airlock_get_resolution = () => {
    throw sessionClosed();
  };
  await t.intents()[RESOLVE].resolve({ id: "dropped", account: t.account(), chain: "solana:devnet", transactions: [TX] });
  await until(() => t.fake.open.size === 0 && t.transport.openSession === null, "the session to end");
  assert.equal(t.fake.callsOf("airlock_get_resolution").length, 1);
  t.fake.on.airlock_get_signing_profile = (params) => ({ profile: { account: params.account } });
  await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() });
  assert.deepEqual(t.fake.methods(3), ["authorize", "airlock_get_signing_profile"]);
});

test("a call Airlock never answers ends the session after the quick timeout", async () => {
  const t = setup({ timing: { quickMs: 30 } });
  await connected(t);
  t.fake.on.airlock_get_resolution = () => new Promise(() => {});
  const err = await t.intents()[GET_RESOLUTION].getResolution({ id: "stuck" }).catch((e) => e);
  assert.match(err.message, /did not answer/);
  assert.equal(asRefusal(err), null);
  await until(() => t.fake.open.size === 0, "the session to end");
});

test("for an async account the session ends at once, and Check asks in a new session", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_resolve = (params) => ({ resolution: resolution(params.id as string, "awaiting-signatures", { timing: "async" }) });
  const r = await t.intents()[RESOLVE].resolve({ id: "multisig", account: t.account(), chain: "solana:devnet", transactions: [TX] });
  assert.equal(r.state, "awaiting-signatures");
  await until(() => t.fake.open.size === 0, "the session to end");
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_resolve"]);
  // Check: a new session, a reauthorize, one read.
  states(t.fake, "multisig", ["done"], { timing: "async" });
  const checked = await t.intents()[GET_RESOLUTION].getResolution({ id: "multisig" });
  assert.equal(checked.state, "done");
  assert.deepEqual(t.fake.methods(3), ["authorize", "airlock_get_resolution"]);
  await until(() => t.fake.open.size === 0, "Check's session to end");
  // A Check that finds it still open does not keep the session for it.
  states(t.fake, "multisig", ["awaiting-signatures"], { timing: "async" });
  await t.intents()[GET_RESOLUTION].getResolution({ id: "multisig" });
  await until(() => t.fake.open.size === 0, "the second Check's session to end");
  assert.deepEqual(t.fake.methods(4), ["authorize", "airlock_get_resolution"]);
});

/** cosign: awaiting-app until the page's signature is in, then sending, then done. */
function cosignWallet(fake: FakeMwa, id: string): { provided: () => boolean } {
  let provided = false;
  fake.on.airlock_resolve = () => ({ resolution: resolution(id, "awaiting-app", { delivery: "cosign" }) });
  fake.on.airlock_get_resolution = () => ({ resolution: resolution(id, provided ? "done" : "awaiting-app", { delivery: "cosign" }) });
  fake.on.airlock_provide_signatures = (params) => {
    const [item] = params.signatures as { transaction: number; signer: string; signature: string }[];
    assert.equal(item.signer, ADDRESS);
    assert.equal(decodeBase58(item.signature)?.length, 64);
    provided = true;
    return { resolution: resolution(id, "sending", { delivery: "cosign" }) };
  };
  return { provided: () => provided };
}

test("cosign: the page's signatures go in the session still open from resolve", async () => {
  const t = setup();
  await connected(t);
  const wallet = cosignWallet(t.fake, "sponsor");
  const r = await t.intents()[RESOLVE].resolve({ id: "sponsor", account: t.account(), chain: "solana:devnet", transactions: [TX], delivery: "cosign" });
  assert.equal(r.state, "awaiting-app");
  const after = await t.intents()[PROVIDE_SIGNATURES].provideSignatures({ id: "sponsor", signatures: [{ transaction: 0, signer: ADDRESS, signature: new Uint8Array(64).fill(3) }] });
  assert.equal(after.state, "sending");
  assert.ok(wallet.provided());
  await until(() => t.fake.open.size === 0, "the session to end once done");
  assert.equal(t.fake.sessions, 2);
  assert.ok(t.fake.methods(2).includes("airlock_provide_signatures"));
  assert.equal(t.fake.methods(2).at(-1), "airlock_get_resolution");
});

test("cosign: once that session has ended, the signatures open a new one", async () => {
  const t = setup({ timing: { pollWindowMs: 20 } });
  await connected(t);
  cosignWallet(t.fake, "late");
  await t.intents()[RESOLVE].resolve({ id: "late", account: t.account(), chain: "solana:devnet", transactions: [TX], delivery: "cosign" });
  await until(() => t.fake.open.size === 0, "resolve's session to end");
  const after = await t.intents()[PROVIDE_SIGNATURES].provideSignatures({ id: "late", signatures: [{ transaction: 0, signer: ADDRESS, signature: new Uint8Array(64).fill(3) }] });
  assert.equal(after.state, "sending");
  assert.deepEqual(t.fake.methods(3).slice(0, 2), ["authorize", "airlock_provide_signatures"]);
  await until(() => t.fake.open.size === 0, "the new session to end");
  assert.equal(t.fake.methods(3).at(-1), "airlock_get_resolution");
});

test("cancel goes as airlock_cancel_resolution", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_cancel_resolution = (params) => ({ resolution: resolution(params.id as string, "cancelled") });
  const r = await t.intents()[CANCEL_RESOLUTION].cancelResolution({ id: "x" });
  assert.equal(r.state, "cancelled");
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_cancel_resolution"]);
});

test("without a tap no new session opens, but a call joins the one open", async () => {
  let tap = true;
  const t = setup({ canOpen: () => tap });
  await t.connect();
  tap = false;
  t.fake.on.airlock_get_signing_profile = (params) => ({ profile: { account: params.account } });
  // Connect's session is still open: the profile read joins it.
  await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() });
  await until(() => t.fake.open.size === 0, "Connect's session to end");
  const err = await t.intents()[GET_SIGNING_PROFILE].getSigningProfile({ account: t.account() }).catch((e) => e);
  assert.equal(err.message, GESTURE_NEEDED);
  assert.equal(asRefusal(err), null);
  assert.equal(t.fake.sessions, 1);
});

test("a message: a signature comes back as bytes, and a memo is followed in the session", async () => {
  const t = setup();
  await connected(t);
  const message = new TextEncoder().encode("hello");
  const signature = new Uint8Array(64).fill(5);
  t.fake.on.airlock_sign_message = (params) => {
    assert.equal(params.account, ADDRESS);
    assert.equal(params.message, base64(message));
    assert.equal(params.chain, "solana:devnet");
    return params.mode === "signature"
      ? { kind: "signature", signedMessage: params.message, signature: encodeBase58(signature) }
      : { kind: "memo", resolution: resolution(params.id as string, "sending") };
  };
  const signed = await t.intents()[SIGN_MESSAGE].signMessage({ account: t.account(), message, mode: "signature", chain: "solana:devnet" });
  assert.equal(signed.kind, "signature");
  if (signed.kind === "signature") {
    assert.deepEqual([...signed.signature], [...signature]);
    assert.deepEqual([...signed.signedMessage], [...message]);
  }
  await until(() => t.fake.open.size === 0, "the signature's session to end");
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_sign_message"]);
  states(t.fake, "memo-message", ["done"]);
  t.intents()[RESOLVE].onResolution((r) => t.heard.push(r));
  const memo = await t.intents()[SIGN_MESSAGE].signMessage({ id: "memo-message", account: t.account(), message, mode: "memo", chain: "solana:devnet" });
  assert.equal(memo.kind, "memo");
  await until(() => t.fake.open.size === 0, "the memo's session to end");
  assert.deepEqual(t.fake.methods(3), ["authorize", "airlock_sign_message", "airlock_get_resolution"]);
  assert.deepEqual(t.heard.map((r) => r.state), ["done"]);
});

test("the fallback: the same bytes through sign_and_send_transactions", async () => {
  const t = setup();
  await connected(t);
  const signature = new Uint8Array(64).fill(9);
  t.fake.on.sign_and_send_transactions = (params) => {
    assert.deepEqual(params.payloads, [base64(TX)]);
    return { signatures: [base64(signature)] };
  };
  const feature = t.wallet.features["solana:signAndSendTransaction"] as {
    signAndSendTransaction(input: { account: WalletAccount; transaction: Uint8Array; chain: string }): Promise<{ signature: Uint8Array }[]>;
  };
  const [output] = await feature.signAndSendTransaction({ account: t.account(), transaction: TX, chain: "solana:devnet" });
  assert.deepEqual([...output.signature], [...signature]);
  assert.deepEqual(t.fake.methods(2), ["authorize", "sign_and_send_transactions"]);
  // Declined there is MWA's own NOT_SIGNED, read as rejected.
  t.fake.on.sign_and_send_transactions = () => {
    throw rpcError(-3, "transaction not signed");
  };
  const err = await feature.signAndSendTransaction({ account: t.account(), transaction: TX, chain: "solana:devnet" }).catch((e) => e);
  assert.equal(asRefusal(err)?.code, "rejected");
});

test("batch: several reads share one session", async () => {
  const t = setup();
  await connected(t);
  t.fake.on.airlock_get_resolution = (params) => ({ resolution: resolution(params.id as string, "awaiting-signatures", { timing: "async" }) });
  await t.transport.batch(async () => {
    for (const id of ["a", "b", "c"]) await t.intents()[GET_RESOLUTION].getResolution({ id });
  });
  await until(() => t.fake.open.size === 0, "the batch's session to end");
  assert.equal(t.fake.sessions, 2);
  assert.deepEqual(t.fake.methods(2), ["authorize", "airlock_get_resolution", "airlock_get_resolution", "airlock_get_resolution"]);
});

test("build.mjs puts a JSON-RPC error's data back into the installed MWA library", () => {
  const require = createRequire(import.meta.url);
  // The package exports lib/esm/index.js to Node; the browser build sits beside it.
  const file = join(dirname(require.resolve("@solana-mobile/mobile-wallet-adapter-protocol")), "index.browser.js");
  assert.match(file.replaceAll("\\", "/"), patch.MWA_LIBRARY);
  const source = readFileSync(file, "utf8");
  assert.ok(!source.includes("jsonRpcMessage.error.data"), "the library as published drops data; if it keeps it now, drop mwa-patch.mjs");
  const patched = patch.keepErrorData(source);
  assert.ok(patched.includes("jsonRpcMessage.error.message, jsonRpcMessage.error.data)"));
  assert.throws(() => patch.keepErrorData("nothing here"), /found it 0 times/);
});
