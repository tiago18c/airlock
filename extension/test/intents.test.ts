import assert from "node:assert/strict";
import { test } from "node:test";
import type { Resolution } from "../src/intents";
import { BridgeError, bytesToBase64, decodeBase58, encodeBase58 } from "../src/protocol";
import { AirlockWallet, POLL_MS } from "../src/wallet";

// The intent features of the page wallet, on an app made of replies.

const KEY = bytesToBase64(new Uint8Array(32).fill(7));
const ADDRESS = encodeBase58(new Uint8Array(32).fill(7));

/** Promises settle, with any mocked timers left alone. */
const settle = () => new Promise((resolve) => setImmediate(resolve));

function resolution(state: Resolution["state"], updatedAt = 1, id = "r1"): Resolution {
  return {
    id,
    account: ADDRESS,
    chain: "solana:devnet",
    timing: "sync",
    delivery: "send",
    state,
    replaced: {},
    transactions: [],
    ran: [],
    updatedAt,
  };
}

type Reply = (method: string, params: any) => unknown;

async function connected(reply: Reply) {
  const calls: { method: string; params: any }[] = [];
  const wallet = new AirlockWallet(async (method, params) => {
    calls.push({ method, params });
    if (method === "connect") return { accounts: [{ publicKey: KEY }] };
    return reply(method, params);
  });
  await (wallet.features["standard:connect"] as { connect: () => Promise<unknown> }).connect();
  return { wallet, calls, account: wallet.accounts[0] };
}

const feature = (wallet: AirlockWallet, name: string) => wallet.features[name] as any;
const push = (wallet: AirlockWallet, value: Resolution) =>
  wallet.receivePush({ v: 1, method: "resolutionChanged", origin: "https://a.test", params: { resolution: value } });

test("a resolve goes to the app with its bytes as base64 and its notes as JSON", async () => {
  const { wallet, calls, account } = await connected(() => ({ resolution: resolution("sending") }));
  const settled = await feature(wallet, "airlock:resolve").resolve({
    id: "mine",
    account,
    chain: "solana:devnet",
    transactions: [new Uint8Array([0x81, 1, 2])],
    delivery: "send",
    accounts: {
      Mint: { kind: "new", replaceable: true },
      Ata: { kind: "derived", pda: { program: "P", seeds: [{ account: "Mint" }, { bytes: new Uint8Array([1, 2, 3]) }] } },
    },
    data: [{ transaction: 0, instruction: 1, offset: 4, account: "Mint" }],
  });
  assert.equal(settled.state, "sending");
  const sent = calls.at(-1)!;
  assert.equal(sent.method, "resolve");
  assert.deepEqual(sent.params, {
    id: "mine",
    account: ADDRESS,
    chain: "solana:devnet",
    transactions: ["gQEC"],
    delivery: "send",
    accounts: {
      Mint: { kind: "new", replaceable: true },
      Ata: { kind: "derived", pda: { program: "P", seeds: [{ account: "Mint" }, { bytes: "AQID" }] } },
    },
    data: [{ transaction: 0, instruction: 1, offset: 4, account: "Mint" }],
  });
  // What the page left out is not sent at all.
  assert.ok(!("order" in sent.params));
});

test("a resolve for an account that is not connected never reaches the app", async () => {
  const { wallet, calls, account } = await connected(() => ({ resolution: resolution("sending") }));
  const stranger = { ...account, address: "11111111111111111111111111111111" };
  const resolve = feature(wallet, "airlock:resolve").resolve;
  await assert.rejects(resolve({ account: stranger, chain: "solana:devnet", transactions: [new Uint8Array([1])] }), /not connected/);
  await assert.rejects(resolve({ account, chain: "solana:devnet", transactions: [] }), /at least one/);
  await assert.rejects(
    resolve({ account, chain: "solana:devnet", transactions: Array.from({ length: 17 }, () => new Uint8Array([1])) }),
    /at most 16/,
  );
  assert.equal(calls.filter((call) => call.method === "resolve").length, 0);
});

test("a resolve settles on the push that says the person approved", async () => {
  const { wallet, account } = await connected(() => ({ resolution: resolution("awaiting-approval") }));
  let done: Resolution | undefined;
  const pending = feature(wallet, "airlock:resolve")
    .resolve({ account, chain: "solana:devnet", transactions: [new Uint8Array([1])] })
    .then((value: Resolution) => (done = value));
  await settle();
  assert.equal(done, undefined);

  // Another resolution's change settles nothing.
  push(wallet, resolution("sending", 2, "other"));
  await settle();
  assert.equal(done, undefined);

  push(wallet, resolution("awaiting-app", 3));
  await pending;
  assert.equal(done!.state, "awaiting-app");
});

test("a push heard before the reply still settles the resolve", async () => {
  let wallet!: AirlockWallet;
  ({ wallet } = await connected(() => {
    push(wallet, resolution("sending", 5));
    return { resolution: resolution("awaiting-approval", 1) };
  }));
  const account = wallet.accounts[0];
  const settled = await feature(wallet, "airlock:resolve").resolve({
    account,
    chain: "solana:devnet",
    transactions: [new Uint8Array([1])],
  });
  assert.equal(settled.state, "sending");
});

test("a resolve whose push was missed settles on a poll, one read every turn", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let state: Resolution["state"] = "awaiting-approval";
  const { wallet, calls, account } = await connected((method) => {
    if (method === "getResolution") return { resolution: resolution(state, 2) };
    return { resolution: resolution("awaiting-approval", 1) };
  });
  let done: Resolution | undefined;
  const pending = feature(wallet, "airlock:resolve")
    .resolve({ account, chain: "solana:devnet", transactions: [new Uint8Array([1])] })
    .then((value: Resolution) => (done = value));
  await settle();
  const polls = () => calls.filter((call) => call.method === "getResolution").length;
  assert.equal(polls(), 0);

  t.mock.timers.tick(POLL_MS);
  await settle();
  assert.equal(polls(), 1);
  assert.equal(calls.at(-1)!.params.id, "r1");
  assert.equal(done, undefined);

  state = "sending";
  t.mock.timers.tick(POLL_MS);
  await pending;
  assert.equal(done!.state, "sending");

  // Nothing waits any more, so nothing is read again.
  t.mock.timers.tick(POLL_MS * 5);
  await settle();
  assert.equal(polls(), 2);
});

test("a poll the extension refuses is tried again, and one the app has forgotten ends the wait", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let answer: () => unknown = () => {
    throw new BridgeError("too_many_requests", "slow down");
  };
  const { wallet, account } = await connected((method) =>
    method === "getResolution" ? answer() : { resolution: resolution("awaiting-approval") },
  );
  let error: BridgeError | undefined;
  const pending = feature(wallet, "airlock:resolve")
    .resolve({ account, chain: "solana:devnet", transactions: [new Uint8Array([1])] })
    .catch((err: BridgeError) => (error = err));
  t.mock.timers.tick(POLL_MS);
  await settle();
  assert.equal(error, undefined);

  answer = () => {
    throw new BridgeError("not_found", "No such resolution.");
  };
  t.mock.timers.tick(POLL_MS);
  await pending;
  assert.equal(error!.code, "not_found");
});

test("a declined, expired or cancelled resolution rejects the resolve with its code", async () => {
  for (const [state, code] of [
    ["declined", "rejected"],
    ["expired", "expired"],
    ["cancelled", "cancelled"],
  ] as const) {
    const { wallet, account } = await connected(() => ({ resolution: resolution("awaiting-approval") }));
    const pending = feature(wallet, "airlock:resolve").resolve({
      account,
      chain: "solana:devnet",
      transactions: [new Uint8Array([1])],
    });
    push(wallet, resolution(state, 2));
    await assert.rejects(pending, (err: BridgeError & { resolution?: Resolution }) => {
      assert.equal(err.code, code);
      assert.equal(err.resolution?.state, state);
      return true;
    });
  }
  // A rejection reads as one to wallet adapters.
  const { wallet, account } = await connected(() => ({ resolution: resolution("declined") }));
  await assert.rejects(
    feature(wallet, "airlock:resolve").resolve({ account, chain: "solana:devnet", transactions: [new Uint8Array([1])] }),
    { name: "WalletUserRejected" },
  );
});

test("a cancel settles a resolve still waiting on the person", async () => {
  const { wallet, account } = await connected((method) =>
    method === "cancelResolution" ? { resolution: resolution("cancelled", 2) } : { resolution: resolution("awaiting-approval") },
  );
  const pending = feature(wallet, "airlock:resolve").resolve({
    account,
    chain: "solana:devnet",
    transactions: [new Uint8Array([1])],
  });
  const cancelled = await feature(wallet, "airlock:cancelResolution").cancelResolution({ id: "r1" });
  assert.equal(cancelled.state, "cancelled");
  await assert.rejects(pending, { code: "cancelled" });
});

test("an unsupported refusal keeps what it names", async () => {
  const { wallet, account } = await connected(() => {
    throw new BridgeError("unsupported", "This account cannot cosign.", "delivery");
  });
  await assert.rejects(
    feature(wallet, "airlock:resolve").resolve({ account, chain: "solana:devnet", transactions: [new Uint8Array([1])] }),
    (err: BridgeError) => err.code === "unsupported" && err.requirement === "delivery",
  );
  const body = BridgeError.from({ code: "unsupported", message: "no", requirement: "order" });
  assert.equal(body.requirement, "order");
  assert.equal(BridgeError.from({ code: "rejected", message: "no" }).requirement, undefined);
});

test("every push for the page is heard until the listener leaves", async () => {
  const { wallet } = await connected(() => ({}));
  const heard: string[] = [];
  const off = feature(wallet, "airlock:resolve").onResolution((value: Resolution) => heard.push(value.state));
  feature(wallet, "airlock:resolve").onResolution(() => {
    throw new Error("a page bug");
  });
  push(wallet, resolution("sending", 1));
  push(wallet, resolution("done", 2));
  off();
  push(wallet, resolution("done", 3));
  assert.deepEqual(heard, ["sending", "done"]);

  // A push that is not a resolution is ignored, and accounts still change.
  wallet.receivePush({ method: "resolutionChanged", params: { resolution: { state: "done" } } });
  wallet.receivePush({ method: "accountsChanged", params: { accounts: [] } });
  assert.equal(wallet.accounts.length, 0);
});

test("the app's signatures go as base58, and the resolution comes back as it is", async () => {
  const back = { ...resolution("sending", 4), extra: { kept: true } };
  const { wallet, calls } = await connected(() => ({ resolution: back }));
  const signature = new Uint8Array(64).fill(9);
  const result = await feature(wallet, "airlock:provideSignatures").provideSignatures({
    id: "r1",
    signatures: [{ transaction: 0, signer: ADDRESS, signature }],
  });
  assert.deepEqual(result, back);
  const sent = calls.at(-1)!;
  assert.equal(sent.method, "provideSignatures");
  assert.deepEqual(sent.params, {
    id: "r1",
    signatures: [{ transaction: 0, signer: ADDRESS, signature: encodeBase58(signature) }],
  });
  assert.deepEqual(decodeBase58(sent.params.signatures[0].signature), signature);
});

test("a profile and a resolution are read for the page", async () => {
  const profile = { account: ADDRESS, timing: "sync", delivery: ["send"], messages: ["memo"] };
  const { wallet, calls, account } = await connected((method) =>
    method === "getSigningProfile" ? { profile } : { resolution: resolution("done", 9) },
  );
  assert.deepEqual(await feature(wallet, "airlock:getSigningProfile").getSigningProfile({ account }), profile);
  assert.deepEqual(calls.at(-1)!.params, { account: ADDRESS });
  const read = await feature(wallet, "airlock:getResolution").getResolution({ id: "r1" });
  assert.equal(read.state, "done");
  assert.deepEqual(calls.at(-1), { method: "getResolution", params: { id: "r1" } });
});

test("a message signed as a memo settles as a resolve does", async () => {
  const { wallet, calls, account } = await connected(() => ({
    kind: "memo",
    resolution: { ...resolution("awaiting-approval"), message: { memo: "hello" } },
  }));
  const pending = feature(wallet, "airlock:signMessage").signMessage({
    account,
    message: new TextEncoder().encode("hello"),
    mode: "memo",
    chain: "solana:devnet",
  });
  await settle();
  // The chain a memo is for is passed on, as in any request.
  assert.deepEqual(calls.at(-1), {
    method: "resolveMessage",
    params: { account: ADDRESS, message: "aGVsbG8=", mode: "memo", chain: "solana:devnet" },
  });
  push(wallet, { ...resolution("sending", 2), message: { memo: "hello" } });
  const output = await pending;
  assert.equal(output.kind, "memo");
  assert.equal(output.resolution.state, "sending");
  assert.equal(output.resolution.message.memo, "hello");
});

test("a message signed by a key comes back as bytes", async () => {
  const signature = new Uint8Array(64).fill(5);
  const { wallet, calls, account } = await connected(() => ({
    kind: "signature",
    signedMessage: "aGk=",
    signature: encodeBase58(signature),
  }));
  const output = await feature(wallet, "airlock:signMessage").signMessage({
    account,
    message: new TextEncoder().encode("hi"),
  });
  assert.equal(output.kind, "signature");
  assert.deepEqual(output.signature, signature);
  assert.equal(new TextDecoder().decode(output.signedMessage), "hi");
  // The wallet chooses the mode when the page does not.
  assert.ok(!("mode" in calls.at(-1)!.params));

  const { wallet: bad, account: other } = await connected(() => ({ kind: "signature", signedMessage: "aGk=", signature: "abc" }));
  await assert.rejects(
    feature(bad, "airlock:signMessage").signMessage({ account: other, message: new Uint8Array([1]) }),
    /not 64 bytes/,
  );
});
