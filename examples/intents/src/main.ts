/** The intents test page: settings at the top, a row per method, the
 * resolutions this page started, and a log of everything said. */

import { transact } from "@solana-mobile/mobile-wallet-adapter-protocol";
import { LAMPORTS_PER_SOL, PublicKey, type Connection, type Keypair } from "@solana/web3.js";
import { getWallets } from "@wallet-standard/app";
import type { Wallet, WalletAccount } from "@wallet-standard/base";
import { registerWallet } from "@wallet-standard/wallet";
import {
  CANCEL_RESOLUTION,
  GET_RESOLUTION,
  GET_SIGNING_PROFILE,
  PROVIDE_SIGNATURES,
  RESOLVE,
  SIGN_MESSAGE,
  type AccountNote,
  type IntentFeatures,
  type Resolution,
  type SigningProfile,
} from "../../../extension/src/intents";
import { base64ToBytes, encodeBase58 } from "../../../extension/src/protocol";
import {
  allRows,
  asRefusal,
  describeError,
  refusalMatches,
  stateTone,
  whatHappens,
  type Api,
  type Ctx,
  type Out,
  type Refusal,
  type RequestView,
  type Row,
} from "./actions";
import { loadAppKey, newAppKey } from "./app-key";
import { registerRelayWallet, relayFromQuery } from "./dev-transport";
import { EventLog, line } from "./log";
import { MwaTransport, createMwaWallet, hasUserActivation, isAndroid, type MwaAuth, type MwaStandardWallet, type Transact } from "./mwa";
import { DEVNET, LOCAL_URL, MAINNET, confirm, connectionFor, explorerUrl, isChain, loadNetwork, saveNetwork, type Network } from "./network";
import { ResolutionStore, type Tracked } from "./resolutions";
import { forget, load, save } from "./storage";
import { address, append, chip, copyButton, h, link, replace, short, time, toast, type Tone } from "./ui";

const log = new EventLog();
const store = new ResolutionStore();

const state = {
  wallets: [] as Wallet[],
  wallet: null as Wallet | null,
  account: null as WalletAccount | null,
  network: loadNetwork(),
  connection: null as unknown as Connection,
  appKey: loadAppKey() as Keypair,
  profile: null as SigningProfile | null,
  api: null as Api | null,
  relayWallet: null as Wallet | null,
  /** Airlock on this phone, through Mobile Wallet Adapter. */
  mwaWallet: null as MwaStandardWallet | null,
  stopListening: [] as (() => void)[],
};

/** Whether the chosen wallet is the one reached through Mobile Wallet Adapter. */
function isMwa(): boolean {
  return state.wallet !== null && state.wallet === state.mwaWallet;
}

/** Whether a call may go to the wallet now without a tap: always, but over
 * MWA only while a session is open or a tap is fresh. A browser opens a new
 * session only on a tap. */
function canCallNow(): boolean {
  return !isMwa() || state.mwaWallet!.transport.openSession !== null || hasUserActivation();
}
state.connection = connectionFor(state.network);

const rows = allRows();
const rowViews = new Map<string, { run: HTMLButtonElement; why: HTMLElement; expect: HTMLElement; request: HTMLElement; result: HTMLOListElement }>();

/* Wallets */

function intentsOf(wallet: Wallet | null): IntentFeatures | null {
  if (!wallet) return null;
  const features = wallet.features as Partial<IntentFeatures>;
  return features[RESOLVE] ? (features as IntentFeatures) : null;
}

function usable(wallet: Wallet): boolean {
  return "standard:connect" in wallet.features && (RESOLVE in wallet.features || "solana:signAndSendTransaction" in wallet.features);
}

/** The page's calls to the wallet, logged, with every resolution kept. */
function makeApi(features: IntentFeatures): Api {
  const logged = async <T>(name: string, params: unknown, call: () => Promise<T>): Promise<T> => {
    log.add("call", name, params);
    try {
      const result = await call();
      log.add("reply", name, result);
      return result;
    } catch (err) {
      log.add("error", `${name}: ${describeError(err)}`);
      throw err;
    }
  };
  const keep = (resolution: Resolution) => {
    store.update(resolution);
    return resolution;
  };
  const kept = (err: unknown, id: string) => {
    const resolution = (err as { resolution?: Resolution }).resolution;
    if (resolution) store.update(resolution);
    else store.fail(id, describeError(err));
  };
  return {
    profile: (account) => logged("getSigningProfile", { account: account.address }, () => features[GET_SIGNING_PROFILE].getSigningProfile({ account })),
    async resolve(label, input) {
      store.start(input.id, label, input.account.address);
      try {
        return keep(await logged("resolve", { ...input, transactions: input.transactions.map((tx) => `<${tx.length} bytes>`) }, () => features[RESOLVE].resolve(input)));
      } catch (err) {
        kept(err, input.id);
        throw err;
      }
    },
    get: async (id) => keep(await logged("getResolution", { id }, () => features[GET_RESOLUTION].getResolution({ id }))),
    provide: async (input) => keep(await logged("provideSignatures", input, () => features[PROVIDE_SIGNATURES].provideSignatures(input))),
    cancel: async (id) => keep(await logged("cancelResolution", { id }, () => features[CANCEL_RESOLUTION].cancelResolution({ id }))),
    async signMessage(label, input) {
      store.start(input.id, label, input.account.address);
      try {
        const output = await logged("signMessage", { ...input, message: new TextDecoder().decode(input.message) }, () => features[SIGN_MESSAGE].signMessage(input));
        if (output.kind === "memo") keep(output.resolution);
        else store.remove(input.id);
        return output;
      } catch (err) {
        kept(err, input.id);
        throw err;
      }
    },
  };
}

function selectWallet(wallet: Wallet | null): void {
  for (const stop of state.stopListening) stop();
  state.stopListening = [];
  state.wallet = wallet;
  state.account = null;
  state.profile = null;
  const features = intentsOf(wallet);
  state.api = features ? makeApi(features) : null;
  // Over MWA the store does not read on its own: each read would open a
  // session. The transport asks in a session it keeps open, and Check asks in a
  // new one.
  store.refresher = state.api && !isMwa() ? (id) => state.api!.get(id) : null;
  if (wallet) {
    save("wallet", wallet.name);
    const events = wallet.features["standard:events"] as { on(event: "change", listener: () => void): () => void } | undefined;
    if (events) state.stopListening.push(events.on("change", () => accountsChanged()));
    if (features) {
      state.stopListening.push(
        features[RESOLVE].onResolution((resolution) => {
          // Over MWA the transport logged the reply it came in.
          if (!isMwa()) log.add("push", "resolutionChanged", resolution);
          store.update(resolution);
        }),
      );
    }
  }
  render();
}

async function connect(silent: boolean): Promise<void> {
  const wallet = state.wallet;
  if (!wallet) return;
  const feature = wallet.features["standard:connect"] as { connect(input?: { silent?: boolean }): Promise<{ accounts: readonly WalletAccount[] }> };
  log.add("call", `standard:connect${silent ? " (silent)" : ""}`);
  try {
    const { accounts } = await feature.connect({ silent });
    log.add("reply", "standard:connect", accounts.map((account) => account.address));
    if (!silent && accounts.length === 0) toast("The wallet connected no account.", "warn");
    // Over MWA the airlock: features are known once get_capabilities answered.
    if (wallet === state.mwaWallet && !!intentsOf(wallet) !== !!state.api) selectWallet(wallet);
    accountsChanged();
    // A silent connect over MWA opened no session, and reading would need one.
    if (canCallNow()) void refreshOpen();
  } catch (err) {
    log.add("error", `standard:connect: ${describeError(err)}`);
    if (!silent) toast(`Not connected: ${describeError(err)}`, "danger");
  }
}

async function disconnect(): Promise<void> {
  const feature = state.wallet?.features["standard:disconnect"] as { disconnect(): Promise<void> } | undefined;
  log.add("call", "standard:disconnect");
  await feature?.disconnect().catch((err) => log.add("error", describeError(err)));
  accountsChanged();
}

function accountsChanged(): void {
  const accounts = state.wallet?.accounts ?? [];
  const wanted = state.account?.address ?? load<string>("account");
  const next = accounts.find((account) => account.address === wanted) ?? accounts[0] ?? null;
  const changed = next?.address !== state.account?.address;
  state.account = next;
  if (changed) {
    state.profile = null;
    if (next) save("account", next.address);
  }
  render();
  if (changed && next && state.api && canCallNow()) void runRow(rows[0]);
}

/* Network and the app key */

function setNetwork(network: Network): void {
  state.network = network;
  state.connection = connectionFor(network);
  saveNetwork(network);
  render();
  void pingNode(els.nodeStatus);
}

async function balanceOf(target: PublicKey, into: HTMLElement): Promise<void> {
  replace(into, "…");
  try {
    const lamports = await state.connection.getBalance(target);
    replace(into, `${(lamports / LAMPORTS_PER_SOL).toLocaleString(undefined, { maximumFractionDigits: 6 })} SOL`);
  } catch (err) {
    replace(into, chip("no answer", "danger"));
    log.add("error", `getBalance: ${describeError(err)}`);
  }
}

async function airdrop(target: PublicKey, into: HTMLElement): Promise<void> {
  log.add("call", "requestAirdrop", { to: target.toBase58(), sol: 1 });
  try {
    const signature = await state.connection.requestAirdrop(target, LAMPORTS_PER_SOL);
    toast("Airdrop sent.", "info", { href: explorerUrl(state.network, "tx", signature), label: "View on explorer" });
    await confirm(state.connection, signature);
    log.add("reply", "requestAirdrop", { signature });
    toast("Airdrop confirmed: 1 SOL.", "ok");
  } catch (err) {
    log.add("error", `requestAirdrop: ${describeError(err)}`);
    toast(`The airdrop did not go through: ${describeError(err)}. A public faucet limits how often it gives.`, "danger");
  }
  void balanceOf(target, into);
}

async function pingNode(into: HTMLElement): Promise<void> {
  replace(into, chip("asking", "neutral"));
  try {
    const slot = await state.connection.getSlot();
    replace(into, chip("answered", "ok"), ` slot ${slot.toLocaleString()}`);
  } catch (err) {
    replace(into, chip("no answer", "danger"), ` ${describeError(err)}`);
  }
}

/* Rows */

function rowBlocked(row: Row): string | null {
  if (!state.wallet) return "Choose a wallet first.";
  if (!state.account) return "Connect an account first.";
  if (row.intents && !state.api) return "Needs airlock:resolve, which this wallet does not have.";
  return null;
}

function renderExpectation(row: Row): void {
  const view = rowViews.get(row.key);
  if (!view) return;
  const expected = state.profile && row.expect ? row.expect(state.profile) : null;
  replace(
    view.expect,
    expected
      ? h("p", { class: "warn" }, `Refusal expected: ${expected.reason} It passes when Airlock answers ${expected.code}${expected.requirement ? ` (${expected.requirement})` : ""}.`)
      : row.note
        ? h("p", { class: "note" }, row.note)
        : null,
  );
}

async function runRow(row: Row): Promise<void> {
  const view = rowViews.get(row.key);
  if (!view || !state.wallet || !state.account) return;
  const blocked = rowBlocked(row);
  if (blocked) return;
  const ctx: Ctx = {
    wallet: state.wallet,
    account: state.account,
    network: state.network,
    connection: state.connection,
    appKey: state.appKey,
    api: state.api,
    profile: state.profile,
    store,
    log,
    setProfile(profile) {
      state.profile = profile;
      for (const each of rows) renderExpectation(each);
      renderAccountProfile();
    },
  };
  replace(view.request);
  replace(view.result);
  view.run.disabled = true;
  view.run.textContent = "Running…";
  const out: Out = {
    request: (request) => replace(view.request, requestView(request)),
    step: (text, tone = "neutral", extra) => append(view.result, step(text, tone, extra)),
    signature(label, signature) {
      const href = explorerUrl(ctx.network, "tx", signature);
      append(view.result, step(`${label}: `, "ok", h("span", {}, h("code", { title: signature }, short(signature)), " ", link(href, "View on explorer"))));
      toast(`${row.title}: ${label.toLowerCase()}.`, "ok", { href, label: "View on explorer" });
    },
    verdict(pass, text, refusal) {
      // A row the profile says will be refused passes only by that refusal.
      if (expected && !refusal) {
        text = `The profile said Airlock would refuse this (${expected.code}${expected.requirement ? `, ${expected.requirement}` : ""}): ${expected.reason} It took it instead. ${text}`;
        pass = false;
      }
      const [word, tone] = pass === null ? ["not checked", "warn"] : pass ? ["pass", "ok"] : ["not as hoped", "danger"];
      append(
        view.result,
        h(
          "li",
          { class: `verdict ${tone}`, "data-pass": String(pass) },
          chip(word, tone as Tone),
          h("div", { class: "verdict-body" }, h("span", {}, text), refusal ? refusalView(refusal) : null),
        ),
      );
    },
    node: (node) => append(view.result, h("li", { class: "step plain" }, node)),
  };
  // What the profile says this row meets, as it stood when the row began.
  const expected = state.profile && row.expect ? row.expect(state.profile) : null;
  try {
    await row.run(ctx, out);
  } catch (err) {
    const refusal = asRefusal(err);
    if (refusal && expected) {
      const matches = refusalMatches(expected, refusal);
      out.verdict(
        matches,
        matches
          ? `Refused, as the profile said: ${expected.reason}`
          : `Refused, but not as the profile said: it should be ${expected.code}${expected.requirement ? ` (${expected.requirement})` : ""}, because ${expected.reason}`,
        refusal,
      );
    } else if (refusal) {
      out.verdict(false, "Airlock refused the request.", refusal);
    } else {
      out.verdict(false, describeError(err));
      toast(`${row.title}: ${describeError(err)}`, "danger");
    }
  } finally {
    view.run.disabled = false;
    view.run.textContent = "Run";
    refreshBalances(true);
  }
}

/** A refusal's code, its requirement when there is one, and the wallet's words. */
function refusalView(refusal: Refusal): HTMLElement {
  return h(
    "dl",
    { class: "refusal" },
    h("dt", {}, "Code"),
    h("dd", {}, h("code", {}, refusal.code)),
    refusal.requirement ? h("dt", {}, "Requirement") : null,
    refusal.requirement ? h("dd", {}, h("code", {}, refusal.requirement)) : null,
    h("dt", {}, "Airlock said"),
    h("dd", {}, refusal.message || "nothing more"),
  );
}

function step(text: string, tone: Tone, extra?: Node): HTMLLIElement {
  return h("li", { class: `step ${tone}` }, h("time", {}, time()), h("span", { class: "dot", "aria-hidden": "true" }), h("span", { class: "text" }, text, extra ?? null));
}

function requestView(view: RequestView): HTMLElement {
  const notes = Object.entries(view.accounts ?? {});
  const raw = view.raw ?? {
    delivery: view.delivery,
    order: view.order,
    accounts: view.accounts,
    data: view.data,
  };
  return h(
    "div",
    { class: "request-view" },
    h("p", {}, view.summary),
    view.transactions.length ? h("ul", { class: "plain-list" }, ...view.transactions.map((text) => h("li", {}, text))) : null,
    view.delivery || view.order
      ? h("div", { class: "chip-row" }, view.delivery ? chip(`delivery ${view.delivery}`, "kind") : null, view.order ? chip(`order ${view.order}`, "kind") : null)
      : null,
    notes.length
      ? h(
          "dl",
          { class: "notes" },
          ...notes.flatMap(([addr, note]) => [h("dt", {}, address(addr, view.names[addr])), h("dd", {}, describeNote(note, view.names))]),
        )
      : null,
    view.data?.length
      ? h(
          "ul",
          { class: "plain-list" },
          ...view.data.map((entry) =>
            h(
              "li",
              {},
              `data: transaction ${entry.transaction}, instruction ${entry.instruction}, byte ${entry.offset}: `,
              "account" in entry ? `the address of ${view.names[entry.account] ?? short(entry.account)}` : `the bump of ${view.names[entry.bumpOf] ?? short(entry.bumpOf)}`,
            ),
          ),
        )
      : null,
    view.aside ? h("p", { class: "aside" }, view.aside) : null,
    h("details", {}, h("summary", {}, "As JSON"), h("pre", {}, JSON.stringify(raw, (_k, v) => (v instanceof Uint8Array ? `<${v.length} bytes>` : v), 2))),
  );
}

function describeNote(note: AccountNote, names: Record<string, string>): string {
  const named = (value: string) => names[value] ?? short(value);
  switch (note.kind) {
    case "new":
      return note.replaceable ? "new, replaceable: the wallet may put a key of its own here" : "new, not replaceable: the app signs for it";
    case "derived":
      if ("pda" in note) {
        const seeds = note.pda.seeds.map((seed) => ("account" in seed ? named(seed.account) : seedText(seed.bytes, names)));
        return `derived: an address of program ${named(note.pda.program)} from ${seeds.join(", ")}`;
      }
      return `derived with seed "${note.withSeed.seed}" from ${named(note.withSeed.base)}`;
    case "feePayer":
      return `fee payer, ${note.replaceable ? "replaceable" : "not replaceable"}${note.signs === "app" ? ", the app's own key (a sponsor)" : ""}`;
    case "signer":
      return "signer: the app signs the result";
  }
}

/** A seed of bytes: the address it is when the page named it, its text when
 * it reads as text, and otherwise its length. */
function seedText(bytes: string | Uint8Array, names: Record<string, string>): string {
  const raw = typeof bytes === "string" ? base64ToBytes(bytes) : bytes;
  if (raw.length === 32) {
    const name = names[encodeBase58(raw)];
    if (name) return `the ${name} program id, as bytes`;
  }
  const text = new TextDecoder().decode(raw);
  return /^[\x20-\x7e]*$/.test(text) ? `"${text}"` : `${raw.length} bytes`;
}

/* Resolutions */

async function refreshOpen(): Promise<void> {
  const api = state.api;
  if (!api) return;
  const open = store.open();
  const readAll = async () => {
    // One at a time, well inside the extension's limit of 30 requests in 10 seconds.
    for (const item of open) {
      try {
        await api.get(item.id);
      } catch (err) {
        store.readFailed(item.id, err, describeError);
      }
      if (!isMwa()) await new Promise((resolve) => setTimeout(resolve, 400));
    }
  };
  if (!isMwa()) return readAll();
  // Over MWA, every read in one session, so Airlock comes forward once.
  if (open.length === 0) return;
  try {
    await state.mwaWallet!.transport.batch(readAll);
  } catch (err) {
    log.add("error", `Check: ${describeError(err)}`);
    toast(`Not checked: ${describeError(err)}`, "danger");
  }
}

/** Over MWA: asks for one resolution in a session of its own, or in the open one. */
function check(id: string): void {
  void state.api?.get(id).catch((err) => {
    store.readFailed(id, err, describeError);
    toast(`Not checked: ${describeError(err)}`, "danger");
  });
}

function resolutionRow(item: Tracked): HTMLTableRowElement {
  const r = item.resolution;
  const gone = item.notFound !== undefined;
  const cancellable = !gone && r && (r.state === "awaiting-approval" || r.state === "awaiting-app");
  const actions = h("div", { class: "row-buttons" });
  if (state.api && !gone && (r || !item.error)) {
    actions.append(
      isMwa()
        ? h("button", { class: "sm", type: "button", title: "Asks Airlock for it in a new session. Airlock comes forward for a moment.", onclick: () => check(item.id) }, "Check")
        : h("button", { class: "sm", type: "button", onclick: () => state.api?.get(item.id).catch((err) => store.readFailed(item.id, err, describeError)) }, "Refresh"),
    );
    if (cancellable) {
      actions.append(
        h("button", { class: "sm", type: "button", onclick: () => state.api?.cancel(item.id).catch((err) => toast(`Not cancelled: ${describeError(err)}`, "danger")) }, "Cancel"),
      );
    }
  }
  // A row Airlock no longer has, or that never became a resolution, can only be forgotten.
  if (gone || (!r && item.error)) {
    actions.append(h("button", { class: "sm ghost", type: "button", onclick: () => store.remove(item.id) }, "Forget"));
  }
  const txs = (r?.transactions ?? []).map((tx, i) =>
    h(
      "div",
      { class: "tx-line" },
      `${i}. ${tx.role}`,
      tx.of?.length ? ` for request's ${tx.of.join(", ")}` : "",
      " ",
      chip(tx.state, tx.state === "landed" ? "ok" : tx.state === "failed" ? "danger" : tx.state === "unknown" ? "warn" : "neutral"),
      tx.signature ? h("span", {}, " ", link(explorerUrl(state.network, "tx", tx.signature), short(tx.signature))) : null,
      tx.wire && !tx.signature ? h("span", { class: "muted" }, " wire held") : null,
    ),
  );
  return h(
    "tr",
    {},
    h("td", {}, h("div", {}, item.label), h("code", { class: "muted", title: item.id }, item.id), h("div", { class: "muted" }, time(item.startedAt))),
    h("td", { class: "nowrap" }, address(item.account)),
    h(
      "td",
      { class: "state-cell" },
      gone ? chip("not found", "warn") : r ? chip(r.state, stateTone(r.state)) : item.error ? chip("refused", "danger") : chip("sent", "neutral"),
      gone
        ? h(
            "div",
            { class: "muted" },
            `Airlock no longer has this resolution${r ? ` (last seen ${r.state})` : ""}. It keeps the latest 200 in its data folder, so an older one, or one made with another data folder, is gone.`,
          )
        : null,
      item.error ? h("div", { class: "muted" }, item.error) : null,
      r && !gone ? waitingFor(r) : null,
      r?.error ? h("div", { class: "muted" }, describeError(r.error)) : null,
      r ? h("div", { class: "muted" }, `updated ${time(r.updatedAt * 1000)}`) : null,
    ),
    h("td", {}, r ? `${r.timing} · ${r.delivery}` : ""),
    // Where each transaction sent in ran sits under what the wallet made: for a
    // multisig the two differ, and for a key they are the same.
    h(
      "td",
      {},
      ...(txs.length ? txs : [h("span", { class: "muted" }, r ? "none yet" : "none")]),
      ...(r?.ran ?? []).map((ran) =>
        h("div", { class: "tx-line" }, `Request's ${ran.transaction} ran in `, link(explorerUrl(state.network, "tx", ran.signature), short(ran.signature))),
      ),
    ),
    h(
      "td",
      {},
      ...Object.entries(r?.replaced ?? {}).map(([from, to]) => h("div", { class: "replaced-line" }, address(from), " → ", address(to))),
    ),
    h("td", {}, actions),
  );
}

/** For a resolution that waits on someone, who: the members of a multisig
 * whose proposal landed, another device, or this page. Null otherwise. */
function waitingFor(r: Resolution): HTMLElement | null {
  // On a phone the wallet cannot tell the page that something changed: the page asks.
  const then = isMwa() ? " On a phone Airlock cannot tell this page, so press Check to ask." : " This row turns done by itself then.";
  const text = (() => {
    switch (r.state) {
      case "awaiting-approval":
        return "Waiting for you to approve or decline it in Airlock.";
      case "awaiting-signatures": {
        const proposal = r.transactions?.find((tx) => tx.role === "proposal");
        if (proposal?.state === "landed") {
          return `Waiting for the multisig's other members: the proposal landed, and its instructions run once enough members approve and one executes it.${then}`;
        }
        return `Waiting for a signature from another device or other people. Airlock sends it once the signature comes back.${then}`;
      }
      case "awaiting-app":
        return "Waiting for this page's signatures (provideSignatures).";
      case "returned":
        return "Handed back to this page to sign and send.";
      default:
        return null;
    }
  })();
  return text ? h("div", { class: "waiting-for" }, text) : null;
}

/* Rendering */

const app = document.getElementById("app")!;
const els = {
  devBanner: h("div"),
  walletSelect: h("select", { id: "wallet", "aria-label": "Wallet" }),
  walletChips: h("span", { class: "chip-row" }),
  connect: h("button", { class: "primary", type: "button" }, "Connect"),
  disconnect: h("button", { class: "ghost", type: "button" }, "Disconnect"),
  autoConnect: h("input", { type: "checkbox" }),
  accountSelect: h("select", { id: "account", "aria-label": "Account" }),
  accountBalance: h("span", { class: "balance" }),
  // What a request for the chosen account does, from its profile.
  accountProfile: h("p", { class: "account-profile", "aria-live": "polite" }),
  accountAirdrop: h("button", { class: "sm", type: "button" }, "Airdrop 1 SOL"),
  network: h("div", { class: "segmented", role: "radiogroup", "aria-label": "Network" }),
  customUrl: h("input", { type: "url", placeholder: LOCAL_URL, "aria-label": "RPC URL" }),
  customChain: h(
    "select",
    { "aria-label": "Chain name" },
    h("option", { value: "solana:devnet" }, "solana:devnet"),
    h("option", { value: "solana:testnet" }, "solana:testnet"),
    h("option", { value: "solana:mainnet" }, "solana:mainnet"),
  ),
  customApply: h("button", { class: "sm", type: "button" }, "Use this node"),
  customRow: h("div", { class: "custom-row" }),
  nodeStatus: h("span", { class: "node-status" }),
  networkNote: h("p", { class: "note" }),
  appKeyAddress: h("span"),
  appKeyBalance: h("span", { class: "balance" }),
  appKeyAirdrop: h("button", { class: "sm", type: "button" }, "Airdrop 1 SOL"),
  appKeyNew: h("button", { class: "sm ghost", type: "button" }, "Make a new key"),
  rows: h("div", { class: "rows" }),
  resolutions: h("tbody"),
  resolutionsEmpty: h("div", { class: "empty" }),
  // Over MWA only: what the page does instead of hearing pushes, and a button to ask.
  checkAll: h("button", { class: "sm", type: "button", hidden: true }, "Check all open"),
  resolutionsMwa: h("p", { class: "hint", hidden: true }),
  log: h("pre", { class: "log", tabindex: "0", "aria-label": "Event log" }),
};

function shell(): void {
  els.connect.addEventListener("click", () => void connect(false));
  els.checkAll.addEventListener("click", () => {
    if (store.open().length === 0) toast("Nothing open to check.", "info");
    else void refreshOpen();
  });
  replace(
    els.resolutionsMwa,
    "On a phone, Airlock cannot tell this page when a resolution changes, so the page asks. After a request that resolves at once, it keeps the session open and asks every 2 seconds, for up to a minute. Otherwise press Check on a row, or Check all open. Each opens a new session, and Airlock comes forward for a moment.",
  );
  els.disconnect.addEventListener("click", () => void disconnect());
  els.autoConnect.checked = load<boolean>("autoConnect") === true;
  els.autoConnect.addEventListener("change", () => save("autoConnect", els.autoConnect.checked));
  els.walletSelect.addEventListener("change", () => {
    selectWallet(state.wallets[Number(els.walletSelect.value)] ?? null);
    if (state.wallet && els.autoConnect.checked) void connect(true);
  });
  els.accountSelect.addEventListener("change", () => {
    state.account = state.wallet?.accounts.find((account) => account.address === els.accountSelect.value) ?? null;
    state.profile = null;
    if (state.account) save("account", state.account.address);
    render();
    if (state.account && state.api) void runRow(rows[0]);
  });
  els.accountAirdrop.addEventListener("click", () => state.account && void airdrop(new PublicKey(state.account.address), els.accountBalance));
  els.appKeyAirdrop.addEventListener("click", () => void airdrop(state.appKey.publicKey, els.appKeyBalance));
  els.appKeyNew.addEventListener("click", () => {
    state.appKey = newAppKey();
    log.add("info", `New app key ${state.appKey.publicKey.toBase58()}`);
    render();
  });
  els.customApply.addEventListener("click", () => {
    const url = els.customUrl.value.trim() || LOCAL_URL;
    try {
      const parsed = new URL(url);
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") throw new Error();
    } catch {
      toast("The RPC URL has to start with http:// or https://.", "danger");
      return;
    }
    const chain = els.customChain.value;
    if (isChain(chain)) setNetwork({ kind: "custom", url, chain });
  });

  const settings = h(
    "section",
    { class: "card settings", "aria-labelledby": "settings-title" },
    h("h2", { id: "settings-title" }, "Settings"),
    h(
      "div",
      { class: "setting" },
      h("div", { class: "setting-label" }, "Wallet"),
      h(
        "div",
        { class: "setting-body" },
        h("div", { class: "control-row" }, els.walletSelect, els.connect, els.disconnect),
        h("div", { class: "control-row" }, els.walletChips, h("label", { class: "check" }, els.autoConnect, "Reconnect when the page loads")),
      ),
    ),
    h(
      "div",
      { class: "setting" },
      h("div", { class: "setting-label" }, "Act as"),
      h(
        "div",
        { class: "setting-body" },
        h("div", { class: "control-row" }, els.accountSelect, els.accountBalance, els.accountAirdrop),
        els.accountProfile,
        h("p", { class: "hint" }, "A connected wallet may give several accounts: a key, a multisig vault, a programmatic signer. Each row acts as this one."),
      ),
    ),
    h(
      "div",
      { class: "setting" },
      h("div", { class: "setting-label" }, "Network"),
      h(
        "div",
        { class: "setting-body" },
        h("div", { class: "control-row" }, els.network, els.nodeStatus),
        els.customRow,
        els.networkNote,
      ),
    ),
    h(
      "div",
      { class: "setting" },
      h("div", { class: "setting-label" }, "App key"),
      h(
        "div",
        { class: "setting-body" },
        h("div", { class: "control-row" }, els.appKeyAddress, els.appKeyBalance, els.appKeyAirdrop, els.appKeyNew),
        h(
          "p",
          { class: "hint" },
          "A throwaway key for testing, which this page made and keeps in this browser's storage in plain text. It stands for the app's own fee sponsor in row 5 and its own signer in row 6, and receives row 3's transfer. Never send it real funds.",
        ),
      ),
    ),
  );

  for (const row of rows) {
    const run = h("button", { type: "button", onclick: () => void runRow(row) }, "Run");
    const why = h("span", { class: "why" });
    const expect = h("div", { class: "expect" });
    const request = h("div", { class: "request" });
    const result = h("ol", { class: "steps" });
    rowViews.set(row.key, { run, why, expect, request, result });
    row.controls?.addEventListener("change", () => renderExpectation(row));
    els.rows.append(
      h(
        "article",
        { class: "action card", id: `row-${row.key}`, "aria-labelledby": `row-${row.key}-title` },
        h(
          "header",
          { class: "action-head" },
          h("span", { class: "num", "aria-hidden": "true" }, String(row.n)),
          h("div", { class: "action-title" }, h("h3", { id: `row-${row.key}-title` }, row.title), h("p", { class: "lede" }, row.summary)),
          h("div", { class: "action-controls" }, row.controls ?? null, h("div", { class: "run" }, why, run)),
        ),
        expect,
        h(
          "div",
          { class: "io" },
          h("section", { class: "io-col" }, h("h4", {}, "Request"), request),
          h("section", { class: "io-col" }, h("h4", {}, "Result"), result),
        ),
      ),
    );
  }

  const logCard = h(
    "section",
    { class: "card", "aria-labelledby": "log-title" },
    h(
      "div",
      { class: "section-head" },
      h("h2", { id: "log-title" }, "Event log"),
      h("div", { class: "row-buttons" }, copyButton(() => log.text(), "Copy all"), h("button", { class: "ghost sm", type: "button", onclick: () => log.clear() }, "Clear")),
    ),
    h("p", { class: "hint" }, "Every call, reply, push and error, with its time."),
    els.log,
  );
  log.subscribe((entry) => {
    if (entry === null) {
      els.log.textContent = "";
      return;
    }
    const atBottom = els.log.scrollTop + els.log.clientHeight >= els.log.scrollHeight - 8;
    els.log.append(h("span", { class: `log-${entry.kind}` }, `${line(entry)}\n`));
    if (atBottom) els.log.scrollTop = els.log.scrollHeight;
  });

  append(
    app,
    h(
      "header",
      { class: "page-head" },
      h("img", { class: "brand-mark", src: "icon.png", alt: "" }),
      h(
        "div",
        {},
        h("p", { class: "eyebrow" }, "Airlock · wallet standard proposal"),
        h("h1", {}, "Intents test page"),
        h(
          "p",
          { class: "lede" },
          "A site sends a complete transaction with notes on its accounts: which the wallet may replace, which derive from others, which the app signs for. The wallet resolves it however the account signs, and the site follows an id. Each row below tries one part of that against the wallet you choose.",
        ),
      ),
    ),
    els.devBanner,
    settings,
    h("h2", { class: "section-title" }, "Methods"),
    h("p", { class: "hint section-hint" }, "Run them top to bottom. Each shows what it sends, then what came back, with a link to the explorer for every signature."),
    els.rows,
    h(
      "section",
      { class: "card", "aria-labelledby": "resolutions-title" },
      h("div", { class: "section-head" }, h("h2", { id: "resolutions-title" }, "Resolutions"), h("div", { class: "row-buttons" }, els.checkAll)),
      h(
        "p",
        { class: "hint" },
        "Every resolution this page started, kept in this browser so it survives a reload. A multisig's proposal waits here in awaiting-signatures until the others approve and execute it, then turns done.",
      ),
      els.resolutionsMwa,
      els.resolutionsEmpty,
      h(
        "div",
        { class: "table-wrap" },
        h(
          "table",
          { class: "resolutions" },
          h(
            "thead",
            {},
            h("tr", {}, ...["Request", "Account", "State", "Timing", "Transactions", "Replaced", ""].map((name) => h("th", { scope: "col" }, name))),
          ),
          els.resolutions,
        ),
      ),
    ),
    logCard,
  );
  store.subscribe(() => renderResolutions());
}

function render(): void {
  // Wallets
  replace(
    els.walletSelect,
    h("option", { value: "" }, state.wallets.length ? "Choose a wallet" : "Looking for wallets…"),
    ...state.wallets.map((wallet, index) =>
      h(
        "option",
        { value: String(index), selected: wallet === state.wallet },
        `${wallet.name}${wallet === state.relayWallet ? " (dev relay)" : ""}${intentsOf(wallet) ? "" : " (standard methods only)"}`,
      ),
    ),
  );
  const intents = intentsOf(state.wallet);
  replace(
    els.walletChips,
    !state.wallet
      ? h(
          "span",
          { class: "hint" },
          state.wallets.length
            ? ""
            : "No wallet found yet. Install the Airlock extension and start Airlock, or open the page with ?wallet=ws://127.0.0.1:<port>. On a phone with Airlock, open it with ?wallet=mwa.",
        )
      : isMwa() && !state.mwaWallet!.transport.auth
        ? null // Over MWA the features are known once Connect asked get_capabilities.
        : intents
          ? chip("airlock:resolve", "ok")
          : chip("no airlock:resolve: only row 11 works", "warn"),
    state.wallet ? (state.account ? chip("connected", "selected") : chip("not connected", "neutral")) : null,
    ...(isMwa() ? mwaChips() : []),
  );
  els.checkAll.hidden = !isMwa() || !state.api || !state.account;
  els.resolutionsMwa.hidden = !isMwa();
  els.connect.disabled = !state.wallet;
  els.disconnect.hidden = !state.account;

  // Accounts
  const accounts = state.wallet?.accounts ?? [];
  replace(
    els.accountSelect,
    accounts.length ? null : h("option", { value: "" }, "Connect to list accounts"),
    ...accounts.map((account) =>
      h("option", { value: account.address, selected: account.address === state.account?.address }, `${account.label ? `${account.label} · ` : ""}${short(account.address)}`),
    ),
  );
  els.accountSelect.disabled = accounts.length === 0;
  els.accountAirdrop.hidden = !state.account || state.network.kind === "mainnet";
  if (!state.account) replace(els.accountBalance);
  renderAccountProfile();

  // Network
  const choices: [Network["kind"], string][] = [
    ["devnet", "Devnet"],
    ["mainnet", "Mainnet"],
    ["custom", "Custom RPC"],
  ];
  replace(
    els.network,
    ...choices.map(([kind, label]) =>
      h(
        "button",
        {
          type: "button",
          class: `tab${state.network.kind === kind ? " on" : ""}`,
          role: "radio",
          "aria-checked": String(state.network.kind === kind),
          onclick: () => {
            if (kind === "devnet") setNetwork(DEVNET);
            else if (kind === "mainnet") setNetwork(MAINNET);
            else {
              els.customUrl.value ||= LOCAL_URL;
              setNetwork({ kind: "custom", url: els.customUrl.value, chain: isChain(els.customChain.value) ? els.customChain.value : "solana:devnet" });
            }
          },
        },
        label,
      ),
    ),
  );
  if (state.network.kind === "custom") {
    els.customUrl.value = state.network.url;
    els.customChain.value = state.network.chain;
  }
  replace(
    els.customRow,
    state.network.kind === "custom"
      ? h("div", { class: "control-row" }, h("label", { class: "grow" }, "RPC URL", els.customUrl), h("label", {}, "Named as", els.customChain), els.customApply)
      : null,
  );
  replace(
    els.networkNote,
    state.network.kind === "mainnet"
      ? h("span", { class: "warn-text" }, "Mainnet: every row that sends spends real SOL from your account. There is no airdrop.")
      : `Requests name the chain ${state.network.chain}, and this page reads and sends through ${state.network.url}. Point Airlock at the same network.`,
  );

  // App key
  const appKey = state.appKey.publicKey.toBase58();
  replace(els.appKeyAddress, h("code", { class: "mono", title: appKey }, appKey), " ", copyButton(() => appKey), " ", link(explorerUrl(state.network, "address", appKey), "Explorer"));
  els.appKeyAirdrop.hidden = state.network.kind === "mainnet";
  refreshBalances();

  // Rows
  for (const row of rows) {
    const view = rowViews.get(row.key)!;
    const blocked = rowBlocked(row);
    if (view.run.textContent !== "Running…") view.run.disabled = blocked !== null;
    replace(view.why, blocked ?? "");
    renderExpectation(row);
  }
  renderResolutions();
}

let balancesFor = "";

/** Reads both balances again. Only when the network, the key or the account
 * changed, unless `force`. */
function refreshBalances(force = false): void {
  const key = `${state.network.url} ${state.appKey.publicKey.toBase58()} ${state.account?.address ?? ""}`;
  if (!force && key === balancesFor) return;
  balancesFor = key;
  void balanceOf(state.appKey.publicKey, els.appKeyBalance);
  if (state.account) void balanceOf(new PublicKey(state.account.address), els.accountBalance);
}

/** Over MWA: whether the wallet listed the airlock: features, the chain the
 * page is authorized for, and a session open now. */
function mwaChips(): (HTMLElement | string)[] {
  const transport = state.mwaWallet!.transport;
  const auth = transport.auth;
  const out: (HTMLElement | string)[] = [chip("Mobile Wallet Adapter", "kind")];
  if (!auth) {
    out.push(h("span", { class: "hint" }, "Press Connect. Airlock comes forward to approve this page."));
    return out;
  }
  out.push(
    transport.speaksIntents()
      ? chip("get_capabilities lists the 6 airlock: features", "ok")
      : chip(`The wallet does not speak intents: it lists no ${transport.missingFeatures().join(", ")}`, "warn"),
  );
  const sameChain = auth.chain === state.network.chain;
  out.push(chip(`authorized for ${auth.chain}`, sameChain ? "neutral" : "warn"));
  if (!sameChain) {
    out.push(h("span", { class: "hint" }, `Requests name ${state.network.chain}, but row 11 goes out on ${auth.chain}. Connect again to change it.`));
  }
  const open = transport.openSession;
  if (open !== null) out.push(chip(`session ${open} open`, "info"));
  return out;
}

/** The wallet for Airlock on this phone, through Mobile Wallet Adapter. */
function makeMwaWallet(): MwaStandardWallet {
  const transport = new MwaTransport({
    // The library's MobileWallet answers any method; the page's type names the ones it calls.
    transact: transact as unknown as Transact,
    identity: { name: "Airlock intents test page", uri: location.origin, icon: "icon.png" },
    chain: () => state.network.chain,
    saved: { load: () => load<MwaAuth>("mwa"), save: (auth) => (auth ? save("mwa", auth) : forget("mwa")) },
    canOpen: hasUserActivation,
    events: {
      log: (kind, text, detail) => log.add(kind, text, detail),
      notFound: (id, error) => store.readFailed(id, error, describeError),
      changed: () => render(),
    },
  });
  return createMwaWallet(transport);
}

/** What a request for the chosen account does: resolves at once, or waits
 * for other people or another device. Empty until its profile is read. */
function renderAccountProfile(): void {
  const profile = state.account && state.profile?.account === state.account.address ? state.profile : null;
  replace(
    els.accountProfile,
    profile ? chip(profile.timing === "sync" ? "resolves at once" : "waits for others", profile.timing === "sync" ? "ok" : "info") : null,
    profile ? " " : null,
    profile
      ? whatHappens(profile)
      : state.account && state.api
        ? canCallNow()
          ? "Reading how this account signs…"
          : "Press Run on row 1 to read how this account signs." // Over MWA after a reload: reading needs a tap.
        : null,
  );
}

function renderResolutions(): void {
  const items = store.all();
  replace(els.resolutions, ...items.map(resolutionRow));
  replace(
    els.resolutionsEmpty,
    items.length ? null : h("p", {}, "No resolutions yet. Each request a row sends is listed here, with its state as it changes."),
  );
  els.resolutionsEmpty.hidden = items.length > 0;
}

/* Start */

function start(): void {
  shell();
  // ?wallet=mwa forces Mobile Wallet Adapter. On Android it is offered anyway.
  const forceMwa = new URLSearchParams(location.search).get("wallet") === "mwa";
  const relay = forceMwa ? null : relayFromQuery(location.search);
  if (forceMwa || (relay === null && isAndroid(navigator.userAgent))) {
    state.mwaWallet = makeMwaWallet();
    registerWallet(state.mwaWallet);
    replace(
      els.devBanner,
      window.isSecureContext
        ? h(
            "p",
            { class: "note banner" },
            "Mobile Wallet Adapter: this page reaches Airlock on this phone. Each call opens a session, and Airlock comes forward for a moment. Airlock cannot tell the page when something changes, so the page asks (see Resolutions).",
          )
        : h(
            "p",
            { class: "error banner" },
            `Mobile Wallet Adapter needs a secure page, and ${location.origin} is not one. Open http://localhost:8124 through adb reverse, or serve the page over https.`,
          ),
    );
  }
  if (relay instanceof Error) {
    replace(els.devBanner, h("p", { class: "error banner" }, relay.message));
  } else if (relay) {
    replace(
      els.devBanner,
      h("p", { class: "note banner" }, `Dev transport: the Airlock wallet on this page talks to the relay at ${relay.href}, not to the extension.`),
    );
    state.relayWallet = registerRelayWallet(relay, {
      frame: (direction, text) => log.add("relay", `${direction === "out" ? "→" : "←"} ${text}`),
      status: (text, ok) => {
        log.add(ok ? "info" : "error", text);
        if (!ok) toast(text, "warn");
      },
    });
  }

  const { get, on } = getWallets();
  const wanted = load<string>("wallet");
  const refresh = () => {
    state.wallets = get().filter(usable);
    if (!state.wallet) {
      // The relay's wallet first, when the page was opened for it, then MWA when
      // forced. On a phone with no wallet of its own that speaks intents, MWA.
      const mwa = state.wallets.find((w) => w === state.mwaWallet);
      const pick =
        state.wallets.find((w) => w === state.relayWallet) ??
        (forceMwa ? mwa : undefined) ??
        state.wallets.find((w) => w.name === wanted && (!state.relayWallet || w === state.relayWallet)) ??
        (mwa && !state.wallets.some((w) => w !== mwa && intentsOf(w)) ? mwa : undefined);
      if (pick) {
        selectWallet(pick);
        // A silent connect over MWA opens no session: it takes up an authorization this browser kept.
        if (els.autoConnect.checked || pick === state.relayWallet || pick === state.mwaWallet) void connect(true);
        return;
      }
    } else if (!state.wallets.includes(state.wallet)) {
      selectWallet(null);
      return;
    }
    render();
  };
  on("register", refresh);
  on("unregister", refresh);
  refresh();
  void pingNode(els.nodeStatus);
  log.add("info", `Page loaded. App key ${state.appKey.publicKey.toBase58()}, network ${state.network.chain} at ${state.network.url}.`);
}

start();
