import assert from "node:assert/strict";
import { test } from "node:test";
import { BridgeError, bytesToBase64 } from "../src/protocol";
import { AirlockWallet } from "../src/wallet";

const KEY = bytesToBase64(new Uint8Array(32).fill(7));
const SENDING = ["solana:signTransaction", "solana:signAndSendTransaction", "solana:signAndSendAllTransactions"];

function walletWith(reply: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown }[] = [];
  const wallet = new AirlockWallet(async (method, params) => {
    calls.push({ method, params });
    return reply(method, params);
  });
  return { wallet, calls };
}

const connect = (wallet: AirlockWallet) =>
  (wallet.features["standard:connect"] as { connect: () => Promise<{ accounts: readonly { address: string }[] }> }).connect();
const sign = (wallet: AirlockWallet, ...inputs: unknown[]) =>
  (wallet.features["solana:signTransaction"] as { signTransaction: (...i: unknown[]) => Promise<{ signedTransaction: Uint8Array }[]> }).signTransaction(...inputs);

test("the wallet offers only what the desktop app can do", () => {
  const { wallet } = walletWith(() => ({}));
  assert.deepEqual(Object.keys(wallet.features).sort(), [
    "solana:signAndSendAllTransactions",
    "solana:signAndSendTransaction",
    "solana:signIn",
    "solana:signMessage",
    "solana:signTransaction",
    "standard:connect",
    "standard:disconnect",
    "standard:events",
  ]);
  const feature = wallet.features["solana:signTransaction"] as { supportedTransactionVersions: readonly unknown[] };
  assert.deepEqual([...feature.supportedTransactionVersions], ["legacy", 0, 1]);
  for (const name of ["solana:signAndSendTransaction", "solana:signAndSendAllTransactions"]) {
    const sends = wallet.features[name] as { supportedTransactionVersions: readonly unknown[] };
    assert.deepEqual([...sends.supportedTransactionVersions], ["legacy", 0, 1]);
  }
});

test("connect exposes the accounts the desktop app returned", async () => {
  const { wallet } = walletWith(() => ({ accounts: [{ publicKey: KEY, label: "Treasury" }] }));
  const { accounts } = await connect(wallet);
  assert.equal(accounts.length, 1);
  assert.equal(wallet.accounts[0].label, "Treasury");
  assert.deepEqual([...wallet.accounts[0].features], SENDING);
});

test("a silent connect that cannot reach the app finds nothing, and a normal one fails", async () => {
  const wallet = new AirlockWallet(async () => {
    throw new Error("app_not_running");
  });
  const features = wallet.features["standard:connect"] as {
    connect: (input?: { silent?: boolean }) => Promise<{ accounts: readonly unknown[] }>;
  };
  assert.deepEqual((await features.connect({ silent: true })).accounts, []);
  await assert.rejects(features.connect(), /app_not_running/);
});

const signMessage = (wallet: AirlockWallet, ...inputs: unknown[]) =>
  (wallet.features["solana:signMessage"] as { signMessage: (...i: unknown[]) => Promise<{ signedMessage: Uint8Array; signature: Uint8Array }[]> }).signMessage(...inputs);
const signIn = (wallet: AirlockWallet, ...inputs: unknown[]) =>
  (wallet.features["solana:signIn"] as { signIn: (...i: unknown[]) => Promise<{ account: { address: string }; signedMessage: Uint8Array; signature: Uint8Array }[]> }).signIn(...inputs);

test("only an account with a key here can sign messages", async () => {
  const OTHER = bytesToBase64(new Uint8Array(32).fill(8));
  const { wallet } = walletWith(() => ({
    accounts: [{ publicKey: KEY, signsMessages: true }, { publicKey: OTHER }],
  }));
  await connect(wallet);
  assert.deepEqual([...wallet.accounts[0].features], [...SENDING, "solana:signMessage"]);
  assert.deepEqual([...wallet.accounts[1].features], SENDING);
});

test("a message is signed for a connected account, one input at a time", async () => {
  const { wallet, calls } = walletWith((method) =>
    method === "connect"
      ? { accounts: [{ publicKey: KEY, signsMessages: true }] }
      : { signedMessage: "aGk=", signature: bytesToBase64(new Uint8Array(64).fill(1)) },
  );
  await connect(wallet);
  const account = wallet.accounts[0];
  const message = new TextEncoder().encode("hi");
  const outputs = await signMessage(wallet, { account, message }, { account, message });
  assert.equal(outputs.length, 2);
  assert.equal(new TextDecoder().decode(outputs[0].signedMessage), "hi");
  assert.equal(outputs[0].signature.length, 64);
  const sent = calls.filter((call) => call.method === "signMessage");
  assert.equal(sent.length, 2);
  assert.deepEqual(sent[0].params, { account: account.address, message: "aGk=" });

  const { wallet: stranger } = walletWith(() => ({}));
  await assert.rejects(signMessage(stranger, { account, message }), /not connected/);
});

test("signing in connects the account it signed with", async () => {
  const { wallet, calls } = walletWith(() => ({
    account: KEY,
    accounts: [{ publicKey: KEY, signsMessages: true }],
    signedMessage: "aGk=",
    signature: bytesToBase64(new Uint8Array(64).fill(1)),
  }));
  const [output] = await signIn(wallet, { domain: "a.test", nonce: "n" });
  assert.equal(wallet.accounts.length, 1);
  assert.equal(output.account.address, wallet.accounts[0].address);
  assert.deepEqual(calls[0].params, { domain: "a.test", nonce: "n" });

  await signIn(wallet);
  assert.equal(calls.length, 2);
});

test("a sign-in reply naming another account is refused", async () => {
  const { wallet } = walletWith(() => ({
    account: bytesToBase64(new Uint8Array(32).fill(9)),
    accounts: [{ publicKey: KEY }],
    signedMessage: "aGk=",
    signature: "aGk=",
  }));
  await assert.rejects(signIn(wallet), /did not return/);
});

const send = (wallet: AirlockWallet, ...inputs: unknown[]) =>
  (wallet.features["solana:signAndSendTransaction"] as { signAndSendTransaction: (...i: unknown[]) => Promise<{ signature: Uint8Array }[]> }).signAndSendTransaction(...inputs);
const sendAll = (wallet: AirlockWallet, inputs: unknown[], options?: unknown) =>
  (wallet.features["solana:signAndSendAllTransactions"] as { signAndSendAllTransactions: (i: unknown[], o?: unknown) => Promise<PromiseSettledResult<{ signature: Uint8Array }>[]> }).signAndSendAllTransactions(inputs, options);
const SIGNATURE = bytesToBase64(new Uint8Array(64).fill(3));

test("sending returns the signature, for a connected account only", async () => {
  const { wallet, calls } = walletWith((method) =>
    method === "connect" ? { accounts: [{ publicKey: KEY }] } : { signatures: [SIGNATURE] },
  );
  await connect(wallet);
  const account = wallet.accounts[0];
  const input = { account, transaction: new Uint8Array([0x81]), chain: "solana:devnet", options: { commitment: "confirmed" } };
  const [output] = await send(wallet, input);
  assert.equal(output.signature.length, 64);
  const sent = calls.at(-1)!;
  assert.equal(sent.method, "signAndSendTransaction");
  const item = (sent.params as { transactions: { chain: string; options: unknown }[] }).transactions[0];
  assert.equal(item.chain, "solana:devnet");
  assert.deepEqual(item.options, { commitment: "confirmed" });

  const { wallet: stranger } = walletWith(() => ({}));
  await assert.rejects(send(stranger, input), /not connected/);
});

test("a reply with the wrong number of signatures is refused", async () => {
  const { wallet } = walletWith((method) =>
    method === "connect" ? { accounts: [{ publicKey: KEY }] } : { signatures: [] },
  );
  await connect(wallet);
  const input = { account: wallet.accounts[0], transaction: new Uint8Array([0x81]), chain: "solana:devnet" };
  await assert.rejects(send(wallet, input), /wrong number/);
});

test("sending all settles each transaction on its own, in the mode asked", async () => {
  const { wallet, calls } = walletWith((method) =>
    method === "connect"
      ? { accounts: [{ publicKey: KEY }] }
      : { results: [{ signature: SIGNATURE }, { error: { code: "send_failed", message: "no" } }] },
  );
  await connect(wallet);
  const input = { account: wallet.accounts[0], transaction: new Uint8Array([0x81]), chain: "solana:devnet" };
  const [first, second] = await sendAll(wallet, [input, input], { mode: "serial" });
  assert.equal(first.status, "fulfilled");
  assert.equal(second.status, "rejected");
  assert.equal((second as PromiseRejectedResult).reason.message, "no");
  assert.equal((calls.at(-1)!.params as { mode: string }).mode, "serial");
  assert.deepEqual(await sendAll(wallet, []), []);
  await assert.rejects(sendAll(wallet, Array.from({ length: 17 }, () => input)), /at most 16/);
});

test("a send that may still land is not reported as a plain failure", async () => {
  const { wallet } = walletWith((method) => {
    if (method === "connect") return { accounts: [{ publicKey: KEY }] };
    throw new BridgeError("send_unknown", "It may still land.");
  });
  const { accounts } = await connect(wallet);
  const send = wallet.features["solana:signAndSendTransaction"] as {
    signAndSendTransaction: (...inputs: unknown[]) => Promise<unknown>;
  };
  await assert.rejects(
    send.signAndSendTransaction({ account: accounts[0], transaction: new Uint8Array([1]), chain: "solana:devnet" }),
    (err: BridgeError) => err.code === "send_unknown",
  );
});

test("a key that is not 32 bytes is refused", async () => {
  const { wallet } = walletWith(() => ({ accounts: [{ publicKey: bytesToBase64(new Uint8Array(31)) }] }));
  await assert.rejects(connect(wallet), /32 bytes/);
});

test("a transaction is signed for a connected account only", async () => {
  const signed = bytesToBase64(new Uint8Array([0x81, 9, 9]));
  const { wallet, calls } = walletWith((method) =>
    method === "connect" ? { accounts: [{ publicKey: KEY }] } : { signedTransactions: [signed] },
  );
  await connect(wallet);
  const account = wallet.accounts[0];
  const input = { account, transaction: new Uint8Array([0x81, 1, 2]), chain: "solana:devnet" };

  const [output] = await sign(wallet, input);
  assert.deepEqual([...output.signedTransaction], [0x81, 9, 9]);
  const sent = calls.at(-1)!.params as { transactions: { transaction: string; account: string }[] };
  assert.equal(sent.transactions[0].transaction, "gQEC");
  assert.equal(sent.transactions[0].account, account.address);

  const stranger = { ...account, address: "11111111111111111111111111111111" };
  await assert.rejects(sign(wallet, { ...input, account: stranger }), /not connected/);
});

test("several transactions go in one request and come back in order", async () => {
  const { wallet, calls } = walletWith((method, params) => {
    if (method === "connect") return { accounts: [{ publicKey: KEY }] };
    const sent = (params as { transactions: { transaction: string }[] }).transactions;
    return { signedTransactions: sent.map((item) => item.transaction) };
  });
  await connect(wallet);
  const account = wallet.accounts[0];
  const inputs = [1, 2, 3].map((n) => ({ account, transaction: new Uint8Array([0x81, n]), chain: "solana:devnet" }));

  const outputs = await sign(wallet, ...inputs);
  assert.deepEqual(outputs.map((output) => output.signedTransaction[1]), [1, 2, 3]);
  const signCalls = calls.filter((call) => call.method === "signTransaction");
  assert.equal(signCalls.length, 1);
  assert.equal((signCalls[0].params as { transactions: unknown[] }).transactions.length, 3);

  assert.deepEqual(await sign(wallet), []);
  const tooMany = Array.from({ length: 17 }, () => inputs[0]);
  await assert.rejects(sign(wallet, ...tooMany), /at most 16/);
});

test("a reply with the wrong number of transactions is refused", async () => {
  const { wallet } = walletWith((method) =>
    method === "connect" ? { accounts: [{ publicKey: KEY }] } : { signedTransactions: [] },
  );
  await connect(wallet);
  const input = { account: wallet.accounts[0], transaction: new Uint8Array([0x81]), chain: "solana:devnet" };
  await assert.rejects(sign(wallet, input), /wrong number/);
});
