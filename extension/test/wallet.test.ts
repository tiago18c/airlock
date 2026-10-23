import assert from "node:assert/strict";
import { test } from "node:test";
import { bytesToBase64 } from "../src/protocol";
import { AirlockWallet } from "../src/wallet";

const KEY = bytesToBase64(new Uint8Array(32).fill(7));

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
    "solana:signTransaction",
    "standard:connect",
    "standard:disconnect",
    "standard:events",
  ]);
  const feature = wallet.features["solana:signTransaction"] as { supportedTransactionVersions: readonly unknown[] };
  assert.deepEqual([...feature.supportedTransactionVersions], ["legacy", 0, 1]);
});

test("connect exposes the accounts the desktop app returned", async () => {
  const { wallet } = walletWith(() => ({ accounts: [{ publicKey: KEY, label: "Treasury" }] }));
  const { accounts } = await connect(wallet);
  assert.equal(accounts.length, 1);
  assert.equal(wallet.accounts[0].label, "Treasury");
  assert.deepEqual([...wallet.accounts[0].features], ["solana:signTransaction"]);
});

test("a key that is not 32 bytes is refused", async () => {
  const { wallet } = walletWith(() => ({ accounts: [{ publicKey: bytesToBase64(new Uint8Array(31)) }] }));
  await assert.rejects(connect(wallet), /32 bytes/);
});

test("one transaction is signed per request, for a connected account only", async () => {
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

  await assert.rejects(sign(wallet, input, input), /one transaction per request/);
  const stranger = { ...account, address: "11111111111111111111111111111111" };
  await assert.rejects(sign(wallet, { ...input, account: stranger }), /not connected/);
});

test("a reply with the wrong number of transactions is refused", async () => {
  const { wallet } = walletWith((method) =>
    method === "connect" ? { accounts: [{ publicKey: KEY }] } : { signedTransactions: [] },
  );
  await connect(wallet);
  const input = { account: wallet.accounts[0], transaction: new Uint8Array([0x81]), chain: "solana:devnet" };
  await assert.rejects(sign(wallet, input), /wrong number/);
});
