// A stand-in for the desktop app behind bridge-host, and for the node it sends
// through, so the page and the relay can be driven with nothing else running.
// It speaks native messaging on stdin and stdout as bridge-host does, and
// plays an app whose person approves every request after a short wait.
//
//   FAKE_RPC_PORT   serve a JSON-RPC node on 127.0.0.1 at this port (none when unset)
//   FAKE_CHAIN      the network the app is on, for memos (default solana:devnet;
//                   "custom" for a network that is none of the public ones)
//   FAKE_DELAY_MS   how long the person takes to approve (default 300)
//
// It checks notes as docs/intents.md's checks say, as far as a test needs:
// enough that a lie or a note on the account is refused, and a replaceable
// mint is swapped, its token account derived again and its data entry
// written. Nothing here is the app's code; the app's own tests are in Rust.

import { createServer } from "node:http";
import { createPrivateKey, createPublicKey, randomBytes, sign as edSign, verify as edVerify } from "node:crypto";
import { Keypair, Message, PublicKey, VersionedTransaction } from "@solana/web3.js";
import { FrameReader, encodeFrame } from "../relay.mjs";

const RPC_PORT = Number(process.env.FAKE_RPC_PORT || 0);
const CHAIN = process.env.FAKE_CHAIN || "solana:devnet";
const DELAY = Number(process.env.FAKE_DELAY_MS ?? 300);
const CHAINS = ["solana:mainnet", "solana:devnet", "solana:testnet"];

const MEMO = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const SYSTEM = "11111111111111111111111111111111";
const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const ATA = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const BLOCKHASH = new PublicKey(new Uint8Array(32).fill(9)).toBase58();

/* Base58, as the bridge writes addresses and signatures. */

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
export function b58(bytes) {
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros += 1;
  const digits = [];
  for (let i = zeros; i < bytes.length; i += 1) {
    let carry = bytes[i];
    for (let j = 0; j < digits.length; j += 1) {
      carry += digits[j] << 8;
      digits[j] = carry % 58;
      carry = (carry / 58) | 0;
    }
    while (carry) {
      digits.push(carry % 58);
      carry = (carry / 58) | 0;
    }
  }
  return "1".repeat(zeros) + digits.reverse().map((d) => B58[d]).join("");
}
export function unb58(text) {
  let zeros = 0;
  while (zeros < text.length && text[zeros] === "1") zeros += 1;
  const bytes = [];
  for (let i = zeros; i < text.length; i += 1) {
    let carry = B58.indexOf(text[i]);
    if (carry < 0) throw new Error(`${text} is not base58`);
    for (let j = 0; j < bytes.length; j += 1) {
      carry += bytes[j] * 58;
      bytes[j] = carry & 0xff;
      carry >>= 8;
    }
    while (carry) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  return Uint8Array.from([...new Array(zeros).fill(0), ...bytes.reverse()]);
}

/* Keys. Fixed seeds, so a run's accounts are the same each time. */

function ed25519Private(seed) {
  return createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(seed)]), format: "der", type: "pkcs8" });
}
function ed25519Public(bytes) {
  return createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(bytes).toString("base64url") }, format: "jwk" });
}
export function verifySignature(publicKey, message, signature) {
  try {
    return edVerify(null, Buffer.from(message), ed25519Public(publicKey), Buffer.from(signature));
  } catch {
    return false;
  }
}

const ACCOUNTS = [
  { seed: new Uint8Array(32).fill(1), label: "Hot key" },
  { seed: new Uint8Array(32).fill(2), label: "Spare key" },
].map((account) => ({ ...account, keypair: Keypair.fromSeed(account.seed), privateKey: ed25519Private(account.seed) }));
const byAddress = new Map(ACCOUNTS.map((account) => [account.keypair.publicKey.toBase58(), account]));

function profile(address) {
  return {
    account: address,
    timing: "sync",
    execution: "top-level",
    feePayer: ["account", "wallet", "app"],
    newAccounts: ["wallet-key", "app-key"],
    delivery: ["send", "cosign", "return"],
    messages: ["signature", "memo"],
    maxTransactions: 16,
  };
}

/* Native messaging. Nothing else may go to stdout. */

const write = (message) => process.stdout.write(encodeFrame(message));
const log = process.env.FAKE_VERBOSE ? (...args) => console.error("[fake-app]", ...args) : () => {};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

class Refusal extends Error {
  constructor(code, message, requirement) {
    super(message);
    this.code = code;
    this.requirement = requirement;
  }
}

/** Origins granted accounts by a connect the person approved. */
const granted = new Set();
/** Resolutions, by origin and id. */
const resolutions = new Map();
/** Requests waiting for the person, by the relay's id, so `cancel` can drop them. */
const waiting = new Map();

function keyOf(origin, id) {
  return `${origin} ${id}`;
}

function accountsWire() {
  return ACCOUNTS.map((account) => ({ publicKey: Buffer.from(account.keypair.publicKey.toBytes()).toString("base64"), label: account.label, signsMessages: true }));
}

/** What a site sees of a resolution: everything but what the wallet holds. */
function publicOf(entry) {
  const { held: _held, ...shown } = entry.resolution;
  return structuredClone(shown);
}

function changed(entry, patch) {
  const before = entry.resolution.updatedAt;
  Object.assign(entry.resolution, patch);
  // Seconds, never going back, and moving on at each change so no reply looks newer than a push.
  entry.resolution.updatedAt = Math.max(Math.floor(Date.now() / 1000), before + 1);
  write({ v: 1, method: "resolutionChanged", origin: entry.origin, params: { resolution: publicOf(entry) } });
}

/** Waits for the person on request `id`. False when the page went away first. */
async function person(id) {
  let cancelled = false;
  waiting.set(id, () => (cancelled = true));
  await sleep(DELAY);
  waiting.delete(id);
  return !cancelled;
}

process.stdin.on("data", (() => {
  const reader = new FrameReader();
  return (chunk) => {
    for (const message of reader.push(chunk)) void handle(message);
  };
})());
process.stdin.on("end", () => process.exit(0));

async function handle(message) {
  const { id, method, params = {}, origin } = message;
  log("<", JSON.stringify(message).slice(0, 300));
  const reply = (result) => write({ v: 1, id, result });
  try {
    if (message.expiresAt !== undefined && message.expiresAt * 1000 < Date.now()) throw new Refusal("expired", "The request expired before Airlock saw it.");
    switch (method) {
      case "status":
        return reply({ ready: true, role: "fake-app" });
      case "pages":
        return reply({});
      case "cancel":
        waiting.get(params.id)?.();
        return reply({});
      case "connect":
        if (params.silent) return reply({ accounts: granted.has(origin) ? accountsWire() : [] });
        if (!(await person(id))) return;
        granted.add(origin);
        return reply({ accounts: accountsWire() });
      case "disconnect":
        granted.delete(origin);
        return reply({});
      case "getSigningProfile":
        return reply({ profile: profile(connected(origin, params.account).keypair.publicKey.toBase58()) });
      case "resolve":
        return reply({ resolution: publicOf(await resolveRequest(origin, params)) });
      case "getResolution":
        return reply({ resolution: publicOf(find(origin, params.id)) });
      case "cancelResolution":
        return reply({ resolution: publicOf(cancel(origin, params.id)) });
      case "provideSignatures":
        return reply({ resolution: publicOf(provide(origin, params)) });
      case "resolveMessage":
        return reply(await resolveMessage(id, origin, params));
      case "signAndSendTransaction":
        return reply(await signAndSend(id, origin, params));
      default:
        throw new Refusal("not_implemented", `The fake app does not answer ${method}.`);
    }
  } catch (err) {
    const error = err instanceof Refusal ? { code: err.code, message: err.message, ...(err.requirement ? { requirement: err.requirement } : {}) } : { code: "internal", message: String(err?.message ?? err) };
    log("refused", JSON.stringify(error));
    write({ v: 1, id, error });
  }
}

function connected(origin, address) {
  const account = byAddress.get(address);
  if (!account || !granted.has(origin)) throw new Refusal("rejected", "This account is not connected to the site.");
  return account;
}

function find(origin, id) {
  const entry = resolutions.get(keyOf(origin, id));
  if (!entry) throw new Refusal("not_found", "No resolution of this site has this id.");
  return entry;
}

/* resolve */

function staticKeys(tx) {
  return tx.message.staticAccountKeys.map((key) => key.toBase58());
}

function signersOf(tx) {
  return staticKeys(tx).slice(0, tx.message.header.numRequiredSignatures);
}

function seedBytes(seed) {
  return "account" in seed ? new PublicKey(seed.account).toBytes() : Buffer.from(seed.bytes, "base64");
}

function derive(note, use = (address) => address) {
  const seeds = note.pda.seeds.map((seed) => ("account" in seed ? new PublicKey(use(seed.account)).toBytes() : seedBytes(seed)));
  return PublicKey.findProgramAddressSync(seeds, new PublicKey(note.pda.program))[0].toBase58();
}

function instructionData(tx, index) {
  return unb58(tx.message.instructions[index].data);
}

async function resolveRequest(origin, params) {
  const account = connected(origin, params.account).keypair.publicKey.toBase58();
  const id = params.id ?? `fake-${randomBytes(6).toString("hex")}`;
  if (!/^[A-Za-z0-9-]{1,80}$/.test(id)) throw new Refusal("bad_request", "An id is 1 to 80 letters, digits and dashes.");
  if (resolutions.has(keyOf(origin, id))) throw new Refusal("duplicate_id", "The site used this id before.");
  if (!CHAINS.includes(params.chain)) throw new Refusal("unsupported", `${params.chain} is not a chain Airlock knows.`, "chain");
  const wires = params.transactions ?? [];
  if (wires.length === 0) throw new Refusal("bad_request", "A request holds at least one transaction.");
  if (wires.length > 16) throw new Refusal("not_implemented", "Airlock signs at most 16 transactions per request.");
  const delivery = params.delivery ?? "send";
  const order = params.order ?? "serial";
  const shown = profile(account);
  if (!shown.delivery.includes(delivery)) throw new Refusal("unsupported", `This account does not offer ${delivery}.`, "delivery");
  if (order === "atomic" && wires.length > 1) throw new Refusal("unsupported", "Airlock's prototype cannot run several transactions in one.", "order");
  const txs = wires.map((wire) => VersionedTransaction.deserialize(Buffer.from(wire, "base64")));
  const notes = params.accounts ?? {};
  const data = params.data ?? [];
  const everywhere = new Set(txs.flatMap(staticKeys));

  // The checks of docs/intents.md, as far as this stand-in goes.
  for (const [address, note] of Object.entries(notes)) {
    if (address === account) throw new Refusal("bad_notes", `${address} is the account acted as, which carries no note.`);
    if (!everywhere.has(address)) throw new Refusal("bad_notes", `${address} is not among the static accounts of any transaction of the request.`);
    if (note.kind === "derived" && note.pda) {
      const derived = derive(note);
      if (derived !== address) throw new Refusal("bad_notes", `The note on ${address} derives ${derived}, not ${address}.`);
    }
    if (note.kind === "new" && !txs.some((tx) => signersOf(tx).includes(address))) {
      throw new Refusal("bad_notes", `The new account ${address} signs no transaction of the request.`);
    }
  }
  for (const [i, entry] of data.entries()) {
    const bytes = txs[entry.transaction] && entry.instruction < txs[entry.transaction].message.instructions.length ? instructionData(txs[entry.transaction], entry.instruction) : null;
    const address = entry.account ?? entry.bumpOf;
    if (!bytes || !everywhere.has(address)) throw new Refusal("bad_notes", `Data entry ${i} names an instruction or an address the request does not hold.`);
    if (entry.account && Buffer.compare(Buffer.from(bytes.subarray(entry.offset, entry.offset + 32)), Buffer.from(new PublicKey(entry.account).toBytes())) !== 0) {
      throw new Refusal("bad_notes", `Data entry ${i} does not name the bytes of ${entry.account}.`);
    }
  }

  // What would need the app's signature, and whether it can be given.
  const appPays = shown.feePayer.includes("app") && delivery !== "send";
  for (const [address, note] of Object.entries(notes)) {
    const kept = (note.kind === "new" && !note.replaceable) || note.kind === "signer" || (note.kind === "feePayer" && note.signs === "app" && (!note.replaceable || appPays));
    if (kept && delivery === "send") {
      const requirement = note.kind === "new" ? "newAccounts" : note.kind;
      throw new Refusal("unsupported", `${address} needs the app's signature, which delivery send cannot take.`, requirement);
    }
  }

  // The rewrite: a key of the wallet's for each replaceable new account, the
  // account's own payer for a replaceable sponsor, then every derived account
  // again, to any depth.
  const replaced = {};
  const walletKeys = [];
  for (const [address, note] of Object.entries(notes)) {
    if (note.kind === "new" && note.replaceable) {
      const key = Keypair.generate();
      walletKeys.push(key);
      replaced[address] = key.publicKey.toBase58();
    }
    if (note.kind === "feePayer" && note.replaceable && !(note.signs === "app" && appPays)) replaced[address] = account;
  }
  for (let round = 0; round < 8; round += 1) {
    let moved = false;
    for (const [address, note] of Object.entries(notes)) {
      if (note.kind !== "derived" || !note.pda) continue;
      const again = derive(note, (seed) => replaced[seed] ?? seed);
      if (again !== address && replaced[address] !== again) {
        replaced[address] = again;
        moved = true;
      }
    }
    if (!moved) break;
  }

  const resolution = {
    id,
    account,
    chain: params.chain,
    timing: "sync",
    delivery,
    state: "awaiting-approval",
    replaced,
    transactions: [],
    ran: [],
    updatedAt: Math.floor(Date.now() / 1000),
  };
  const entry = { origin, resolution, txs, data, notes, walletKeys, composed: [], owed: [] };
  resolutions.set(keyOf(origin, id), entry);
  void approveLater(entry);
  return entry;
}

/** The transaction as the wallet composes it: each replacement in the
 * account list and at each data entry, every signature slot empty. */
function compose(tx, index, entry) {
  if (tx.version !== "legacy") return new VersionedTransaction(tx.message);
  const use = (address) => entry.resolution.replaced[address] ?? address;
  const message = tx.message;
  const instructions = message.instructions.map((ix, i) => {
    const bytes = unb58(ix.data);
    for (const note of entry.data) {
      if (note.transaction === index && note.instruction === i && note.account && use(note.account) !== note.account) {
        bytes.set(new PublicKey(use(note.account)).toBytes(), note.offset);
      }
    }
    return { ...ix, data: b58(bytes) };
  });
  return new VersionedTransaction(
    new Message({
      header: message.header,
      accountKeys: message.accountKeys.map((key) => new PublicKey(use(key.toBase58()))),
      recentBlockhash: message.recentBlockhash,
      instructions,
    }),
  );
}

/** Signs `tx` with every key of the wallet's that it asks for. */
function signAsWallet(tx, entry) {
  const signers = signersOf(tx);
  const keys = [byAddress.get(entry.resolution.account).keypair, ...entry.walletKeys].filter((key) => signers.includes(key.publicKey.toBase58()));
  if (keys.length) tx.sign(keys);
}

function owedOn(tx, entry) {
  const mine = new Set([entry.resolution.account, ...entry.walletKeys.map((key) => key.publicKey.toBase58())]);
  return signersOf(tx).filter((signer) => !mine.has(signer));
}

async function approveLater(entry) {
  await sleep(DELAY);
  if (entry.resolution.state !== "awaiting-approval") return;
  entry.composed = entry.txs.map((tx, i) => compose(tx, i, entry));
  const transactions = entry.composed.map((_, i) => ({ role: "direct", of: [i], state: "pending" }));
  const owed = entry.composed.map((tx, i) => ({ transaction: i, signers: owedOn(tx, entry) })).filter((item) => item.signers.length);
  if (entry.resolution.delivery === "cosign" && owed.length) {
    // Every slot empty for the app; the wallet signs only when it sends.
    entry.owed = owed;
    changed(entry, {
      state: "awaiting-app",
      transactions: transactions.map((item, i) => ({ ...item, wire: Buffer.from(entry.composed[i].serialize()).toString("base64") })),
      awaiting: owed,
    });
    return;
  }
  entry.composed.forEach((tx) => signAsWallet(tx, entry));
  if (entry.resolution.delivery === "return") {
    entry.owed = owed;
    changed(entry, {
      state: "returned",
      transactions: transactions.map((item, i) => ({ ...item, wire: Buffer.from(entry.composed[i].serialize()).toString("base64") })),
      awaiting: owed,
    });
    return;
  }
  await send(entry, transactions);
}

async function send(entry, transactions) {
  changed(entry, { state: "sending", awaiting: undefined, transactions });
  await sleep(Math.min(DELAY, 200));
  const ran = [];
  const landed = entry.composed.map((tx, i) => {
    const signature = land(tx);
    ran.push({ transaction: i, signature });
    return { role: "direct", of: [i], state: "landed", signature };
  });
  changed(entry, { state: "done", transactions: landed, ran });
}

function cancel(origin, id) {
  const entry = find(origin, id);
  if (!["awaiting-approval", "awaiting-app"].includes(entry.resolution.state)) {
    throw new Refusal("bad_state", `The resolution is ${entry.resolution.state}, which does not take cancelResolution.`);
  }
  changed(entry, { state: "cancelled", awaiting: undefined });
  return entry;
}

function provide(origin, params) {
  const entry = find(origin, params.id);
  if (entry.resolution.state !== "awaiting-app") {
    throw new Refusal("bad_state", `The resolution is ${entry.resolution.state}, which does not take provideSignatures.`);
  }
  for (const given of params.signatures ?? []) {
    const owed = entry.owed.find((item) => item.transaction === given.transaction);
    const tx = entry.composed[given.transaction];
    if (!owed || !tx || !owed.signers.includes(given.signer)) throw new Refusal("bad_signature", `${given.signer} is not owed on transaction ${given.transaction}.`);
    const signature = unb58(given.signature);
    if (!verifySignature(new PublicKey(given.signer).toBytes(), tx.message.serialize(), signature)) {
      throw new Refusal("bad_signature", `The signature of ${given.signer} does not verify for transaction ${given.transaction}.`);
    }
    tx.addSignature(new PublicKey(given.signer), signature);
    owed.signers = owed.signers.filter((signer) => signer !== given.signer);
  }
  entry.owed = entry.owed.filter((item) => item.signers.length);
  if (entry.owed.length) {
    changed(entry, { awaiting: entry.owed });
    return entry;
  }
  entry.composed.forEach((tx) => signAsWallet(tx, entry));
  const transactions = entry.resolution.transactions.map(({ wire: _wire, ...item }) => item);
  void send(entry, transactions);
  return entry;
}

/* Messages */

async function resolveMessage(id, origin, params) {
  const account = connected(origin, params.account);
  const message = Buffer.from(params.message ?? "", "base64");
  const mode = params.mode ?? "signature";
  if (mode === "signature") {
    if (!(await person(id))) throw new Refusal("expired", "The page closed.");
    return { kind: "signature", signedMessage: message.toString("base64"), signature: b58(edSign(null, message, account.privateKey)) };
  }
  // The chain the site names, as in any request; with none, the network
  // the app is on when it can name it.
  if (params.chain !== undefined && !CHAINS.includes(params.chain)) {
    throw new Refusal("unsupported", `Airlock resolves requests on ${CHAINS.join(", ")}, not on ${params.chain}.`, "chain");
  }
  const chain = params.chain ?? (CHAINS.includes(CHAIN) ? CHAIN : null);
  if (!chain) throw new Refusal("unsupported", "Airlock is on a network that is none of the public ones, so a memo that names no chain cannot say which it is on. Name the chain.", "chain");
  const text = message.toString("utf8");
  if (Buffer.from(text, "utf8").compare(message) !== 0 || message.length > 512) {
    throw new Refusal("unsupported", "A memo is UTF-8 text of at most 512 bytes.", "messages");
  }
  const address = account.keypair.publicKey;
  const memoTx = new VersionedTransaction(
    new Message({
      header: { numRequiredSignatures: 1, numReadonlySignedAccounts: 0, numReadonlyUnsignedAccounts: 1 },
      accountKeys: [address, new PublicKey(MEMO)],
      recentBlockhash: BLOCKHASH,
      instructions: [{ programIdIndex: 1, accounts: [0], data: b58(message) }],
    }),
  );
  const resolutionId = params.id ?? `fake-${randomBytes(6).toString("hex")}`;
  if (resolutions.has(keyOf(origin, resolutionId))) throw new Refusal("duplicate_id", "The site used this id before.");
  const resolution = {
    id: resolutionId,
    account: address.toBase58(),
    chain,
    timing: "sync",
    delivery: "send",
    state: "awaiting-approval",
    replaced: {},
    transactions: [],
    ran: [],
    message: { memo: text },
    updatedAt: Math.floor(Date.now() / 1000),
  };
  const entry = { origin, resolution, txs: [memoTx], data: [], notes: {}, walletKeys: [], composed: [], owed: [] };
  resolutions.set(keyOf(origin, resolutionId), entry);
  void approveLater(entry);
  return { kind: "memo", resolution: publicOf(entry) };
}

async function signAndSend(id, origin, params) {
  const items = params.transactions ?? [];
  for (const item of items) connected(origin, item.account);
  if (!(await person(id))) throw new Refusal("expired", "The page closed.");
  const signatures = items.map((item) => {
    const tx = VersionedTransaction.deserialize(Buffer.from(item.transaction, "base64"));
    tx.sign([byAddress.get(item.account).keypair]);
    return Buffer.from(unb58(land(tx))).toString("base64");
  });
  return { signatures };
}

/* The ledger the fake node reads: what each landed transaction did, as far
 * as the page reads it back. */

const ledger = new Map();
const landedTxs = new Map();
let slot = 1000;
const DEFAULT_LAMPORTS = 2_500_000_000;

function accountAt(address) {
  if (!ledger.has(address)) ledger.set(address, { lamports: DEFAULT_LAMPORTS, owner: SYSTEM, data: Buffer.alloc(0), implicit: true });
  return ledger.get(address);
}

function rent(space) {
  return (128 + space) * 6960;
}

/** Applies what `tx` does and keeps it. Its signature is its first. */
function land(tx) {
  const keys = staticKeys(tx);
  slot += 1;
  for (const ix of tx.message.instructions) {
    const program = keys[ix.programIdIndex];
    const accounts = ix.accounts.map((i) => keys[i]);
    const data = Buffer.from(unb58(ix.data));
    apply(program, accounts, data);
  }
  const signature = b58(tx.signatures[0]);
  landedTxs.set(signature, { wire: Buffer.from(tx.serialize()).toString("base64"), slot, version: tx.version });
  watchReturned(tx, signature);
  return signature;
}

function apply(program, accounts, data) {
  if (program === SYSTEM && data.length >= 4) {
    const tag = data.readUInt32LE(0);
    if (tag === 0) {
      const [from, to] = accounts;
      const lamports = Number(data.readBigUInt64LE(4));
      const space = Number(data.readBigUInt64LE(12));
      accountAt(from).lamports -= lamports;
      ledger.set(to, { lamports, owner: new PublicKey(data.subarray(20, 52)).toBase58(), data: Buffer.alloc(space) });
    } else if (tag === 2) {
      const [from, to] = accounts;
      const lamports = Number(data.readBigUInt64LE(4));
      accountAt(from).lamports -= lamports;
      accountAt(to).lamports += lamports;
    }
    return;
  }
  if (program === TOKEN || program === TOKEN_2022) {
    const target = ledger.get(accounts[0]);
    if (data[0] === 20 && target) {
      // InitializeMint2: the authority, supply 0, decimals, initialized.
      target.data.writeUInt32LE(1, 0);
      data.copy(target.data, 4, 2, 34);
      target.data[44] = data[1];
      target.data[45] = 1;
      if (target.data.length > 165) target.data[165] = 1;
    } else if (data[0] === 39 && data[1] === 0 && target && target.data.length >= 234) {
      // InitializeMetadataPointer: the extension after the account type.
      target.data[165] = 1;
      target.data.writeUInt16LE(18, 166);
      target.data.writeUInt16LE(64, 168);
      data.copy(target.data, 170, 2, 66);
    } else if (data[0] === 7) {
      const [mint, destination] = accounts;
      const amount = data.readBigUInt64LE(1);
      const mintAccount = ledger.get(mint);
      const holder = ledger.get(destination);
      if (mintAccount) mintAccount.data.writeBigUInt64LE(mintAccount.data.readBigUInt64LE(36) + amount, 36);
      if (holder) holder.data.writeBigUInt64LE(holder.data.readBigUInt64LE(64) + amount, 64);
    }
    return;
  }
  if (program === ATA) {
    const [payer, address, owner, mint, , tokenProgram] = accounts;
    const size = tokenProgram === TOKEN_2022 ? 170 : 165;
    const body = Buffer.alloc(size);
    new PublicKey(mint).toBuffer().copy(body, 0);
    new PublicKey(owner).toBuffer().copy(body, 32);
    body[108] = 1;
    if (size > 165) {
      // An account type, then ImmutableOwner, which takes no bytes.
      body[165] = 2;
      body.writeUInt16LE(7, 166);
    }
    accountAt(payer).lamports -= rent(size);
    ledger.set(address, { lamports: rent(size), owner: tokenProgram, data: body });
  }
}

/** A transaction handed back to the app (`returned`) that the app sent: the
 * wallet watches for it, since the fee payer's signature is its own. */
function watchReturned(tx, signature) {
  const message = Buffer.from(tx.message.serialize());
  for (const entry of resolutions.values()) {
    if (entry.resolution.state !== "returned") continue;
    const index = entry.composed.findIndex((composed) => Buffer.from(composed.message.serialize()).equals(message));
    if (index < 0) continue;
    const transactions = entry.resolution.transactions.map((item, i) => (i === index ? { role: item.role, of: item.of, state: "landed", signature } : item));
    const done = transactions.every((item) => item.state === "landed");
    setTimeout(() => changed(entry, { transactions, ...(done ? { state: "done", ran: transactions.map((item, i) => ({ transaction: i, signature: item.signature })), awaiting: undefined } : {}) }), 50);
  }
}

/* The fake node: JSON-RPC over HTTP, on loopback. */

function rpc(method, params) {
  const context = { context: { slot } };
  switch (method) {
    case "getLatestBlockhash":
      return { ...context, value: { blockhash: BLOCKHASH, lastValidBlockHeight: slot + 150 } };
    case "getSlot":
      return slot;
    case "getBalance":
      return { ...context, value: accountAt(params[0]).lamports };
    case "getMinimumBalanceForRentExemption":
      return rent(params[0]);
    case "requestAirdrop": {
      accountAt(params[0]).lamports += params[1];
      const signature = b58(randomBytes(64));
      landedTxs.set(signature, { wire: null, slot });
      return signature;
    }
    case "sendTransaction": {
      const tx = VersionedTransaction.deserialize(Buffer.from(params[0], "base64"));
      const message = tx.message.serialize();
      signersOf(tx).forEach((signer, i) => {
        if (!verifySignature(new PublicKey(signer).toBytes(), message, tx.signatures[i])) {
          throw Object.assign(new Error(`Transaction signature verification failure: ${signer}`), { rpcCode: -32003 });
        }
      });
      return land(tx);
    }
    case "getSignatureStatuses":
      return { ...context, value: params[0].map((signature) => (landedTxs.has(signature) ? { slot: landedTxs.get(signature).slot, confirmations: null, err: null, confirmationStatus: "confirmed" } : null)) };
    case "getAccountInfo":
      return { ...context, value: accountInfo(params[0]) };
    case "getMultipleAccounts":
      return { ...context, value: params[0].map(accountInfo) };
    case "getTransaction": {
      const found = landedTxs.get(params[0]);
      if (!found?.wire) return null;
      return {
        slot: found.slot,
        blockTime: Math.floor(Date.now() / 1000),
        version: found.version === "legacy" ? "legacy" : 0,
        transaction: [found.wire, "base64"],
        meta: { err: null, fee: 5000, innerInstructions: [], loadedAddresses: { writable: [], readonly: [] }, logMessages: [], preBalances: [], postBalances: [] },
      };
    }
    default:
      throw Object.assign(new Error(`The fake node does not answer ${method}.`), { rpcCode: -32601 });
  }
}

function accountInfo(address) {
  const found = ledger.get(address);
  if (!found || found.implicit) return null;
  return { data: [found.data.toString("base64"), "base64"], executable: false, lamports: found.lamports, owner: found.owner, rentEpoch: 0, space: found.data.length };
}

if (RPC_PORT) {
  const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type, solana-client", "access-control-allow-methods": "POST, OPTIONS" };
  createServer((request, response) => {
    if (request.method === "OPTIONS") {
      response.writeHead(204, cors).end();
      return;
    }
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      let call;
      try {
        call = JSON.parse(body);
      } catch {
        response.writeHead(400, cors).end();
        return;
      }
      let answer;
      try {
        answer = { jsonrpc: "2.0", id: call.id, result: rpc(call.method, call.params ?? []) };
      } catch (err) {
        answer = { jsonrpc: "2.0", id: call.id, error: { code: err.rpcCode ?? -32000, message: err.message } };
      }
      response.writeHead(200, { ...cors, "content-type": "application/json" }).end(JSON.stringify(answer));
    });
  }).listen(RPC_PORT, "127.0.0.1");
}
