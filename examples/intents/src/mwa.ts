/** Airlock on a phone, through Mobile Wallet Adapter (docs/intents.md, "Airlock's
 * prototype on Android"). The page opened in Chrome on Android talks to the
 * Airlock app over MWA's local, encrypted WebSocket. Airlock answers six
 * methods beside MWA's own: airlock_get_signing_profile, airlock_resolve,
 * airlock_get_resolution, airlock_provide_signatures, airlock_cancel_resolution
 * and airlock_sign_message, with the bridge messages' params and results.
 *
 * The rest of the page sees the same wallet as the extension's: the
 * extension's own AirlockWallet, whose bridge calls this module turns into MWA
 * calls. So `actions.ts` and the rows run one code path for both.
 *
 * What differs on a phone:
 *  - Each call needs a session, and a new session brings Airlock forward for a
 *    moment. A browser opens one only on a tap, so a call that would need a new
 *    session without one is refused here, with words that say to tap.
 *  - MWA has no way for the wallet to speak first. After a request that
 *    resolves at once (`timing: "sync"`) the session stays open, and the page
 *    asks airlock_get_resolution every 2 seconds, for up to a minute, until it
 *    ends. For one that waits for others (`async`) the session ends at once,
 *    and the page asks again in a new session when the person presses Check.
 *
 * The library is not imported here: `transact` is handed in, so the tests run
 * this against a fake wallet with no WebSocket.
 */

import { ENDED, INTENT_FEATURES, type Resolution } from "../../../extension/src/intents";
import {
  BridgeError,
  QUICK_TTL_MS,
  REQUEST_TTL_MS,
  base64ToBytes,
  encodeBase58,
  type AccountWire,
  type BridgeMethod,
} from "../../../extension/src/protocol";
import { AirlockWallet, ICON } from "../../../extension/src/wallet";
import type { Wallet, WalletAccount } from "@wallet-standard/base";

/** What the page tells the wallet about itself. `icon` is relative to `uri`. */
export interface MwaIdentity {
  name: string;
  uri: string;
  icon: string;
}

/** An account as MWA gives it: `address` is the public key in base64. */
export interface MwaAccount {
  address: string;
  label?: string;
  features?: readonly string[];
}

export interface MwaAuthorization {
  auth_token: string;
  accounts: MwaAccount[];
}

/** What the page asks of the library's MobileWallet in a session. The library
 * turns any camelCase name into a snake_case JSON-RPC method, which is how the
 * airlock_ methods are reached: `airlockResolve(p)` sends `airlock_resolve`.
 * `authorize` with an `auth_token` is MWA 2.0's reauthorize. */
export interface MwaWallet {
  authorize(params: { identity: MwaIdentity; chain: string; auth_token?: string }): Promise<MwaAuthorization>;
  getCapabilities(): Promise<{ features?: readonly string[] }>;
  signAndSendTransactions(params: { payloads: string[] }): Promise<{ signatures: string[] }>;
  airlockGetSigningProfile(params: unknown): Promise<unknown>;
  airlockResolve(params: unknown): Promise<unknown>;
  airlockGetResolution(params: unknown): Promise<unknown>;
  airlockProvideSignatures(params: unknown): Promise<unknown>;
  airlockCancelResolution(params: unknown): Promise<unknown>;
  airlockSignMessage(params: unknown): Promise<unknown>;
}

/** The library's `transact`: opens a session, runs `callback` in it, and ends
 * the session when the callback's promise settles. */
export type Transact = <T>(callback: (wallet: MwaWallet) => Promise<T>) => Promise<T>;

/** What the page keeps of an authorization, so a reload stays connected. */
export interface MwaAuth {
  token: string;
  /** The chain it was given for. A reauthorize names the same one. */
  chain: string;
  accounts: AccountWire[];
  /** `get_capabilities`'s features, as the wallet listed them. */
  features: string[];
}

export interface MwaTiming {
  /** How often the page asks while a session stays open. */
  pollMs: number;
  /** How long a session stays open for a resolution, from the last request that left it unfinished. */
  pollWindowMs: number;
  /** How long a session stays open after Connect, so the profile read that follows goes in it. */
  lingerMs: number;
  /** The longest wait for a call that never waits for the person. */
  quickMs: number;
  /** The longest wait for one that does: authorize, resolve, a message, a send. */
  waitMs: number;
}

export const MWA_TIMING: MwaTiming = {
  pollMs: 2000,
  pollWindowMs: 60_000,
  lingerMs: 1000,
  quickMs: QUICK_TTL_MS,
  waitMs: REQUEST_TTL_MS + 60_000,
};

export interface MwaEvents {
  /** A line for the page's event log. Calls the rows make are logged by the page itself. */
  log(kind: "call" | "reply" | "error" | "info", text: string, detail?: unknown): void;
  /** A resolution the page cannot read any more (`not_found`), met while asking in a session. */
  notFound(id: string, error: BridgeError): void;
  /** The connection, the features or a session's state changed. */
  changed(): void;
}

export interface MwaOptions {
  transact: Transact;
  identity: MwaIdentity;
  /** The chain the page is on now, which Connect authorizes for. */
  chain(): string;
  events: MwaEvents;
  /** Where the authorization is kept between visits. */
  saved?: { load(): MwaAuth | null; save(auth: MwaAuth | null): void };
  timing?: Partial<MwaTiming>;
  /** Whether a new session may open now. In a browser, only on a tap. */
  canOpen?(): boolean;
  now?(): number;
}

/** The six methods, by the bridge message each stands for. */
export const MWA_METHODS = {
  getSigningProfile: "airlockGetSigningProfile",
  resolve: "airlockResolve",
  getResolution: "airlockGetResolution",
  provideSignatures: "airlockProvideSignatures",
  cancelResolution: "airlockCancelResolution",
  resolveMessage: "airlockSignMessage",
} as const satisfies Partial<Record<BridgeMethod, keyof MwaWallet>>;

type IntentMethod = keyof typeof MWA_METHODS;

/** The JSON-RPC method name the library sends for a MobileWallet method. */
export function rpcName(method: string): string {
  return method.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
}

/** Calls that wait for the person, so they get the long timeout. */
const WAITS: readonly string[] = ["connect", "resolve", "resolveMessage", "signAndSendTransaction"];

export const GESTURE_NEEDED =
  "Opening a session with Airlock needs a tap on this page: the browser opens the wallet only on one. Press the button again.";

/** MWA's own JSON-RPC error codes, as words the rows know. Airlock's own
 * refusals come with `data.code` and never reach this table. */
const MWA_CODES: Record<number, string> = {
  [-1]: "rejected", // ERROR_AUTHORIZATION_FAILED
  [-2]: "bad_request", // ERROR_INVALID_PAYLOADS
  [-3]: "rejected", // ERROR_NOT_SIGNED: the person declined
  [-4]: "not_submitted", // ERROR_NOT_SUBMITTED
  [-5]: "not_implemented", // ERROR_TOO_MANY_PAYLOADS
  [-32601]: "method_not_found",
  [-32602]: "invalid_params",
  [-32603]: "internal",
};

/** What the library threw, as the page's errors.
 *
 * - A JSON-RPC error whose `data` is `{ code, requirement? }` is Airlock's
 *   refusal: a BridgeError with that code and requirement, and the message.
 *   The JSON-RPC code number is not read.
 * - A JSON-RPC error with no such `data` is MWA's own, named from its number.
 * - Anything the library says about the session itself (no wallet, closed,
 *   timed out, not a secure context) is a plain Error, not a refusal.
 *
 * The library as published drops a JSON-RPC error's `data`; build.mjs puts it
 * back (mwa-patch.mjs). */
export function mwaError(err: unknown): Error {
  if (err instanceof BridgeError) return err;
  if (!err || typeof err !== "object") return new Error(String(err));
  const e = err as { name?: unknown; code?: unknown; message?: unknown; data?: unknown };
  const message = typeof e.message === "string" ? e.message : "";
  const data = e.data && typeof e.data === "object" ? (e.data as { code?: unknown; requirement?: unknown }) : null;
  if (data && typeof data.code === "string") {
    return new BridgeError(data.code, message, typeof data.requirement === "string" ? data.requirement : undefined);
  }
  if (typeof e.code === "number") {
    return new BridgeError(MWA_CODES[e.code] ?? `mwa_${e.code}`, message || `The wallet answered error ${e.code}.`);
  }
  if (typeof e.code === "string" && e.code.startsWith("ERROR_")) {
    return new Error(sessionWords(e.code, message), { cause: err });
  }
  return err instanceof Error ? err : new Error(message || String(err));
}

function sessionWords(code: string, message: string): string {
  switch (code) {
    case "ERROR_WALLET_NOT_FOUND":
      return "No wallet app took the Mobile Wallet Adapter link. Is Airlock installed on this phone? A browser also opens it only on a tap.";
    case "ERROR_SECURE_CONTEXT_REQUIRED":
      return "Mobile Wallet Adapter needs a secure page: https, or http://localhost (with adb reverse). This page is not one.";
    case "ERROR_SESSION_TIMEOUT":
      return "Airlock did not open the session in time. Is Airlock installed, and was it let come forward?";
    case "ERROR_SESSION_CLOSED":
      return `The session with Airlock closed. ${message}`;
    default:
      return `${code}: ${message}`;
  }
}

function isSessionGone(err: unknown): boolean {
  const code = err && typeof err === "object" ? (err as { code?: unknown }).code : undefined;
  return code === "ERROR_SESSION_CLOSED" || code === "ERROR_SESSION_TIMEOUT" || err instanceof TimedOut;
}

class TimedOut extends Error {}

interface Session {
  n: number;
  wallet: MwaWallet;
  /** Calls and the poll using it now. At zero it ends, after `lingerMs` for Connect. */
  users: number;
  closed: boolean;
  end(): void;
  linger: ReturnType<typeof setTimeout> | null;
  /** Resolutions asked about while the session stays open. */
  watched: Set<string>;
  deadline: number;
  polling: boolean;
}

function accountWire(account: MwaAccount): AccountWire {
  const publicKey = base64ToBytes(account.address);
  if (publicKey.length !== 32) throw new BridgeError("internal", "An account Airlock gave is not 32 bytes.");
  return {
    publicKey: account.address,
    label: account.label,
    // MWA lists solana:signMessages on an account that signs messages; with no list, assume it does.
    signsMessages: account.features ? account.features.includes("solana:signMessages") : true,
  };
}

function resolutionIn(result: unknown): Resolution | null {
  const resolution = result && typeof result === "object" ? (result as { resolution?: unknown }).resolution : undefined;
  if (!resolution || typeof resolution !== "object") return null;
  const r = resolution as Resolution;
  return typeof r.id === "string" && typeof r.state === "string" ? r : null;
}

function sameAccounts(a: readonly AccountWire[], b: readonly AccountWire[]): boolean {
  return a.length === b.length && a.every((item, i) => item.publicKey === b[i].publicKey && item.label === b[i].label);
}

export class MwaTransport {
  readonly identity: MwaIdentity;
  readonly timing: MwaTiming;
  #transact: Transact;
  #chain: () => string;
  #events: MwaEvents;
  #saved: NonNullable<MwaOptions["saved"]> | null;
  #canOpen: () => boolean;
  #now: () => number;
  #auth: MwaAuth | null;
  #session: Promise<Session> | null = null;
  #current: Session | null = null;
  #count = 0;
  /** Set by the wallet: a resolution read in a session, for onResolution. */
  onResolution: ((resolution: Resolution) => void) | null = null;
  /** Set by the wallet: the accounts changed on a reauthorize, or were dropped. */
  onAccounts: ((accounts: AccountWire[]) => void) | null = null;

  constructor(options: MwaOptions) {
    this.#transact = options.transact;
    this.identity = options.identity;
    this.#chain = options.chain;
    this.#events = options.events;
    this.#saved = options.saved ?? null;
    this.#canOpen = options.canOpen ?? (() => true);
    this.#now = options.now ?? (() => Date.now());
    this.timing = { ...MWA_TIMING, ...options.timing };
    this.#auth = this.#saved?.load() ?? null;
  }

  get auth(): MwaAuth | null {
    return this.#auth;
  }

  /** Whether the wallet listed all six airlock: features. Null before it was asked. */
  speaksIntents(): boolean | null {
    if (!this.#auth) return null;
    return INTENT_FEATURES.every((feature) => this.#auth!.features.includes(feature));
  }

  /** The airlock: features the wallet did not list. */
  missingFeatures(): string[] {
    return this.#auth ? INTENT_FEATURES.filter((feature) => !this.#auth!.features.includes(feature)) : [];
  }

  /** Whether a session is open now, and which. */
  get openSession(): number | null {
    return this.#current && !this.#current.closed ? this.#current.n : null;
  }

  /** The bridge call `method`, made over MWA. AirlockWallet calls this. */
  async call(method: BridgeMethod, params?: unknown): Promise<unknown> {
    switch (method) {
      case "connect":
        return this.#connect((params as { silent?: boolean } | undefined)?.silent === true);
      case "disconnect":
        this.forget();
        return {};
      case "signAndSendTransaction":
        return this.#signAndSend(params);
      default:
        if (method in MWA_METHODS) return this.#intent(method as IntentMethod, params);
        throw new BridgeError("not_implemented", `${method} is not offered over Mobile Wallet Adapter on this page.`);
    }
  }

  /** Runs `body` with one session held for all of it, so the calls it makes
   * share that session: one new session at most, and Airlock comes forward once. */
  async batch<T>(body: () => Promise<T>): Promise<T> {
    const session = await this.#acquire(false);
    try {
      return await body();
    } finally {
      this.#release(session);
    }
  }

  /** Drops the authorization this page holds. No session is opened for it. */
  forget(): void {
    this.#auth = null;
    this.#saved?.save(null);
    this.onAccounts?.([]);
    this.#events.changed();
  }

  async #connect(silent: boolean): Promise<{ accounts: AccountWire[] }> {
    if (silent) {
      // A silent connect opens no session: it is a question. A kept authorization answers it.
      if (!this.#auth) throw new BridgeError("rejected", "Not connected yet. Press Connect.");
      this.#events.log("info", `Mobile Wallet Adapter: kept authorization for ${this.#auth.chain}, ${this.#auth.accounts.length} accounts. It is checked on the next call.`);
      return { accounts: this.#auth.accounts };
    }
    const chain = this.#chain();
    const session = await this.#acquire(true);
    try {
      this.#events.log("call", `authorize (session ${session.n})`, { identity: this.identity, chain });
      const authorization = await this.#timed(session, session.wallet.authorize({ identity: this.identity, chain }), this.timing.waitMs, "authorize");
      const accounts = (authorization.accounts ?? []).map(accountWire);
      this.#events.log("reply", "authorize", { accounts: accounts.map((a) => encodeBase58(base64ToBytes(a.publicKey))) });
      this.#events.log("call", `get_capabilities (session ${session.n})`);
      const capabilities = await this.#timed(session, session.wallet.getCapabilities(), this.timing.quickMs, "get_capabilities");
      const features = Array.isArray(capabilities?.features) ? capabilities.features.filter((f): f is string => typeof f === "string") : [];
      this.#events.log("reply", "get_capabilities", capabilities);
      this.#setAuth({ token: authorization.auth_token, chain, accounts, features });
      const missing = this.missingFeatures();
      if (missing.length) this.#events.log("error", `The wallet does not list ${missing.join(", ")}: it does not speak intents. Only the fallback row can run.`);
      return { accounts };
    } catch (err) {
      throw this.#failed(session, err);
    } finally {
      this.#release(session, true);
    }
  }

  async #intent(method: IntentMethod, params: unknown): Promise<unknown> {
    const name = MWA_METHODS[method];
    const session = await this.#acquire(false);
    try {
      const call = session.wallet[name](params);
      const result = await this.#timed(session, call, WAITS.includes(method) ? this.timing.waitMs : this.timing.quickMs, rpcName(name));
      const resolution = resolutionIn(result);
      if (resolution && (method === "resolve" || method === "resolveMessage" || method === "provideSignatures")) this.#watch(session, resolution);
      return result;
    } catch (err) {
      throw this.#failed(session, err);
    } finally {
      this.#release(session);
    }
  }

  async #signAndSend(params: unknown): Promise<unknown> {
    const transactions = ((params as { transactions?: { transaction: string; chain?: string }[] })?.transactions ?? []);
    const session = await this.#acquire(false);
    try {
      const chains = new Set(transactions.map((tx) => tx.chain).filter(Boolean));
      if (this.#auth && chains.size && !chains.has(this.#auth.chain)) {
        this.#events.log("info", `sign_and_send_transactions goes out on ${this.#auth.chain}, the chain this page was authorized for, not ${[...chains].join(", ")}. Connect again to change it.`);
      }
      return await this.#timed(
        session,
        session.wallet.signAndSendTransactions({ payloads: transactions.map((tx) => tx.transaction) }),
        this.timing.waitMs,
        "sign_and_send_transactions",
      );
    } catch (err) {
      throw this.#failed(session, err);
    } finally {
      this.#release(session);
    }
  }

  #setAuth(auth: MwaAuth): void {
    this.#auth = auth;
    this.#saved?.save(auth);
    this.#events.changed();
  }

  /** A session to use: the open one, or a new one. A new one starts with
   * authorize (Connect) or a reauthorize with the kept token (every other call). */
  async #acquire(fresh: boolean): Promise<Session> {
    for (;;) {
      const pending = this.#session;
      if (!pending) break;
      const open = await pending.catch(() => null);
      if (open && !open.closed) {
        open.users += 1;
        if (open.linger) {
          clearTimeout(open.linger);
          open.linger = null;
        }
        return open;
      }
      if (this.#session === pending) this.#session = null;
    }
    if (!fresh && !this.#auth) throw new BridgeError("rejected", "Not connected: press Connect first.");
    if (!this.#canOpen()) throw new Error(GESTURE_NEEDED);
    const opening = this.#open(fresh);
    this.#session = opening;
    try {
      const session = await opening;
      session.users += 1;
      return session;
    } catch (err) {
      if (this.#session === opening) this.#session = null;
      throw mwaError(err);
    }
  }

  #open(fresh: boolean): Promise<Session> {
    const n = (this.#count += 1);
    this.#events.log("info", `Session ${n}: opening. Airlock comes forward for a moment.`);
    return new Promise((resolve, reject) => {
      let started: Session | null = null;
      this.#transact(async (wallet) => {
        let end!: () => void;
        const held = new Promise<void>((done) => (end = done));
        const session: Session = { n, wallet, users: 0, closed: false, end, linger: null, watched: new Set(), deadline: 0, polling: false };
        if (!fresh) await this.#reauthorize(session);
        started = session;
        this.#current = session;
        resolve(session);
        this.#events.changed();
        await held;
      })
        .catch((err) => {
          if (!started) reject(err);
          else this.#events.log("error", `Session ${n}: ${mwaError(err).message}`);
        })
        .finally(() => {
          const session = started as Session | null;
          if (!session) return; // It never opened: the caller reports why.
          session.closed = true;
          if (this.#current === session) this.#current = null;
          this.#events.log("info", `Session ${n}: ended.`);
          this.#events.changed();
        });
    });
  }

  /** MWA 2.0's reauthorize: authorize with the kept token, for its chain. A
   * token the wallet no longer takes is dropped, and the page shows no accounts. */
  async #reauthorize(session: Session): Promise<void> {
    const auth = this.#auth!;
    this.#events.log("call", `authorize with the kept token (session ${session.n})`, { chain: auth.chain });
    let result: MwaAuthorization;
    try {
      result = await this.#timed(session, session.wallet.authorize({ identity: this.identity, chain: auth.chain, auth_token: auth.token }), this.timing.quickMs * 4, "authorize");
    } catch (err) {
      const error = mwaError(err);
      if (error instanceof BridgeError && error.code === "rejected") {
        this.forget();
        throw new BridgeError("rejected", "Airlock no longer takes this page's authorization. Press Connect again.");
      }
      throw error;
    }
    const accounts = Array.isArray(result?.accounts) && result.accounts.length ? result.accounts.map(accountWire) : auth.accounts;
    const token = typeof result?.auth_token === "string" && result.auth_token ? result.auth_token : auth.token;
    this.#events.log("reply", "authorize", { tokenChanged: token !== auth.token, accounts: accounts.length });
    const moved = !sameAccounts(accounts, auth.accounts);
    if (moved || token !== auth.token) this.#setAuth({ ...auth, token, accounts });
    if (moved) this.onAccounts?.(accounts);
  }

  #release(session: Session, linger = false): void {
    session.users -= 1;
    if (session.users > 0 || session.closed) return;
    if (linger && this.timing.lingerMs > 0) {
      session.linger = setTimeout(() => {
        session.linger = null;
        if (session.users === 0) this.#end(session);
      }, this.timing.lingerMs);
      return;
    }
    this.#end(session);
  }

  #end(session: Session): void {
    if (session.closed) return;
    session.closed = true;
    if (this.#current === session) this.#current = null;
    session.end();
  }

  /** The error to throw for a failed call. A session that broke is ended, so
   * the next call opens a new one. */
  #failed(session: Session, err: unknown): Error {
    if (isSessionGone(err)) this.#end(session);
    return mwaError(err);
  }

  #timed<T>(session: Session, call: Promise<T>, ms: number, what: string): Promise<T> {
    let timer: ReturnType<typeof setTimeout>;
    const late = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new TimedOut(`${what}: Airlock did not answer in ${Math.round(ms / 1000)} s. The session is ended.`)), ms);
    });
    return Promise.race([call, late]).finally(() => clearTimeout(timer)).catch((err) => {
      if (err instanceof TimedOut) this.#end(session);
      throw err;
    });
  }

  /** Keeps the session open for a resolution that resolves at once and has
   * not ended, and asks for it every `pollMs` until it ends, for up to
   * `pollWindowMs`. One that waits for others leaves the session to end. */
  #watch(session: Session, resolution: Resolution): void {
    if (session.closed || ENDED.includes(resolution.state) || resolution.timing !== "sync") return;
    session.watched.add(resolution.id);
    session.deadline = Math.max(session.deadline, this.#now() + this.timing.pollWindowMs);
    if (session.polling) return;
    session.polling = true;
    session.users += 1;
    void this.#poll(session);
  }

  async #poll(session: Session): Promise<void> {
    this.#events.log(
      "info",
      `Session ${session.n} stays open: the page asks airlock_get_resolution every ${this.timing.pollMs / 1000} s, for up to ${this.timing.pollWindowMs / 1000} s, until it ends.`,
    );
    let why = "";
    try {
      while (!session.closed && session.watched.size > 0 && !why) {
        await new Promise((done) => setTimeout(done, this.timing.pollMs));
        if (session.closed) {
          why = "the session closed";
          break;
        }
        if (this.#now() >= session.deadline) {
          why = `${this.timing.pollWindowMs / 1000} s went by. Press Check to ask again`;
          break;
        }
        for (const id of [...session.watched]) {
          try {
            this.#events.log("call", `airlock_get_resolution (session ${session.n})`, { id });
            const result = await this.#timed(session, session.wallet.airlockGetResolution({ id }), this.timing.quickMs, "airlock_get_resolution");
            const resolution = resolutionIn(result);
            if (!resolution) throw new BridgeError("internal", "Airlock did not return a resolution.");
            this.#events.log("reply", "airlock_get_resolution", resolution);
            if (ENDED.includes(resolution.state)) session.watched.delete(id);
            this.onResolution?.(resolution);
          } catch (err) {
            const error = this.#failed(session, err);
            if (error instanceof BridgeError && error.code === "not_found") {
              session.watched.delete(id);
              this.#events.notFound(id, error);
              continue;
            }
            this.#events.log("error", `airlock_get_resolution: ${error.message}`);
            why = `asking failed (${error.message})`;
            break;
          }
        }
      }
    } finally {
      const left = session.watched.size;
      session.watched.clear();
      session.polling = false;
      this.#events.log("info", `Session ${session.n}: stopped asking${why ? `: ${why}` : ", every resolution it followed has ended"}.${left ? ` ${left} still open.` : ""}`);
      this.#release(session);
    }
  }
}

/** The wallet the page lists for Mobile Wallet Adapter: the extension's
 * AirlockWallet with its calls made over MWA. Only what goes over MWA is
 * listed, and the airlock: features only while the wallet's capabilities do
 * not say it lacks them. */
export interface MwaStandardWallet extends Wallet {
  readonly transport: MwaTransport;
}

const KEPT_FEATURES = ["standard:connect", "standard:disconnect", "standard:events", "solana:signAndSendTransaction"];

export function createMwaWallet(transport: MwaTransport): MwaStandardWallet {
  const inner = new AirlockWallet((method, params) => transport.call(method, params));
  transport.onResolution = (resolution) => inner.receivePush({ method: "resolutionChanged", params: { resolution } });
  transport.onAccounts = (accounts) => inner.setAccounts(accounts);
  const all = inner.features;
  return {
    version: "1.0.0",
    name: "Mobile Wallet Adapter",
    icon: ICON,
    chains: inner.chains,
    transport,
    get accounts(): readonly WalletAccount[] {
      return inner.accounts;
    },
    get features(): Wallet["features"] {
      const intents = transport.speaksIntents() !== false;
      return Object.fromEntries(
        Object.entries(all).filter(([name]) => KEPT_FEATURES.includes(name) || (intents && (INTENT_FEATURES as readonly string[]).includes(name))),
      );
    },
  };
}

/** A phone's browser, where MWA is how a page reaches a wallet. */
export function isAndroid(userAgent: string): boolean {
  return /\bAndroid\b/i.test(userAgent);
}

/** Whether a new session may open: in a browser that says, only while a tap is fresh. */
export function hasUserActivation(): boolean {
  const activation = (globalThis.navigator as (Navigator & { userActivation?: { isActive: boolean } }) | undefined)?.userActivation;
  return activation ? activation.isActive : true;
}
