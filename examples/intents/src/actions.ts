/** The rows of the page: one action each, which says what it sends, sends
 * it, and shows what came back. */

import { Keypair, PublicKey, SystemProgram, Transaction, type Connection, type TransactionInstruction } from "@solana/web3.js";
import type { Wallet, WalletAccount } from "@wallet-standard/base";
import type {
  AccountNote,
  DataNote,
  Delivery,
  Order,
  ProvideSignaturesInput,
  Resolution,
  ResolutionState,
  ResolveInput,
  SignMessageInput,
  SignMessageOutput,
  SigningProfile,
} from "../../../extension/src/intents";
import { BridgeError, base64ToBytes, bytesToBase64, encodeBase58 } from "../../../extension/src/protocol";
import { compareComposed } from "./compare";
import { sign, verify } from "./ed25519";
import {
  ATA_PROGRAM,
  MEMO_PROGRAM,
  MINT_SIZE,
  MINT_WITH_POINTER_SIZE,
  TOKEN_2022_PROGRAM,
  TOKEN_PROGRAM,
  associatedTokenAddress,
  createAssociatedTokenAccount,
  createMintAccount,
  initializeMint2,
  memo,
  mintTo,
  readMetadataPointer,
  readMint,
  readTokenAccount,
} from "./instructions";
import type { EventLog } from "./log";
import { proveMemo, type MemoProof } from "./memo-check";
import { confirm, rpcFor, sleep, type Network } from "./network";
import type { ResolutionStore } from "./resolutions";
import { token2022MintRequest } from "./token-2022";
import { address, h, type Tone } from "./ui";
import { missingSigners, parseTransaction, withSignature } from "./wire";

/** The wallet's intent methods, as the page calls them: each call is logged,
 * and each resolution lands in the store. */
export interface Api {
  profile(account: WalletAccount): Promise<SigningProfile>;
  resolve(label: string, input: ResolveInput & { id: string }): Promise<Resolution>;
  get(id: string): Promise<Resolution>;
  provide(input: ProvideSignaturesInput): Promise<Resolution>;
  cancel(id: string): Promise<Resolution>;
  signMessage(label: string, input: SignMessageInput & { id: string }): Promise<SignMessageOutput>;
}

export interface Ctx {
  wallet: Wallet;
  account: WalletAccount;
  network: Network;
  connection: Connection;
  appKey: Keypair;
  /** Null for a wallet without `airlock:resolve`. */
  api: Api | null;
  profile: SigningProfile | null;
  store: ResolutionStore;
  log: EventLog;
  setProfile(profile: SigningProfile | null): void;
}

export interface RequestView {
  summary: string;
  transactions: string[];
  delivery?: Delivery;
  order?: Order;
  accounts?: Record<string, AccountNote>;
  data?: DataNote[];
  /** What each address in the notes is, for people. */
  names: Record<string, string>;
  /** Said under the notes, such as why there is no `data` entry. */
  aside?: string;
  raw?: unknown;
}

/** A wallet's refusal, as the page shows it: the code, what the account could
 * not do when the wallet says, and the wallet's own words. */
export interface Refusal {
  code: string;
  message: string;
  requirement?: string;
}

export interface Out {
  request(view: RequestView): void;
  step(text: string, tone?: Tone, extra?: Node): void;
  signature(label: string, signature: string): void;
  /** How the row ended: as hoped, or not, or (null) that the page could not
   * check it. A refusal is shown beside the text. */
  verdict(pass: boolean | null, text: string, refusal?: Refusal): void;
  node(node: Node): void;
}

/** A refusal the profile says a row will meet: the code and requirement the
 * wallet should answer with, and why, in words. */
export interface Expectation {
  reason: string;
  code: string;
  requirement?: string;
}

/** Whether `refusal` is the one `expected` names. */
export function refusalMatches(expected: Expectation, refusal: Refusal): boolean {
  return refusal.code === expected.code && (expected.requirement === undefined || refusal.requirement === expected.requirement);
}

export interface Row {
  n: number;
  key: string;
  title: string;
  summary: string;
  /** Needs `airlock:resolve`. Only the fallback does not. */
  intents: boolean;
  controls?: HTMLElement;
  /** The refusal the account's profile says this row will meet, if it does.
   * The row passes only when the wallet refuses with that code and
   * requirement. */
  expect?(profile: SigningProfile): Expectation | null;
  /** Said above the row whatever the profile: a row whose pass is a refusal
   * it checks itself. */
  note?: string;
  run(ctx: Ctx, out: Out): Promise<void>;
}

const LAMPORTS_PER_SOL = 1_000_000_000;

export function newId(): string {
  const random = new Uint8Array(6);
  crypto.getRandomValues(random);
  return `page-${Date.now().toString(36)}-${[...random].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

/** The refusal an error stands for, or null when it is not one: a wallet's
 * refusal has a code. */
export function asRefusal(err: unknown): Refusal | null {
  if (!err || typeof err !== "object" || !("code" in err)) return null;
  const e = err as { code: unknown; message?: unknown; requirement?: unknown };
  if (typeof e.code !== "string") return null;
  return {
    code: e.code,
    message: typeof e.message === "string" ? e.message : "",
    requirement: typeof e.requirement === "string" ? e.requirement : undefined,
  };
}

/** One line for the log and toasts: the code, the requirement, the message. */
export function describeError(err: unknown): string {
  const refusal = asRefusal(err);
  if (refusal) return `${refusal.code}${refusal.requirement ? ` (requirement: ${refusal.requirement})` : ""}: ${refusal.message}`;
  return err instanceof Error ? err.message : String(err);
}

function key(account: WalletAccount): PublicKey {
  return new PublicKey(account.address);
}

async function build(connection: Connection, payer: PublicKey, instructions: TransactionInstruction[], signers: Keypair[] = []): Promise<Uint8Array> {
  const { blockhash } = await connection.getLatestBlockhash();
  const tx = new Transaction({ feePayer: payer, recentBlockhash: blockhash }).add(...instructions);
  if (signers.length) tx.partialSign(...signers);
  return new Uint8Array(tx.serialize({ requireAllSignatures: false, verifySignatures: false }));
}

const STATE_WORDS: Record<ResolutionState, string> = {
  "awaiting-approval": "waiting for you to approve it in Airlock",
  "awaiting-signatures":
    "approved in Airlock; other people or another device still have to sign, which may take a long time or never happen. This row keeps following it, and so does Resolutions below",
  "awaiting-app": "signed in Airlock; this page owes its signatures",
  returned: "handed back to this page to sign and send",
  sending: "going out",
  done: "every transaction sent in has run",
  failed: "it landed and failed, or can no longer land",
  unknown: "it may still land; do not send it again as something new",
  declined: "you declined it",
  expired: "nobody decided in time",
  cancelled: "cancelled",
};

export function stateTone(state: ResolutionState | undefined): Tone {
  switch (state) {
    case "done":
      return "ok";
    case "failed":
    case "declined":
      return "danger";
    case "unknown":
    case "expired":
    case "awaiting-app":
    case "returned":
      return "warn";
    case "cancelled":
    case undefined:
      return "neutral";
    default:
      return "info";
  }
}

function stateLine(resolution: Resolution): string {
  return `${resolution.state}: ${STATE_WORDS[resolution.state] ?? "a state this page does not know"}.`;
}

/** Shows each new state of `id` until it is in `until` or has ended. */
async function follow(ctx: Ctx, out: Out, id: string, until: readonly ResolutionState[]): Promise<Resolution> {
  let last = ctx.store.get(id)?.resolution?.state;
  const off = ctx.store.subscribe((item) => {
    const state = item.resolution?.state;
    if (item.id !== id || !state || state === last) return;
    last = state;
    out.step(stateLine(item.resolution!), stateTone(state));
  });
  try {
    return await ctx.store.waitFor(id, until);
  } finally {
    off();
  }
}

/** What a resolution that ended says: where each transaction ran, what was replaced, and why it failed. */
function report(out: Out, resolution: Resolution): void {
  const replaced = Object.entries(resolution.replaced ?? {});
  if (replaced.length) {
    out.node(
      h(
        "div",
        { class: "replaced" },
        h("span", { class: "label" }, "Replaced"),
        ...replaced.map(([from, to]) => h("div", {}, address(from), " became ", address(to))),
      ),
    );
  }
  for (const ran of resolution.ran ?? []) out.signature(`Transaction ${ran.transaction} ran`, ran.signature);
  for (const tx of resolution.transactions ?? []) {
    if (tx.error) out.step(`The ${tx.role} transaction ${tx.state}: ${tx.error}`, "danger");
  }
  if (resolution.error) out.step(describeError(resolution.error), "danger");
}

/** What a resolution's first state after awaiting-approval says about who
 * moved it there. After a restart Airlock answers failed or unknown itself. */
function decidedLine(resolution: Resolution): string {
  switch (resolution.state) {
    case "failed":
    case "unknown":
      return `Airlock answered. ${stateLine(resolution)}`;
    default:
      return `You approved it. Now ${stateLine(resolution)}`;
  }
}

/** Sends a request and waits for the person, as `resolve` does. */
async function resolve(ctx: Ctx, out: Out, label: string, input: Omit<ResolveInput, "account" | "chain" | "id">): Promise<Resolution> {
  const api = need(ctx);
  const id = newId();
  out.step(`resolve sent as ${id}. Once Airlock takes it, approve or decline it there.`, "info");
  const resolution = await api.resolve(label, { ...input, id, account: ctx.account, chain: ctx.network.chain });
  out.step(decidedLine(resolution), stateTone(resolution.state));
  return resolution;
}

function need(ctx: Ctx): Api {
  if (!ctx.api) throw new BridgeError("unsupported", "This wallet has no airlock:resolve.");
  return ctx.api;
}

/** Follows a sent request to its end and says how it went. */
async function toTheEnd(ctx: Ctx, out: Out, resolution: Resolution): Promise<Resolution> {
  const ended = await follow(ctx, out, resolution.id, []);
  report(out, ended);
  return ended;
}

/** Runs `send`, which the wallet should refuse with `code`. The refusal is
 * the pass, shown with its code, requirement and message. */
async function expectRefusal(ctx: Ctx, out: Out, code: string, why: string, send: () => Promise<Resolution>): Promise<void> {
  let resolution: Resolution;
  try {
    resolution = await send();
  } catch (err) {
    const refusal = asRefusal(err);
    if (!refusal) throw err;
    if (refusal.code === code) out.verdict(true, `Refused with ${code}, as it should be: ${why}`, refusal);
    else out.verdict(false, `Refused, but with ${refusal.code} rather than ${code}: ${why}`, refusal);
    return;
  }
  out.verdict(false, `The wallet took the request (${resolution.state}). It should have refused it with ${code}: ${why}`);
  if (resolution.state === "awaiting-app") await need(ctx).cancel(resolution.id);
}

const SELF_PAYS =
  "Your account pays the fee, and carries no note: who pays is the wallet's to choose, as the blockhash is.";

function names(ctx: Ctx, more: Record<string, string> = {}): Record<string, string> {
  return { [ctx.account.address]: "your account", [ctx.appKey.publicKey.toBase58()]: "app key", ...more };
}

function memoText(what: string): string {
  return `Airlock intents test page: ${what}, ${new Date().toISOString()}`;
}

/** A select for a row's options, named so a script can set it. */
function options(name: string, label: string, choices: [string, string][]): HTMLSelectElement {
  return h("select", { "aria-label": label, name }, ...choices.map(([value, text]) => h("option", { value }, text)));
}

/* 1. Signing profile */

export function profileRow(): Row {
  return {
    n: 1,
    key: "profile",
    title: "Signing profile",
    summary: "Asks how the chosen account signs, before anything is built. The rows below read it to say what to expect.",
    intents: true,
    async run(ctx, out) {
      out.request({
        summary: "getSigningProfile, for the chosen account. Only a connected site may ask.",
        transactions: [],
        names: names(ctx),
        raw: { account: ctx.account.address },
      });
      const profile = await need(ctx).profile(ctx.account);
      ctx.setProfile(profile);
      out.node(profileTable(profile));
      out.verdict(true, explainProfile(profile));
    },
  };
}

export function profileTable(profile: SigningProfile): HTMLElement {
  const list = (values: readonly string[] | undefined) => (values?.length ? values.join(", ") : "none");
  const rows: [string, string][] = [
    ["timing", profile.timing],
    ["execution", profile.execution + (profile.carrier ? ` (carrier ${profile.carrier})` : "")],
    ["feePayer", list(profile.feePayer)],
    ["newAccounts", list(profile.newAccounts)],
    ["delivery", list(profile.delivery)],
    ["messages", list(profile.messages)],
    ["maxTransactions", String(profile.maxTransactions)],
  ];
  return h(
    "table",
    { class: "facts" },
    h("tbody", {}, ...rows.map(([name, value]) => h("tr", {}, h("th", {}, name), h("td", {}, h("code", {}, value))))),
  );
}

/** What happens to a request for this account, in a sentence: it resolves
 * at once, or it waits for other people or another device. */
export function whatHappens(profile: SigningProfile): string {
  const where =
    profile.execution === "top-level"
      ? "its instructions run at the top of a transaction"
      : `its instructions run inside a program (a carrier${profile.carrier ? `, ${profile.carrier.slice(0, 4)}…` : ""})`;
  return profile.timing === "sync"
    ? `Resolves at once: once you approve in Airlock it is signed and sent while the request is open, and ${where}.`
    : `Waits: after you approve in Airlock, other people or another device still have to sign, so it may finish much later, or never. Follow it under Resolutions; ${where}.`;
}

/** Whether the wallet can sign for a new account the page marks replaceable:
 * with a key of its own (`wallet-key`), or through its carrier program, at
 * an address the program derives (`carrier`). */
export function signsForNewAccounts(profile: SigningProfile): boolean {
  return Boolean(profile.newAccounts?.includes("wallet-key") || profile.newAccounts?.includes("carrier"));
}

/** One or two sentences: what the profile means for the rows below. */
export function explainProfile(profile: SigningProfile): string {
  const refused: string[] = [];
  if (!signsForNewAccounts(profile)) refused.push("4 (the wallet cannot sign for a new account)");
  if (!profile.delivery?.includes("cosign")) refused.push("5 (cosign is not offered for it)");
  if (!profile.delivery?.includes("return")) refused.push("6 (return is not offered)");
  if ((profile.maxTransactions ?? 1) < 2) refused.push("7 (one transaction at most)");
  if (!profile.messages?.includes("signature")) refused.push("10 in signature mode (no key signs messages; memo works)");
  return (
    `${whatHappens(profile)}` +
    (refused.length ? ` Expect a refusal on row ${refused.join(", row ")}.` : "") +
    " Rows 8 and 9, and row 7 with atomic, pass when Airlock refuses them" +
    (refused.length ? "." : "; every other row should go through.")
  );
}

/* 2. Send a memo */

export function memoRow(): Row {
  return {
    n: 2,
    key: "memo",
    title: "Send a memo",
    summary: "The smallest request: one memo your account signs and pays for. Airlock composes, signs and sends it.",
    intents: true,
    async run(ctx, out) {
      const account = key(ctx.account);
      const text = memoText("a memo");
      const transaction = await build(ctx.connection, account, [memo(text, [account])]);
      out.request({
        summary: `One transaction, delivery send, with no notes. ${SELF_PAYS}`,
        transactions: [`Memo "${text}", signed by your account`],
        delivery: "send",
        names: names(ctx),
      });
      const resolution = await resolve(ctx, out, "Send a memo", { transactions: [transaction], delivery: "send" });
      const ended = await toTheEnd(ctx, out, resolution);
      out.verdict(ended.state === "done", ended.state === "done" ? "The memo is on chain." : `It ended ${ended.state}.`);
    },
  };
}

/* 3. Transfer SOL */

export function transferRow(): Row {
  return {
    n: 3,
    key: "transfer",
    title: "Transfer SOL",
    summary: "0.001 SOL from your account to the app key.",
    intents: true,
    async run(ctx, out) {
      const account = key(ctx.account);
      const lamports = LAMPORTS_PER_SOL / 1000;
      const transaction = await build(ctx.connection, account, [
        SystemProgram.transfer({ fromPubkey: account, toPubkey: ctx.appKey.publicKey, lamports }),
      ]);
      out.request({
        summary: `One System transfer, delivery send, with no notes. ${SELF_PAYS}`,
        transactions: [`Transfer 0.001 SOL from your account to the app key`],
        delivery: "send",
        names: names(ctx),
      });
      const resolution = await resolve(ctx, out, "Transfer SOL", { transactions: [transaction], delivery: "send" });
      const ended = await toTheEnd(ctx, out, resolution);
      out.verdict(ended.state === "done", ended.state === "done" ? "The app key received 0.001 SOL." : `It ended ${ended.state}.`);
    },
  };
}

/* 4. Create a token mint and its token account */

export function mintRow(): Row {
  const kind = options("token", "Token program", [
    ["classic", "SPL Token: a classic mint"],
    ["token-2022", "Token-2022 with a metadata pointer"],
  ]);
  return {
    n: 4,
    key: "mint",
    title: "Create a token mint and its token account",
    summary:
      "One transaction: create a mint with a key this page makes, initialize it, create your account's token account for it, and mint 1000 to it. The wallet may swap the mint's key for one of its own, or for an address its carrier program signs for (a multisig's), and then derives the token account again. With Token-2022, the mint's own address is also written in an instruction's data, which a data note declares.",
    intents: true,
    controls: h("label", {}, "Mint", kind),
    // The mint is marked replaceable, delivery send: a key of the wallet's
    // own or its carrier program signs for it.
    expect: (profile) =>
      !signsForNewAccounts(profile)
        ? { reason: "this account cannot sign for a new account (newAccounts).", code: "unsupported", requirement: "newAccounts" }
        : null,
    async run(ctx, out) {
      if (kind.value === "token-2022") await mintToken2022(ctx, out);
      else await mintClassic(ctx, out);
    },
  };
}

async function mintClassic(ctx: Ctx, out: Out): Promise<void> {
  const account = key(ctx.account);
  const mint = Keypair.generate();
  const [ata] = associatedTokenAddress(account, mint.publicKey);
  const lamports = await ctx.connection.getMinimumBalanceForRentExemption(MINT_SIZE);
  // The page signs with the mint's key, as an app does today. The wallet
  // drops that signature if it puts its own key in the mint's place.
  const transaction = await build(
    ctx.connection,
    account,
    [
      createMintAccount(account, mint.publicKey, lamports),
      initializeMint2(mint.publicKey, 0, account),
      createAssociatedTokenAccount(account, ata, account, mint.publicKey),
      mintTo(mint.publicKey, ata, account, 1000n),
    ],
    [mint],
  );
  const mintAddress = mint.publicKey.toBase58();
  const ataAddress = ata.toBase58();
  const accounts: Record<string, AccountNote> = {
    [mintAddress]: { kind: "new", replaceable: true },
    [ataAddress]: {
      kind: "derived",
      pda: {
        program: ATA_PROGRAM.toBase58(),
        seeds: [{ account: ctx.account.address }, { account: TOKEN_PROGRAM.toBase58() }, { account: mintAddress }],
      },
    },
  };
  out.request({
    summary: `One transaction of four instructions, delivery send. ${SELF_PAYS}`,
    transactions: [
      "0. System CreateAccount: the new mint, 82 bytes, owned by SPL Token",
      "1. SPL Token InitializeMint2: 0 decimals, your account as mint authority",
      "2. Associated Token Account Create: your account's token account for the mint",
      "3. SPL Token MintTo: 1000 to that token account",
    ],
    delivery: "send",
    accounts,
    data: [],
    names: names(ctx, { [mintAddress]: "new mint", [ataAddress]: "token account", [ATA_PROGRAM.toBase58()]: "Associated Token Account", [TOKEN_PROGRAM.toBase58()]: "SPL Token" }),
    aside:
      "No data entries: neither address sits in any instruction's data. InitializeMint2 carries the mint authority, not the mint, and the Associated Token Account program derives the bump itself.",
  });
  const resolution = await resolve(ctx, out, "Create a mint", { transactions: [transaction], delivery: "send", accounts, data: [] });
  const ended = await toTheEnd(ctx, out, resolution);
  if (ended.state !== "done") {
    out.verdict(false, `It ended ${ended.state}.`);
    return;
  }
  const read = await readMintAndAccount(ctx, out, ended, mintAddress, ataAddress);
  if (!read) return;
  const holds = read.ata.mint === read.usedMint && read.mint.initialized;
  out.verdict(holds, holds ? "The mint and its token account exist." : "The token account does not belong to that mint.");
}

async function mintToken2022(ctx: Ctx, out: Out): Promise<void> {
  const account = key(ctx.account);
  const mint = Keypair.generate();
  const lamports = await ctx.connection.getMinimumBalanceForRentExemption(MINT_WITH_POINTER_SIZE);
  const request = token2022MintRequest(account, mint.publicKey, lamports);
  const transaction = await build(ctx.connection, account, request.instructions, [mint]);
  const mintAddress = mint.publicKey.toBase58();
  const ataAddress = request.ata.toBase58();
  out.request({
    summary: `One transaction of five instructions under Token-2022, delivery send. ${SELF_PAYS}`,
    transactions: [
      `0. System CreateAccount: the new mint, ${MINT_WITH_POINTER_SIZE} bytes (a mint with one extension), owned by Token-2022`,
      "1. Token-2022 InitializeMetadataPointer: your account may change the pointer, and the metadata is in the mint itself",
      "2. Token-2022 InitializeMint2: 0 decimals, your account as mint authority",
      "3. Associated Token Account Create: your account's Token-2022 token account for the mint",
      "4. Token-2022 MintTo: 1000 to that token account",
    ],
    delivery: "send",
    accounts: request.accounts,
    data: request.data,
    names: names(ctx, { [mintAddress]: "new mint", [ataAddress]: "token account", [TOKEN_2022_PROGRAM.toBase58()]: "Token-2022", [ATA_PROGRAM.toBase58()]: "Associated Token Account" }),
    aside:
      "The pointer names the mint, so the mint's address is in instruction 1's data, at byte 34 (after the two tags and the authority). The data entry tells the wallet to write its own key there too if it swaps the mint's. The token account's seeds give the Token-2022 program id as bytes.",
  });
  const resolution = await resolve(ctx, out, "Create a Token-2022 mint", {
    transactions: [transaction],
    delivery: "send",
    accounts: request.accounts,
    data: request.data,
  });
  const ended = await toTheEnd(ctx, out, resolution);
  if (ended.state !== "done") {
    out.verdict(false, `It ended ${ended.state}.`);
    return;
  }
  const read = await readMintAndAccount(ctx, out, ended, mintAddress, ataAddress);
  if (!read) return;
  const pointer = readMetadataPointer(read.mintData);
  if (!pointer) {
    out.verdict(false, "The mint has no metadata pointer.");
    return;
  }
  out.step(
    `Metadata pointer read from the chain: it names ${pointer.metadata === read.usedMint ? "the mint itself" : pointer.metadata === mintAddress ? "the mint this page made, not the one Airlock used" : "another account"}.`,
    pointer.metadata === read.usedMint ? "ok" : "danger",
    pointer.metadata ? address(pointer.metadata, "metadata") : undefined,
  );
  const holds = pointer.metadata === read.usedMint && read.ata.mint === read.usedMint && read.mint.initialized;
  const swapped = read.usedMint !== mintAddress;
  out.verdict(
    holds,
    holds
      ? swapped
        ? "The pointer names the mint Airlock made, so the data entry was written."
        : "The pointer names the mint. Airlock kept the page's key, so the data entry had nothing to rewrite."
      : "The pointer does not name the mint that was created.",
  );
}

/** Reads the mint and the token account a resolution made, by the addresses
 * it used. Says why and returns null when the node does not have them yet. */
async function readMintAndAccount(ctx: Ctx, out: Out, ended: Resolution, mintAddress: string, ataAddress: string) {
  const usedMint = ended.replaced?.[mintAddress] ?? mintAddress;
  const usedAta = ended.replaced?.[ataAddress] ?? ataAddress;
  if (usedMint === mintAddress) out.step("The wallet kept the page's mint key.", "neutral");
  const [mintInfo, ataInfo] = await ctx.connection.getMultipleAccountsInfo([new PublicKey(usedMint), new PublicKey(usedAta)]);
  if (!mintInfo || !ataInfo) {
    out.verdict(false, `The node does not have ${!mintInfo ? "the mint" : "the token account"} yet. Look it up on the explorer in a moment, or run the row again.`);
    return null;
  }
  const mintData = new Uint8Array(mintInfo.data);
  const mint = readMint(mintData);
  const ata = readTokenAccount(new Uint8Array(ataInfo.data));
  out.step(`Mint read from the chain: supply ${mint.supply}, ${mint.decimals} decimals.`, "ok", address(usedMint, "mint"));
  out.step(`Token account read from the chain: holds ${ata.amount} of that mint.`, "ok", address(usedAta, "token account"));
  return { usedMint, usedAta, mint, mintData, ata };
}

/* 5. Sponsored fee, cosign */

export function cosignRow(): Row {
  const replaceable = h("input", { type: "checkbox", name: "replaceable" });
  return {
    n: 5,
    key: "cosign",
    title: "Sponsored fee, cosign",
    summary:
      "The app key pays the fee. Airlock composes and you approve; then this page checks what came back, signs as the sponsor, and hands its signature over for Airlock to send.",
    intents: true,
    controls: h("label", { class: "check" }, replaceable, "The wallet may replace the sponsor (replaceable)"),
    expect: (profile) =>
      !profile.delivery?.includes("cosign")
        ? { reason: "cosign is not offered for this account.", code: "unsupported", requirement: "delivery" }
        : !replaceable.checked && !profile.feePayer?.includes("app")
          ? { reason: "this account does not take the app's fee payer.", code: "unsupported", requirement: "feePayer" }
          : null,
    async run(ctx, out) {
      const account = key(ctx.account);
      const sponsor = ctx.appKey.publicKey;
      const text = memoText("a memo whose fee the app pays");
      const transaction = await build(ctx.connection, sponsor, [memo(text, [account])]);
      // The sponsor is another address than the account: what a feePayer note is for.
      const accounts: Record<string, AccountNote> = {
        [sponsor.toBase58()]: { kind: "feePayer", replaceable: replaceable.checked, signs: "app" },
      };
      out.request({
        summary: "One memo, delivery cosign. The app key pays and signs after you approve.",
        transactions: [`Memo "${text}", signed by your account, fee paid by the app key`],
        delivery: "cosign",
        accounts,
        names: names(ctx),
      });
      const resolution = await resolve(ctx, out, "Sponsored fee", { transactions: [transaction], delivery: "cosign", accounts });
      const owed = await follow(ctx, out, resolution.id, ["awaiting-app", "sending"]);
      if (owed.state !== "awaiting-app") {
        if (owed.state === "sending") out.step("Airlock owes nothing to this page: it replaced the sponsor and sends on its own.", "info");
        const ended = await toTheEnd(ctx, out, owed);
        out.verdict(ended.state === "done", `It ended ${ended.state}.`);
        return;
      }
      const signatures: ProvideSignaturesInput["signatures"] = [];
      for (const awaiting of owed.awaiting ?? []) {
        // An index into the resolution's transactions, which is also what
        // provideSignatures counts by.
        const composed = owed.transactions[awaiting.transaction];
        if (!composed?.wire) throw new Error(`Airlock asked for signatures on transaction ${awaiting.transaction} but sent no wire.`);
        const wire = base64ToBytes(composed.wire);
        const verdict = compareComposed({
          sent: transaction,
          composed: wire,
          transaction: composed.of?.[0] ?? 0,
          account: ctx.account.address,
          notes: accounts,
          replaced: owed.replaced,
          role: composed.role,
        });
        out.step(`Compared with what was sent: ${verdict.detail}`, verdict.verdict === "same" ? "ok" : "danger");
        if (verdict.verdict !== "same") {
          await need(ctx).cancel(owed.id);
          out.verdict(false, "Not signed, and cancelled: the page signs only what it can check.");
          return;
        }
        const message = parseTransaction(wire).message;
        for (const signer of awaiting.signers) {
          if (signer !== sponsor.toBase58()) throw new Error(`Airlock asks for ${signer}, a key this page does not hold.`);
          signatures.push({ transaction: awaiting.transaction, signer, signature: await sign(ctx.appKey.secretKey, message) });
        }
        out.step(`Signed transaction ${awaiting.transaction} as the app key.`, "ok");
      }
      const after = await need(ctx).provide({ id: owed.id, signatures });
      out.step(`provideSignatures answered. ${stateLine(after)}`, stateTone(after.state));
      const ended = await toTheEnd(ctx, out, after);
      out.verdict(ended.state === "done", ended.state === "done" ? "Sent, with the app key as sponsor." : `It ended ${ended.state}.`);
    },
  };
}

/* 6. Return to the app */

export function returnRow(): Row {
  return {
    n: 6,
    key: "return",
    title: "Return to the app",
    summary:
      "A memo the app key must sign too. After you approve, Airlock hands the signed transaction back; this page checks it, adds its signature and sends it through its own node.",
    intents: true,
    expect: (profile) =>
      !profile.delivery?.includes("return") ? { reason: "return is not offered for this account.", code: "unsupported", requirement: "delivery" } : null,
    async run(ctx, out) {
      const account = key(ctx.account);
      const app = ctx.appKey.publicKey;
      const text = memoText("a memo the app signs too");
      const transaction = await build(ctx.connection, account, [memo(text, [account, app])]);
      const accounts: Record<string, AccountNote> = {
        [app.toBase58()]: { kind: "signer", signs: "app" },
      };
      out.request({
        summary: `One memo, delivery return. The app key is a required signer, so the transaction comes back to the page. ${SELF_PAYS}`,
        transactions: [`Memo "${text}", signed by your account and the app key`],
        delivery: "return",
        accounts,
        names: names(ctx),
      });
      const resolution = await resolve(ctx, out, "Return to the app", { transactions: [transaction], delivery: "return", accounts });
      const back = await follow(ctx, out, resolution.id, ["returned"]);
      if (back.state !== "returned") {
        report(out, back);
        out.verdict(false, `It ended ${back.state} without coming back.`);
        return;
      }
      // `awaiting` says which transactions the page owes signatures on, and whose.
      const owed = back.awaiting ?? [];
      if (owed.length === 0) {
        out.verdict(false, "Airlock returned the request but did not say which signatures it owes (awaiting is missing).");
        return;
      }
      let sent = 0;
      for (const awaiting of owed) {
        const composed = back.transactions[awaiting.transaction];
        if (!composed?.wire) throw new Error(`Airlock asked for signatures on transaction ${awaiting.transaction} but sent no wire.`);
        let wire = base64ToBytes(composed.wire);
        const verdict = compareComposed({
          sent: transaction,
          composed: wire,
          transaction: composed.of?.[0] ?? 0,
          account: ctx.account.address,
          notes: accounts,
          replaced: back.replaced,
          role: composed.role,
        });
        out.step(`Compared with what was sent: ${verdict.detail}`, verdict.verdict === "same" ? "ok" : "danger");
        if (verdict.verdict !== "same") {
          out.verdict(false, "Not signed and not sent: the page signs only what it can check. The wallet's signature is already out.");
          return;
        }
        for (const signer of awaiting.signers) {
          if (signer !== app.toBase58()) throw new Error(`Airlock asks for ${signer}, a key this page does not hold.`);
          wire = withSignature(wire, signer, await sign(ctx.appKey.secretKey, parseTransaction(wire).message));
        }
        const missing = missingSigners(parseTransaction(wire));
        if (missing.length) throw new Error(`Transaction ${awaiting.transaction} still lacks a signature from ${missing.join(", ")}.`);
        out.step(`Signed transaction ${awaiting.transaction} as the app key. Sending it through ${ctx.network.url}.`, "info");
        const signature = await ctx.connection.sendRawTransaction(wire);
        out.signature("Sent by the page", signature);
        await confirm(ctx.connection, signature);
        out.step("Confirmed.", "ok");
        sent += 1;
      }
      out.verdict(sent > 0, "Signed and sent by the page. Airlock watches the chain for it.");
    },
  };
}

/* 7. Two transactions, in order */

export function orderRow(): Row {
  const order = options("order", "Order", [
    ["serial", "serial: the second runs once the first succeeded"],
    ["any", "any: each stands alone"],
    ["atomic", "atomic: both in one transaction (refused by the prototype)"],
  ]);
  return {
    n: 7,
    key: "order",
    title: "Two transactions, in order",
    summary: "Two memos in one request, so one approval covers both. Choose atomic to see the prototype refuse it.",
    intents: true,
    controls: h("label", {}, "Order", order),
    // Atomic is a refusal the row checks itself.
    note: "With atomic, this row passes when Airlock refuses it (unsupported, order): the prototype does not run several transactions as one.",
    expect: (profile) =>
      order.value !== "atomic" && (profile.maxTransactions ?? 1) < 2
        ? { reason: "this account takes one transaction at a time (maxTransactions).", code: "unsupported", requirement: "order" }
        : null,
    async run(ctx, out) {
      const account = key(ctx.account);
      const chosen = order.value as Order;
      const texts = [memoText("memo 1 of 2"), memoText("memo 2 of 2")];
      const transactions: Uint8Array[] = [];
      for (const text of texts) transactions.push(await build(ctx.connection, account, [memo(text, [account])]));
      out.request({
        summary: `Two transactions, order ${chosen}, delivery send, with no notes. ${SELF_PAYS}`,
        transactions: texts.map((text, i) => `${i}. Memo "${text}"`),
        delivery: "send",
        order: chosen,
        names: names(ctx),
      });
      const send = () => resolve(ctx, out, `Two transactions (${chosen})`, { transactions, order: chosen, delivery: "send" });
      if (chosen === "atomic") {
        await expectRefusal(ctx, out, "unsupported", "the prototype does not run several transactions as one.", send);
        return;
      }
      const ended = await toTheEnd(ctx, out, await send());
      out.verdict(ended.state === "done", ended.state === "done" ? "Both ran." : `It ended ${ended.state}.`);
    },
  };
}

/* 8. Notes that lie */

export function lyingRow(): Row {
  return {
    n: 8,
    key: "lying",
    title: "Notes that lie",
    summary:
      "A derived note whose seeds give another address than the one in the transaction. The wallet checks every note before it uses it, so it must refuse with bad_notes. A refusal is a pass.",
    intents: true,
    note: "This row passes when Airlock refuses it with bad_notes.",
    async run(ctx, out) {
      const account = key(ctx.account);
      const encoder = new TextEncoder();
      const [real] = PublicKey.findProgramAddressSync([encoder.encode("airlock-intents"), encoder.encode("real")], MEMO_PROGRAM);
      const [claimed] = PublicKey.findProgramAddressSync([encoder.encode("airlock-intents"), encoder.encode("claimed")], MEMO_PROGRAM);
      const transaction = await build(ctx.connection, account, [
        memo(memoText("notes that lie"), [account]),
        SystemProgram.transfer({ fromPubkey: account, toPubkey: real, lamports: 0 }),
      ]);
      const accounts: Record<string, AccountNote> = {
        [real.toBase58()]: {
          kind: "derived",
          pda: { program: MEMO_PROGRAM.toBase58(), seeds: [{ bytes: encoder.encode("airlock-intents") }, { bytes: encoder.encode("claimed") }] },
        },
      };
      out.request({
        summary: `A memo and a transfer of 0 lamports. The note says the recipient derives from the seeds "airlock-intents" and "claimed", which give ${claimed.toBase58()}, not the address in the transaction.`,
        transactions: ["0. Memo, signed by your account", `1. Transfer 0 lamports to ${real.toBase58()}`],
        delivery: "send",
        accounts,
        names: names(ctx, { [real.toBase58()]: "recipient" }),
        aside: "Seed bytes go to the wallet as base64.",
      });
      await expectRefusal(ctx, out, "bad_notes", "the derivation gives another address (check 2).", () =>
        resolve(ctx, out, "Notes that lie", { transactions: [transaction], delivery: "send", accounts }),
      );
    },
  };
}

/* 9. A note on the account itself */

export function accountNoteRow(): Row {
  return {
    n: 9,
    key: "account-note",
    title: "A note on the account itself",
    summary:
      "The memo of row 2, with a feePayer note on your account. The account acted as carries no note: who pays its fee is already the wallet's to choose. So the wallet must refuse with bad_notes. A refusal is a pass.",
    intents: true,
    note: "This row passes when Airlock refuses it with bad_notes.",
    async run(ctx, out) {
      const account = key(ctx.account);
      const text = memoText("a note on the account itself");
      const transaction = await build(ctx.connection, account, [memo(text, [account])]);
      const accounts: Record<string, AccountNote> = { [ctx.account.address]: { kind: "feePayer", replaceable: true } };
      out.request({
        summary: "One memo, delivery send, with a feePayer note filed under your account, as an older draft of the proposal asked.",
        transactions: [`Memo "${text}", signed by your account`],
        delivery: "send",
        accounts,
        names: names(ctx),
        aside: "Check 6: an address carries one note, and the account acted as appears under no key of accounts.",
      });
      await expectRefusal(ctx, out, "bad_notes", "the account acted as carries no note (check 6).", () =>
        resolve(ctx, out, "A note on the account", { transactions: [transaction], delivery: "send", accounts }),
      );
    },
  };
}

/* 10. Sign a message */

export function messageRow(): Row {
  const mode = options("mode", "Mode", [
    ["", "the wallet's choice"],
    ["signature", "signature: a key signs the bytes"],
    ["memo", "memo: a transaction carries the message"],
  ]);
  return {
    n: 10,
    key: "message",
    title: "Sign a message",
    summary:
      "A key signs the message, or, for an account with no key such as a multisig, a transaction carries it as a memo. Either way the page checks the proof itself.",
    intents: true,
    controls: h("label", {}, "Mode", mode),
    expect: (profile) =>
      mode.value === "signature" && !profile.messages?.includes("signature")
        ? { reason: "no key signs a message as this account; choose memo or the wallet's choice.", code: "unsupported", requirement: "messages" }
        : mode.value === "memo" && !profile.messages?.includes("memo")
          ? { reason: "this account does not sign messages as memos.", code: "unsupported", requirement: "messages" }
          : null,
    async run(ctx, out) {
      const nonce = [...crypto.getRandomValues(new Uint8Array(8))].map((b) => b.toString(16).padStart(2, "0")).join("");
      const text = `${location.host} asks you to sign this message to test Airlock's intents.\nNonce: ${nonce}\nIssued at: ${new Date().toISOString()}`;
      const message = new TextEncoder().encode(text);
      const chosen = (mode.value || undefined) as "signature" | "memo" | undefined;
      out.request({
        summary: `airlock:signMessage, mode ${chosen ?? "left to the wallet"}, chain ${ctx.network.chain}. The message names this site and a nonce, since anyone can show a proof again. A memo is a transaction, so the page names the chain it is for, as in any request.`,
        transactions: [text],
        names: names(ctx),
        raw: { mode: chosen, chain: ctx.network.chain, message: text },
      });
      out.step("Sent. Approve or decline it in Airlock.", "info");
      const output: SignMessageOutput = await need(ctx).signMessage("Sign a message", {
        id: newId(),
        account: ctx.account,
        message,
        mode: chosen,
        chain: ctx.network.chain,
      });
      if (output.kind === "signature") {
        const same = output.signedMessage.length === message.length && output.signedMessage.every((b, i) => b === message[i]);
        if (!same) out.step("The wallet signed the message wrapped in Solana's offchain format, as a hardware wallet does.", "info");
        out.step(`Signature ${encodeBase58(output.signature)}`, "neutral");
        let valid = false;
        try {
          valid = await verify(ctx.account.publicKey as Uint8Array, output.signedMessage, output.signature);
        } catch (err) {
          out.verdict(false, `This browser cannot check an ed25519 signature: ${err instanceof Error ? err.message : err}`);
          return;
        }
        out.verdict(valid, valid ? "The signature verifies for your account (ed25519)." : "The signature does not verify for your account.");
        return;
      }
      const walletChain = output.resolution.chain;
      const sameNetwork = walletChain === ctx.network.chain;
      out.step(
        `Signed as a memo: resolution ${output.resolution.id}, on ${walletChain}. ${stateLine(output.resolution)}`,
        stateTone(output.resolution.state),
      );
      if (!sameNetwork) {
        out.step(`The page asked for ${ctx.network.chain}, and Airlock put it on ${walletChain}.`, "danger");
      }
      const ended = await toTheEnd(ctx, out, output.resolution);
      const proof = ended.ran?.[0]?.signature;
      if (ended.state !== "done" || !proof) {
        out.verdict(false, `It ended ${ended.state}, so there is no proof to read.`);
        return;
      }
      if (!sameNetwork) {
        out.verdict(
          false,
          `The memo is on ${walletChain}, though the page named ${ctx.network.chain}: Airlock should resolve it on the chain the request names.`,
        );
        return;
      }
      out.step("Reading the transaction from the node, to check it.", "info");
      let proven: MemoProof | null = null;
      for (let attempt = 0; attempt < 10 && !proven; attempt += 1) {
        proven = await proveMemo(rpcFor(ctx.network), proof, ctx.account.address, message);
        if (!proven) await sleep(1500);
      }
      if (!proven) {
        out.verdict(false, "The node does not have the transaction yet. Look it up on the explorer.");
        return;
      }
      if (proven.verdict === "unchecked") {
        out.step(
          proven.succeeded === true ? "The transaction succeeded." : proven.succeeded === false ? "The transaction failed." : "The node gave no status.",
          proven.succeeded === true ? "ok" : proven.succeeded === false ? "danger" : "warn",
        );
        out.verdict(proven.succeeded === false ? false : null, proven.detail);
        return;
      }
      if (proven.version === 1) out.step("The node showed it only when asked for version 1 transactions (maxSupportedTransactionVersion 1).", "neutral");
      out.verdict(proven.verdict === "holds", proven.detail);
    },
  };
}

/* 11. Fallback */

export function fallbackRow(): Row {
  return {
    n: 11,
    key: "fallback",
    title: "Fallback: signAndSendTransaction",
    summary:
      "The memo of row 2 through solana:signAndSendTransaction, with no notes. A wallet without the new methods signs the same bytes; it just cannot be told what it may change.",
    intents: false,
    async run(ctx, out) {
      const feature = ctx.wallet.features["solana:signAndSendTransaction"] as
        | { signAndSendTransaction(...inputs: unknown[]): Promise<{ signature: Uint8Array }[]> }
        | undefined;
      if (!feature) throw new Error("This wallet has no solana:signAndSendTransaction either.");
      const account = key(ctx.account);
      const text = memoText("a memo through the fallback");
      const transaction = await build(ctx.connection, account, [memo(text, [account])]);
      out.request({
        summary: "The same kind of transaction as row 2, sent the way every wallet takes it today.",
        transactions: [`Memo "${text}", signed by your account`],
        names: names(ctx),
        raw: { chain: ctx.network.chain, transaction: bytesToBase64(transaction) },
      });
      ctx.log.add("call", "solana:signAndSendTransaction", { chain: ctx.network.chain, bytes: transaction.length });
      out.step("Sent. Approve or decline it in the wallet.", "info");
      const [output] = await feature.signAndSendTransaction({ account: ctx.account, transaction, chain: ctx.network.chain });
      const signature = encodeBase58(output.signature);
      ctx.log.add("reply", "solana:signAndSendTransaction", { signature });
      out.signature("Sent by the wallet", signature);
      await confirm(ctx.connection, signature);
      out.verdict(true, "Confirmed. The wallet signed the same bytes, with nothing declared.");
    },
  };
}

export function allRows(): Row[] {
  return [
    profileRow(),
    memoRow(),
    transferRow(),
    mintRow(),
    cosignRow(),
    returnRow(),
    orderRow(),
    lyingRow(),
    accountNoteRow(),
    messageRow(),
    fallbackRow(),
  ];
}
